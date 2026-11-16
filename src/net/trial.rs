//! The sync trial (stage 1 of `.claude/notes/network-plan.md`), simulated: a
//! primary and two followers, each an `Engine` driven by its own clock (an
//! offset and a drift), the followers' shared clocks kept by clock exchanges
//! over a simulated network, and commands timed by the shared clock sent over
//! it. At instants of true time (the simulator's) it looks at the frame each
//! would show. Everything runs on simulated time, so an hour takes seconds.
//!
//! Two ways of playing are compared: `Mode::Synced` (commands carry a shared
//! time; engines tick by the shared clock, `Engine::tick_shared`) and
//! `Mode::AsIs` (each computer does a command when it arrives and ticks by its
//! own clock, as NewQ does today with GO from a remote).
//!
//! The cues are a Wait cue standing for a video (its time is what a video's
//! frame is chosen by, `pts <= t`) and a timeline group with a long Wait clip:
//! the tests in this file with real media show that a video cue's and a
//! timeline clip's shown frames follow those times.

use super::clock::{Exchange, SharedClock};
use super::msg::{Command, Msg};
use super::sim::{Network, NodeClock, Queue, Rng};
use crate::engine::{ActiveKind, Engine};
use crate::show::{CueId, CueKind, CueType, GroupMode, Show};
use crate::timeline::{PauseFreeze, TimelineKey};
use std::collections::{HashMap, HashSet};

/// The engines' frame rate (each by its own clock).
const TICK_HZ: f64 = 60.0;
/// How often (true time) the frames are compared: not a multiple of a frame.
const SAMPLE_EVERY: f64 = 0.0371;
/// A command is sent again this often until it is acknowledged.
const RESEND: f64 = 0.025;
/// A follower asks for the time this often before it is locked, and after.
const ASK_UNLOCKED: f64 = 0.1;
const ASK_LOCKED: f64 = 0.5;
/// How long the primary takes to answer an exchange.
const ANSWER: f64 = 0.000_03;
/// The start of a cue is judged over this long (true seconds).
const START_WINDOW: f64 = 2.0;
/// The frame rates the cue is judged at.
const RATES: [f64; 2] = [30.0, 60.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Synced,
    AsIs,
}

#[derive(Clone, Debug)]
pub struct Trial {
    pub network: Network,
    pub mode: Mode,
    pub seed: u64,
    /// Seconds of show after the first GO.
    pub length: f64,
    /// How far ahead (shared seconds) a command is timed.
    pub lead: f64,
}

/// The differences between the computers over a stretch of samples.
#[derive(Clone, Debug, Default)]
pub struct Diffs {
    pub samples: usize,
    /// Samples where the frame index differed, at 30 and 60 fps.
    pub differ: [usize; 2],
    /// The largest difference in frames, at 30 and 60 fps, and in seconds.
    pub worst: [i64; 2],
    pub worst_secs: f64,
    /// Samples where some had made a jump (a seek) and some not yet: over
    /// `STRADDLE` apart, and left out of the rest.
    pub straddling: usize,
}

/// Content times this far apart are either side of a jump, not a drift.
const STRADDLE: f64 = 0.5;

impl Diffs {
    /// Content times of every computer at one instant.
    fn add(&mut self, times: &[f64]) {
        if times.len() < 2 {
            return;
        }
        let lo = times.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = times.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        if hi - lo > STRADDLE {
            self.straddling += 1;
            return;
        }
        self.samples += 1;
        self.worst_secs = self.worst_secs.max(hi - lo);
        for (k, fps) in RATES.iter().enumerate() {
            let frames: Vec<i64> = times.iter().map(|t| frame(*t, *fps)).collect();
            let d = frames.iter().max().unwrap_or(&0) - frames.iter().min().unwrap_or(&0);
            if d != 0 {
                self.differ[k] += 1;
            }
            self.worst[k] = self.worst[k].max(d);
        }
    }

    pub fn share(&self, k: usize) -> f64 {
        if self.samples == 0 {
            0.0
        } else {
            self.differ[k] as f64 / self.samples as f64
        }
    }
}

/// The frame shown at a cue time `t`: the last whose `pts <= t` (`video.rs`).
pub fn frame(t: f64, fps: f64) -> i64 {
    (t * fps + 1e-9).floor() as i64
}

/// What a trial came to.
#[derive(Clone, Debug, Default)]
pub struct Outcome {
    /// True seconds from the first exchange to each follower's lock.
    pub lock_after: Vec<f64>,
    /// |shared clock error| of the followers, seconds, sampled after lock.
    pub clock_err: Vec<f64>,
    /// The largest error bound the followers believed, after lock.
    pub bound_max: f64,
    /// The video cue: as chosen by each computer's clock at the instant, and as
    /// shown (chosen at its last frame tick before the instant).
    pub cue_chosen: Diffs,
    pub cue_shown: Diffs,
    /// Its first `START_WINDOW` once all had it up.
    pub cue_start: Diffs,
    pub run_chosen: Diffs,
    pub run_shown: Diffs,
    /// The timeline just after its seek, and just after GO after its pause.
    pub run_seek: Diffs,
    pub run_resume: Diffs,
    /// Where each computer's timeline clock stood while paused, the spread.
    pub pause_spread: f64,
    /// The command arrivals: shared seconds to spare (negative: late).
    pub margins: Vec<f64>,
}

