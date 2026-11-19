//! The timeline tests' hands and eyes. `Driver` plays the timelines of a test
//! show lap after lap as an operator would: it starts each with GO, presses GO
//! after a set hold where a pause waits for it, leaves a loop after a few
//! passes, and (the long test) seeks while the clock runs. `Watch` looks at the
//! runs after every tick and notes what `analysis::analyse_timeline` judges:
//! the clock against real time, when each clip the clock reaches comes up, and
//! how long the run stands getting its clips ready.
//!
//! Both only use what the transports and remotes use (`go_plan`, `go`,
//! `timeline_seek`, `timeline_clock`, `timeline`), so the test plays the
//! timelines the way a show does.

use super::analysis::{Arrival, CueSpell, Stretch, TimelineNotes};
use crate::engine::{Engine, GoPlan, RunState, TimelineGo};
use crate::show::{CueId, Show, TimelineKey};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// What the driver does in a lap.
#[derive(Clone, Debug, PartialEq)]
pub struct Script {
    /// The timelines a lap plays, one after the other.
    pub legs: Vec<TimelineKey>,
    /// Seconds a pause that waits for GO is left standing before GO is pressed.
    pub hold: f64,
    /// Passes of a loop pause played before GO leaves it.
    pub loop_passes: u32,
    /// Seeks made while the first leg runs: when its clock reaches the first
    /// number it is moved to the second (each once a lap, in turn).
    pub seeks: Vec<(f64, f64)>,
}

/// What the driver did, for the session.
#[derive(Clone, Debug, PartialEq)]
pub enum Happened {
    LapBegan,
    LapEnded,
    /// For `events.log`.
    Said(String),
    /// It can't go on: why.
    Stuck(String),
}

/// How long a timeline that was started may take to be seen doing anything.
const START_PATIENCE: Duration = Duration::from_secs(15);

pub struct Driver {
    pub script: Script,
    repeats: bool,
    leg: usize,
    leg_since: Option<Instant>,
    /// The leg's run has been seen doing something since it was started.
    went: bool,
    paused_since: Option<Instant>,
    /// A loop being played: where its clock was last, and the passes seen.
    looping: Option<(f64, u32)>,
    seeks_done: usize,
    /// The laps begun.
    pub laps: usize,
    /// No more laps (a test of one).
    pub finished: bool,
    /// GO presses at pauses, loops left and seeks made, in all.
    pub presses: u32,
    pub leaves: u32,
    pub seeks: u32,
}

fn name(show: &Show, key: TimelineKey) -> String {
    match key {
        TimelineKey::List(n) => show.list_name(n),
        TimelineKey::Group(g) => show.cue(g).map_or_else(String::new, |c| c.name.clone()),
    }
}

impl Driver {
    pub fn new(script: Script, repeats: bool) -> Driver {
        Driver {
            script,
            repeats,
            leg: 0,
            leg_since: None,
            went: false,
            paused_since: None,
            looping: None,
            seeks_done: 0,
            laps: 0,
            finished: false,
            presses: 0,
            leaves: 0,
            seeks: 0,
        }
    }

    /// The timeline being played now.
    pub fn key(&self) -> Option<TimelineKey> {
        self.script.legs.get(self.leg).copied()
    }

