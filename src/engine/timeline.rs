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
//! Starting part way in (plan 2.6, 2.7): a seek works out what plays at the
//! new time (`timeline::chase`), lets go of what the run had, starts the clips
//! alive there held, part way into their content, and leaves the rest waiting.
//! A running run then *cues* (`RunState::Cueing`): its clock stands until every
//! clip has its first frame or `CUE_LIMIT` has passed, and all are let go on one
//! tick. A run that was not running is *parked*: paused there, its clips held on
//! the real outputs (the picture at the playhead), and GO or Run lets them go.
//! A Jump cue and a Run cue with a place (`ControlCue::at`) are seeks.
//!
//! A Stop (the cue, or the editor's) stops the clock at once and fades what the
//! run plays out over the cue's fade (`ControlCue::stop_fade`, 3 s unless set;
//! 0 cuts); a second Stop meanwhile cuts. Running the timeline again during the
//! fade does what it does once it has stopped, the fading clips going on fading
//! under what comes in.
//!
//! Ramps follow the clock too (ruling L2): a fade a run starts (a clip's own
//! fade-in, a fade cue it fires, a crossfade's or a dip's, a fade-out by its
//! clock) carries the run (`Param::follow`) and moves only as far as that run's
//! clock moves (`Runs::steps`), so it waits at a pause and goes on at GO. A fade
//! started any other way (by hand, by a cue fired off the timeline, a Stop's)
//! carries none and moves with the show's time, as does every ramp once its run
//! has stopped or gone. Content that runs out while such a fade waits stays on
//! its last frame (or silent) for it.
//!
//! A clip at opacity 0 (or a sound at silence) on a timeline with
//! `start_on_fade_in` waits, unseen, for its fade-in (`model::lead_in`): its
//! content starts from its in point when its opacity keys rise (at its lead-in,
//! clip time V), or when a fade cue raises it (`timeline_wake`), and its own
//! time (`elapsed`) is V then, so its keyframes stay where they were drawn.

use super::{
    ActiveCue, ActiveKind, ClockStep, ClockSteps, Engine, LiveTransform, Param, PendingCue, StartAt,
};
use crate::show::{
    ControlAction, ControlCue, Cue, CueId, CueKind, Curve, FadeCue, GroupCue, GroupMode, MIN_DB,
    MediaCue, PauseFreeze, RunOut, Show, TIMELINE_STOP_FADE, TimelineKey,
};
use crate::timeline::chase::{self, Alive, Waiting};
use crate::timeline::model::{self, LeadIn};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// Times closer than this are the same time.
pub(super) const SAME_TIME: f64 = 1e-9;
/// How far ahead of a timeline's clock its clips are got ready (decoders
/// started), at most `MAX_PRELOAD_CHILDREN` of them.
pub(super) const PRELOAD_HORIZON: f64 = 10.0;
/// A guard: no run starts more than this many cues in one step.
const MAX_STARTS: usize = 10_000;
/// How long a seek's clips are given to have their first frames before the
/// clock runs on without the slow ones.
const CUE_LIMIT: Duration = Duration::from_secs(2);
/// A guard: jumps (and runs from a place) one frame may make; more is a loop
/// of jumps that would never let the frame end.
const MAX_JUMPS: u32 = 8;
/// A video that holds its last frame, started at its end, is started this long
/// before it so that it has a frame to hold.
const LAST_FRAME: f64 = 0.2;

/// Where a timeline's run is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RunState {
    /// The clock still, nothing playing.
    Stopped,
    /// Clips started held, waiting for all to be ready; the clock still.
    Cueing,
    Running,
    /// The clock still; `freeze` says what holds. `waits_for_go`: GO runs it on.
    Paused {
        freeze: PauseFreeze,
        waits_for_go: bool,
    },
}

