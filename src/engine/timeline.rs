//! The engine's side of timelines: a run per timeline, with a clock of its own
//! (`Run::pos`), and what GO, pause, stop and seek do to it. Hooks in
//! `engine.rs` call in here.
//!
//! A run keeps its waiting clips in `Engine::pending`, tagged with the run
//! (`PendingCue::run`, due when the clock reaches `PendingCue::at`), so every
//! way of cancelling a pending cue still works on them. Each tick a running
//! clock moves on and starts what it reaches one at a time, in time order and
//! at equal times control cues first, then fades and stops, then clips
//! (`model::due_order`): a pause cue stops the clock before anything after it
//! starts, even in one long tick.
//!
//! A clip with a free-run pause inside it (or that ran free or was frozen on its
//! own while the clock stood or ran) is *clocked*: its fade-out and end come from
//! the clock (`ActiveCue::ends_at`), so after the pause they happen where they
//! sit on the timeline, whatever its content has got to. If the content runs
//! out first, `MediaCue::run_out` says what it does (`clock_end`).
//!
//! A pause that freezes content leaves live inputs (cameras, NDI, live sound)
//! live unless it says they freeze too (`ControlCue::freeze_live`); then a
//! camera holds its frame and live sound is muted, and what arrives meanwhile is
//! let go, so they are live again at once when the run goes on.
//!
//! Not built yet (the next package): starting part-way in (seek, parking,
//! jumps, `ControlCue::at`), the cueing gate, and clips that wait for their
//! fade-in. A stopped list timeline that is run again starts what comes after
//! its playhead and nothing of what was already under it.

use super::{ActiveCue, ActiveKind, Engine, PendingCue, StartAt};
use crate::show::{
    ControlAction, ControlCue, Cue, CueId, CueKind, GroupCue, GroupMode, MIN_DB, MediaCue,
    PauseFreeze, RunOut, Show, TimelineKey,
};
use crate::timeline::model;
use std::collections::HashMap;

/// Times closer than this are the same time.
pub(super) const SAME_TIME: f64 = 1e-9;
/// How far ahead of a timeline's clock its clips are got ready (decoders
/// started), at most `MAX_PRELOAD_CHILDREN` of them.
pub(super) const PRELOAD_HORIZON: f64 = 10.0;
/// A guard: no run starts more than this many cues in one step.
const MAX_STARTS: usize = 10_000;

/// Where a timeline's run is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RunState {
    /// The clock still, nothing playing.
    Stopped,
    /// Clips started held, waiting for all to be ready; the clock still.
    /// (Comes with starting part-way in.)
    #[allow(dead_code)]
    Cueing,
    Running,
    /// The clock still; `freeze` says what holds. `waits_for_go`: GO runs it on.
    Paused {
        freeze: PauseFreeze,
        waits_for_go: bool,
    },
}

/// A timeline's run, for the editor and the transport.
// The editor (`ui/timeline.rs`) doesn't read it yet.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunView {
    pub key: TimelineKey,
    pub state: RunState,
    pub pos: f64,
    pub next_pause: Option<f64>,
    pub length: Option<f64>,
    /// What GO will bring in or fade, for the read-out.
    pub on_go: Option<CueId>,
}

/// One timeline's run.
struct Run {
    /// Its instance number: the group's own, or one taken for a list's run. Its
    /// clips carry it (`ActiveCue::run`, `PendingCue::run`), and those it starts
    /// at 0 are together by it (`ActiveCue::together`).
    instance: u64,
    state: RunState,
    /// The clock: seconds from the timeline's start.
    pos: f64,
    /// What its pause froze (instances), to let go of when it runs on. A clip
    /// frozen by a Pause of its own is not among them: only its Resume lets it go.
    frozen: Vec<u64>,
    /// The pause's clips (`PauseFreeze::Clips`) and whether it freezes live
    /// inputs, for a clip that starts while it is paused.
    pause_clips: Vec<CueId>,
    pause_live: bool,
}

/// The runs of the timelines that have been touched.
#[derive(Default)]
pub struct Runs {
    map: HashMap<TimelineKey, Run>,
}

impl Runs {
    /// Each run's clock, by its instance.
    pub(super) fn clocks(&self) -> HashMap<u64, f64> {
        self.map.values().map(|r| (r.instance, r.pos)).collect()
    }

    /// The instances of the runs whose clocks are running.
    pub(super) fn running(&self) -> Vec<u64> {
        self.map
            .values()
            .filter(|r| r.state == RunState::Running)
            .map(|r| r.instance)
            .collect()
    }