    /// One look, after the engine's tick. Returns what it did.
    pub fn tick(&mut self, show: &Show, engine: &mut Engine, now: Instant) -> Vec<Happened> {
        let mut out = Vec::new();
        let Some(key) = self.key().filter(|_| !self.finished) else {
            return out;
        };
        let Some(since) = self.leg_since else {
            self.begin(show, engine, key, now, &mut out);
            return out;
        };
        let (state, pos) = engine
            .timeline_clock(key)
            .unwrap_or((RunState::Stopped, 0.0));
        let busy = engine.timelines().contains(&key);
        self.went |= busy;
        if !self.went {
            if now.duration_since(since) > START_PATIENCE {
                out.push(Happened::Stuck(format!(
                    "the timeline {} was started and never ran",
                    name(show, key)
                )));
            }
            return out;
        }

        // A pause that waits for GO: GO, once it has stood a while.
        if matches!(
            state,
            RunState::Paused {
                waits_for_go: true,
                ..
            }
        ) {
            let at = *self.paused_since.get_or_insert(now);
            if now.duration_since(at).as_secs_f64() >= self.script.hold {
                self.paused_since = None;
                match engine.go_plan(show) {
                    GoPlan::Timeline {
                        key: k,
                        does: TimelineGo::Resume | TimelineGo::Play,
                        ..
                    } if k == key => engine.go(show),
                    other => {
                        // Not what GO would do here (it should be): go on all the same.
                        out.push(Happened::Said(format!(
                            "Test mode: GO would not continue the timeline ({other:?}); it is continued by hand"
                        )));
                        engine.timeline_continue(show, key);
                    }
                }
                self.presses += 1;
                out.push(Happened::Said(format!(
                    "Test mode: GO after {:.0} s at the pause at {pos:.1} s on {}",
                    self.script.hold,
                    name(show, key)
                )));
            }
        } else {
            self.paused_since = None;
        }

        // A loop: GO once it has gone round enough, just after it went back,
        // so that the last pass is a whole one every lap.
        if state == RunState::Running && self.script.loop_passes > 0 {
            match engine.timeline(show, key).looping.filter(|l| !l.last_pass) {
                Some(l) => {
                    // (A run is looping once it has gone back the first time.)
                    let (last, mut passes) = self.looping.unwrap_or((pos, 1));
                    if pos < last - 0.25 {
                        passes += 1;
                    }
                    self.looping = Some((pos, passes));
                    if passes >= self.script.loop_passes && pos - l.from < 1.0 {
                        if let GoPlan::Timeline {
                            does: TimelineGo::LeaveLoop { .. },
                            ..
                        } = engine.go_plan(show)
                        {
                            engine.go(show);
                        } else {
                            engine.timeline_leave(show, key);
                        }
                        self.leaves += 1;
                        self.looping = None;
                        out.push(Happened::Said(format!(
                            "Test mode: GO leaves the loop on {} after {passes} passes",
                            name(show, key)
                        )));
                    }
                }
                None => self.looping = None,
            }
        }

        // Seeks while the first leg runs.
        if self.leg == 0
            && state == RunState::Running
            && let Some(&(when, to)) = self.script.seeks.get(self.seeks_done)
            && pos >= when
        {
            self.seeks_done += 1;
            self.seeks += 1;
            engine.timeline_seek(show, key, to, true);
            out.push(Happened::Said(format!(
                "Test mode: seek from {pos:.1} s to {to:.1} s on {}",
                name(show, key)
            )));
        }

        // Over (and faded out): the next leg, or the next lap.
        if !busy && state == RunState::Stopped {
            self.leg += 1;
            self.leg_since = None;
            self.went = false;
            self.looping = None;
            self.paused_since = None;
            self.seeks_done = 0;
            if self.leg >= self.script.legs.len() {
                self.leg = 0;
                out.push(Happened::LapEnded);
                self.finished = !self.repeats;
            }
        }
        out
    }

    /// Start a leg: its list is worked, and GO starts it.
    fn begin(
        &mut self,
        show: &Show,
        engine: &mut Engine,
        key: TimelineKey,
        now: Instant,
        out: &mut Vec<Happened>,
    ) {
        if self.leg == 0 {
            self.laps += 1;
            out.push(Happened::LapBegan);
        }
        let list = match key {
            TimelineKey::List(n) => n,
            TimelineKey::Group(g) => show.cue(g).map_or(0, |c| c.list),
        };
        engine.switch_list(show, list);
        match key {
            // From the top: a list remembers where it stopped.
            TimelineKey::List(_) => {
                if engine.timeline_clock(key).is_some_and(|(_, pos)| pos > 0.0) {
                    engine.timeline_seek(show, key, 0.0, false);
                }
            }
            TimelineKey::Group(g) => engine.playhead = Some(g),
        }
        let plan = engine.go_plan(show);
        let what = plan
            .describe(show)
            .map_or_else(|| name(show, key), |(does, _)| does);
        engine.go(show);
        out.push(Happened::Said(format!(
            "Test mode: lap {}, GO: {what}",
            self.laps
        )));
        self.leg_since = Some(now);
        self.went = false;
    }
}

/// What a run looked like at the last look.
#[derive(Clone, Copy)]
struct Look {
    at: Instant,
    state: RunState,
    pos: f64,
}

/// A stretch being measured: when it began (seconds into the test), its real
/// time and the clock less that so far.
#[derive(Clone, Copy)]
struct Open {
    t: f64,
    wall: f64,
    drift: f64,
}