/// A timeline's run, for the editor and the transport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RunView {
    pub key: TimelineKey,
    pub state: RunState,
    pub pos: f64,
    pub next_pause: Option<f64>,
    pub length: Option<f64>,
    /// What GO will bring in or fade, for the read-out.
    pub on_go: Option<CueId>,
    /// Paused where a seek (the ruler, a marker, a Jump) put it, or where a
    /// pause caught it cueing: GO or Run cues what is there and plays on.
    pub parked: bool,
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
    /// Parked by a seek: paused at the playhead, its clips (if any) held there.
    /// Going on from here cues them first, if they are not all ready.
    parked: bool,
    /// When it began to cue (`RunState::Cueing`).
    cue_since: Option<Instant>,
    /// The clips a seek without the picture left unstarted: they are started
    /// when the run goes on.
    unstarted: Vec<Alive>,
}

impl Run {
    fn new(instance: u64, state: RunState, pos: f64) -> Run {
        Run {
            instance,
            state,
            pos,
            frozen: Vec::new(),
            pause_clips: Vec::new(),
            pause_live: false,
            parked: false,
            cue_since: None,
            unstarted: Vec::new(),
        }
    }

    /// Stopped where it is.
    fn stop(&mut self) {
        self.state = RunState::Stopped;
        self.frozen.clear();
        self.parked = false;
        self.cue_since = None;
        self.unstarted.clear();
    }
}

/// The runs of the timelines that have been touched.
#[derive(Default)]
pub struct Runs {
    map: HashMap<TimelineKey, Run>,
    /// Jumps made this frame (`MAX_JUMPS`).
    jumps: u32,
}

/// What a run placed at a new time does there.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Then {
    /// Cue its clips, then run.
    Run,
    /// Park: paused there, with its clips held on the outputs (`picture`) or not started.
    Park { picture: bool },
}

impl Runs {
    /// Each run's clock, by its instance.
    pub(super) fn clocks(&self) -> HashMap<u64, f64> {
        self.map.values().map(|r| (r.instance, r.pos)).collect()
    }