    fn by_instance(&self, instance: u64) -> Option<(TimelineKey, &Run)> {
        self.map
            .iter()
            .find(|(_, r)| r.instance == instance)
            .map(|(k, r)| (*k, r))
    }

    /// Every run stopped where it is (their clips are being stopped).
    fn stop_all(&mut self) {
        for r in self.map.values_mut() {
            r.state = RunState::Stopped;
            r.frozen.clear();
        }
    }
}

/// How a timeline's clip plays (`Engine::timeline_clip`).
#[derive(Clone, Debug, Default)]
pub(super) struct ClipTiming {
    /// Its fade-out and end are its timeline clock's.
    pub clocked: bool,
    /// Where on the timeline its slot ends.
    pub ends_at: Option<f64>,
    /// The cue as it is to be played: a clocked clip that loops when it runs out
    /// (`RunOut::Loop`) is played as a loop that never ends, and the clock ends it.
    pub looped: Option<Cue>,
}

/// Where a cue starts on its timeline: its pre-wait (never before 0).
fn start_of(cue: &Cue) -> f64 {
    if cue.pre_wait.is_finite() {
        cue.pre_wait.max(0.0)
    } else {
        0.0
    }
}

/// A top-level cue of a timeline list: fired on its own it ignores its
/// pre-wait, and it never continues into the next cue.
pub(super) fn on_a_timeline_list(show: &Show, id: CueId) -> bool {
    matches!(model::key_of(show, id), Some(TimelineKey::List(_)))
}

/// A live input: a camera (or NDI source) or live sound.
pub(super) fn is_live(show: &Show, id: CueId) -> bool {
    matches!(
        show.cue(id).map(|c| &c.kind),
        Some(CueKind::Camera(_) | CueKind::LiveAudio(_))
    )
}

/// Whether a child of the group at (or following) the playhead is got ready
/// with it: in a timeline, only one that starts within `PRELOAD_HORIZON`.
pub(super) fn starts_soon(g: &GroupCue, child: &Cue) -> bool {
    g.mode != GroupMode::Timeline || g.slides.is_some() || start_of(child) <= PRELOAD_HORIZON
}

/// A clip's content has left its timeline's clock: from now on the clock keeps
/// its end (if that is known, or it has none).
pub(super) fn detach(a: &mut ActiveCue) {
    a.detached = true;
    if a.ends_at.is_some() || a.duration.is_none() {
        a.clocked = true;
    }
}

/// The fade on running out (`RunOut::End`): `run_out_fade`, else the cue's own
/// fade-out or the default one.
fn run_out_fade(m: &MediaCue) -> f64 {
    match m.run_out_fade {
        Some(f) if f.is_finite() => f.max(0.0),
        _ => m.manual_fade_out(),
    }
}

/// Fade a clip out over `secs`, from where it is.
fn fade_away(a: &mut ActiveCue, m: &MediaCue, secs: f64) {
    a.auto_faded = true;
    a.opacity.ramp_to(0.0, secs.max(0.0), m.curve);
    a.volume.ramp_to(MIN_DB, secs.max(0.0), m.curve);
}

/// A clocked clip, each tick (`on_clock`: how far its timeline's clock has to go
/// to the end of its slot; `moving`: the clock is running). Its own fade-out
/// begins that long before the clock reaches the end, and it ends there; neither
/// while the clock stands (a pause right at the fade-out's start holds it).
/// Content that has run ahead of the clock and will run out first, set to end
/// (`RunOut::End`), fades out to finish as it runs out. Returns whether it is over.
pub(super) fn clock_end(
    a: &mut ActiveCue,
    m: &MediaCue,
    on_clock: Option<f64>,
    moving: bool,
) -> bool {
    if a.detached
        && m.run_out == RunOut::End
        && !a.auto_faded
        && let (Some(d), Some(clock)) = (a.duration, on_clock)
    {
        let left = d - a.elapsed;
        if left < clock - SAME_TIME && left <= run_out_fade(m) {
            fade_away(a, m, left);
        }
    }
    let Some(left) = on_clock.filter(|_| moving) else {
        return false;
    };
    if !a.auto_faded && m.fade_out > 0.0 && left <= m.fade_out {
        fade_away(a, m, left);
    }
    // A video holding its last frame for good (no fade-out) keeps it, as it would
    // off a timeline.
    left <= SAME_TIME && !(a.held && m.hold_last_frame && !a.auto_faded)
}

