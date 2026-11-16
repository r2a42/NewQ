//! Computers playing together, without sockets: a primary and followers, each a
//! whole `Engine` ticking at 60 Hz by its own drifting clock, the followers'
//! shared clocks kept by exchanges over a simulated network (`sim.rs`), and the
//! commands and heartbeats of `play.rs` carried over it. The operator's inputs
//! are made on the primary's engine as the app makes them (`go`, `fire`,
//! `timeline_seek`...), caught by its gate and timed by its `Leader`.

use super::*;
use crate::engine::{ActiveKind, Engine, NetMode, RunState};
use crate::net::clock::{Exchange, SharedClock};
use crate::net::msg::Msg as ClockMsg;
use crate::net::sim::{Network, NodeClock, Queue, Rng};
use crate::net::trial::{TrialShow, trial_show};

const TICK_HZ: f64 = 60.0;
const SAMPLE: f64 = 0.0371;

/// At a tick: its clock, the cue's time, the timeline's (and whether it runs).
type Note = (f64, Option<f64>, Option<(f64, bool)>);

/// What the operator does on the primary.
#[derive(Clone, Copy, Debug)]
enum Act {
    Go,
    Fire,
    RunTimeline,
    Seek(f64),
    Pause,
    StopAll,
}

#[derive(Clone, Copy, Debug)]
enum Ev {
    Tick(usize),
    Ask(usize),
    Act(usize),
    Sample,
    /// A follower starts again from nothing (clock, engine, everything).
    Restart(usize),
}

enum Wire {
    Clock(Vec<u8>),
    Command(Timed),
    Ack(u64, u64),
    State(PlayState),
}

struct Node {
    clock: NodeClock,
    est: Option<SharedClock>,
    engine: Engine,
    leader: Option<Leader>,
    follower: Option<Follower>,
    phase: f64,
    k: u64,
    seq: u32,
    asks: Vec<(u32, f64)>,
    last_local: Option<f64>,
    /// At its last tick: its clock, the cue's time and the timeline's.
    note: Option<Note>,
}

impl Node {
    fn new(clock: NodeClock, primary: bool, rng: &mut Rng) -> Node {
        let mut engine = Engine::for_tests();
        engine.net_set_mode(if primary {
            NetMode::Primary
        } else {
            NetMode::Follower
        });
        Node {
            clock,
            est: (!primary).then(SharedClock::default),
            engine,
            leader: primary.then(|| Leader::new(7)),
            follower: (!primary).then(Follower::default),
            phase: rng.range(0.0, 1.0 / TICK_HZ),
            k: 0,
            seq: 0,
            asks: Vec::new(),
            last_local: None,
            note: None,
        }
    }

    /// The shared time it believes at true time `t` (its own for the primary),
    /// once locked.
    fn shared(&self, t: f64) -> Option<f64> {
        let local = self.clock.local(t);
        match &self.est {
            None => Some(local),
            Some(e) => e.now(local).filter(|n| n.locked).map(|n| n.time),
        }
    }
}

struct World {
    ts: TrialShow,
    nodes: Vec<Node>,
    net: Network,
    rng: Rng,
    q: Queue<Ev>,
    wires: Queue<(usize, usize, Wire)>,
    script: Vec<(f64, Act)>,
    /// Commands to this follower are lost while the true time is in this span.
    lose_commands: Option<(usize, f64, f64)>,
    /// One extra delay (s) on commands to a follower: (who, delay).
    late: Option<(usize, f64)>,
    lead: f64,
    /// The largest frame difference seen (60 fps) after `judge_from`, and
    /// where it was.
    worst: i64,
    worst_at: f64,
    judge_from: f64,
    /// Spans (true time) not judged: a restart's catching up.
    skip: Vec<(f64, f64)>,
    samples: usize,
    straddles: usize,
}

fn frame(t: f64) -> i64 {
    (t * TICK_HZ + 1e-9).floor() as i64
}