    /// The clocks of the runs that go on (not stopped) on this tick: where each
    /// is and how far it moved from `before` (at most `dt`; nothing for a run
    /// that wasn't there). The ramps a run started move as far (ruling L2); a
    /// stopped run's, or one that has gone, move with the show's time.
    pub(super) fn steps(&self, before: &HashMap<u64, f64>, dt: f64) -> ClockSteps {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        self.map
            .values()
            .filter(|r| r.state != RunState::Stopped)
            .map(|r| {
                let moved = before
                    .get(&r.instance)
                    .map_or(0.0, |b| (r.pos - b).max(0.0).min(dt));
                (r.instance, ClockStep { pos: r.pos, moved })
            })
            .collect()
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
            r.stop();
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

/// Where on its timeline a clip's own time is 0: its start, or for a picture
/// that dips in, half way through the dip, where it is started (the chase's
/// `zero`). Its keyframes are read at the clock less this.
pub(super) fn zero_of(cue: &Cue) -> f64 {
    let half = match &cue.kind {
        CueKind::Video(m) | CueKind::Image(m) => Some(m),
        CueKind::Text(t) => Some(&t.media),
        CueKind::Shape(s) => Some(&s.media),
        CueKind::Camera(c) => Some(&c.media),
        _ => None,
    }
    .filter(|m| m.transition.dip_color().is_some())
    .map_or(0.0, |m| (m.fade_in / 2.0).max(0.0));
    start_of(cue) + half
}

/// A clip's lead-in (plan 2.5) as its timeline would play it: where its opacity
/// keys rise, or the first fade cue that raises it; 0 for a cue on no timeline,
/// or one that doesn't wait.
pub(super) fn lead_of(show: &Show, cue: &Cue) -> f64 {
    let Some(key) = model::key_of(show, cue.id) else {
        return 0.0;
    };
    match model::lead_in(show, key, cue) {
        LeadIn::None => 0.0,
        LeadIn::At(v) => v.max(0.0),
        LeadIn::UntilFaded => model::fade_wake(show, key, cue).unwrap_or(0.0),
    }
}

/// A seek's clip as the chase found it: its levels (fades in progress go on over
/// what is left), where fade cues moved its picture, and a fade-and-stop going.
pub(super) fn chase_levels(a: &mut ActiveCue, m: &MediaCue, ch: &Alive) {
    a.faded = ch.faded;
    if let Some((now, to, left, curve)) = ch.opacity {
        a.opacity = Param::new(now);
        a.opacity.ramp_to(to, left, curve);
    }
    if let Some((now, to, left, curve)) = ch.volume_db {
        a.volume = Param::new(now);
        a.volume.ramp_to(to, left, curve);
    }
    if let Some(g) = ch.geometry {
        let mut lt = LiveTransform::from(&m.motion.transform_at(&m.transform, a.key_time()));
        for (param, now, to) in [
            (&mut lt.x, g.now.x, g.to.x),
            (&mut lt.y, g.now.y, g.to.y),
            (&mut lt.scale_x, g.now.scale_x, g.to.scale_x),
            (&mut lt.scale_y, g.now.scale_y, g.to.scale_y),
            (&mut lt.rotation, g.now.rotation, g.to.rotation),
        ] {
            if let (Some(now), Some(to)) = (now, to) {
                *param = Param::new(now);
                param.ramp_to(to, g.left, g.curve);
            }
        }
        a.live_transform = Some(lt);
    }
    if let Some(left) = ch.stop_after {
        if !a.opacity.ramping() {
            let v = a.opacity.value;
            a.opacity.ramp_to(v, left, Curve::Linear);
        }
        a.stopping = true;
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

/// Fade a clip out over `secs`, from where it is. Begun while its clock runs
/// (`clock`: the run and where its clock is), the fade is the clock's and waits
/// if it stops.
fn fade_away(a: &mut ActiveCue, m: &MediaCue, secs: f64, clock: Option<(u64, f64)>) {
    a.auto_faded = true;
    a.opacity.ramp_to(0.0, secs.max(0.0), m.curve);
    a.volume.ramp_to(MIN_DB, secs.max(0.0), m.curve);
    a.opacity.follow(clock);
    a.volume.follow(clock);
}

/// A clocked clip, each tick (`on_clock`: how far its timeline's clock has to go
/// to the end of its slot; `clock`: its run and where its clock is, while it is
/// running). Its own fade-out
/// begins that long before the clock reaches the end, and it ends there; neither
/// while the clock stands (a pause right at the fade-out's start holds it).
/// Content that has run ahead of the clock and will run out first, set to end
/// (`RunOut::End`), fades out to finish as it runs out. Returns whether it is over.
pub(super) fn clock_end(
    a: &mut ActiveCue,
    m: &MediaCue,
    on_clock: Option<f64>,
    clock: Option<(u64, f64)>,
) -> bool {
    let moving = clock.is_some();
    if a.detached
        && m.run_out == RunOut::End
        && !a.auto_faded
        && let (Some(d), Some(to_go)) = (a.duration, on_clock)
    {
        let left = d - a.elapsed;
        if left < to_go - SAME_TIME && left <= run_out_fade(m) {
            // Begun while the clock stands, it follows the content, which runs
            // on: it fades as the content ends (ruling 5).
            fade_away(a, m, left, clock);
        }
    }
    let Some(left) = on_clock.filter(|_| moving) else {
        return false;
    };
    if !a.auto_faded && m.fade_out > 0.0 && left <= m.fade_out {
        fade_away(a, m, left, clock);
    }
    // A video holding its last frame for good (no fade-out) keeps it, as it would
    // off a timeline.
    left <= SAME_TIME && !(a.held && m.hold_last_frame && !a.auto_faded)
}

/// Whether `cue` is a timeline pause of the timeline `key` (it is one of its cues).
fn is_pause_of(cue: &Cue, key: TimelineKey) -> bool {
    matches!(&cue.kind, CueKind::Control(c)
        if c.action == ControlAction::TimelinePause && c.timeline.is_none_or(|k| k == key))
}

impl Engine {
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
                parked: false,
            };
        };
        let (state, pos, instance, parked) = (run.state, run.pos, run.instance, run.parked);
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
            parked,
        }
    }

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
    /// list from its playhead (what is under it cued first).
    fn timeline_start(&mut self, show: &Show, key: TimelineKey) {
        if !model::is_timeline(show, key) {
            self.problem("There is no such timeline");
            return;
        }
        match key {
            TimelineKey::Group(id) => self.fire(show, id),
            TimelineKey::List(_) => {
                let pos = self.runs.map.get(&key).map_or(0.0, |r| r.pos);
                // As firing a cue does, running lets go of the show's pause.
                if self.paused {
                    self.set_paused(false);
                }
                if pos > 0.0 {
                    if !self.timeline_place(show, key, pos, Then::Run) {
                        self.log("GO: the timeline is at its end");
                    }
                    return;
                }
                let instance = self.alloc_instance();
                self.timeline_begin(show, key, instance);
                self.timeline_step(show, key, 0.0);
            }
        }
    }