/// The show every computer has: the "video" and a timeline group.
pub struct TrialShow {
    pub show: Show,
    pub cue: CueId,
    pub group: CueId,
}

pub fn trial_show() -> TrialShow {
    let mut show = Show::default();
    let mut c = show.make_cue(CueType::Wait, None, show.cues.len());
    if let CueKind::Wait(w) = &mut c.kind {
        w.duration = 1e6;
    }
    let cue = c.id;
    show.cues.push(c);
    let mut g = show.make_cue(CueType::Group, None, show.cues.len());
    if let CueKind::Group(gc) = &mut g.kind {
        gc.mode = GroupMode::Timeline;
    }
    let group = g.id;
    show.cues.push(g);
    // A long clip from 0, and one far ahead so that a seek always has
    // something still to come (a Wait clip is passed, not chased).
    for at in [0.0, 1e5] {
        let mut clip = show.make_cue(CueType::Wait, None, show.cues.len());
        if let CueKind::Wait(w) = &mut clip.kind {
            w.duration = 1e6;
        }
        clip.parent = Some(group);
        clip.pre_wait = at;
        show.cues.push(clip);
    }
    TrialShow { show, cue, group }
}

/// What a computer saw at its last frame tick.
#[derive(Clone, Copy, Debug, Default)]
struct Note {
    /// Its clock (shared, or its own) at the tick.
    src: f64,
    cue: Option<f64>,
    /// The timeline's clock, and whether it runs.
    run: Option<(f64, bool)>,
}

struct Node {
    clock: NodeClock,
    /// A follower's shared clock (the primary's is its own clock).
    est: Option<SharedClock>,
    engine: Engine,
    phase: f64,
    k: u64,
    note: Option<Note>,
    /// Commands to do: by their time (synced), or at the next tick.
    todo: Vec<(f64, Command)>,
    seen: HashSet<u32>,
    asks: HashMap<u32, f64>,
    seq: u32,
    locked_at: Option<f64>,
    last_local: Option<f64>,
}

impl Node {
    fn new(clock: NodeClock, follower: bool, rng: &mut Rng) -> Node {
        Node {
            clock,
            est: follower.then(SharedClock::default),
            engine: Engine::for_tests(),
            phase: rng.range(0.0, 1.0 / TICK_HZ),
            k: 0,
            note: None,
            todo: Vec::new(),
            seen: HashSet::new(),
            asks: HashMap::new(),
            seq: 0,
            locked_at: None,
            last_local: None,
        }
    }

    /// Its clock at true time `t`: the shared one (the primary's own), or in
    /// `AsIs` its own.
    fn src(&self, t: f64, mode: Mode) -> Option<f64> {
        let local = self.clock.local(t);
        match (&self.est, mode) {
            (_, Mode::AsIs) | (None, _) => Some(local),
            (Some(est), Mode::Synced) => est.now(local).map(|n| n.time),
        }
    }
}

enum Ev {
    Tick(usize),
    Ask(usize),
    /// Bytes arriving at the primary (from a follower) or at a follower.
    ToPrimary(usize, Vec<u8>),
    ToFollower(usize, Vec<u8>),
    Issue(usize),
    Resend(usize, u32),
    Sample,
}

fn apply(e: &mut Engine, show: &Show, what: Command) {
    match what {
        Command::Go => e.go(show),
        Command::Fire(id) => e.fire(show, id),
        Command::StopAll => e.stop_all(0.0),
        Command::TimelineGo(k) => e.timeline_go_one(show, k),
        Command::TimelinePause(k) => {
            e.timeline_pause(show, k, PauseFreeze::Everything, &[], false, true)
        }
        Command::TimelineSeek(k, pos) => e.timeline_seek(show, k, pos, true),
    }
}