impl World {
    fn new(followers: usize, net: Network, seed: u64) -> World {
        let mut rng = Rng::new(seed);
        let clocks = [
            NodeClock {
                offset: 1000.0,
                ppm: 0.0,
            },
            NodeClock {
                offset: 1000.0 - 4.2,
                ppm: 25.0,
            },
            NodeClock {
                offset: 1000.0 + 7.9,
                ppm: -25.0,
            },
        ];
        let nodes: Vec<Node> = (0..=followers)
            .map(|i| Node::new(clocks[i], i == 0, &mut rng))
            .collect();
        let mut q = Queue::default();
        q.push(0.0, Ev::Sample);
        for (i, n) in nodes.iter().enumerate() {
            q.push(n.phase, Ev::Tick(i));
            if i > 0 {
                q.push(rng.range(0.0, 0.05), Ev::Ask(i));
            }
        }
        World {
            ts: trial_show(),
            nodes,
            net,
            rng,
            q,
            wires: Queue::default(),
            script: Vec::new(),
            lose_commands: None,
            late: None,
            lead: 0.05,
            worst: 0,
            worst_at: 0.0,
            judge_from: f64::INFINITY,
            skip: Vec::new(),
            samples: 0,
            straddles: 0,
        }
    }

    fn send(&mut self, now: f64, from: usize, to: usize, w: Wire) {
        let link = if to == 0 { self.net.up } else { self.net.down };
        let Some(mut d) = link.delay(&mut self.rng) else {
            return;
        };
        if let Wire::Command(_) = &w {
            if self
                .lose_commands
                .is_some_and(|(who, a, b)| who == to && now >= a && now < b)
            {
                return;
            }
            if let Some((who, extra)) = self.late
                && who == to
            {
                d += extra;
                self.late = None;
            }
        }
        self.wires.push(now + d, (from, to, w));
    }

    /// The operator does these at these true times.
    fn acts(&mut self, acts: &[(f64, Act)]) {
        for a in acts {
            self.q.push(a.0, Ev::Act(self.script.len()));
            self.script.push(*a);
        }
    }

    /// Run until true time `end`.
    fn run(&mut self, end: f64) {
        loop {
            let next_q = self.q.peek_time();
            let next_w = self.wires.peek_time();
            let t = match (next_q, next_w) {
                (Some(a), Some(b)) => a.min(b),
                (Some(a), None) => a,
                (None, Some(b)) => b,
                (None, None) => break,
            };
            if t > end {
                break;
            }
            if next_w == Some(t) {
                let (now, (from, to, w)) = self.wires.pop().unwrap();
                self.deliver(now, from, to, w);
            } else {
                let (now, ev) = self.q.pop().unwrap();
                self.event(now, ev);
            }
        }
    }

    fn deliver(&mut self, now: f64, from: usize, to: usize, w: Wire) {
        let show = &self.ts.show;
        match w {
            Wire::Clock(bytes) => match ClockMsg::decode(&bytes) {
                Ok(ClockMsg::ClockAsk { seq, t1 }) => {
                    let t2 = self.nodes[0].clock.local(now);
                    let reply = ClockMsg::ClockReply {
                        seq,
                        t1,
                        t2,
                        t3: t2 + 0.000_03,
                    }
                    .encode();
                    self.send(now, 0, from, Wire::Clock(reply));
                }
                Ok(ClockMsg::ClockReply { seq, t1, t2, t3 }) => {
                    let n = &mut self.nodes[to];
                    if let Some(i) = n.asks.iter().position(|a| *a == (seq, t1)) {
                        n.asks.remove(i);
                        let t4 = n.clock.local(now);
                        if let Some(e) = n.est.as_mut() {
                            e.update(Exchange { t1, t2, t3, t4 }, t4);
                        }
                    }
                }
                _ => {}
            },
            Wire::Command(t) => {
                let n = &mut self.nodes[to];
                let Some(f) = n.follower.as_mut() else { return };
                let (epoch, seq) = f.receive(t, &mut n.engine, show);
                self.send(now, to, 0, Wire::Ack(epoch, seq));
            }
            Wire::Ack(epoch, seq) => {
                if let Some(l) = self.nodes[0].leader.as_mut() {
                    l.ack(id(from), epoch, seq);
                }
            }
            Wire::State(s) => {
                if let Some(f) = self.nodes[to].follower.as_mut() {
                    f.heard(s);
                }
            }
        }
    }