    /// A new run of `key` from its top (instance `instance`): its clips wait for
    /// its clock. An earlier run of it gives up what it was still waiting to
    /// start (and a parked one its clips).
    pub(super) fn timeline_begin(&mut self, show: &Show, key: TimelineKey, instance: u64) {
        if let Some(old) = self.runs.map.remove(&key) {
            if old.parked {
                self.clear_clips(old.instance, false);
            }
            self.cancel_run(&old);
        }
        // Nothing has come in at the top: no length is asked for.
        let ch = chase::chase(show, key, 0.0, &mut |_| None);
        self.push_upcoming(show, instance, 0.0, &ch.upcoming);
        self.runs
            .map
            .insert(key, Run::new(instance, RunState::Running, 0.0));
    }

    /// What a run waits to start, waiting for its clock (or a fade cue).
    fn push_upcoming(&mut self, show: &Show, instance: u64, pos: f64, upcoming: &[Waiting]) {
        for w in upcoming {
            let Some(cue) = show.cue(w.id) else {
                continue;
            };
            // Fired: no longer waiting to be got ready.
            self.loads.remove(&w.id);
            // One waiting for a fade cue is due by that, not by the clock; its
            // place is where the first fade that raises it is, for getting it ready.
            let (at, waits_for_fade) = match w.at {
                Some(at) => (at, false),
                None => (start_of(cue) + w.lead, true),
            };
            self.pending.push(PendingCue {
                cue_id: w.id,
                remaining: (at - pos).max(0.0),
                total: at,
                reading: None,
                run: Some(instance),
                at,
                waits_for_fade,
                start: StartAt {
                    run: Some(instance),
                    together: Some(instance),
                    from: w.lead,
                    lead: w.lead,
                    ..StartAt::default()
                },
            });
        }
    }

    /// Everything of the run `instance`: its clips playing (and a dip's matte
    /// on its way to one), and what it was waiting to start, goes at once.
    /// `fading`: those fading out (a Stop's fade) go on fading.
    fn clear_clips(&mut self, instance: u64, fading: bool) {
        let i = Some(instance);
        self.pending.retain(|p| p.run != i && p.start.run != i);
        self.active.retain(|a| {
            (fading && a.stopping)
                || (a.run != i && !matches!(&a.kind, ActiveKind::Matte(m) if m.start.run == i))
        });
    }

    /// Open the timeline group `g` for a seek: the group is on, with a run the
    /// seek sets and nothing of it started. Its instance, if it could be.
    fn timeline_open(&mut self, show: &Show, g: CueId) -> Option<u64> {
        let first = self.next_instance;
        self.start_at(
            show,
            g,
            StartAt {
                hold: true,
                ..StartAt::default()
            },
        );
        self.active
            .iter()
            .find(|a| {
                a.cue_id == g && a.instance >= first && matches!(a.kind, ActiveKind::Group(_))
            })
            .map(|a| a.instance)
    }