/// Run one trial.
pub fn run(t: &Trial) -> Outcome {
    let ts = trial_show();
    let show = &ts.show;
    let key = TimelineKey::Group(ts.group);
    let mut rng = Rng::new(t.seed);
    // The primary's clock is the shared one; the followers are 50 ppm apart
    // (each 25 from it) and seconds off.
    let clocks = [
        NodeClock {
            offset: 1000.0,
            ppm: 0.0,
        },
        NodeClock {
            offset: 1000.0 - 4.237,
            ppm: 25.0,
        },
        NodeClock {
            offset: 1000.0 + 7.911,
            ppm: -25.0,
        },
    ];
    let mut nodes: Vec<Node> = clocks
        .iter()
        .enumerate()
        .map(|(i, c)| Node::new(*c, i > 0, &mut rng))
        .collect();
    let mut q: Queue<Ev> = Queue::default();
    for (i, n) in nodes.iter().enumerate() {
        q.push(n.clock.true_at(n.clock.local(0.0) + n.phase), Ev::Tick(i));
        if i > 0 {
            q.push(rng.range(0.0, 0.05), Ev::Ask(i));
        }
    }
    let mut out = Outcome::default();
    // The commands, once both followers are locked: (shared time to issue, what).
    let mut plan: Vec<(f64, Command)> = Vec::new();
    let mut issued_seq = 0u32;
    let mut unacked: HashSet<(usize, u32)> = HashSet::new();
    let mut sent: HashMap<u32, (f64, Command)> = HashMap::new();
    let mut end = f64::INFINITY;
    // True times of the GO, the seek and the GO after the pause.
    let mut marks: Vec<f64> = Vec::new();
    let mut pause_at = None;
    let mut started = false;

    while let Some((now, ev)) = q.pop() {
        if now > end {
            break;
        }
        match ev {
            Ev::Tick(i) => {
                let n = &mut nodes[i];
                n.k += 1;
                let local = n.clock.local(now);
                // Next frame by its own clock, read a little late now and then.
                let next = n
                    .clock
                    .true_at(n.phase + n.clock.local(0.0) + n.k as f64 / TICK_HZ)
                    + rng.range(0.0, 0.001);
                q.push(next.max(now), Ev::Tick(i));
                let Some(src) = n.src(now, t.mode) else {
                    continue;
                };
                match t.mode {
                    Mode::Synced => {
                        n.todo.sort_by(|a, b| a.0.total_cmp(&b.0));
                        while n.todo.first().is_some_and(|c| c.0 <= src) {
                            let (at, what) = n.todo.remove(0);
                            n.engine.tick_shared(show, at);
                            apply(&mut n.engine, show, what);
                        }
                        n.engine.tick_shared(show, src);
                    }
                    Mode::AsIs => {
                        for (_, what) in std::mem::take(&mut n.todo) {
                            apply(&mut n.engine, show, what);
                        }
                        let dt = n.last_local.map_or(0.0, |l| local - l);
                        n.engine.tick(show, dt);
                    }
                }
                n.last_local = Some(local);
                let e = &n.engine;
                n.note = Some(Note {
                    src,
                    cue: e
                        .active
                        .iter()
                        .find(|a| a.cue_id == ts.cue && matches!(a.kind, ActiveKind::Wait))
                        .map(|a| a.elapsed),
                    run: e.timeline_clock(key).and_then(|(state, pos)| match state {
                        crate::engine::RunState::Stopped => None,
                        s => Some((pos, s == crate::engine::RunState::Running)),
                    }),
                });
            }
            Ev::Ask(i) => {
                let n = &mut nodes[i];
                let locked = n.est.as_ref().is_some_and(|c| c.locked());
                let every = if locked { ASK_LOCKED } else { ASK_UNLOCKED };
                q.push(now + every * rng.range(0.9, 1.1), Ev::Ask(i));
                n.seq += 1;
                let t1 = n.clock.local(now);
                n.asks.insert(n.seq, t1);
                let msg = Msg::ClockAsk { seq: n.seq, t1 }.encode();
                if let Some(d) = t.network.up.delay(&mut rng) {
                    q.push(now + d, Ev::ToPrimary(i, msg));
                }
            }
            Ev::ToPrimary(from, bytes) => match Msg::decode(&bytes) {
                Ok(Msg::ClockAsk { seq, t1 }) => {
                    let t2 = nodes[0].clock.local(now);
                    let reply = Msg::ClockReply {
                        seq,
                        t1,
                        t2,
                        t3: t2 + ANSWER,
                    }
                    .encode();
                    if let Some(d) = t.network.down.delay(&mut rng) {
                        q.push(now + ANSWER + d, Ev::ToFollower(from, reply));
                    }
                }
                Ok(Msg::Ack { seq }) => {
                    unacked.remove(&(from, seq));
                }
                _ => {}
            },
            Ev::ToFollower(to, bytes) => match Msg::decode(&bytes) {
                Ok(Msg::ClockReply { seq, t1, t2, t3 }) => {
                    let n = &mut nodes[to];
                    if n.asks.remove(&seq) != Some(t1) {
                        continue;
                    }
                    let t4 = n.clock.local(now);
                    let Some(est) = n.est.as_mut() else {
                        continue;
                    };
                    est.update(Exchange { t1, t2, t3, t4 }, t4);
                    if est.locked() && n.locked_at.is_none() {
                        n.locked_at = Some(now);
                    }
                    // Both locked: the show begins a little later.
                    if plan.is_empty() && nodes[1..].iter().all(|n| n.locked_at.is_some()) {
                        out.lock_after = nodes[1..]
                            .iter()
                            .map(|n| n.locked_at.unwrap_or(now))
                            .collect();
                        let s0 = nodes[0].clock.local(now) + 2.0;
                        let third = t.length / 3.0;
                        plan = vec![
                            (s0, Command::Fire(ts.cue)),
                            (s0, Command::TimelineGo(key)),
                            (s0 + third, Command::TimelineSeek(key, 100.0)),
                            (s0 + 2.0 * third, Command::TimelinePause(key)),
                            (s0 + 2.0 * third + 30.0, Command::TimelineGo(key)),
                        ];
                        for (k, (at, _)) in plan.iter().enumerate() {
                            q.push(nodes[0].clock.true_at(*at), Ev::Issue(k));
                        }
                        let first = nodes[0].clock.true_at(s0);
                        end = first + t.lead + t.length;
                        q.push(first, Ev::Sample);
                    }
                }
                Ok(Msg::Command { seq, at, what }) => {
                    let ack = Msg::Ack { seq }.encode();
                    if let Some(d) = t.network.up.delay(&mut rng) {
                        q.push(now + d, Ev::ToPrimary(to, ack));
                    }
                    let n = &mut nodes[to];
                    if n.seen.insert(seq) {
                        if let Some(src) = n.src(now, Mode::Synced) {
                            out.margins.push(at - src);
                        }
                        n.todo.push((at, what));
                    }
                }
                _ => {}
            },
            Ev::Issue(k) => {
                let (issue, what) = plan[k];
                let at = issue + t.lead;
                issued_seq += 1;
                let seq = issued_seq;
                sent.insert(seq, (at, what));
                nodes[0].todo.push((at, what));
                let at_true = nodes[0].clock.true_at(at);
                match what {
                    Command::TimelineSeek(..) => marks.push(at_true),
                    Command::TimelineGo(_) if k > 1 => marks.push(at_true),
                    Command::TimelinePause(_) => pause_at = Some(at_true),
                    _ => {}
                }
                for to in 1..nodes.len() {
                    unacked.insert((to, seq));
                    q.push(now, Ev::Resend(to, seq));
                }
            }
            Ev::Resend(to, seq) => {
                if !unacked.contains(&(to, seq)) {
                    continue;
                }
                let (at, what) = sent[&seq];
                let msg = Msg::Command { seq, at, what }.encode();
                if let Some(d) = t.network.down.delay(&mut rng) {
                    q.push(now + d, Ev::ToFollower(to, msg));
                }
                q.push(now + RESEND, Ev::Resend(to, seq));
            }
            Ev::Sample => {
                q.push(now + SAMPLE_EVERY, Ev::Sample);
                sample(
                    &mut nodes,
                    now,
                    t.mode,
                    &marks,
                    pause_at,
                    &mut started,
                    &mut out,
                );
            }
        }
    }
    out
}