    fn event(&mut self, now: f64, ev: Ev) {
        match ev {
            Ev::Tick(i) => self.tick(now, i),
            Ev::Ask(i) => {
                let n = &mut self.nodes[i];
                let locked = n.est.as_ref().is_some_and(|e| e.locked());
                let every = if locked { 0.5 } else { 0.1 };
                n.seq += 1;
                let t1 = n.clock.local(now);
                n.asks.push((n.seq, t1));
                let bytes = ClockMsg::ClockAsk { seq: n.seq, t1 }.encode();
                self.q.push(now + every, Ev::Ask(i));
                self.send(now, i, 0, Wire::Clock(bytes));
            }
            Ev::Act(k) => {
                let (_, act) = self.script[k];
                let key = TimelineKey::Group(self.ts.group);
                let (e, show) = (&mut self.nodes[0].engine, &self.ts.show);
                match act {
                    Act::Go => e.go(show),
                    Act::Fire => e.fire(show, self.ts.cue),
                    Act::RunTimeline => e.timeline_go_one(show, key),
                    Act::Seek(pos) => e.timeline_seek(show, key, pos, true),
                    Act::Pause => {
                        e.timeline_pause(show, key, PauseFreeze::Everything, &[], false, true)
                    }
                    Act::StopAll => e.stop_all(0.0),
                }
            }
            Ev::Restart(i) => {
                let c = self.nodes[i].clock;
                let mut fresh = Node::new(c, false, &mut self.rng);
                fresh.phase = self.nodes[i].phase;
                fresh.k = self.nodes[i].k;
                self.nodes[i] = fresh;
            }
            Ev::Sample => {
                self.q.push(now + SAMPLE, Ev::Sample);
                self.sample(now);
            }
        }
    }

    fn tick(&mut self, now: f64, i: usize) {
        let followers: Vec<NodeId> = (1..self.nodes.len()).map(id).collect();
        let lead = self.lead;
        let show = &self.ts.show;
        let n = &mut self.nodes[i];
        n.k += 1;
        let next = n
            .clock
            .true_at(n.clock.local(0.0) + n.phase + n.k as f64 / TICK_HZ);
        self.q.push(next.max(now), Ev::Tick(i));
        let local = n.clock.local(now);
        let dt = n.last_local.map_or(0.0, |l| local - l);
        n.last_local = Some(local);
        let mut outs = Vec::new();
        if let Some(l) = n.leader.as_mut() {
            let cmds = n.engine.net_take();
            outs.extend(l.issue(cmds, local, lead, &followers));
            outs.extend(l.resend(local));
            l.step(&mut n.engine, show, local);
            if let Some(s) = l.beat(&n.engine, 1, local) {
                outs.push(Out::State(s));
            }
        } else if let Some(f) = n.follower.as_mut() {
            let shared = n
                .est
                .as_ref()
                .and_then(|e| e.now(local))
                .filter(|s| s.locked || n.engine.net_shared_time().is_some())
                .map(|s| s.time);
            f.step(&mut n.engine, show, shared, dt, Some(1));
        }
        let src = n.shared(now);
        if let Some(src) = src {
            let cue = n
                .engine
                .active
                .iter()
                .find(|a| a.cue_id == self.ts.cue && matches!(a.kind, ActiveKind::Wait))
                .map(|a| a.elapsed);
            let run = n
                .engine
                .timeline_clock(TimelineKey::Group(self.ts.group))
                .and_then(|(s, p)| match s {
                    RunState::Stopped => None,
                    s => Some((p, s == RunState::Running)),
                });
            n.note = Some((src, cue, run));
        }
        for o in outs {
            match o {
                Out::Command(to, t) => {
                    let tos: Vec<usize> = match to {
                        Some(to) => vec![to.0 as usize],
                        None => (1..self.nodes.len()).collect(),
                    };
                    for to in tos {
                        self.send(now, 0, to, Wire::Command(t.clone()));
                    }
                }
                Out::State(s) => {
                    for to in 1..self.nodes.len() {
                        self.send(now, 0, to, Wire::State(s.clone()));
                    }
                }
            }
        }
    }

    /// The cue's and the timeline's time on each computer at true time `now`.
    fn times(&self, now: f64) -> Vec<(Option<f64>, Option<f64>)> {
        self.nodes
            .iter()
            .map(|n| {
                let (Some((src, cue, run)), Some(at)) = (n.note, n.shared(now)) else {
                    return (None, None);
                };
                let gone = at - src;
                (
                    cue.map(|c| c + gone),
                    run.map(|(p, running)| if running { p + gone } else { p }),
                )
            })
            .collect()
    }