/// A clip the clock reached, waiting to come up.
struct Due {
    key: TimelineKey,
    cue: CueId,
    due: f64,
    at: f64,
    /// What it was first seen waiting for.
    waited: Option<&'static str>,
}

/// The engine's limit on getting a seek's clips ready (`CUE_LIMIT`), less a
/// look's worth.
const GAVE_UP_AFTER: f64 = 1.95;
/// A stretch shorter than this says nothing about drift.
const MIN_STRETCH: f64 = 1.0;
/// A clock that moved further from the real time than this in one look was
/// put somewhere else (a jump), not drifting.
const JUMPED: f64 = 0.5;

#[derive(Default)]
pub struct Watch {
    last: HashMap<TimelineKey, Look>,
    open: HashMap<TimelineKey, Open>,
    due: HashMap<u64, Due>,
    /// Instances already judged or left out.
    seen: HashSet<u64>,
    cueing: HashMap<TimelineKey, (Instant, f64)>,
    /// Where each run's clock last began to run on its own (from the top, or
    /// after cueing): what it reaches from there is judged.
    from: HashMap<TimelineKey, f64>,
    pub notes: TimelineNotes,
    /// The clips that came up on the clock, in turn.
    pub came: Vec<CueId>,
}

impl Watch {
    /// The computer slept: nothing measured across it counts.
    pub fn slept(&mut self) {
        self.open.clear();
        self.last.clear();
    }

    /// Look at the timelines `keys` after a tick, at `t` seconds into the test,
    /// in lap `lap`.
    pub fn tick(
        &mut self,
        show: &Show,
        engine: &Engine,
        keys: &[TimelineKey],
        now: Instant,
        t: f64,
        lap: usize,
    ) {
        for &key in keys {
            let Some((state, pos)) = engine.timeline_clock(key) else {
                continue;
            };
            let prev = self.last.insert(
                key,
                Look {
                    at: now,
                    state,
                    pos,
                },
            );
            self.drift(key, prev, state, pos, now, t);
            self.cueing(key, state, now, t, lap);
            self.arrivals(show, engine, key, prev, state, pos, now, t);
        }
    }

    fn close(&mut self, key: TimelineKey) {
        if let Some(o) = self.open.remove(&key)
            && o.wall >= MIN_STRETCH
        {
            self.notes.stretches.push(Stretch {
                at: o.t,
                secs: o.wall,
                drift: o.drift,
            });
        }
    }

    fn drift(
        &mut self,
        key: TimelineKey,
        prev: Option<Look>,
        state: RunState,
        pos: f64,
        now: Instant,
        t: f64,
    ) {
        let Some(p) = prev.filter(|p| p.state == RunState::Running) else {
            self.close(key);
            return;
        };
        if state != RunState::Running {
            self.close(key);
            return;
        }
        let wall = now.duration_since(p.at).as_secs_f64();
        let clock = pos - p.pos;
        // The app doesn't count the time before the first tick after it was
        // idle as any cue's (it may have slept up to half a second), so a run
        // started from nothing stands for that one tick: by design, not drift.
        if clock == 0.0 && !self.open.contains_key(&key) {
            return;
        }
        if clock < -1e-6 || (clock - wall).abs() > JUMPED {
            // Put somewhere else (a loop going round, a jump): a new stretch.
            self.close(key);
            return;
        }
        let o = self.open.entry(key).or_insert(Open {
            t,
            wall: 0.0,
            drift: 0.0,
        });
        o.wall += wall;
        o.drift += clock - wall;
    }