/// Whether `cue` is a timeline pause of the timeline `key` (it is one of its cues).
#[allow(dead_code)]
fn is_pause_of(cue: &Cue, key: TimelineKey) -> bool {
    matches!(&cue.kind, CueKind::Control(c)
        if c.action == ControlAction::TimelinePause && c.timeline.is_none_or(|k| k == key))
}

impl Engine {
    // For the editor (`ui/timeline.rs`), which doesn't call it yet.
    #[allow(dead_code)]
    /// A timeline's state; `Stopped` at 0 when it has never been touched.
    pub fn timeline(&mut self, show: &Show, key: TimelineKey) -> RunView {
        let clips = model::clips(show, key, &mut |c| self.cue_duration(show, c));
        let length = model::length(&clips);
        let Some(run) = self.runs.map.get(&key) else {
            return RunView {
                key,
                state: RunState::Stopped,
                pos: 0.0,
                next_pause: self.next_pause_from(show, key, 0.0),
                length,
                on_go: None,
            };
        };
        let (state, pos, instance) = (run.state, run.pos, run.instance);
        let waiting = |p: &&PendingCue| p.run == Some(instance);
        let next_pause = if state == RunState::Stopped {
            self.next_pause_from(show, key, pos)
        } else {
            self.pending
                .iter()
                .filter(waiting)
                .filter(|p| show.cue(p.cue_id).is_some_and(|c| is_pause_of(c, key)))
                .map(|p| p.at)
                .min_by(f64::total_cmp)
        };
        // Paused: the first thing the clock will reach that is not a control cue.
        let on_go = matches!(state, RunState::Paused { .. })
            .then(|| {
                self.pending
                    .iter()
                    .filter(waiting)
                    .filter(|p| show.cue(p.cue_id).is_some_and(|c| model::due_order(c) > 0))
                    .min_by(|a, b| a.at.total_cmp(&b.at))
                    .map(|p| p.cue_id)
            })
            .flatten();
        RunView {
            key,
            state,
            pos,
            next_pause,
            length,
            on_go,
        }
    }

    // For the editor (`ui/timeline.rs`), which doesn't call it yet.
    #[allow(dead_code)]
    /// The first pause cue of a timeline at or after `pos`.
    fn next_pause_from(&self, show: &Show, key: TimelineKey, pos: f64) -> Option<f64> {
        model::members(show, key)
            .into_iter()
            .filter(|c| c.armed && is_pause_of(c, key) && start_of(c) >= pos - SAME_TIME)
            .map(start_of)
            .min_by(f64::total_cmp)
    }

    // For the editor (`ui/timeline.rs`), which doesn't call it yet.
    #[allow(dead_code)]
    /// Those not `Stopped`.
    pub fn timelines(&self) -> Vec<TimelineKey> {
        self.runs
            .map
            .iter()
            .filter(|(_, r)| r.state != RunState::Stopped)
            .map(|(k, _)| *k)
            .collect()
    }

    /// Run from where it is: Stopped/parked -> cue then run; Paused -> resume; Running -> no-op.
    pub fn timeline_run(&mut self, show: &Show, key: TimelineKey) {
        match self.runs.map.get(&key).map(|r| r.state) {
            Some(RunState::Running) => self.log("The timeline is already running"),
            Some(RunState::Cueing) => {}
            Some(RunState::Paused { .. }) => self.timeline_resume(show, key),
            Some(RunState::Stopped) | None => self.timeline_start(show, key),
        }
    }

    /// Start a timeline that is stopped: a group from the top (it is fired), a
    /// list from its playhead.
    fn timeline_start(&mut self, show: &Show, key: TimelineKey) {
        if !model::is_timeline(show, key) {
            self.problem("There is no such timeline");
            return;
        }
        match key {
            TimelineKey::Group(id) => self.fire(show, id),
            TimelineKey::List(_) => {
                let pos = self.runs.map.get(&key).map_or(0.0, |r| r.pos);
                if pos > 0.0 && !self.has_more(show, key, pos) {
                    self.log("GO: the timeline is at its end");
                    return;
                }
                // As firing a cue does, running lets go of the show's pause.
                if self.paused {
                    self.set_paused(false);
                }
                let instance = self.alloc_instance();
                self.timeline_begin(show, key, instance, pos);
                self.timeline_step(show, key, 0.0);
            }
        }
    }

    /// Whether a run from `pos` would start anything.
    fn has_more(&self, show: &Show, key: TimelineKey, pos: f64) -> bool {
        model::members(show, key)
            .into_iter()
            .any(|c| c.armed && Self::comes_after(c, pos))
    }