    fn sample(&mut self, now: f64) {
        if now < self.judge_from || self.skip.iter().any(|(a, b)| now >= *a && now < *b) {
            return;
        }
        let times = self.times(now);
        for pick in [0usize, 1] {
            let v: Vec<Option<f64>> = times
                .iter()
                .map(|t| if pick == 0 { t.0 } else { t.1 })
                .collect();
            // Present on all or none (a start a moment apart is judged by the
            // frames once it is up everywhere).
            if v.iter().all(Option::is_some) {
                let f: Vec<i64> = v.iter().map(|t| frame(t.unwrap_or(0.0))).collect();
                let d = f.iter().max().unwrap_or(&0) - f.iter().min().unwrap_or(&0);
                // Either side of a jump (a seek done at one tick here and the
                // next there): counted apart, as the trial does.
                if d as f64 > 0.5 * TICK_HZ {
                    self.straddles += 1;
                    continue;
                }
                self.samples += 1;
                if d > self.worst {
                    self.worst = d;
                    self.worst_at = now;
                }
            }
        }
    }

    /// Everyone locked by true time `t`?
    fn locked(&self, t: f64) -> bool {
        self.nodes.iter().all(|n| n.shared(t).is_some())
    }
}

fn id(i: usize) -> NodeId {
    NodeId(i as u64)
}

/// A show in which every follower's cue and timeline match the primary's.
fn all_have(w: &World, now: f64, cue: bool, run: Option<RunState>) -> bool {
    let key = TimelineKey::Group(w.ts.group);
    w.nodes.iter().all(|n| {
        let has_cue = n.engine.active.iter().any(|a| a.cue_id == w.ts.cue);
        let state = n
            .engine
            .timeline_clock(key)
            .map_or(RunState::Stopped, |(s, _)| s);
        let _ = now;
        has_cue == cue
            && run.is_none_or(|r| std::mem::discriminant(&r) == std::mem::discriminant(&state))
    })
}

/// GO, a timeline run, a seek, a pause and GO again, then Stop All, on two
/// followers over a show LAN: the frames agree to within one throughout.
#[test]
fn three_computers_keep_one_frame_through_go_seek_pause_and_stop_all() {
    let mut w = World::new(2, Network::wired(), 11);
    // Lock first (a few seconds of simulated time).
    w.run(5.0);
    assert!(w.locked(5.0), "the followers didn't lock");
    w.judge_from = 5.0;
    w.acts(&[
        (5.5, Act::Fire),
        (5.5, Act::RunTimeline),
        (20.0, Act::Seek(100.0)),
        (30.0, Act::Pause),
        (35.0, Act::RunTimeline),
        (45.0, Act::StopAll),
    ]);
    // (The script's acts are pushed by `run`; the earlier run had none.)
    w.run(44.0);
    assert!(all_have(&w, 44.0, true, Some(RunState::Running)));
    assert!(w.samples > 1000, "{}", w.samples);
    assert!(w.worst <= 1, "{} frames apart at {}", w.worst, w.worst_at);
    // The seek and the pause were by the shared clock: the timeline's clock
    // is past 100 s on all, and stood the same while paused.
    let key = TimelineKey::Group(w.ts.group);
    for n in &w.nodes {
        let (s, pos) = n.engine.timeline_clock(key).unwrap();
        assert_eq!(s, RunState::Running);
        assert!(pos > 105.0 && pos < 120.0, "{pos}");
    }
    w.run(47.0);
    assert!(all_have(&w, 47.0, false, Some(RunState::Stopped)));
    // No follower had anything to correct: the commands did it all.
    for n in &w.nodes[1..] {
        let f = n.follower.as_ref().unwrap();
        assert_eq!(f.corrections, 0, "{:?}", f.corrected);
    }
}

/// GO on the primary's playhead reaches a follower as GO with that playhead,
/// and both move it on alike.
#[test]
fn go_moves_every_playhead_alike() {
    let mut w = World::new(1, Network::wired(), 3);
    w.run(4.0);
    for n in &mut w.nodes {
        n.engine.playhead = Some(w.ts.cue);
    }
    w.acts(&[(4.5, Act::Go)]);
    w.run(6.0);
    let p: Vec<_> = w.nodes.iter().map(|n| n.engine.playhead).collect();
    assert_eq!(p[0], p[1]);
    assert!(all_have(&w, 6.0, true, None));
}

