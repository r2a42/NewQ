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
//! Overlapping clips crossfade on a timeline with `crossfade_overlaps` (plan
//! 2.9): a clip coming in over an earlier one on its track (a picture's layer
//! on a display they share, a sound's output) crossfades over the overlap
//! (`chase::overlap`), on the displays they share, and the earlier one has no
//! fade-out of its own (`ActiveCue::crossed_by`). Like every ramp a run starts,
//! the crossfade follows the clock.
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
/// A clip that starts no later than this after the end of the one before it on
/// its track abuts it: the one before is kept up until it comes in.
const JOIN_GAP: f64 = 0.05;

/// Where a seek to the start of a timeline goes: a seek to exactly 0 is "from
/// the top" and parks with nothing up (`timeline_seek`), so the editor's ruler
/// and a remote's jump ask for a hair past it, which shows the first frame
/// (more than "the same time", less than a frame).
pub const FIRST_FRAME: f64 = 0.001;
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

/// A timeline playing a section round and round, until GO (a loop pause,
/// `ControlCue::loop_from`, ruling L3).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Looping {
    /// The loop pause cue; the section runs from `from` to it (`to`).
    pub cue: CueId,
    pub from: f64,
    pub to: f64,
    /// GO leaves at once (the cue's `leave_at_once`), rather than letting the
    /// pass playing finish.
    pub at_once: bool,
    /// GO has been pressed: this pass is the last, and the timeline carries
    /// straight on past the cue.
    pub last_pass: bool,
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
    /// Stopped, with what it played still fading out (a Stop's fade).
    pub fading: bool,
    /// Playing a section round and round until GO (a loop pause).
    pub looping: Option<Looping>,
    /// A timeline list stopped at its end (played to it, or run from a place
    /// with nothing after it): GO does nothing (`TimelineGo::AtEnd`).
    pub at_end: bool,
    /// Changes when the run is begun, put at a new place (a seek, a jump, a
    /// loop going back), stopped, paused or let go on: not as its clock runs.
    pub mark: RunMark,
}

/// What `RunView::mark` compares: a playhead drag on the ruler that sees it
/// change while held is cancelled, so it never undoes a remote's seek or GO.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunMark {
    instance: Option<u64>,
    placed: u64,
    /// 0 stopped, 1 cueing or running (one becomes the other by itself), 2 paused.
    state: u8,
    parked: bool,
}

/// What GO does to a timeline (`Engine::go_plan`, which `timeline_go` follows).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TimelineGo {
    /// A stopped timeline list runs from its playhead.
    Start,
    /// A run waiting at a pause goes on.
    Resume,
    /// A parked run cues what is at its playhead and plays on from there.
    Play,
    /// A timeline list that is running: GO does nothing until it pauses.
    Running,
    /// A timeline list getting its clips ready: GO does nothing.
    Cueing,
    /// A run playing a section round and round (a loop pause): GO lets this
    /// pass finish and carries on past the cue, or (`at_once`) leaves at once.
    LeaveLoop { at_once: bool },
    /// A timeline list that has played to its end: GO does nothing until its
    /// playhead is moved.
    AtEnd,
}

/// A clip a timeline's run plays (`Engine::timeline_clips_up`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipUp {
    pub cue: CueId,
    pub instance: u64,
    /// Where on the clock its content was due to begin: its start (half way
    /// into a dip) plus its lead-in.
    pub due: f64,
    /// It has its first frame (or its sound has begun).
    pub up: bool,
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
    /// What the run before a seek played (instances, no longer its run's),
    /// kept up until this one is cued, and since when: its new clips are not
    /// seen until then, so the picture changes all at once (`timeline_settle`).
    outgoing: Vec<u64>,
    outgoing_since: Option<Instant>,
    /// Clips alive on both sides of the seek, jumped where they are (decoders
    /// swapped once they have a frame): seen all along.
    carried: Vec<u64>,
    /// Playing a section round and round (a loop pause).
    looping: Option<Looping>,
    /// Stopped at its end (a list): see `RunView::at_end`.
    at_end: bool,
    /// `Runs::placed` when it was begun or put at a new place (`RunView::mark`).
    placed: u64,
    /// Ticking by a shared clock (`Engine::tick_shared`): the shared time at
    /// which its clock was (or would have been) at 0, while it runs or cues to
    /// run. Its clock is then that time's distance, not a sum of ticks.
    anchor: Option<f64>,
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
            outgoing: Vec::new(),
            outgoing_since: None,
            carried: Vec::new(),
            looping: None,
            at_end: false,
            placed: 0,
            anchor: None,
        }
    }

    /// Stopped where it is.
    fn stop(&mut self) {
        self.state = RunState::Stopped;
        self.frozen.clear();
        self.parked = false;
        self.cue_since = None;
        self.unstarted.clear();
        self.outgoing.clear();
        self.outgoing_since = None;
        self.carried.clear();
        self.looping = None;
    }
}