    /// Whether a run from `pos` starts `cue`: what starts later, and a clip (not a
    /// control cue: a pause there has been passed) right at `pos`.
    fn comes_after(cue: &Cue, pos: f64) -> bool {
        let t = start_of(cue);
        pos <= 0.0 || t > pos + SAME_TIME || (t >= pos - SAME_TIME && model::due_order(cue) > 0)
    }

    /// A new run of `key` from `from` (instance `instance`): its clips wait for
    /// its clock. An earlier run of it gives up what it was still waiting to start.
    pub(super) fn timeline_begin(
        &mut self,
        show: &Show,
        key: TimelineKey,
        instance: u64,
        from: f64,
    ) {
        if let Some(old) = self.runs.map.remove(&key) {
            self.cancel_run(&old);
        }
        let members: Vec<&Cue> = match key {
            TimelineKey::Group(id) => show.armed_children(id),
            TimelineKey::List(_) => model::members(show, key)
                .into_iter()
                .filter(|c| c.armed)
                .collect(),
        };
        for c in members {
            if !Self::comes_after(c, from) {
                continue;
            }
            let at = start_of(c);
            // Fired: no longer waiting to be got ready.
            self.loads.remove(&c.id);
            self.pending.push(PendingCue {
                cue_id: c.id,
                remaining: (at - from).max(0.0),
                total: at,
                reading: None,
                run: Some(instance),
                at,
                start: StartAt {
                    run: Some(instance),
                    together: Some(instance),
                },
            });
        }
        self.runs.map.insert(
            key,
            Run {
                instance,
                state: RunState::Running,
                pos: from.max(0.0),
                frozen: Vec::new(),
                pause_clips: Vec::new(),
                pause_live: false,
            },
        );
    }

    /// A run is over or replaced: what it was waiting to start never starts, and
    /// what its pause froze is let go.
    fn cancel_run(&mut self, run: &Run) {
        let i = run.instance;
        self.pending
            .retain(|p| p.run != Some(i) && p.start.run != Some(i));
        let paused = self.paused;
        for a in &mut self.active {
            if run.frozen.contains(&a.instance) {
                a.unfreeze(paused);
            }
        }
    }

    fn still_running(&self, key: TimelineKey, instance: u64) -> bool {
        self.runs
            .map
            .get(&key)
            .is_some_and(|r| r.instance == instance && r.state == RunState::Running)
    }

    /// Move a running clock on by `dt`, starting what it reaches one at a time,
    /// in order. Something it starts that stops the clock (a pause cue, a stop)
    /// leaves the clock there and the rest waiting.
    pub(super) fn timeline_step(&mut self, show: &Show, key: TimelineKey, dt: f64) {
        let Some(run) = self.runs.map.get(&key) else {
            return;
        };
        if run.state != RunState::Running {
            return;
        }
        let instance = run.instance;
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let target = run.pos + dt;
        let order = |p: &PendingCue| {
            let c = show.cue(p.cue_id);
            (
                c.map_or(2, model::due_order),
                show.cue_index(p.cue_id).unwrap_or(usize::MAX),
            )
        };
        for _ in 0..MAX_STARTS {
            let next = self
                .pending
                .iter()
                .enumerate()
                .filter(|(_, p)| p.run == Some(instance) && p.at <= target + SAME_TIME)
                .min_by(|(_, a), (_, b)| {
                    a.at.total_cmp(&b.at).then_with(|| order(a).cmp(&order(b)))
                })
                .map(|(i, _)| i);
            let Some(i) = next else {
                if let Some(r) = self.runs.map.get_mut(&key) {
                    r.pos = r.pos.max(target);
                }
                self.refresh_waits();
                return;
            };
            let p = self.pending.remove(i);
            if let Some(r) = self.runs.map.get_mut(&key) {
                r.pos = r.pos.max(p.at);
            }
            self.start_at(show, p.cue_id, p.start);
            if !self.still_running(key, instance) {
                self.refresh_waits();
                return;
            }
        }
        self.problem("A timeline started too many cues at once; the rest wait for the next frame");
    }

    /// The runs' clocks move on with the show (not while it is paused).
    pub(super) fn timeline_tick(&mut self, show: &Show, dt: f64) {
        if self.paused {
            return;
        }
        let running: Vec<TimelineKey> = self
            .runs
            .map
            .iter()
            .filter(|(_, r)| r.state == RunState::Running)
            .map(|(k, _)| *k)
            .collect();
        for key in running {
            self.timeline_step(show, key, dt);
        }
    }