/// A command that arrives after its time starts the cue part way, where it
/// would have been: the follower's cue is a frame or less from the primary's.
#[test]
fn a_late_command_is_placed_where_it_would_have_been() {
    let mut w = World::new(1, Network::wired(), 5);
    w.run(4.0);
    w.judge_from = 4.0;
    w.lead = 0.05;
    // Held up 200 ms: 150 past its time (more, and the heartbeat would have
    // started it first: `a_lost_command_is_recovered_by_the_heartbeat`).
    w.late = Some((1, 0.2));
    w.acts(&[(4.5, Act::Fire)]);
    // Judged once it is up everywhere (the follower has it from 4.95 s).
    w.skip = vec![(4.0, 5.0)];
    w.run(10.0);
    assert!(all_have(&w, 10.0, true, None));
    assert!(w.worst <= 1, "{} frames apart at {}", w.worst, w.worst_at);
    let f = w.nodes[1].follower.as_ref().unwrap();
    assert_eq!(f.corrections, 0, "{:?}", f.corrected);
}

/// Commands lost on the way are made good by the heartbeat: the cue and the
/// timeline start on the follower part way, and stay within a frame after.
#[test]
fn a_lost_command_is_recovered_by_the_heartbeat() {
    let mut w = World::new(1, Network::wired(), 9);
    w.run(4.0);
    w.lose_commands = Some((1, 4.0, 20.0));
    w.acts(&[(4.5, Act::Fire), (4.5, Act::RunTimeline)]);
    w.run(8.0);
    assert!(all_have(&w, 8.0, true, Some(RunState::Running)));
    let f = w.nodes[1].follower.as_ref().unwrap();
    assert!(f.corrections >= 2, "{:?}", f.corrected);
    w.judge_from = 8.0;
    w.run(15.0);
    assert!(w.worst <= 1, "{} frames apart at {}", w.worst, w.worst_at);
    // And a seek lost too is made good.
    w.acts(&[(15.5, Act::Seek(300.0))]);
    w.skip = vec![(15.5, 18.0)];
    w.run(25.0);
    let key = TimelineKey::Group(w.ts.group);
    let pos = w.nodes[1].engine.timeline_clock(key).unwrap().1;
    assert!(pos > 305.0, "{pos}");
    assert!(w.worst <= 1, "{} frames apart at {}", w.worst, w.worst_at);
}

/// A follower that starts again mid-cue and mid-timeline (a crash, a display
/// computer switched on late) comes up in step from the heartbeat alone.
#[test]
fn a_follower_restarted_mid_show_catches_up() {
    let mut w = World::new(2, Network::wired(), 21);
    w.run(4.0);
    w.acts(&[(4.5, Act::Fire), (4.5, Act::RunTimeline)]);
    w.run(20.0);
    w.q.push(20.0, Ev::Restart(2));
    w.run(21.0);
    assert!(w.nodes[2].engine.active.is_empty());
    // Locked again, then two heartbeats to act.
    w.run(30.0);
    assert!(all_have(&w, 30.0, true, Some(RunState::Running)));
    w.judge_from = 30.0;
    w.run(40.0);
    assert!(w.worst <= 1, "{} frames apart at {}", w.worst, w.worst_at);
    let f = w.nodes[2].follower.as_ref().unwrap();
    assert!(f.in_step);
    // Paused on the primary: it pauses with it, at the same place.
    w.acts(&[(40.5, Act::Pause)]);
    w.run(45.0);
    let key = TimelineKey::Group(w.ts.group);
    let pos: Vec<f64> = w
        .nodes
        .iter()
        .map(|n| n.engine.timeline_clock(key).unwrap().1)
        .collect();
    assert!((pos[0] - pos[2]).abs() < 1.0 / 60.0, "{pos:?}");
}

/// On Wi-Fi with its lead, the frames still agree within one.
#[test]
fn on_wifi_with_its_lead_the_frames_agree_within_one() {
    let mut w = World::new(2, Network::wifi(), 33);
    w.lead = lead_for(Some(0.03), true);
    w.run(10.0);
    assert!(w.locked(10.0));
    w.judge_from = 10.0;
    w.acts(&[
        (10.5, Act::Fire),
        (10.5, Act::RunTimeline),
        (30.0, Act::Seek(50.0)),
    ]);
    w.run(60.0);
    assert!(all_have(&w, 60.0, true, Some(RunState::Running)));
    assert!(w.straddles <= 2, "{}", w.straddles);
    assert!(w.worst <= 1, "{} frames apart at {}", w.worst, w.worst_at);
}