/// The runs of the timelines that have been touched.
#[derive(Default)]
pub struct Runs {
    map: HashMap<TimelineKey, Run>,
    /// Jumps made this frame (`MAX_JUMPS`).
    jumps: u32,
    /// Counts the times a run has been put at a new place (`timeline_place`):
    /// a clock step that started a jump stops there.
    placed: u64,
    /// Ticking by a shared clock: the shared time a run's clock step has got
    /// to while it starts what it reaches (a jump made there is anchored to it).
    stepping_at: Option<f64>,
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
    /// Ticking by a shared clock, at the start of a tick (`Engine::tick_shared`):
    /// a run that went on since the last (GO, a resume) is anchored to that
    /// tick's time `at`, where its clock stood; one that stands is not anchored.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn anchor_runs(&mut self, at: f64) {
        for r in self.map.values_mut() {
            match r.state {
                RunState::Running | RunState::Cueing => {
                    r.anchor.get_or_insert(at - r.pos);
                }
                RunState::Stopped | RunState::Paused { .. } => r.anchor = None,
            }
        }
    }

    /// The runs (instances) running by a shared clock.
    pub(super) fn anchored(&self) -> HashSet<u64> {
        self.map
            .values()
            .filter(|r| r.state == RunState::Running && r.anchor.is_some())
            .map(|r| r.instance)
            .collect()
    }

    /// The shared time a run's clock step has got to (see `stepping_at`).
    pub(super) fn shared_at(&self) -> Option<f64> {
        self.stepping_at
    }

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

    /// The runs (instances) whose new clips are not seen yet: a seek's, while
    /// what it replaces stays up.
    pub(super) fn hiding(&self) -> Vec<u64> {
        self.map
            .values()
            .filter(|r| !r.outgoing.is_empty())
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
    // A picture on several displays: off the ones it was replaced on, fading
    // out of some alone.
    for s in &mut a.screens {
        if ch.off.contains(&s.output) {
            s.gone = true;
        } else if let Some(&(_, (now, to, left, curve))) =
            ch.leaving.iter().find(|(o, _)| *o == s.output)
        {
            let mut exit = Param::new(now);
            exit.ramp_to(to, left, curve);
            let going = exit.ramping();
            s.gone = !going && to <= 0.0;
            s.exit = going.then_some(exit);
        }
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
/// (`RunOut::End`), fades out to finish as it runs out. `own_fade_out`: false for
/// a clip a later one crossfades over (that takes it out). Returns whether it is over.
pub(super) fn clock_end(
    a: &mut ActiveCue,
    m: &MediaCue,
    on_clock: Option<f64>,
    clock: Option<(u64, f64)>,
    own_fade_out: bool,
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
    if own_fade_out && !a.auto_faded && m.fade_out > 0.0 && left <= m.fade_out {
        fade_away(a, m, left, clock);
    }
    // A video holding its last frame for good (no fade-out) keeps it, as it would
    // off a timeline.
    left <= SAME_TIME && !(a.held && m.hold_last_frame && !a.auto_faded)
}

/// Where the control cue `cue` on the timeline `key` puts a timeline, if it is
/// a jump: a Jump, a Run from a place (the timeline it names, at the time), or
/// a loop going back to its section's start.
fn jump_of(show: &Show, key: TimelineKey, cue: &Cue) -> Option<(TimelineKey, f64)> {
    let CueKind::Control(c) = &cue.kind else {
        return None;
    };
    let goes = c.timeline.unwrap_or(key);
    match c.action {
        ControlAction::TimelineJump | ControlAction::TimelineRun => {
            Some((goes, model::resolve(show, goes, c.at?)?))
        }
        // A loop goes back to its section's start, on its own timeline.
        ControlAction::TimelinePause if c.timeline.is_none_or(|k| k == key) => {
            Some((key, model::resolve(show, key, c.loop_from?)?))
        }
        _ => None,
    }
}

/// Whether `cue` is a timeline pause of the timeline `key` (it is one of its
/// cues) that stops its clock: not a loop.
fn is_pause_of(cue: &Cue, key: TimelineKey) -> bool {
    matches!(&cue.kind, CueKind::Control(c)
        if c.action == ControlAction::TimelinePause
            && !c.is_loop()
            && c.timeline.is_none_or(|k| k == key))
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
                fading: false,
                looping: None,
                at_end: false,
                mark: RunMark::default(),
            };
        };
        let (state, pos, instance, parked) = (run.state, run.pos, run.instance, run.parked);
        let looping = run.looping;
        let at_end = self.at_end(show, key, run);
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
        let on_go = matches!(state, RunState::Paused { .. })
            .then(|| self.on_go_of(show, instance))
            .flatten();
        let mark = RunMark {
            instance: Some(instance),
            placed: run.placed,
            state: match state {
                RunState::Stopped => 0,
                RunState::Cueing | RunState::Running => 1,
                RunState::Paused { .. } => 2,
            },
            parked,
        };
        RunView {
            key,
            state,
            pos,
            next_pause,
            length,
            on_go,
            parked,
            fading: state == RunState::Stopped && self.run_fading(instance),
            looping,
            at_end,
            mark,
        }
    }

    /// Whether a run is a timeline list's stopped at its end: it played to it
    /// (or was run from a place with nothing after it), and no clip has been
    /// put after its playhead since.
    fn at_end(&self, show: &Show, key: TimelineKey, run: &Run) -> bool {
        run.at_end
            && run.state == RunState::Stopped
            && !model::members(show, key)
                .into_iter()
                .any(|c| c.armed && start_of(c) > run.pos + SAME_TIME)
    }

    /// The first thing the clock of the run `instance` will reach that is not a
    /// control cue: what GO brings in or fades when the run is paused.
    fn on_go_of(&self, show: &Show, instance: u64) -> Option<CueId> {
        self.pending
            .iter()
            .filter(|p| p.run == Some(instance))
            .filter(|p| show.cue(p.cue_id).is_some_and(|c| model::due_order(c) > 0))
            .min_by(|a, b| a.at.total_cmp(&b.at))
            .map(|p| p.cue_id)
    }

    /// The first pause cue of a timeline at or after `pos`.
    fn next_pause_from(&self, show: &Show, key: TimelineKey, pos: f64) -> Option<f64> {
        model::members(show, key)
            .into_iter()
            .filter(|c| c.armed && is_pause_of(c, key) && start_of(c) >= pos - SAME_TIME)
            .map(start_of)
            .min_by(f64::total_cmp)
    }

    /// The timelines that are doing something: not `Stopped`, or stopped with
    /// what they played still fading out (a Stop's fade). For OSC feedback.
    pub fn timelines(&self) -> Vec<TimelineKey> {
        let mut out: Vec<TimelineKey> = self
            .runs
            .map
            .iter()
            .filter(|(_, r)| r.state != RunState::Stopped || self.run_fading(r.instance))
            .map(|(k, _)| *k)
            .collect();
        // The same order every time (a map has none of its own).
        out.sort_by_key(|k| match *k {
            TimelineKey::List(n) => (0, u64::from(n)),
            TimelineKey::Group(g) => (1, g.0),
        });
        out
    }

    /// Where a timeline's clock is and what its run is doing, if it has a run:
    /// a light look for Test Mode's notes (`timeline` works out more).
    pub fn timeline_clock(&self, key: TimelineKey) -> Option<(RunState, f64)> {
        self.runs.map.get(&key).map(|r| (r.state, r.pos))
    }

    /// Whether what the run `instance` played is still fading out after a Stop.
    fn run_fading(&self, instance: u64) -> bool {
        let i = Some(instance);
        self.active.iter().any(|a| {
            a.stopping
                && (a.run == i || matches!(&a.kind, ActiveKind::Matte(m) if m.start.run == i))
        })
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
        self.runs.placed += 1;
        if let Some(old) = self.runs.map.remove(&key) {
            if old.parked {
                self.clear_clips(old.instance, false);
            }
            self.cancel_run(&old);
        }
        // Nothing has come in at the top: no length is asked for.
        let ch = chase::chase(show, key, 0.0, &mut |_| None);
        self.push_upcoming(show, instance, 0.0, &ch.upcoming);
        let mut r = Run::new(instance, RunState::Running, 0.0);
        r.placed = self.runs.placed;
        self.runs.map.insert(key, r);
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

    /// Put the run of `key` at `pos` (plan 2.6): what it had waiting goes, the
    /// clips alive at `pos` are started held, part way in, and the rest wait for
    /// the clock. `then` says whether it then cues and runs, or parks. With the
    /// picture, a clip alive on both sides is jumped where it is (`Seek`, its
    /// decoders swapped once they have a frame), and what else the run played
    /// stays up until the new clips are cued, unseen till then, so the picture
    /// never shows them half built (`timeline_settle`). False if there is
    /// nothing there or after it to run (it stays stopped there).
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
        self.runs.placed += 1;
        let old = self.runs.map.remove(&key);
        // An earlier seek's clips, kept up for it, go now.
        if let Some(old) = &old {
            self.drop_outgoing(old);
        }
        // Run again while a Stop's fade is going: that goes on under what comes
        // in (which replaces it on its layer as it always would).
        let fading = old.as_ref().is_some_and(|o| o.state == RunState::Stopped);
        let mut olds = vec![instance];
        olds.extend(old.as_ref().map(|o| o.instance).filter(|i| *i != instance));
        let ch = chase::chase(show, key, pos, &mut |c| self.cue_duration(show, c));
        let run = then == Then::Run;
        if run && ch.alive.is_empty() && ch.upcoming.is_empty() {
            for &i in &olds {
                self.clear_clips(i, fading);
            }
            let mut stopped = Run::new(instance, RunState::Stopped, pos);
            stopped.at_end = matches!(key, TimelineKey::List(_));
            stopped.placed = self.runs.placed;
            self.runs.map.insert(key, stopped);
            // A group with nothing left to play is over.
            if matches!(key, TimelineKey::Group(_)) {
                self.active.retain(|a| a.instance != instance);
            }
            return false;
        }
        let picture = match then {
            Then::Run => true,
            Then::Park { picture } => picture,
        };
        // What it was waiting to start never starts.
        self.pending.retain(|p| {
            !olds
                .iter()
                .any(|i| p.run == Some(*i) || p.start.run == Some(*i))
        });
        // Clips alive on both sides: jumped where they are.
        let mut alive = ch.alive.clone();
        let mut carried: Vec<(u64, Alive)> = Vec::new();
        if picture {
            alive.retain(|x| match self.carriable(show, &olds, x) {
                Some(i) => {
                    carried.push((i, x.clone()));
                    false
                }
                None => true,
            });
        }
        // The rest of what it played: kept up (pictures and sounds) until the new
        // clips are cued, or gone at once.
        let mut outgoing = Vec::new();
        let mut gone = Vec::new();
        for a in &mut self.active {
            let theirs = a.run.is_some_and(|r| olds.contains(&r))
                || matches!(&a.kind, ActiveKind::Matte(m) if m.start.run.is_some_and(|r| olds.contains(&r)));
            if !theirs || carried.iter().any(|c| c.0 == a.instance) || (fading && a.stopping) {
                continue;
            }
            if picture
                && matches!(
                    a.kind,
                    ActiveKind::Video(_) | ActiveKind::Image(_) | ActiveKind::Audio(_)
                )
            {
                // No longer a clip of a run: it plays on by itself meanwhile,
                // replacing nothing.
                a.run = None;
                a.entry = None;
                a.follow(None);
                outgoing.push(a.instance);
            } else {
                gone.push(a.instance);
            }
        }
        self.active.retain(|a| !gone.contains(&a.instance));
        self.push_upcoming(show, instance, pos, &ch.upcoming);
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
        r.placed = self.runs.placed;
        r.cue_since = run.then(Instant::now);
        // By a shared clock: it runs from `pos` as of now, however long its
        // clips take to be cued (it catches up once they are).
        r.anchor = self.shared_now().filter(|_| run).map(|now| now - pos);
        if !picture {
            r.unstarted = ch.alive.clone();
        }
        r.outgoing_since = (!outgoing.is_empty()).then(Instant::now);
        r.outgoing = outgoing;
        // A seek inside a section being looped goes on looping; one out of it
        // ends the loop.
        r.looping = old
            .as_ref()
            .and_then(|o| o.looping)
            .filter(|l| pos >= l.from - SAME_TIME && pos < l.to - SAME_TIME);
        r.carried = carried.iter().map(|c| c.0).collect();
        self.runs.map.insert(key, r);
        for (i, x) in carried {
            if !self.timeline_carry(show, key, instance, i, &x, pos) {
                // It couldn't be jumped: started afresh.
                if let Some(r) = self.runs.map.get_mut(&key) {
                    r.carried.retain(|c| *c != i);
                    r.outgoing.push(i);
                    r.outgoing_since.get_or_insert_with(Instant::now);
                }
                if let Some(a) = self.active.iter_mut().find(|a| a.instance == i) {
                    a.run = None;
                    a.entry = None;
                    a.follow(None);
                }
                alive.push(x);
            }
        }
        if picture {
            self.start_alive(show, instance, alive);
        }
        self.refresh_waits();
        if run && self.run_cued(instance) {
            self.timeline_release(show, key);
        }
        self.timeline_settle();
        true
    }

    /// The clip playing for `x` in the run (one of `olds`) before a seek, if it
    /// can be jumped where it is: a file playing, not on its way out, not on its
    /// last frame or jumping already, nor landed on its last frame.
    fn carriable(&self, show: &Show, olds: &[u64], x: &Alive) -> Option<u64> {
        if x.at_last_frame
            || !matches!(
                show.cue(x.id).map(|c| &c.kind),
                Some(CueKind::Video(_) | CueKind::Audio(_))
            )
        {
            return None;
        }
        self.active
            .iter()
            .filter(|a| a.cue_id == x.id && a.run.is_some_and(|r| olds.contains(&r)))
            .filter(|a| matches!(a.kind, ActiveKind::Video(_) | ActiveKind::Audio(_)))
            .find(|a| a.started && !a.stopping && !a.held && a.seeking.is_none())
            .map(|a| a.instance)
    }

    /// Jump the playing clip `inst` to where the chase found it (`x`), as a clip
    /// of the run `instance` now at `pos`: its decoders for there are started
    /// (or what a jump got ready is taken) and swapped in once they have a frame,
    /// its levels set as the chase says, and it is held until the run goes on, as
    /// a seek's clip is. False if it couldn't be.
    fn timeline_carry(
        &mut self,
        show: &Show,
        key: TimelineKey,
        instance: u64,
        inst: u64,
        x: &Alive,
        pos: f64,
    ) -> bool {
        let Some(cue) = show.cue(x.id) else {
            return false;
        };
        let timing = self.timeline_clip(show, cue, x.lead);
        let played = timing.looped.clone().unwrap_or_else(|| cue.clone());
        let Some(m) = played.kind.file_media() else {
            return false;
        };
        let info = match self.probe.get(self.ff.as_ref(), &m.file) {
            crate::media::ProbeState::Ready(info) => info.clone(),
            _ => return false,
        };
        let p = match self.take_prepared(&played, x.from, x.lead) {
            Some(p) => p,
            None => match self.prepare_from(&played, &info, x.from, x.lead) {
                Ok(p) => p,
                Err(e) => {
                    log::debug!("cue {} could not be jumped: {e}", cue.number);
                    return false;
                }
            },
        };
        let (player, voice) = match p.kind {
            super::PreparedKind::Video { player, voice } => (Some(player), voice),
            super::PreparedKind::Audio(voice) => (None, Some(voice)),
            _ => return false,
        };
        let crossed = self.timeline_crossed_by(show, cue);
        let clock = self.run_clock(Some(instance));
        let paused = self.paused;
        let Some(m) = cue.kind.media() else {
            return false;
        };
        let Some(a) = self.active.iter_mut().find(|a| a.instance == inst) else {
            return false;
        };
        a.seeking = Some(super::Seek {
            player,
            voice,
            base: x.from,
            file_from: p.file_from,
            spawned_at: Instant::now(),
            duration: Some(p.duration.map(|d| d + x.from)),
            keep_time: false,
        });
        a.pass_len = p.pass_len;
        a.speed = p.speed;
        a.lead = x.lead;
        a.run = Some(instance);
        a.together = Some(instance);
        a.entry = None;
        let zero = zero_of(cue);
        a.clip_zero = Some(zero);
        a.clock_time = (pos - zero).max(0.0);
        // Its time once jumped is `x.from` (`apply_seek`).
        a.clock_offset = x.from - a.clock_time;
        a.clocked = timing.clocked;
        a.ends_at = timing.ends_at;
        a.detached = false;
        a.held = false;
        a.joined = false;
        a.crossed_by = crossed;
        // Its levels as playing there gives them.
        a.faded = false;
        a.auto_faded = false;
        a.relevel = false;
        a.live_transform = None;
        a.opacity = Param::new(m.motion.opacity_at(m.opacity, a.key_time()));
        a.volume = Param::new(m.volume_db);
        if a.is_visual() {
            a.screens = m
                .displays(show)
                .into_iter()
                .map(super::displays::Screen::on)
                .collect();
        }
        chase_levels(a, m, x);
        a.follow(clock);
        // Held until the run lets it go (one frozen by a Pause of its own, until
        // its Resume).
        a.freeze(false, paused);
        if x.frozen || x.detached {
            detach(a);
        }
        if !x.frozen
            && let Some(r) = self.runs.map.get_mut(&key)
        {
            r.frozen.push(inst);
        }
        true
    }

    /// What a run before a seek played, kept up for it (`Run::outgoing`), goes.
    fn drop_outgoing(&mut self, run: &Run) {
        if !run.outgoing.is_empty() {
            self.active.retain(|a| !run.outgoing.contains(&a.instance));
        }
    }

    /// A seek's clips have all come (or have had `CUE_LIMIT`): what they
    /// replace goes, and they are seen.
    fn timeline_settle(&mut self) {
        let due: Vec<TimelineKey> = self
            .runs
            .map
            .iter()
            .filter(|(_, r)| !r.outgoing.is_empty())
            .filter(|(_, r)| {
                self.run_cued(r.instance)
                    || r.outgoing_since.is_none_or(|s| s.elapsed() >= CUE_LIMIT)
            })
            .map(|(k, _)| *k)
            .collect();
        for key in due {
            if let Some(r) = self.runs.map.get_mut(&key) {
                let gone = std::mem::take(&mut r.outgoing);
                r.outgoing_since = None;
                r.carried.clear();
                self.active.retain(|a| !gone.contains(&a.instance));
            }
        }
    }

    /// A clip a seek has started that is not to be seen yet: its run's new set
    /// is still being cued, what it replaces still up.
    pub(super) fn timeline_unseen(&self, a: &ActiveCue) -> bool {
        a.run.is_some_and(|r| {
            self.runs.by_instance(r).is_some_and(|(_, run)| {
                !run.outgoing.is_empty() && !run.carried.contains(&a.instance)
            })
        })
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
                .all(|a| a.run != i || (a.started && a.seeking.is_none()) || a.is_matte())
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
        // (Not every clip may have come, if it had `CUE_LIMIT`: what is up goes on.)
        if let Some(run) = self.runs.map.get_mut(&key) {
            let gone = std::mem::take(&mut run.outgoing);
            run.outgoing_since = None;
            run.carried.clear();
            self.active.retain(|a| !gone.contains(&a.instance));
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
    pub fn timeline_run_from(&mut self, show: &Show, key: TimelineKey, pos: f64) {
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

    /// Matrix crosspoint fades wait with the clock too (ruling L2): a fade cue
    /// a run fired, with crosspoints to fade, whose run's clock stands (paused,
    /// or cueing) holds the crosspoints of the voices it aims at where they are
    /// (`Voice::hold_crosspoints`, a flag the mixer reads), and they go on at
    /// their rate when the clock moves, the run stops or goes, or the fade is
    /// over. After each tick. (A crosspoint fade begun by hand on a voice that
    /// such a fade also aims at waits with it.)
    pub(super) fn timeline_hold_crosspoints(&mut self, show: &Show) {
        let mut held: Vec<CueId> = Vec::new();
        for a in &self.active {
            let stands = a.run.is_some_and(|r| {
                self.runs.by_instance(r).is_some_and(|(_, r)| {
                    matches!(r.state, RunState::Paused { .. } | RunState::Cueing)
                })
            });
            if !stands || !matches!(a.kind, ActiveKind::Fade) {
                continue;
            }
            if let Some(CueKind::Fade(f)) = show.cue(a.cue_id).map(|c| &c.kind)
                && !f.matrix.is_empty()
                && let Some(t) = f.target
            {
                held.extend(show.with_children(t));
            }
        }
        for a in &self.active {
            if let Some(v) = a.voice()
                && v.has_crosspoints()
            {
                let hold = held.contains(&a.cue_id);
                if v.crosspoints_held() != hold {
                    v.hold_crosspoints(hold);
                }
            }
        }
    }

    /// Gapless joins (plan 2.6, problem 8): the pictures of a running (or
    /// cueing) run whose slot the next clip on their track abuts (it starts no
    /// later than `JOIN_GAP` after their end) and that hasn't come in yet (it
    /// waits for the clock, for its file, or for its first frame). They are
    /// kept up, on their last frame, until it replaces them; a clip whose run
    /// stands at a pause there ends as it would.
    pub(super) fn timeline_joins(&self, show: &Show) -> HashSet<u64> {
        let mut out = HashSet::new();
        for a in &self.active {
            let (Some(r), Some(end)) = (a.run, a.ends_at) else {
                continue;
            };
            if !a.is_visual() || a.stopping {
                continue;
            }
            let goes = self
                .runs
                .by_instance(r)
                .is_some_and(|(_, run)| matches!(run.state, RunState::Running | RunState::Cueing));
            let Some(cue) = show.cue(a.cue_id).filter(|_| goes) else {
                continue;
            };
            let abuts = |id: CueId, at: f64| {
                at <= end + JOIN_GAP
                    && id != a.cue_id
                    && show
                        .cue(id)
                        .is_some_and(|b| chase::same_track(show, cue, b))
            };
            let coming = self.pending.iter().any(|p| {
                (p.run == Some(r) && !p.waits_for_fade && abuts(p.cue_id, p.at))
                    || (p.run.is_none()
                        && p.start.run == Some(r)
                        && show
                            .cue(p.cue_id)
                            .is_some_and(|c| abuts(c.id, start_of(c) + p.start.lead)))
            }) || self.active.iter().any(|b| {
                b.run == Some(r)
                    && !b.started
                    && b.instance > a.instance
                    && show
                        .cue(b.cue_id)
                        .is_some_and(|c| abuts(c.id, start_of(c) + b.lead))
            });
            if coming {
                out.insert(a.instance);
            }
        }
        out
    }

    /// The earlier clips of its timeline a clip starting now crossfades over,
    /// and for how long (`chase::overlap`; only with `crossfade_overlaps`).
    pub(super) fn timeline_overlap(&mut self, show: &Show, cue: &Cue) -> Option<chase::Overlap> {
        let key = model::key_of(show, cue.id)?;
        chase::overlap(show, key, cue, &mut |c| self.cue_duration(show, c))
    }

    /// The later clip of its timeline that crossfades over a clip starting now
    /// on all its displays, if one does (`chase::crossed_by`).
    pub(super) fn timeline_crossed_by(&mut self, show: &Show, cue: &Cue) -> Option<CueId> {
        let key = model::key_of(show, cue.id)?;
        chase::crossed_by(show, key, cue, &mut |c| self.cue_duration(show, c))
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
        match self
            .pending
            .iter()
            .find(|p| p.cue_id == id && p.run.is_some())
        {
            Some(p) if (p.start.from - p.start.lead).abs() < 1e-9 => p.start.lead,
            Some(_) => 0.0,
            // One going round again (`timeline_go_round`), from its lead-in.
            None => self
                .active
                .iter()
                .find(|a| a.cue_id == id && a.detached && a.run.is_some())
                .map_or(0.0, |a| a.lead),
        }
    }

    /// A clip whose content ran out ahead of its clock, set to loop but not
    /// played as one (it left the clock at run time: plan 2.4), goes round again
    /// from its in point: its decoders for there (got ready as its end neared,
    /// `timeline_round_wanted`) are swapped in once they have a frame, its last
    /// frame shown meanwhile. If they can't be started, it holds, as before.
    pub(super) fn timeline_go_round(&mut self, show: &Show, instance: u64) {
        let Some(a) = self.active.iter().find(|a| a.instance == instance) else {
            return;
        };
        let (id, lead) = (a.cue_id, a.lead);
        let Some(cue) = show.cue(id).cloned() else {
            return;
        };
        let Some(file) = cue.kind.file_media().map(|m| m.file.clone()) else {
            return;
        };
        let p = match self.take_prepared(&cue, lead, lead) {
            Some(p) => p,
            None => {
                let info = match self.probe.get(self.ff.as_ref(), &file) {
                    crate::media::ProbeState::Ready(info) => info.clone(),
                    _ => return,
                };
                match self.prepare_from(&cue, &info, lead, lead) {
                    Ok(p) => p,
                    Err(e) => {
                        log::debug!("cue {} could not go round again: {e}", cue.number);
                        return;
                    }
                }
            }
        };
        let (player, voice) = match p.kind {
            super::PreparedKind::Video { player, voice } => (Some(player), voice),
            super::PreparedKind::Audio(voice) => (None, Some(voice)),
            _ => return,
        };
        if let Some(a) = self.active.iter_mut().find(|a| a.instance == instance) {
            log::debug!("cue {} goes round again", cue.number);
            a.seeking = Some(super::Seek {
                player,
                voice,
                base: lead,
                file_from: 0.0,
                spawned_at: Instant::now(),
                duration: Some(p.duration.map(|d| d + lead)),
                keep_time: false,
            });
        }
    }

    /// The clips that will go round again (`timeline_go_round`) within
    /// `PRELOAD_HORIZON`: their content is to run out before their clock's end.
    pub(super) fn timeline_round_wanted(&self, show: &Show) -> Vec<CueId> {
        let clocks = self.runs.clocks();
        self.active
            .iter()
            .filter(|a| a.detached && a.clocked && a.started && a.pass_len.is_none())
            .filter(|a| !a.stopping && a.seeking.is_none())
            .filter(|a| {
                let (Some(d), Some(end), Some(pos)) =
                    (a.duration, a.ends_at, a.run.and_then(|r| clocks.get(&r)))
                else {
                    return false;
                };
                let left = d - a.elapsed;
                left <= PRELOAD_HORIZON && left < end - pos
            })
            .filter(|a| {
                show.cue(a.cue_id).is_some_and(|c| {
                    matches!(c.kind, CueKind::Video(_) | CueKind::Audio(_))
                        && c.kind.media().is_some_and(|m| m.run_out == RunOut::Loop)
                })
            })
            .map(|a| a.cue_id)
            .collect()
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
        self.drop_outgoing(run);
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
        let placed = self.runs.placed;
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        // By a shared clock, the clock is where that says (never back).
        let anchor = run.anchor.filter(|_| self.sync.is_some());
        let target = match (anchor, self.sync) {
            (Some(a), Some(s)) => (s.now - a).max(run.pos),
            _ => run.pos + dt,
        };
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
            // A jump it makes there is made at the shared time the clock was there.
            let pos = self.runs.map.get(&key).map_or(p.at, |r| r.pos);
            self.runs.stepping_at = anchor.map(|a| a + pos);
            self.start_at(show, p.cue_id, p.start);
            self.runs.stepping_at = None;
            // Stopped, paused, or put somewhere else (a jump, which has run on
            // from there itself if it could).
            if !self.still_running(key, instance) || self.runs.placed != placed {
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
        self.timeline_settle();
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
        // A seek's clips that have come this tick are seen at once.
        self.timeline_settle();
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
                    if let Some(run) = self.runs.map.get_mut(&key) {
                        run.at_end = true;
                    }
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

    /// What a run's jump lands in, got ready ahead of it (plan 2.6, problem 6):
    /// for each Jump cue, and Run cue with a place, that a run's clock is within
    /// `PRELOAD_HORIZON` of, the clips the chase finds alive where it goes.
    /// Files part way in as (cue, clip time, lead-in), for `prepare_from`; the
    /// rest (and a file landed in at its in point) as cues to get ready as usual.
    /// At most `MAX_PRELOAD_CHILDREN` in all.
    pub(super) fn timeline_jump_wanted(
        &mut self,
        show: &Show,
    ) -> (Vec<CueId>, Vec<(CueId, f64, f64)>) {
        let mut targets: Vec<(TimelineKey, f64)> = Vec::new();
        for (key, run) in &self.runs.map {
            if run.state == RunState::Stopped {
                continue;
            }
            for p in &self.pending {
                if p.run != Some(run.instance) || p.at - run.pos > PRELOAD_HORIZON {
                    continue;
                }
                let Some(cue) = show.cue(p.cue_id).filter(|c| c.armed) else {
                    continue;
                };
                // (A loop on its last pass is not going back.)
                let last = run.looping.is_some_and(|l| l.cue == cue.id && l.last_pass);
                if let Some(t) = jump_of(show, *key, cue)
                    && !last
                    && !targets.contains(&t)
                {
                    targets.push(t);
                }
            }
        }
        let (mut land, mut part) = (Vec::new(), Vec::new());
        for (key, t) in targets {
            let ch = chase::chase(show, key, t, &mut |c| self.cue_duration(show, c));
            for x in ch.alive {
                if land.len() + part.len() >= super::MAX_PRELOAD_CHILDREN {
                    break;
                }
                let Some(cue) = show.cue(x.id).filter(|c| c.kind.preloads()) else {
                    continue;
                };
                let file = matches!(cue.kind, CueKind::Video(_) | CueKind::Audio(_));
                if file && (x.from - x.lead).abs() > 1e-9 {
                    // (One that holds its last frame starts a moment before
                    // its end: it is not got ready.)
                    if !x.at_last_frame {
                        part.push((x.id, x.from, x.lead));
                    }
                } else if !land.contains(&x.id) {
                    land.push(x.id);
                }
            }
        }
        (land, part)
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

    /// What GO on the cue list `list` does to a timeline, if anything: on a
    /// timeline list, run it (or resume it, or nothing while it runs or cues);
    /// otherwise the latest timeline group of the list that waits for GO (at a
    /// pause cue of its own, or parked) goes on. `timeline_go` does this, and
    /// `Engine::go_plan` reads it out, so the two can't disagree. The run GO
    /// goes on with (paused or parked), and the first clip or fade it reaches.
    pub(super) fn timeline_go_for(
        &self,
        show: &Show,
        list: u32,
    ) -> Option<(TimelineKey, TimelineGo, Option<CueId>)> {
        let goes_on = |r: &Run| {
            let does = if r.parked {
                TimelineGo::Play
            } else {
                TimelineGo::Resume
            };
            (does, self.on_go_of(show, r.instance))
        };
        // Playing a section round and round: GO is for leaving it.
        let leaves = |r: &Run| {
            r.looping
                .filter(|l| !l.last_pass && matches!(r.state, RunState::Running | RunState::Cueing))
                .map(|l| (TimelineGo::LeaveLoop { at_once: l.at_once }, None))
        };
        let key = TimelineKey::List(list);
        if model::is_timeline(show, key) {
            let run = self.runs.map.get(&key);
            let (does, then) = match (run.and_then(leaves), run.map(|r| r.state)) {
                (Some(leave), _) => leave,
                (None, Some(RunState::Running)) => (TimelineGo::Running, None),
                (None, Some(RunState::Cueing)) => (TimelineGo::Cueing, None),
                (None, Some(RunState::Paused { .. })) => {
                    run.map_or((TimelineGo::Resume, None), goes_on)
                }
                (None, Some(RunState::Stopped))
                    if run.is_some_and(|r| self.at_end(show, key, r)) =>
                {
                    (TimelineGo::AtEnd, None)
                }
                (None, Some(RunState::Stopped) | None) => (TimelineGo::Start, None),
            };
            return Some((key, does, then));
        }
        let (key, run) = self
            .runs
            .map
            .iter()
            .filter(|(k, r)| {
                matches!(k, TimelineKey::Group(g) if show.cue(*g).is_some_and(|c| c.list == list))
                    && (matches!(
                        r.state,
                        RunState::Paused {
                            waits_for_go: true,
                            ..
                        }
                    ) || leaves(r).is_some())
                    && self.active.iter().any(|a| a.instance == r.instance)
            })
            .max_by_key(|(_, r)| r.instance)?;
        let (does, then) = leaves(run).unwrap_or_else(|| goes_on(run));
        Some((*key, does, then))
    }

    /// GO for the cue list `list`, on a timeline (`timeline_go_for` says what
    /// it does). Whether GO was taken.
    pub(super) fn timeline_go(&mut self, show: &Show, list: u32) -> bool {
        let Some((key, does, _)) = self.timeline_go_for(show, list) else {
            return false;
        };
        match does {
            TimelineGo::Running => {
                self.log("GO: the timeline is running; GO does nothing until it pauses")
            }
            TimelineGo::Cueing => {}
            TimelineGo::AtEnd => {
                self.log("GO: the timeline is at its end; move its playhead to run it again")
            }
            TimelineGo::LeaveLoop { .. } => self.timeline_leave_loop(show, key),
            TimelineGo::Start | TimelineGo::Resume | TimelineGo::Play => {
                if let TimelineKey::Group(g) = key
                    && let Some(c) = show.cue(g)
                {
                    log::info!("GO resumes {} ({})", c.number, c.name);
                }
                // GO lets go of the show's pause, as firing a cue does.
                if self.paused {
                    self.set_paused(false);
                }
                self.timeline_run(show, key);
            }
        }
        true
    }

    /// GO aimed at one timeline (a remote's `/timeline/<name>/go`): what GO on
    /// its list would do to it (`timeline_go_for`), whichever list is being
    /// worked. A stopped group is fired, a stopped list runs from its playhead,
    /// a paused run goes on, a loop is left as its cue says; while it runs or
    /// cues, or a list stands at its end, nothing (and the log says why).
    pub fn timeline_go_one(&mut self, show: &Show, key: TimelineKey) {
        if !model::is_timeline(show, key) {
            self.problem("There is no such timeline");
            return;
        }
        let run = self.runs.map.get(&key);
        let state = run.map(|r| r.state);
        let looping = run.and_then(|r| r.looping).is_some_and(|l| !l.last_pass)
            && matches!(state, Some(RunState::Running | RunState::Cueing));
        let at_end = run.is_some_and(|r| self.at_end(show, key, r));
        match state {
            _ if looping => self.timeline_leave_loop(show, key),
            Some(RunState::Running) => {
                self.log("GO: the timeline is running; GO does nothing until it pauses")
            }
            Some(RunState::Cueing) => {}
            Some(RunState::Stopped) if at_end => {
                self.log("GO: the timeline is at its end; move its playhead to run it again")
            }
            _ => {
                if self.paused {
                    self.set_paused(false);
                }
                self.timeline_run(show, key);
            }
        }
    }

    /// A paused timeline goes on (a remote's resume); anything else is left as
    /// it is, and the log says so.
    pub fn timeline_continue(&mut self, show: &Show, key: TimelineKey) {
        match self.runs.map.get(&key).map(|r| r.state) {
            Some(RunState::Paused { .. }) => {
                if self.paused {
                    self.set_paused(false);
                }
                self.timeline_resume(show, key);
            }
            _ => self.log("The timeline is not paused"),
        }
    }

    /// A loop playing on a timeline is left (a remote's leave-loop), as GO
    /// would: this pass is the last, or at once when its cue says so.
    pub fn timeline_leave(&mut self, show: &Show, key: TimelineKey) {
        let looping = self
            .runs
            .map
            .get(&key)
            .and_then(|r| r.looping)
            .is_some_and(|l| !l.last_pass);
        if looping {
            self.timeline_leave_loop(show, key);
        } else {
            self.log("The timeline is not playing a loop");
        }
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
            ControlAction::TimelinePause if c.is_loop() => {
                self.timeline_loop(show, key, cue, c, &name)
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

    /// A loop pause (`ControlCue::loop_from`, ruling L3) reached by its run's
    /// clock: the timeline jumps back to the section's start and plays the
    /// section again (a seek, so everything in it is cued and let go together,
    /// got ready ahead as for any jump), and so each time round, until GO. On
    /// the pass GO marked the last, it lets the clock carry straight on.
    fn timeline_loop(
        &mut self,
        show: &Show,
        key: TimelineKey,
        cue: &Cue,
        c: &ControlCue,
        name: &str,
    ) {
        if model::key_of(show, cue.id) != Some(key) {
            self.problem(format!(
                "Cue {name}: a loop plays a section of its own timeline, not another"
            ));
            return;
        }
        let to = start_of(cue);
        let Some(from) = c.loop_from.and_then(|p| model::resolve(show, key, p)) else {
            self.problem(format!(
                "Cue {name}: the place its section starts is not on the timeline"
            ));
            return;
        };
        if from >= to - SAME_TIME {
            self.problem(format!(
                "Cue {name}: its section has to start before it on the timeline"
            ));
            return;
        }
        let Some(run) = self.runs.map.get_mut(&key) else {
            return;
        };
        if run.looping.is_some_and(|l| l.cue == cue.id && l.last_pass) {
            run.looping = None;
            log::info!("cue {name}: the loop's last pass is done; the timeline goes on");
            return;
        }
        if run.state != RunState::Running || !self.may_jump(name) {
            return;
        }
        let looping = Looping {
            cue: cue.id,
            from,
            to,
            at_once: c.leave_at_once,
            last_pass: false,
        };
        self.timeline_place(show, key, from, Then::Run);
        if let Some(run) = self.runs.map.get_mut(&key) {
            run.looping = Some(looping);
        }
    }

    /// GO while a section loops: the pass playing is the last (the timeline
    /// carries on past the cue when it gets there), or with `leave_at_once` the
    /// timeline leaves now, going on from the cue.
    fn timeline_leave_loop(&mut self, show: &Show, key: TimelineKey) {
        let Some(run) = self.runs.map.get_mut(&key) else {
            return;
        };
        let Some(l) = run.looping.filter(|l| !l.last_pass) else {
            return;
        };
        if !l.at_once {
            run.looping = Some(Looping {
                last_pass: true,
                ..l
            });
            self.log("GO: the loop finishes this pass and the timeline goes on");
            return;
        }
        run.looping = None;
        self.log("GO: the timeline leaves the loop");
        // From the cue's place: it has been passed there.
        if self.may_jump("GO") {
            self.timeline_place(show, key, l.to, Then::Run);
        }
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

    /// The pictures and sounds a timeline's run has started and still plays, for
    /// Test Mode's check that clips come up on time: where on the clock each was
    /// due to begin and whether it has its first frame (or sound) yet.
    pub fn timeline_clips_up(&self, key: TimelineKey) -> Vec<ClipUp> {
        let Some(run) = self.runs.map.get(&key) else {
            return Vec::new();
        };
        self.active
            .iter()
            .filter(|a| a.run == Some(run.instance) && !a.stopping)
            .filter(|a| {
                matches!(
                    a.kind,
                    ActiveKind::Video(_) | ActiveKind::Image(_) | ActiveKind::Audio(_)
                )
            })
            .filter_map(|a| {
                Some(ClipUp {
                    cue: a.cue_id,
                    instance: a.instance,
                    due: a.clip_zero? + a.lead.max(0.0),
                    up: a.started && a.seeking.is_none(),
                })
            })
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