    /// A waiting clip's `remaining` is the time its clock has to go to it.
    fn refresh_waits(&mut self) {
        let clocks = self.runs.clocks();
        for p in &mut self.pending {
            if let Some(pos) = p.run.and_then(|r| clocks.get(&r)) {
                p.remaining = (p.at - pos).max(0.0);
            }
        }
    }

    /// After each tick: a timeline group's time is its clock's; a run whose group
    /// has gone, or a list's whose clips are all done, is stopped where it is.
    pub(super) fn timeline_after_tick(&mut self, _show: &Show) {
        let mut ended = Vec::new();
        for (key, run) in &self.runs.map {
            if run.state == RunState::Stopped {
                continue;
            }
            match key {
                TimelineKey::Group(_) => {
                    match self.active.iter_mut().find(|a| a.instance == run.instance) {
                        Some(g) => g.elapsed = run.pos,
                        None => ended.push(*key),
                    }
                }
                TimelineKey::List(_) => {
                    if run.state == RunState::Running && !self.run_busy(run) {
                        ended.push(*key);
                    }
                }
            }
        }
        for key in ended {
            if let Some(run) = self.runs.map.get(&key) {
                let instance = run.instance;
                self.timeline_stopped(instance);
                if matches!(key, TimelineKey::List(_)) {
                    log::debug!("a timeline list has played to its end");
                }
            }
        }
    }

    /// Whether a run still has something to start, or a clip playing or kept by its clock.
    fn run_busy(&self, run: &Run) -> bool {
        let i = run.instance;
        self.pending
            .iter()
            .any(|p| p.run == Some(i) || p.start.run == Some(i))
            || self.active.iter().any(|a| {
                a.run == Some(i)
                    && (!a.held
                        || (a.clocked && a.ends_at.is_some_and(|e| e > run.pos + SAME_TIME)))
            })
    }

    /// Whether the timeline group of `instance` may end now (its children have):
    /// not while its run is not running, nor while a clip its clock keeps has
    /// time left. Any other group may.
    pub(super) fn timeline_group_may_end(&self, instance: u64) -> bool {
        let Some((TimelineKey::Group(_), run)) = self.runs.by_instance(instance) else {
            return true;
        };
        run.state == RunState::Running
            && !self.active.iter().any(|a| {
                a.run == Some(instance)
                    && a.clocked
                    && !a.stopping
                    && a.ends_at.is_some_and(|e| e > run.pos + SAME_TIME)
            })
    }

    /// The instance `instance` was stopped: if it is a run's (a timeline group),
    /// the run stops where it is.
    pub(super) fn timeline_stopped(&mut self, instance: u64) {
        let Some((key, _)) = self.runs.by_instance(instance) else {
            return;
        };
        if let Some(mut run) = self.runs.map.remove(&key) {
            self.cancel_run(&run);
            run.state = RunState::Stopped;
            run.frozen.clear();
            self.runs.map.insert(key, run);
        }
    }

    /// Everything was stopped: every run stops where it is.
    pub(super) fn timeline_stop_all(&mut self) {
        self.runs.stop_all();
    }

    /// Forget every run (reset, or another show).
    pub(super) fn timeline_forget(&mut self) {
        self.runs.map.clear();
    }

    /// The runs, stopped, put aside while a test show runs.
    pub(super) fn take_runs(&mut self) -> Runs {
        let mut runs = std::mem::take(&mut self.runs);
        runs.stop_all();
        runs
    }

    /// The show has changed: a run whose timeline is no longer one is let go.
    pub(super) fn timeline_sync(&mut self, show: &Show) {
        let gone: Vec<TimelineKey> = self
            .runs
            .map
            .keys()
            .filter(|k| !model::is_timeline(show, **k))
            .copied()
            .collect();
        for key in gone {
            if let Some(run) = self.runs.map.remove(&key) {
                self.cancel_run(&run);
            }
        }
    }

    /// While a run is running (or cueing), the engine is busy.
    pub(super) fn timeline_busy(&self) -> bool {
        self.runs.map.iter().any(|(k, r)| {
            matches!(r.state, RunState::Running | RunState::Cueing)
                && match k {
                    TimelineKey::Group(_) => self.active.iter().any(|a| a.instance == r.instance),
                    TimelineKey::List(_) => true,
                }
        })
    }