    /// Put the run of `key` at `pos` (plan 2.6): what it had playing or waiting
    /// goes, the clips alive at `pos` are started held, part way in, and the rest
    /// wait for the clock. `then` says whether it then cues and runs, or parks.
    /// False if there is nothing there or after it to run (it stays stopped there).
    fn timeline_place(&mut self, show: &Show, key: TimelineKey, pos: f64, then: Then) -> bool {
        let pos = if pos.is_finite() { pos.max(0.0) } else { 0.0 };
        let instance = match key {
            TimelineKey::Group(g) => {
                let playing = self
                    .runs
                    .map
                    .get(&key)
                    .map(|r| r.instance)
                    .filter(|i| self.active.iter().any(|a| a.instance == *i));
                match playing.or_else(|| self.timeline_open(show, g)) {
                    Some(i) => i,
                    None => {
                        self.log("The timeline has nothing in it to play");
                        return false;
                    }
                }
            }
            TimelineKey::List(_) => self.alloc_instance(),
        };
        if let Some(old) = self.runs.map.remove(&key) {
            // Run again while a Stop's fade is going: that goes on under what
            // comes in (which replaces it on its layer as it always would).
            self.clear_clips(old.instance, old.state == RunState::Stopped);
        }
        self.clear_clips(instance, false);
        let ch = chase::chase(show, key, pos, &mut |c| self.cue_duration(show, c));
        let run = then == Then::Run;
        if run && ch.alive.is_empty() && ch.upcoming.is_empty() {
            self.runs
                .map
                .insert(key, Run::new(instance, RunState::Stopped, pos));
            // A group with nothing left to play is over.
            if matches!(key, TimelineKey::Group(_)) {
                self.active.retain(|a| a.instance != instance);
            }
            return false;
        }
        self.push_upcoming(show, instance, pos, &ch.upcoming);
        let picture = match then {
            Then::Run => true,
            Then::Park { picture } => picture,
        };
        let mut r = Run::new(
            instance,
            if run {
                RunState::Cueing
            } else {
                RunState::Paused {
                    freeze: PauseFreeze::Everything,
                    waits_for_go: true,
                }
            },
            pos,
        );
        r.parked = !run;
        r.cue_since = run.then(Instant::now);
        if !picture {
            r.unstarted = ch.alive.clone();
        }
        self.runs.map.insert(key, r);
        if picture {
            self.start_alive(show, instance, ch.alive);
        }
        self.refresh_waits();
        if run && self.run_cued(instance) {
            self.timeline_release(show, key);
        }
        true
    }

    /// Start a seek's clips, held, each as the chase found it.
    fn start_alive(&mut self, show: &Show, instance: u64, alive: Vec<Alive>) {
        for a in alive {
            let id = a.id;
            self.start_at(
                show,
                id,
                StartAt {
                    run: Some(instance),
                    together: Some(instance),
                    from: a.from,
                    lead: a.lead,
                    hold: true,
                    chase: Some(a),
                },
            );
        }
    }

    /// Whether every clip of the run `instance` that has been started has its
    /// first frame (or sound): none waiting for its file, none still starting.
    fn run_cued(&self, instance: u64) -> bool {
        let i = Some(instance);
        !self
            .pending
            .iter()
            .any(|p| p.run.is_none() && p.start.run == i)
            && self
                .active
                .iter()
                .all(|a| a.run != i || a.started || a.is_matte())
    }

    /// A cueing run goes: its clips are let go together and its clock runs.
    fn timeline_release(&mut self, show: &Show, key: TimelineKey) {
        let paused = self.paused;
        let Some(run) = self.runs.map.get_mut(&key) else {
            return;
        };
        if run.state != RunState::Cueing {
            return;
        }
        run.state = RunState::Running;
        run.cue_since = None;
        let frozen = std::mem::take(&mut run.frozen);
        for a in &mut self.active {
            if frozen.contains(&a.instance) {
                a.unfreeze(paused);
            }
        }
        self.timeline_step(show, key, 0.0);
    }

    /// Move the playhead. Keeps Running running (its clips cued afresh first);
    /// otherwise parks, with the picture (clips started held) when
    /// `show_picture`, without when not.
    pub fn timeline_seek(&mut self, show: &Show, key: TimelineKey, pos: f64, show_picture: bool) {
        if !model::is_timeline(show, key) {
            self.problem("There is no such timeline");
            return;
        }
        let running = self.runs.map.get(&key).is_some_and(|r| {
            matches!(r.state, RunState::Running | RunState::Cueing)
                && match key {
                    TimelineKey::Group(_) => self.active.iter().any(|a| a.instance == r.instance),
                    TimelineKey::List(_) => true,
                }
        });
        let then = if running {
            Then::Run
        } else {
            Then::Park {
                picture: show_picture,
            }
        };
        if !self.timeline_place(show, key, pos, then) && running {
            self.log("The timeline has nothing there or after it to play");
        }
    }