    fn cueing(&mut self, key: TimelineKey, state: RunState, now: Instant, t: f64, lap: usize) {
        if state == RunState::Cueing {
            self.cueing.entry(key).or_insert((now, t));
        } else if let Some((since, at)) = self.cueing.remove(&key) {
            let secs = now.duration_since(since).as_secs_f64();
            self.notes.cueing.push(CueSpell {
                at,
                lap,
                secs,
                gave_up: secs >= GAVE_UP_AFTER,
            });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn arrivals(
        &mut self,
        show: &Show,
        engine: &Engine,
        key: TimelineKey,
        prev: Option<Look>,
        state: RunState,
        pos: f64,
        now: Instant,
        t: f64,
    ) {
        let clips = engine.timeline_clips_up(key);
        // Only what the clock reached running on its own is judged: a seek's
        // clips (started part way in while it cued) are cueing's business. A
        // clip whose start came late (its file wasn't ready) is still judged,
        // by its place.
        match (prev.map(|p| p.state), state) {
            (_, RunState::Stopped | RunState::Cueing) => {
                self.from.insert(key, pos);
            }
            (None | Some(RunState::Stopped), _) => {
                self.from.insert(key, 0.0);
            }
            _ => {
                // Put somewhere else between two looks without a look at it
                // cueing (a seek whose clips were all ready is released at
                // once; a loop going round): from about where it was put. A
                // clip it started part way in is not one the clock reached (it
                // was once judged by its place, "33.5 s late").
                if let Some(p) = prev {
                    let wall = now.saturating_duration_since(p.at).as_secs_f64();
                    let ran = if p.state == RunState::Running {
                        wall
                    } else {
                        0.0
                    };
                    if pos < p.pos - 1e-6 || pos - (p.pos + ran) > JUMPED {
                        self.from.insert(key, (pos - wall).max(0.0));
                    }
                }
            }
        }
        let from = self.from.get(&key).copied().unwrap_or(f64::INFINITY);
        for c in &clips {
            if self.seen.contains(&c.instance) || self.due.contains_key(&c.instance) {
                continue;
            }
            match state {
                RunState::Running | RunState::Paused { .. }
                    if c.due >= from - 1e-6 && c.due <= pos + 1e-6 =>
                {
                    self.due.insert(
                        c.instance,
                        Due {
                            key,
                            cue: c.cue,
                            due: c.due,
                            at: t,
                            waited: None,
                        },
                    );
                }
                _ => {
                    self.seen.insert(c.instance);
                }
            }
        }
        let name = |id: CueId| show.cue(id).map_or_else(String::new, |c| c.name.clone());
        let run_on: HashSet<u64> = clips.iter().map(|c| c.instance).collect();
        let mut done = Vec::new();
        // (Each timeline's own: another's clips are not among `clips`.)
        for (instance, d) in self.due.iter_mut().filter(|(_, d)| d.key == key) {
            if let Some(c) = clips.iter().find(|c| c.instance == *instance) {
                if d.waited.is_none() {
                    d.waited = c.waiting;
                }
                if c.up {
                    self.notes.arrivals.push(Arrival {
                        at: d.at,
                        name: name(d.cue),
                        late: Some((pos - d.due).max(0.0)),
                        waited: d.waited,
                    });
                    self.came.push(d.cue);
                    done.push(*instance);
                }
            } else if !run_on.contains(instance) {
                // Gone before it came up: never came, if the clock had passed it
                // by more than a late clip's bound (else it was let go of early).
                if state != RunState::Cueing
                    && pos - d.due > super::analysis::LATE_PROBLEM
                    && engine.timeline_clock(key).is_some()
                {
                    self.notes.arrivals.push(Arrival {
                        at: d.at,
                        name: name(d.cue),
                        late: None,
                        waited: d.waited,
                    });
                }
                done.push(*instance);
            }
        }
        for i in done {
            self.due.remove(&i);
            self.seen.insert(i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::audio::AudioOut;
    use crate::show::{ControlAction, CueKind};
    use crate::testmode::Level;
    use crate::testmode::assets::{self, CATALOG, Kind, Source};
    use crate::testmode::plan::{self, SoundChoice};

    /// The catalog played by the test folder's media (as the plan tests do).
    fn stand_ins() -> Option<(
        crate::media::ffmpeg::Ffmpeg,
        HashMap<&'static str, assets::Resolved>,
    )> {
        let (ff, clip) = crate::media::test_media("testsrc_10s.mp4")?;
        let tone = clip.with_file_name("tone_5s.wav");
        let still = clip.with_file_name("color_red.png");
        if !tone.is_file() || !still.is_file() {
            return None;
        }
        let mut res = HashMap::new();
        for a in CATALOG {
            let (path, kind) = match a.kind {
                Kind::Video => (&clip, Kind::Video),
                Kind::Image => (&still, Kind::Image),
                Kind::Audio => (&tone, Kind::Audio),
            };
            let mut r = assets::check(&ff, path, kind).ok()?;
            r.source = Source::Downloaded;
            res.insert(a.id, r);
        }
        Some((ff, res))
    }

    /// Timeline short, one lap through an engine of its own with the sound mixed
    /// and thrown away, at twice real time: the driver gets it through its
    /// pause with GO, it plays to its Stop, every clip the clock reaches comes
    /// up, and the engine reports nothing wrong.
    #[test]
    fn the_short_timeline_test_plays_to_its_end_with_go_pressed_at_its_pause() {
        let _heavy = crate::testmode::heavy();
        let Some((ff, res)) = stand_ins() else {
            assert!(
                std::env::var("NEWQ_REQUIRE_MEDIA").is_err(),
                "no test media"
            );
            return;
        };
        let built = plan::build_timeline(
            &Show::default(),
            Level::Short,
            &res,
            &[],
            &SoundChoice::default(),
        );
        let show = built.show;
        let script = built.timeline.expect("a script");
        let keys = script.legs.clone();
        let mut engine = Engine::new();
        engine.ff = Some(ff);
        engine.audio = AudioOut::silent();
        let mut driver = Driver::new(script, false);
        let mut watch = Watch::default();
        const SPEED: f64 = 2.0;
        let began = Instant::now();
        let base = began;
        let (mut last, mut sim) = (began, 0.0f64);
        let mut problems = Vec::new();
        let (mut lap_began, mut lap_ended) = (None, None);
        let mut was_active = false;
        while lap_ended.is_none() {
            assert!(
                began.elapsed() < Duration::from_secs(90),
                "the lap never ended: {:?}",
                engine.timeline_clock(keys[0])
            );
            std::thread::sleep(Duration::from_millis(8));
            let real = Instant::now();
            let step = real.duration_since(last).as_secs_f64() * SPEED;
            last = real;
            sim += step;
            let now = base + Duration::from_secs_f64(sim);
            // As the app does: time with nothing playing is no cue's time.
            engine.tick(&show, if was_active { step } else { 0.0 });
            for h in driver.tick(&show, &mut engine, now) {
                match h {
                    Happened::LapBegan => lap_began = Some(sim),
                    Happened::LapEnded => lap_ended = Some(sim),
                    Happened::Stuck(why) => panic!("stuck: {why}"),
                    Happened::Said(_) => {}
                }
            }
            was_active = engine.is_active();
            watch.tick(&show, &engine, &keys, now, sim, driver.laps);
            problems.extend(engine.take_problems());
        }
        engine.stop_all(0.0);
        assert!(driver.finished && driver.laps == 1);
        assert_eq!(driver.presses, 1, "GO at the one pause");
        assert!(problems.is_empty(), "{problems:?}");
        // Every picture, sound and title of the list came up on the clock.
        let clips: Vec<&crate::show::Cue> = show
            .cues
            .iter()
            .filter(|c| c.kind.media().is_some() && !matches!(c.kind, CueKind::Fade(_)))
            .collect();
        assert!(clips.len() >= 9, "{}", clips.len());
        for c in &clips {
            assert!(
                watch.came.contains(&c.id),
                "{} never came up on the clock: {:?}",
                c.name,
                watch.notes.arrivals
            );
        }
        assert!(watch.notes.arrivals.iter().all(|a| a.late.is_some()));
        assert!(show.cues.iter().any(
            |c| matches!(&c.kind, CueKind::Control(k) if k.action == ControlAction::TimelineStop)
        ));
        // The clock was measured and kept time; the lap took what the plan says.
        assert!(!watch.notes.stretches.is_empty());
        let worst = watch
            .notes
            .stretches
            .iter()
            .map(|s| s.drift.abs())
            .fold(0.0, f64::max);
        assert!(worst < 0.1, "{:?}", watch.notes.stretches);
        let took = lap_ended.unwrap() - lap_began.unwrap();
        assert!(
            (took - built.lap_secs).abs() < 6.0,
            "the lap took {took:.1} s of show time; the plan says {:.1}",
            built.lap_secs
        );
    }

    /// A seek the watch never saw cueing (it cued and ran on between two
    /// looks, as a seek whose clips were all ready does at once): what it
    /// started part way in is not judged as late by its place, while what the
    /// clock then reaches is. Texts, no media.
    #[test]
    fn a_seek_not_seen_cueing_is_not_a_late_clip() {
        use crate::show::{CueType, TimelineSettings};
        let mut show = Show::default();
        let l = show.add_list("Programme");
        show.lists.last_mut().unwrap().timeline = Some(TimelineSettings::for_editor());
        let mut add = |at: f64, len: f64| {
            let mut c = show.make_cue(CueType::Text, None, show.cues.len());
            c.pre_wait = at;
            c.list = l;
            if let CueKind::Text(t) = &mut c.kind {
                t.media.end = Some(len);
            }
            let id = c.id;
            show.cues.push(c);
            id
        };
        let first = add(0.5, 3.0);
        let long = add(10.0, 20.0);
        let later = add(16.0, 3.0);
        let key = TimelineKey::List(l);
        let mut e = Engine::for_tests();
        e.output_sizes.insert(show.outputs[0].id, (64, 36));
        let mut watch = Watch::default();
        let base = Instant::now();
        let mut t = 0.0;
        let mut step = |e: &mut Engine, watch: &mut Watch, look: bool| {
            std::thread::sleep(Duration::from_millis(2));
            e.tick(&show, 0.02);
            t += 0.02;
            if look {
                watch.tick(&show, e, &[key], base + Duration::from_secs_f64(t), t, 1);
            }
        };
        e.timeline_run(&show, key);
        while e.timeline_clock(key).is_none_or(|(_, pos)| pos < 4.0) {
            step(&mut e, &mut watch, true);
        }
        // To 14: the long clip starts 4 s in. Not looked at until it runs on.
        e.timeline_seek(&show, key, 14.0, true);
        let began = Instant::now();
        while e
            .timeline_clock(key)
            .is_none_or(|(s, _)| s != RunState::Running)
        {
            assert!(began.elapsed() < Duration::from_secs(10), "it never ran on");
            step(&mut e, &mut watch, false);
        }
        while e.timeline_clock(key).is_none_or(|(_, pos)| pos < 18.0) {
            step(&mut e, &mut watch, true);
        }
        let name = |id: CueId| show.cue(id).unwrap().name.clone();
        assert!(watch.came.contains(&first), "{:?}", watch.notes.arrivals);
        assert!(watch.came.contains(&later), "{:?}", watch.notes.arrivals);
        assert!(!watch.came.contains(&long), "{:?}", watch.notes.arrivals);
        assert!(
            watch
                .notes
                .arrivals
                .iter()
                .all(|a| a.late.is_some_and(|x| x < 0.5)),
            "{} judged: {:?}",
            name(long),
            watch.notes.arrivals
        );
    }

    /// Two timelines watched at once: each one's clips are judged by its own
    /// clock (another's are not "gone" from it). Texts, no media.
    #[test]
    fn the_watch_judges_each_timelines_clips_by_its_own_run() {
        use crate::show::{CueType, GroupMode, TimelineSettings};
        let mut show = Show::default();
        let text = |show: &mut Show, at: f64| {
            let mut c = show.make_cue(CueType::Text, None, show.cues.len());
            c.pre_wait = at;
            if let CueKind::Text(t) = &mut c.kind {
                t.media.end = Some(3.0);
            }
            c
        };
        let mut g = show.make_cue(CueType::Group, None, 0);
        if let CueKind::Group(gc) = &mut g.kind {
            gc.mode = GroupMode::Timeline;
        }
        let gid = g.id;
        show.cues.push(g);
        let mut a = text(&mut show, 0.5);
        a.parent = Some(gid);
        let a = {
            let id = a.id;
            show.cues.push(a);
            id
        };
        let l = show.add_list("Programme");
        show.lists.last_mut().unwrap().timeline = Some(TimelineSettings::for_editor());
        let mut b = text(&mut show, 0.5);
        b.list = l;
        let b = {
            let id = b.id;
            show.cues.push(b);
            id
        };
        let keys = [TimelineKey::List(l), TimelineKey::Group(gid)];
        let mut e = Engine::for_tests();
        e.output_sizes.insert(show.outputs[0].id, (64, 36));
        let mut watch = Watch::default();
        let base = Instant::now();
        // The list runs; the group starts 2 s later, while the list's clip plays.
        e.timeline_run(&show, keys[0]);
        let mut t = 0.0;
        while t < 8.0 {
            std::thread::sleep(Duration::from_millis(2));
            if (t - 2.0f64).abs() < 0.01 {
                e.timeline_run(&show, keys[1]);
            }
            e.tick(&show, 0.02);
            t += 0.02;
            watch.tick(&show, &e, &keys, base + Duration::from_secs_f64(t), t, 1);
        }
        assert!(
            watch.notes.arrivals.iter().all(|x| x.late.is_some()),
            "{:?}",
            watch.notes.arrivals
        );
        assert!(
            watch.came.contains(&a) && watch.came.contains(&b),
            "{:?}",
            watch.notes.arrivals
        );
    }
}