    /// The clips a run's clock is nearing, to be got ready: those due within
    /// `PRELOAD_HORIZON`, soonest first, at most `MAX_PRELOAD_CHILDREN`.
    pub(super) fn timeline_wanted(&self, show: &Show) -> Vec<CueId> {
        let clocks = self.runs.clocks();
        let mut soon: Vec<(f64, CueId)> = self
            .pending
            .iter()
            .filter_map(|p| {
                let pos = *clocks.get(&p.run.or(p.start.run)?)?;
                // One waiting for its file to be read is due now.
                let at = if p.run.is_some() { p.at } else { pos };
                (at - pos <= PRELOAD_HORIZON).then_some((at, p.cue_id))
            })
            .filter(|(_, id)| show.cue(*id).is_some_and(|c| c.kind.preloads()))
            .collect();
        soon.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut out: Vec<CueId> = Vec::new();
        for (_, id) in soon {
            if out.len() >= super::MAX_PRELOAD_CHILDREN {
                break;
            }
            if !out.contains(&id) {
                out.push(id);
            }
        }
        out
    }

    /// How a cue plays as a clip of its timeline: whether its clock keeps its
    /// end, where that is, and whether it is played as a loop (see `ClipTiming`).
    /// Nothing for a cue on no timeline, or whose length isn't known yet.
    pub(super) fn timeline_clip(&mut self, show: &Show, cue: &Cue) -> ClipTiming {
        let Some(key) = model::key_of(show, cue.id) else {
            return ClipTiming::default();
        };
        let Some(m) = cue.kind.media() else {
            return ClipTiming::default();
        };
        let Some(len) = self.cue_duration(show, cue) else {
            return ClipTiming::default();
        };
        let clocked = model::clocked(show, key, cue, len);
        let loops_for_ever = m.loop_media && m.loop_count == 0;
        let looped = (clocked
            && m.run_out == RunOut::Loop
            && !loops_for_ever
            && matches!(cue.kind, CueKind::Video(_) | CueKind::Audio(_)))
        .then(|| {
            let mut c = cue.clone();
            if let Some(m) = c.kind.media_mut() {
                m.loop_media = true;
                m.loop_count = 0;
            }
            c
        });
        ClipTiming {
            clocked,
            ends_at: len.map(|l| start_of(cue) + l.max(0.0)),
            looped,
        }
    }

    /// A cue the run `run` has started is its clip: it carries the run, and if
    /// the run's pause holds it (it was waiting for its file), it holds too.
    pub(super) fn timeline_adopt(
        &mut self,
        show: &Show,
        active: &mut ActiveCue,
        run: u64,
        clip: ClipTiming,
    ) {
        active.run = Some(run);
        active.clocked = clip.clocked;
        active.ends_at = clip.ends_at;
        let paused = self.paused;
        let Some((key, r)) = self.runs.by_instance(run) else {
            return;
        };
        let RunState::Paused { freeze, .. } = r.state else {
            return;
        };
        let live = is_live(show, active.cue_id);
        let holds = match freeze {
            PauseFreeze::Nothing => false,
            PauseFreeze::Everything => true,
            PauseFreeze::Clips => r.pause_clips.contains(&active.cue_id),
        } && (!live || r.pause_live);
        if holds {
            active.freeze(live, paused);
            if let Some(r) = self.runs.map.get_mut(&key) {
                r.frozen.push(active.instance);
            }
        } else if show
            .cue(active.cue_id)
            .is_some_and(|c| c.kind.media().is_some())
        {
            detach(active);
        }
    }

    /// Stop the clock. `clips` only matters for `PauseFreeze::Clips`. `live`:
    /// live inputs among the frozen clips freeze too (a camera holds its frame,
    /// live sound mutes).
    pub fn timeline_pause(
        &mut self,
        show: &Show,
        key: TimelineKey,
        freeze: PauseFreeze,
        clips: &[CueId],
        live: bool,
        waits_for_go: bool,
    ) {
        let paused = self.paused;
        let Some(run) = self.runs.map.get_mut(&key) else {
            self.log("The timeline is not running");
            return;
        };
        let waits_for_go = match run.state {
            RunState::Running => waits_for_go,
            // Paused again (the transport's pause on a cue's): it holds more.
            RunState::Paused {
                waits_for_go: was, ..
            } => was || waits_for_go,
            RunState::Stopped | RunState::Cueing => {
                self.log("The timeline is not running");
                return;
            }
        };
        run.state = RunState::Paused {
            freeze,
            waits_for_go,
        };
        run.pause_clips = clips.to_vec();
        run.pause_live = live;
        let instance = run.instance;
        let mut frozen = Vec::new();
        for a in &mut self.active {
            if a.run != Some(instance) || a.is_matte() {
                continue;
            }
            let Some(cue) = show.cue(a.cue_id) else {
                continue;
            };
            let is_live = is_live(show, a.cue_id);
            let holds = match freeze {
                PauseFreeze::Nothing => false,
                PauseFreeze::Everything => true,
                PauseFreeze::Clips => clips.contains(&a.cue_id),
            } && (!is_live || live);
            if a.hold {
                // Frozen already (by its own Pause, or this run's): left as it is.
            } else if holds {
                a.freeze(is_live, paused);
                frozen.push(a.instance);
            } else if cue.kind.media().is_some() && !a.stopping {
                // Its content runs on while the clock stands.
                detach(a);
            }
        }
        if let Some(run) = self.runs.map.get_mut(&key) {
            run.frozen.extend(frozen);
        }
    }