    /// A Jump (or a Run from a place) by a cue named `from`: at most
    /// `MAX_JUMPS` in a frame. Whether it was made.
    fn may_jump(&mut self, from: &str) -> bool {
        self.runs.jumps += 1;
        if self.runs.jumps > MAX_JUMPS {
            if self.runs.jumps == MAX_JUMPS + 1 {
                self.problem(format!(
                    "Cue {from}: the timeline has jumped {MAX_JUMPS} times in one frame; \
                     this jump is not made"
                ));
            }
            return false;
        }
        true
    }

    /// Run from `pos`: cue what is there and run on from it.
    fn timeline_run_from(&mut self, show: &Show, key: TimelineKey, pos: f64) {
        if self.paused {
            self.set_paused(false);
        }
        if !self.timeline_place(show, key, pos, Then::Run) {
            self.log("The timeline has nothing there or after it to play");
        }
    }

    /// A fade cue is raising `targets` (its target and a group's children): the
    /// timeline clips among them that wait for it start now, their content from
    /// its in point and their own time at their lead-in (where the clock is,
    /// less their start), so the fade then brings them up. Whether any did.
    pub(super) fn timeline_wake(&mut self, show: &Show, f: &FadeCue, targets: &[CueId]) -> bool {
        if !self.pending.iter().any(|p| p.waits_for_fade) {
            return false;
        }
        let clocks = self.runs.clocks();
        let mut woken = Vec::new();
        self.pending.retain(|p| {
            if !p.waits_for_fade || !targets.contains(&p.cue_id) {
                return true;
            }
            let Some(cue) = show.cue(p.cue_id) else {
                return true;
            };
            let sound = matches!(cue.kind, CueKind::Audio(_) | CueKind::LiveAudio(_));
            let raises = if sound {
                f.volume_db.is_some_and(|v| v > MIN_DB)
            } else {
                f.opacity.is_some_and(|o| o > 0.0)
            };
            if !raises {
                return true;
            }
            let pos = p.run.and_then(|r| clocks.get(&r)).copied().unwrap_or(0.0);
            woken.push((p.cue_id, p.start.clone(), (pos - start_of(cue)).max(0.0)));
            false
        });
        let any = !woken.is_empty();
        for (id, mut at, v) in woken {
            at.from = v;
            at.lead = v;
            self.start_at(show, id, at);
        }
        any
    }

    /// The run `run` and where its clock is, for a ramp it starts to follow
    /// (`Param::follow`); None for no run, or one that has stopped or gone.
    pub(super) fn run_clock(&self, run: Option<u64>) -> Option<(u64, f64)> {
        let (_, r) = self.runs.by_instance(run?)?;
        (r.state != RunState::Stopped).then_some((r.instance, r.pos))
    }

    /// A media cue the run `run` is starting: where its own time is 0 on the run's
    /// clock (`zero_of`), and its time by that clock now. None for any other cue,
    /// or a run that is gone.
    pub(super) fn timeline_clock_time(&self, cue: &Cue, run: u64) -> Option<(f64, f64)> {
        cue.kind.media()?;
        let (_, r) = self.runs.by_instance(run)?;
        let zero = zero_of(cue);
        Some((zero, (r.pos - zero).max(0.0)))
    }

    /// The clip time a clip about to be got ready is to follow its keyframed
    /// speed from: a timeline's clip waiting out its lead-in, from its lead-in.
    pub(super) fn timeline_preload_t0(&self, id: CueId) -> f64 {
        self.pending
            .iter()
            .find(|p| p.cue_id == id && p.run.is_some())
            .filter(|p| (p.start.from - p.start.lead).abs() < 1e-9)
            .map_or(0.0, |p| p.start.lead)
    }