#[test]
fn a_show_goes_in_pieces_and_comes_back_whole() {
    let json: String = "é{\"cues\":[]}".repeat(SHOW_PART / 3);
    let parts = show_parts(3, &json);
    assert!(parts.len() > 1);
    let mut a = ShowAssembly::default();
    let mut got = None;
    // Out of order, and an old version's piece in the middle is ignored.
    let mut ps = parts.clone();
    ps.reverse();
    for (k, p) in ps.into_iter().enumerate() {
        if k == 1 {
            let mut old = show_parts(2, "{}")[0].clone();
            old.parts = 9;
            assert!(a.take(old).is_none());
        }
        got = got.or(a.take(p));
    }
    let (v, h, back) = got.expect("whole");
    assert_eq!((v, h), (3, show_hash(&json)));
    assert_eq!(back, json);
}

#[test]
fn the_lead_follows_the_round_trip() {
    assert_eq!(lead_for(None, false), LEAD_MIN);
    assert_eq!(lead_for(Some(0.002), false), LEAD_MIN);
    assert!((lead_for(Some(0.03), false) - 0.13).abs() < 1e-9);
    assert_eq!(lead_for(Some(1.0), false), LEAD_MAX);
    assert_eq!(lead_for(Some(0.002), true), 0.3);
}

#[test]
fn commands_and_heartbeats_go_over_the_session_and_back() {
    use crate::net::proto::{self, Msg};
    let t = Timed {
        epoch: 1,
        seq: 2,
        at: 3.5,
        cmd: Command::TimelinePause {
            key: TimelineKey::List(2),
            freeze: PauseFreeze::Everything,
            clips: vec![CueId(4)],
            live: true,
            waits_for_go: false,
        },
    };
    let s = PlayState {
        seq: 2,
        cues: vec![CueNow {
            cue: CueId(1),
            anchor: 9.0,
            stopping: false,
        }],
        runs: vec![RunNow {
            key: TimelineKey::Group(CueId(5)),
            state: RunWord::Running,
            pos: 1.0,
            anchor: Some(8.0),
        }],
        ..Default::default()
    };
    for m in [
        Msg::Play(Box::new(t)),
        Msg::PlayAck { epoch: 1, seq: 2 },
        Msg::PlayState(Box::new(s)),
        Msg::ShowPart(Box::new(show_parts(1, "{}")[0].clone())),
        Msg::Restart,
        Msg::StartAtLogin { on: true },
    ] {
        let frame = proto::encode(&m);
        assert_eq!(proto::decode(&frame[4..]).unwrap(), m);
    }
}

/// Send cues are the primary's alone (ruling R8): fired on the primary with
/// two followers present, the message goes once.
#[test]
fn a_send_cue_fires_once_with_two_followers() {
    use crate::show::CueType;
    let mut show = Show::default();
    let c = show.make_cue(CueType::Send, None, 0);
    let id = c.id;
    show.cues.push(c);
    let mut p = Engine::for_tests();
    p.net_set_mode(NetMode::Primary);
    let mut f: Vec<(Engine, Follower)> = (0..2)
        .map(|_| {
            let mut e = Engine::for_tests();
            e.net_set_mode(NetMode::Follower);
            (e, Follower::default())
        })
        .collect();
    let mut leader = Leader::new(1);
    p.tick_shared(&show, 10.0);
    for (e, _) in &mut f {
        e.tick_shared(&show, 10.0);
    }
    // The operator fires it; a follower's own input is dropped.
    p.fire(&show, id);
    f[0].0.fire(&show, id);
    assert_eq!(p.net_sent(), 0, "caught, not done yet");
    let outs = leader.issue(p.net_take(), 10.0, 0.05, &[NodeId(1), NodeId(2)]);
    for o in outs {
        if let Out::Command(None, t) = o {
            for (e, fo) in &mut f {
                fo.receive(t.clone(), e, &show);
            }
        }
    }
    leader.step(&mut p, &show, 10.1);
    for (e, fo) in &mut f {
        fo.step(e, &show, Some(10.1), 0.0, Some(1));
    }
    assert_eq!(p.net_sent(), 1);
    for (e, _) in &f {
        assert_eq!(e.net_sent(), 0);
    }
}