    /// Run a paused run on: what its pause froze goes on, and the clock starts
    /// what it had stopped at.
    fn timeline_resume(&mut self, show: &Show, key: TimelineKey) {
        let paused = self.paused;
        let Some(run) = self.runs.map.get_mut(&key) else {
            return;
        };
        if !matches!(run.state, RunState::Paused { .. }) {
            return;
        }
        run.state = RunState::Running;
        run.pause_clips.clear();
        run.pause_live = false;
        let frozen = std::mem::take(&mut run.frozen);
        for a in &mut self.active {
            if frozen.contains(&a.instance) {
                a.unfreeze(paused);
            }
        }
        self.timeline_step(show, key, 0.0);
    }

    /// Stop its clips (fade seconds; 0 = cut), keep the playhead where it is.
    pub fn timeline_stop(&mut self, _show: &Show, key: TimelineKey, fade: f64) {
        let Some(run) = self.runs.map.get(&key) else {
            return;
        };
        let instance = run.instance;
        let clips: Vec<u64> = self
            .active
            .iter()
            .filter(|a| a.run == Some(instance))
            .map(|a| a.instance)
            .collect();
        self.timeline_stopped(instance);
        // A group goes with its children.
        if matches!(key, TimelineKey::Group(_))
            && self.active.iter().any(|a| a.instance == instance)
        {
            self.stop_instance(instance, fade);
        }
        for i in clips {
            self.stop_instance(i, fade);
        }
    }

    /// GO for the cue list `list`: on a timeline list, run it (or resume it;
    /// while it runs GO does nothing). Otherwise a timeline group of this list
    /// waiting at a pause cue of its own is resumed. Whether GO was taken.
    pub(super) fn timeline_go(&mut self, show: &Show, list: u32) -> bool {
        let key = TimelineKey::List(list);
        if model::is_timeline(show, key) {
            match self.runs.map.get(&key).map(|r| r.state) {
                Some(RunState::Running) => {
                    self.log("GO: the timeline is running; GO does nothing until it pauses")
                }
                Some(RunState::Cueing) => {}
                _ => {
                    // GO lets go of the show's pause, as firing a cue does.
                    if self.paused {
                        self.set_paused(false);
                    }
                    self.timeline_run(show, key);
                }
            }
            return true;
        }
        let waiting = self
            .runs
            .map
            .iter()
            .filter(|(k, r)| {
                matches!(k, TimelineKey::Group(g) if show.cue(*g).is_some_and(|c| c.list == list))
                    && matches!(
                        r.state,
                        RunState::Paused {
                            waits_for_go: true,
                            ..
                        }
                    )
                    && self.active.iter().any(|a| a.instance == r.instance)
            })
            .max_by_key(|(_, r)| r.instance)
            .map(|(k, _)| *k);
        let Some(key) = waiting else {
            return false;
        };
        if let TimelineKey::Group(g) = key
            && let Some(c) = show.cue(g)
        {
            log::info!("GO resumes {} ({})", c.number, c.name);
        }
        if self.paused {
            self.set_paused(false);
        }
        self.timeline_resume(show, key);
        true
    }