    /// A clip started part way in (`at.part_way()`): past the end of its content
    /// a file has nothing to play (None), unless it is a video holding its last
    /// frame, which starts just before its end to have that frame to show.
    pub(super) fn part_way_start(
        &mut self,
        show: &Show,
        cue: &Cue,
        at: StartAt,
    ) -> Option<StartAt> {
        if !at.part_way() || !matches!(cue.kind, CueKind::Video(_) | CueKind::Audio(_)) {
            return Some(at);
        }
        let Some(Some(len)) = self.cue_duration(show, cue) else {
            // Open-ended, or not known yet: it plays and ends by itself.
            return Some(at);
        };
        let end = at.lead + len;
        if at.from < end - LAST_FRAME {
            return Some(at);
        }
        let holds_last = matches!(&cue.kind, CueKind::Video(m) if m.hold_last_frame);
        if !holds_last {
            log::debug!(
                "cue {} has nothing left to play where it is started",
                cue.number
            );
            return None;
        }
        let mut at = at;
        at.from = (end - LAST_FRAME).max(at.lead);
        Some(at)
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
                .filter(|(_, p)| {
                    p.run == Some(instance) && !p.waits_for_fade && p.at <= target + SAME_TIME
                })
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

    /// The runs' clocks move on with the show (not while it is paused). A run
    /// cueing goes once its clips are ready, or have had `CUE_LIMIT`.
    pub(super) fn timeline_tick(&mut self, show: &Show, dt: f64) {
        self.runs.jumps = 0;
        if self.paused {
            return;
        }
        let cueing: Vec<(TimelineKey, u64, Option<Instant>)> = self
            .runs
            .map
            .iter()
            .filter(|(_, r)| r.state == RunState::Cueing)
            .map(|(k, r)| (*k, r.instance, r.cue_since))
            .collect();
        for (key, instance, since) in cueing {
            let cued = self.run_cued(instance);
            if cued || since.is_none_or(|s| s.elapsed() >= CUE_LIMIT) {
                if !cued {
                    self.log("Not every clip was ready in time; the timeline runs on without them");
                }
                self.timeline_release(show, key);
            }
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
    pub(super) fn timeline_after_tick(&mut self, show: &Show) {
        // Clips waiting for a fade cue that their run will no longer reach never start.
        if self.pending.iter().any(|p| p.waits_for_fade) {
            let fades: HashSet<u64> = self
                .pending
                .iter()
                .filter(|p| {
                    !p.waits_for_fade
                        && show
                            .cue(p.cue_id)
                            .is_some_and(|c| matches!(c.kind, CueKind::Fade(_)))
                })
                .filter_map(|p| p.run)
                .collect();
            self.pending
                .retain(|p| !p.waits_for_fade || p.run.is_some_and(|r| fades.contains(&r)));
        }
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
            .any(|p| (p.run == Some(i) && !p.waits_for_fade) || p.start.run == Some(i))
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
            run.stop();
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
                // A parked run's clips were held for it alone: they go too.
                if run.parked || run.state == RunState::Cueing {
                    self.clear_clips(run.instance, false);
                }
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
            // (A start part way in waiting for its file decodes from its own place.)
            .filter(|p| !(p.reading.is_some() && p.start.part_way()))
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
    /// `lead`: its lead-in, as it is started (its slot is that much longer).
    pub(super) fn timeline_clip(&mut self, show: &Show, cue: &Cue, lead: f64) -> ClipTiming {
        let Some(key) = model::key_of(show, cue.id) else {
            return ClipTiming::default();
        };
        let Some(m) = cue.kind.media() else {
            return ClipTiming::default();
        };
        let Some(len) = self.cue_duration(show, cue) else {
            return ClipTiming::default();
        };
        let len = len.map(|l| lead + l.max(0.0));
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

    /// A cue the run `at.run` has started is its clip: it carries the run, and if
    /// the run's pause holds it (it was waiting for its file), it holds too. A
    /// seek's clip (`at.hold`) is held until its run lets it go, or (frozen by a
    /// Pause of its own) until its Resume.
    pub(super) fn timeline_adopt(
        &mut self,
        show: &Show,
        active: &mut ActiveCue,
        at: &StartAt,
        clip: ClipTiming,
    ) {
        let Some(run) = at.run else {
            return;
        };
        active.run = Some(run);
        active.clocked = clip.clocked;
        active.ends_at = clip.ends_at;
        let paused = self.paused;
        let Some((key, r)) = self.runs.by_instance(run) else {
            return;
        };
        if at.hold {
            let own = at.chase.as_ref().is_some_and(|c| c.frozen);
            let detached = at.chase.as_ref().is_some_and(|c| c.detached);
            active.freeze(false, paused);
            if (own || detached)
                && show
                    .cue(active.cue_id)
                    .is_some_and(|c| c.kind.media().is_some())
            {
                detach(active);
            }
            if !own && let Some(r) = self.runs.map.get_mut(&key) {
                r.frozen.push(active.instance);
            }
            return;
        }
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
            // Cueing: it parks there, its clips held; going on cues them again.
            RunState::Cueing => {
                run.parked = true;
                run.cue_since = None;
                waits_for_go
            }
            RunState::Stopped => {
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
        run.pause_clips.clear();
        run.pause_live = false;
        if run.parked {
            // Parked: its clips are cued (those a seek without the picture left
            // are started now) and let go together, at once if they are ready.
            run.parked = false;
            run.state = RunState::Cueing;
            run.cue_since = Some(Instant::now());
            let instance = run.instance;
            let alive = std::mem::take(&mut run.unstarted);
            self.start_alive(show, instance, alive);
            if self.run_cued(instance) {
                self.timeline_release(show, key);
            }
            return;
        }
        run.state = RunState::Running;
        let frozen = std::mem::take(&mut run.frozen);
        for a in &mut self.active {
            if frozen.contains(&a.instance) {
                a.unfreeze(paused);
            }
        }
        self.timeline_step(show, key, 0.0);
    }

    /// Stop its clips, fading them out (picture and sound) over `fade` seconds
    /// (0 = a cut), and keep the playhead where it is. The run stops at once: its
    /// clock stands and nothing more of it starts; it is over when the last of
    /// them has gone. A Stop while an earlier one's fade is still going cuts.
    pub fn timeline_stop(&mut self, _show: &Show, key: TimelineKey, fade: f64) {
        let Some(run) = self.runs.map.get(&key) else {
            return;
        };
        let instance = run.instance;
        let i = Some(instance);
        // Its clips, and a dip's matte on its way to one.
        let theirs = |a: &ActiveCue| {
            a.run == i || matches!(&a.kind, ActiveKind::Matte(m) if m.start.run == i)
        };
        let fading =
            run.state == RunState::Stopped && self.active.iter().any(|a| theirs(a) && a.stopping);
        let fade = if fading || !fade.is_finite() {
            0.0
        } else {
            fade.max(0.0)
        };
        let clips: Vec<u64> = self
            .active
            .iter()
            .filter(|a| theirs(a))
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
            ControlAction::TimelineRun => match c.at {
                None => self.timeline_run(show, key),
                Some(p) => match model::resolve(show, key, p) {
                    Some(t) if self.may_jump(&name) => self.timeline_run_from(show, key, t),
                    Some(_) => {}
                    None => self.problem(format!(
                        "Cue {name}: the place it runs from is not on the timeline"
                    )),
                },
            },
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
            ControlAction::TimelineStop => self.timeline_stop(show, key, c.stop_fade()),
            ControlAction::TimelineJump => match c.at.map(|p| model::resolve(show, key, p)) {
                None => self.problem(format!("Cue {name}: it names no place to jump to")),
                Some(None) => self.problem(format!(
                    "Cue {name}: the place it jumps to is not on the timeline"
                )),
                Some(Some(t)) => {
                    if self.may_jump(&name) {
                        self.timeline_seek(show, key, t, true);
                    }
                }
            },
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
                ControlAction::TimelineStop => self.timeline_stop(show, key, TIMELINE_STOP_FADE),
                _ => self.problem(format!(
                    "Cue {from}: a jump needs a place; use a Jump cue that names one"
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

    /// The clips of a run waiting to fade in, for the editor: those the clock
    /// has reached that wait out their lead-in, and those waiting for a fade cue.
    pub fn timeline_waiting(&self, key: TimelineKey) -> Vec<CueId> {
        let Some(run) = self.runs.map.get(&key) else {
            return Vec::new();
        };
        self.pending
            .iter()
            .filter(|p| p.run == Some(run.instance))
            .filter(|p| {
                p.waits_for_fade
                    || (p.start.lead > 0.0 && run.pos + SAME_TIME >= p.at - p.start.lead)
            })
            .map(|p| p.cue_id)
            .collect()
    }
}

#[cfg(test)]
mod tests;