/// Look at every computer at true time `now`.
fn sample(
    nodes: &mut [Node],
    now: f64,
    mode: Mode,
    marks: &[f64],
    pause_at: Option<f64>,
    started: &mut bool,
    out: &mut Outcome,
) {
    let truth = nodes[0].clock.local(now);
    for n in &nodes[1..] {
        if let (Some(est), Some(_)) = (&n.est, n.locked_at)
            && let Some(s) = est.now(n.clock.local(now))
        {
            out.clock_err.push((s.time - truth).abs());
            out.bound_max = out.bound_max.max(s.error);
        }
    }
    // (what its last tick saw, its clock now, a timeline command due by now
    // that its next tick will do)
    type Seen = (Note, Option<f64>, Option<(f64, Command)>);
    let notes: Vec<Seen> = nodes
        .iter()
        .filter_map(|n| {
            let src = n.src(now, mode);
            let due = match mode {
                Mode::Synced => n
                    .todo
                    .iter()
                    .filter(|c| src.is_some_and(|s| c.0 <= s))
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .copied(),
                Mode::AsIs => None,
            };
            n.note.map(|note| (note, src, due))
        })
        .collect();
    if notes.len() < nodes.len() {
        return;
    }
    // A cue's time at the instant: as its last tick had it (shown), and carried
    // on by its clock to the instant (chosen).
    let cue: Vec<(f64, f64)> = notes
        .iter()
        .filter_map(|(n, src, _)| {
            let t = n.cue?;
            Some((t, t + src.map_or(0.0, |s| s - n.src)))
        })
        .collect();
    if cue.len() == nodes.len() {
        let shown: Vec<f64> = cue.iter().map(|c| c.0).collect();
        let chosen: Vec<f64> = cue.iter().map(|c| c.1).collect();
        if !*started {
            *started = true;
            out.cue_start = Diffs::default();
        }
        out.cue_shown.add(&shown);
        out.cue_chosen.add(&chosen);
        if out.cue_start.samples < (START_WINDOW / SAMPLE_EVERY) as usize {
            out.cue_start.add(&chosen);
        }
    }
    let run: Vec<(f64, f64, bool)> = notes
        .iter()
        .filter_map(|(n, src, due)| {
            let (pos, running) = n.run?;
            let src = src.unwrap_or(n.src);
            let on = if running { src - n.src } else { 0.0 };
            // By its clock, a command due since its last tick has been done.
            let chosen = match due {
                Some((at, Command::TimelineSeek(_, p))) => p + (src - at),
                Some((at, Command::TimelineGo(_))) if !running => pos + (src - at),
                Some((at, Command::TimelinePause(_))) if running => pos + (at - n.src),
                _ => pos + on,
            };
            Some((pos, chosen, running))
        })
        .collect();
    if run.len() == nodes.len() {
        let shown: Vec<f64> = run.iter().map(|r| r.0).collect();
        let chosen: Vec<f64> = run.iter().map(|r| r.1).collect();
        out.run_shown.add(&shown);
        out.run_chosen.add(&chosen);
        let near = |m: Option<&f64>| m.is_some_and(|m| now >= *m && now < m + START_WINDOW);
        if near(marks.first()) {
            out.run_seek.add(&chosen);
        }
        if near(marks.get(1)) {
            out.run_resume.add(&chosen);
        }
        if run.iter().all(|r| !r.2) && pause_at.is_some_and(|p| now > p + 1.0) {
            let lo = shown.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = shown.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            out.pause_spread = out.pause_spread.max(hi - lo);
        }
    }
}