    /// A control cue's timeline action (on its own timeline, or the one it
    /// names), and a Pause of a timeline group (which freezes live inputs if the
    /// cue says). Whether it was one.
    pub(super) fn timeline_control(&mut self, show: &Show, cue: &Cue, c: &ControlCue) -> bool {
        let name = format!("{} ({})", cue.number, cue.name);
        if c.action == ControlAction::Pause
            && let Some(target) = c.target
        {
            return self.timeline_route(show, c.action, target, c.freeze_live, &name);
        }
        if !c.action.is_timeline() {
            return false;
        }
        let own = model::key_of(show, cue.id);
        let Some(key) = c.timeline.or(own) else {
            self.problem(format!(
                "Cue {name}: it is not on a timeline and names none"
            ));
            return true;
        };
        if !model::is_timeline(show, key) {
            self.problem(format!("Cue {name}: the timeline it names is not there"));
            return true;
        }
        match c.action {
            ControlAction::TimelineRun => {
                if c.at.is_some() {
                    self.log(format!(
                        "Cue {name}: running from a chosen point is not built yet; \
                         the timeline runs from where it is"
                    ));
                }
                self.timeline_run(show, key);
            }
            ControlAction::TimelinePause => {
                // A pause on its own timeline waits for GO there.
                self.timeline_pause(
                    show,
                    key,
                    c.freeze,
                    &c.clips,
                    c.freeze_live,
                    own == Some(key),
                );
            }
            ControlAction::TimelineStop => self.timeline_stop(show, key, 0.0),
            ControlAction::TimelineJump => self.log(format!(
                "Cue {name}: jumping in a timeline is not built yet"
            )),
            _ => {}
        }
        true
    }

    /// `action` aimed at `target`, when that is a timeline group: its Pause
    /// freezes the run (clock and all), Resume runs it on, Reset forgets it too,
    /// and the timeline actions are its run's. Whether that was all there was
    /// to do (false: do as for any cue).
    pub(super) fn timeline_route(
        &mut self,
        show: &Show,
        action: ControlAction,
        target: CueId,
        live: bool,
        from: &str,
    ) -> bool {
        let key = TimelineKey::Group(target);
        let is_timeline = model::is_timeline(show, key);
        if action.is_timeline() {
            if !is_timeline {
                self.problem(format!("Cue {from}: its target is not a timeline"));
                return true;
            }
            match action {
                ControlAction::TimelineRun => self.timeline_run(show, key),
                ControlAction::TimelinePause => {
                    self.timeline_pause(show, key, PauseFreeze::Everything, &[], live, false)
                }
                ControlAction::TimelineStop => self.timeline_stop(show, key, 0.0),
                _ => self.log(format!(
                    "Cue {from}: jumping in a timeline is not built yet"
                )),
            }
            return true;
        }
        if !is_timeline {
            return false;
        }
        // Its run, if the group is playing.
        let state = self
            .runs
            .map
            .get(&key)
            .filter(|r| self.active.iter().any(|a| a.instance == r.instance))
            .map(|r| r.state);
        match action {
            ControlAction::Pause => match state {
                Some(RunState::Running | RunState::Paused { .. }) => {
                    self.timeline_pause(show, key, PauseFreeze::Everything, &[], live, false);
                    true
                }
                _ => false,
            },
            ControlAction::Resume => match state {
                Some(RunState::Paused { .. }) => {
                    self.timeline_resume(show, key);
                    true
                }
                // Running: what was held on its own is let go, as for any group.
                _ => false,
            },
            ControlAction::Reset => {
                if let Some(run) = self.runs.map.remove(&key) {
                    self.cancel_run(&run);
                }
                false
            }
            _ => false,
        }
    }

    // For the editor (`ui/timeline.rs`), which doesn't call it yet.
    #[allow(dead_code)]
    /// Move the playhead. Keeps Running running; otherwise parks, with the picture
    /// (clips started held) when `show_picture`, without when not.
    pub fn timeline_seek(
        &mut self,
        _show: &Show,
        _key: TimelineKey,
        _pos: f64,
        _show_picture: bool,
    ) {
        // Starting part-way in comes with the next package.
        self.log("Moving a timeline's playhead is not built yet");
    }

    // For the editor (`ui/timeline.rs`), which doesn't call it yet.
    #[allow(dead_code)]
    /// The clips of a run that are free-running, for the editor.
    pub fn timeline_detached(&self, key: TimelineKey) -> Vec<CueId> {
        let Some(run) = self.runs.map.get(&key) else {
            return Vec::new();
        };
        self.active
            .iter()
            .filter(|a| a.run == Some(run.instance) && a.detached && !a.stopping)
            .filter(|a| !matches!(a.kind, ActiveKind::Matte(_)))
            .map(|a| a.cue_id)
            .collect()
    }

    // For the editor (`ui/timeline.rs`), which doesn't call it yet.
    #[allow(dead_code)]
    /// The clips of a run waiting to fade in, for the editor.
    pub fn timeline_waiting(&self, _key: TimelineKey) -> Vec<CueId> {
        // Clips that wait for their fade-in come with the next package.
        Vec::new()
    }
}

#[cfg(test)]
mod tests;