/// Seconds a command took to reach every follower (resent every `RESEND`
/// until it got there), over `n` commands.
pub fn deliveries(net: &Network, followers: usize, n: usize, seed: u64) -> Vec<f64> {
    let mut rng = Rng::new(seed);
    (0..n)
        .map(|_| {
            (0..followers)
                .map(|_| {
                    let mut k = 0.0;
                    loop {
                        if let Some(d) = net.down.delay(&mut rng) {
                            break k * RESEND + d;
                        }
                        k += 1.0;
                    }
                })
                .fold(0.0, f64::max)
        })
        .collect()
}

/// The value at `q` (0..1) of `v`, sorted in place.
pub fn quantile(v: &mut [f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let i = ((v.len() - 1) as f64 * q).round() as usize;
    v[i]
}

/// Every network, both ways of playing, as a Markdown report.
pub fn report(length: f64) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let ms = |x: f64| format!("{:.3}", x * 1000.0);
    for net in Network::all() {
        let _ = writeln!(s, "### {}\n", net.name);
        let mut d = deliveries(&net, 2, 200_000, 99);
        let (p50, p99, p9999, max) = (
            quantile(&mut d, 0.5),
            quantile(&mut d, 0.99),
            quantile(&mut d, 0.9999),
            quantile(&mut d, 1.0),
        );
        let lead = ((max * 1.5 + 0.005) * 100.0).ceil() / 100.0;
        let mut rows = Vec::new();
        for mode in [Mode::Synced, Mode::AsIs] {
            let o = run(&Trial {
                network: net.clone(),
                mode,
                seed: 7,
                length,
                lead,
            });
            rows.push((mode, o));
        }
        let synced = &rows[0].1;
        let mut err = synced.clock_err.clone();
        let _ = writeln!(
            s,
            "Clock (followers, after lock, {} samples): median {} ms, p99 {} ms, worst {} ms; \
             largest error bound believed {} ms; locked after {} s.\n",
            err.len(),
            ms(quantile(&mut err, 0.5)),
            ms(quantile(&mut err, 0.99)),
            ms(quantile(&mut err, 1.0)),
            ms(synced.bound_max),
            synced
                .lock_after
                .iter()
                .map(|t| format!("{t:.2}"))
                .collect::<Vec<_>>()
                .join(" and "),
        );
        let _ = writeln!(
            s,
            "Command delivery to both followers (200 000 simulated, resent every {} ms): median {} ms, \
             p99 {} ms, p99.99 {} ms, worst {} ms. Lead used: {} ms; least to spare in the trial: {} ms.\n",
            ms(RESEND),
            ms(p50),
            ms(p99),
            ms(p9999),
            ms(max),
            ms(lead),
            ms(synced.margins.iter().copied().fold(f64::INFINITY, f64::min)),
        );
        let _ = writeln!(
            s,
            "| Mode | What | Samples | Differ 30 fps | Differ 60 fps | Worst (frames 30/60) | Worst (ms) | Either side of a jump |"
        );
        let _ = writeln!(s, "|---|---|---|---|---|---|---|---|");
        for (mode, o) in &rows {
            let name = match mode {
                Mode::Synced => "Synced",
                Mode::AsIs => "As is",
            };
            for (what, d) in [
                ("Cue, start (first 2 s)", &o.cue_start),
                ("Cue, clock at the instant", &o.cue_chosen),
                ("Cue, as shown (last frame tick)", &o.cue_shown),
                ("Timeline, clock at the instant", &o.run_chosen),
                ("Timeline, as shown", &o.run_shown),
                ("Timeline, 2 s after the seek", &o.run_seek),
                ("Timeline, 2 s after GO from the pause", &o.run_resume),
            ] {
                let _ = writeln!(
                    s,
                    "| {name} | {what} | {} | {:.2} % | {:.2} % | {} / {} | {} | {} |",
                    d.samples,
                    d.share(0) * 100.0,
                    d.share(1) * 100.0,
                    d.worst[0],
                    d.worst[1],
                    ms(d.worst_secs),
                    d.straddling,
                );
            }
            let _ = writeln!(
                s,
                "| {name} | Timeline paused: spread of the clocks | | | | | {} | |",
                ms(o.pause_spread)
            );
        }
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::RunState;
    use crate::show::Transition;
    use std::net::UdpSocket;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant};

    fn quick(net: Network, mode: Mode) -> Outcome {
        run(&Trial {
            network: net,
            mode,
            seed: 3,
            length: 240.0,
            lead: 0.5,
        })
    }

    #[test]
    fn on_a_wired_lan_the_computers_choose_the_same_frame_by_the_shared_clock() {
        let o = quick(Network::wired(), Mode::Synced);
        assert_eq!(o.lock_after.len(), 2);
        assert!(o.lock_after.iter().all(|t| *t < 10.0), "{:?}", o.lock_after);
        let mut err = o.clock_err.clone();
        assert!(
            quantile(&mut err, 1.0) < 0.000_5,
            "{}",
            quantile(&mut err, 1.0)
        );
        assert!(o.margins.iter().all(|m| *m > 0.0), "a command came late");
        for d in [&o.cue_chosen, &o.run_chosen, &o.run_seek, &o.run_resume] {
            assert!(d.samples > 0);
            assert!(d.worst[1] <= 1 && d.worst_secs < 0.001, "{d:?}");
            assert!(d.share(1) < 0.05, "{d:?}");
        }
        assert!(o.pause_spread < 0.001, "{}", o.pause_spread);
        // Shown at each one's last frame tick, unaligned: at most a frame or so apart.
        assert!(o.cue_shown.worst_secs < 1.5 / TICK_HZ, "{:?}", o.cue_shown);
    }

    #[test]
    fn on_wifi_the_shared_clock_still_keeps_them_within_a_frame() {
        let o = quick(Network::wifi(), Mode::Synced);
        assert!(o.margins.iter().all(|m| *m > 0.0), "a command came late");
        for d in [&o.cue_chosen, &o.run_chosen] {
            assert!(d.worst[0] <= 1 && d.worst_secs < 0.010, "{d:?}");
        }
    }

    #[test]
    fn as_nearly_as_today_they_drift_apart_and_start_apart() {
        let synced = quick(Network::poor_wired(), Mode::Synced);
        let as_is = quick(Network::poor_wired(), Mode::AsIs);
        // 50 ppm apart for minutes: well past a frame at 60 fps by the end.
        assert!(as_is.cue_chosen.worst_secs > synced.cue_chosen.worst_secs * 5.0);
        assert!(
            as_is.cue_chosen.worst_secs > 0.005,
            "{:?}",
            as_is.cue_chosen
        );
        assert!(as_is.pause_spread > synced.pause_spread);
    }

    #[test]
    fn a_trial_is_the_same_each_time() {
        let a = quick(Network::wifi(), Mode::Synced);
        let b = quick(Network::wifi(), Mode::Synced);
        assert_eq!(a.clock_err, b.clock_err);
        assert_eq!(a.margins, b.margins);
    }

    /// The whole report, an hour each way on each network (a few minutes of
    /// processor time): `cargo test net::trial -- --ignored --nocapture`.
    /// `NEWQ_TRIAL_OUT` names a file to write it to.
    #[test]
    #[ignore]
    fn the_hour_long_report() {
        let r = report(3600.0);
        println!("{r}");
        if let Ok(path) = std::env::var("NEWQ_TRIAL_OUT") {
            std::fs::write(path, &r).unwrap();
        }
    }

    /// A Wait cue's own time on an engine ticked by a shared clock.
    fn wait_time(e: &Engine, id: CueId) -> Option<f64> {
        e.active.iter().find(|a| a.cue_id == id).map(|a| a.elapsed)
    }

    #[test]
    fn by_a_shared_clock_a_pre_wait_ends_and_a_pause_holds_exactly_between_ticks() {
        let mut ts = trial_show();
        ts.show.cues[0].pre_wait = 0.25;
        let (show, id) = (&ts.show, ts.cue);
        let mut e = Engine::for_tests();
        e.tick_shared(show, 10.0);
        e.fire(show, id);
        // Ticks at an odd rate, none on 10.25: the cue's time is still the
        // clock's less 10.25 at each.
        let mut t = 10.0;
        while t < 11.0 {
            t += 1.0 / 47.0;
            e.tick_shared(show, t);
            if t >= 10.25 + 1.0 / 47.0 {
                let got = wait_time(&e, id).unwrap();
                assert!((got - (t - 10.25)).abs() < 1e-9, "{got} at {t}");
            }
        }
        // Paused for 2 s of shared time: its time stands, then goes on from there.
        let before = wait_time(&e, id).unwrap();
        e.set_paused(true);
        for k in 1..=100 {
            e.tick_shared(show, t + k as f64 * 0.02);
        }
        assert!((wait_time(&e, id).unwrap() - before).abs() < 1e-9);
        e.set_paused(false);
        e.tick_shared(show, t + 2.5);
        let got = wait_time(&e, id).unwrap();
        assert!((got - (before + 0.5)).abs() < 1e-9, "{got}");
        // The shared clock going back is taken as standing still.
        e.tick_shared(show, t);
        assert!((wait_time(&e, id).unwrap() - got).abs() < 1e-9);
    }

    #[test]
    fn without_a_shared_clock_the_engine_sums_its_ticks_as_before() {
        let ts = trial_show();
        let mut e = Engine::for_tests();
        e.fire(&ts.show, ts.cue);
        for _ in 0..10 {
            e.tick(&ts.show, 0.1);
        }
        let got = wait_time(&e, ts.cue).unwrap();
        assert!((got - 1.0).abs() < 1e-9, "{got}");
    }

    // ----- Real sockets, and real media -------------------------------------

    /// Busy-wait `d`: a sleep can overshoot by a scheduler tick.
    fn spin(d: Duration) {
        let t = Instant::now();
        while t.elapsed() < d {
            std::hint::spin_loop();
        }
    }

    #[test]
    fn the_clock_locks_over_real_udp_on_loopback() {
        // The primary's clock is ours plus OFFSET; each way is held up DELAY.
        const OFFSET: f64 = 1234.567;
        const DELAY: Duration = Duration::from_micros(1500);
        let base = Instant::now();
        let secs = move || base.elapsed().as_secs_f64();
        let primary = UdpSocket::bind("127.0.0.1:0").unwrap();
        let follower = UdpSocket::bind("127.0.0.1:0").unwrap();
        follower.connect(primary.local_addr().unwrap()).unwrap();
        primary
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        follower
            .set_read_timeout(Some(Duration::from_millis(500)))
            .unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let answers = std::thread::spawn(move || {
            let mut buf = [0u8; 1500];
            while !stop2.load(Ordering::Relaxed) {
                let Ok((n, from)) = primary.recv_from(&mut buf) else {
                    continue;
                };
                if let Ok(Msg::ClockAsk { seq, t1 }) = Msg::decode(&buf[..n]) {
                    let t2 = secs() + OFFSET;
                    let t3 = secs() + OFFSET;
                    spin(DELAY);
                    let reply = Msg::ClockReply { seq, t1, t2, t3 }.encode();
                    let _ = primary.send_to(&reply, from);
                }
            }
        });
        let mut clock = SharedClock::default();
        let mut buf = [0u8; 1500];
        let mut seq = 0;
        let mut after_lock = 0;
        let t0 = Instant::now();
        while after_lock < 40 && t0.elapsed() < Duration::from_secs(60) {
            seq += 1;
            let t1 = secs();
            spin(DELAY);
            follower.send(&Msg::ClockAsk { seq, t1 }.encode()).unwrap();
            if let Ok(n) = follower.recv(&mut buf)
                && let Ok(Msg::ClockReply {
                    seq: s,
                    t1: back,
                    t2,
                    t3,
                }) = Msg::decode(&buf[..n])
                && s == seq
                && back == t1
            {
                let t4 = secs();
                clock.update(Exchange { t1, t2, t3, t4 }, t4);
                if clock.locked() {
                    after_lock += 1;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        stop.store(true, Ordering::Relaxed);
        answers.join().unwrap();
        assert!(clock.locked(), "never locked");
        let local = secs();
        let now = clock.now(local).unwrap();
        let err = now.time - (local + OFFSET);
        // The delays are equal each way, so only the loopback's own jitter is left.
        assert!(err.abs() < 0.002, "off by {err}");
        assert!(
            now.error >= err.abs(),
            "the bound {} under {err}",
            now.error
        );
    }

    /// The time of the frame a video cue (or a clip) shows.
    fn shown(e: &Engine, id: CueId) -> Option<f64> {
        e.shown_frame(id)
    }

    /// Hold the shared clock at `t` on every engine until each shows `want`
    /// (its decoders catch up), for at most a minute of real time.
    fn settle(es: &mut [Engine], show: &Show, id: CueId, t: f64, want: f64) {
        let t0 = Instant::now();
        loop {
            for e in es.iter_mut() {
                e.tick_shared(show, t);
            }
            if es
                .iter()
                .all(|e| shown(e, id).is_some_and(|p| (p - want).abs() < 1e-6))
            {
                return;
            }
            let got: Vec<Option<f64>> = es.iter().map(|e| shown(e, id)).collect();
            assert!(
                t0.elapsed() < Duration::from_secs(60),
                "at {t} wanted {want}, shown {got:?}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn video_cue(show: &mut Show, path: std::path::PathBuf) -> CueId {
        let mut c = show.make_cue(CueType::Video, Some(path), show.cues.len());
        let m = c.kind.media_mut().unwrap();
        m.transition = Transition::Cut;
        m.fade_in = 0.0;
        m.fade_out = 0.0;
        let id = c.id;
        show.cues.push(c);
        id
    }

    #[test]
    fn a_warm_and_a_cold_engine_show_the_same_frame_of_a_real_video_by_the_shared_clock() {
        let Some((ff, path)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let mut show = Show::default();
        let id = video_cue(&mut show, path);
        let mut es = [Engine::for_tests(), Engine::for_tests()];
        for e in &mut es {
            e.ff = Some(ff.clone());
            e.tick_shared(&show, 0.0);
        }
        // One has the cue ready ahead; the other starts FFmpeg at GO.
        es[0].load_ahead(&show, id);
        // GO at shared time 1 on both, then on at a frame tick of each's own
        // (60 and 50 a second, out of step).
        for e in &mut es {
            e.tick_shared(&show, 1.0);
            e.fire(&show, id);
        }
        let (mut a, mut b): (f64, f64) = (1.0, 1.0);
        for check in [1.5, 2.25, 4.0] {
            while a < check || b < check {
                if a <= b {
                    a = (a + 1.0 / 60.0).min(check);
                    es[0].tick_shared(&show, a);
                } else {
                    b = (b + 1.0 / 50.0).min(check);
                    es[1].tick_shared(&show, b);
                }
            }
            // The frame due at the cue's time, whose pts <= it.
            let want = frame(check - 1.0, 30.0) as f64 / 30.0;
            settle(&mut es, &show, id, check, want);
        }
    }

    #[test]
    fn a_timeline_clip_on_two_engines_shows_the_frame_its_clock_says_after_a_seek() {
        let Some((ff, path)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let mut show = Show::default();
        let mut g = show.make_cue(CueType::Group, None, 0);
        if let CueKind::Group(gc) = &mut g.kind {
            gc.mode = GroupMode::Timeline;
        }
        let gid = g.id;
        show.cues.push(g);
        let clip = video_cue(&mut show, path);
        {
            let c = show.cue_mut(clip).unwrap();
            c.parent = Some(gid);
            c.pre_wait = 0.5;
        }
        let key = TimelineKey::Group(gid);
        let mut es = [Engine::for_tests(), Engine::for_tests()];
        for e in &mut es {
            e.ff = Some(ff.clone());
            e.tick_shared(&show, 0.0);
            e.tick_shared(&show, 1.0);
            e.timeline_go_one(&show, key);
        }
        // The clip comes in at clock 0.5 (shared 1.5); judged at shared 2.
        for e in &mut es {
            for k in 1..=60 {
                e.tick_shared(&show, 1.0 + k as f64 / 60.0);
            }
        }
        settle(&mut es, &show, clip, 2.0, frame(0.5, 30.0) as f64 / 30.0);
        // Seek to clock 4 at shared 2: it cues (FFmpeg starts again), then
        // catches up with the shared clock; at shared 3 the clock is at 5.
        for e in &mut es {
            e.timeline_seek(&show, key, 4.0, true);
        }
        for k in 1..=60 {
            for e in &mut es {
                e.tick_shared(&show, 2.0 + k as f64 / 60.0);
            }
        }
        for e in &es {
            let (state, pos) = e.timeline_clock(key).unwrap();
            assert!(
                state == RunState::Cueing
                    || (state == RunState::Running && (pos - 5.0).abs() < 1e-6),
                "{state:?} {pos}"
            );
        }
        // Clock 5 is the clip's 4.5 s.
        settle(&mut es, &show, clip, 3.0, frame(4.5, 30.0) as f64 / 30.0);
        for e in &es {
            let (state, pos) = e.timeline_clock(key).unwrap();
            assert_eq!(state, RunState::Running);
            assert!((pos - 5.0).abs() < 1e-6, "{pos}");
        }
    }
}
