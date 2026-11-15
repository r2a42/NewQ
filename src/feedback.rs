//! OSC feedback: NewQ tells a controller (a TouchOSC page, a lighting desk, a
//! show-control computer) what the show is doing, by OSC over UDP to the
//! address given in the remote settings.
//!
//! What is sent, all under `/newq`:
//!
//! - `/newq/playhead <number> <name>`: the cue at the playhead (empty when none)
//! - `/newq/paused <0|1>`
//! - `/newq/running <n>`: how many cues are playing or waiting
//! - `/newq/cue/<number>/status <idle|prewait|playing>`: when a cue's state changes
//! - `/newq/cue/<number>/progress <0..1>`: for a cue with a length, in 2% steps
//! - `/newq/go <what> <then>`: what GO will do now ("Continue Act 1", "Act 1
//!   is running" with "GO does nothing until it pauses", "GO 3 · Song")
//! - `/newq/timeline/<list name|group's cue number>/state <word>`: every
//!   timeline list, and a timeline group while it does something (`state_word`)
//! - `/newq/timeline/<…>/position <seconds>`: its clock, in tenths of a second
//!
//! `snapshot` reads the state, `changes` is the difference between two of
//! them as messages (pure, so it is tested), and `Feedback` keeps the socket
//! and the pace (ten times a second at most, with the whole of the small
//! state repeated every few seconds so that a lost packet or a controller
//! started late catches up).

use crate::engine::{CueStatus, Engine, GoPlan, RunState, RunView};
use crate::output::{OscArg, osc_message};
use crate::show::{CueId, Show, TimelineKey};
use crate::timeline::model;
use std::collections::BTreeMap;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::time::{Duration, Instant};

const INTERVAL: Duration = Duration::from_millis(100);
const REPEAT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum State {
    PreWait,
    Playing,
}

impl State {
    fn word(self) -> &'static str {
        match self {
            State::PreWait => "prewait",
            State::Playing => "playing",
        }
    }
}

/// What a controller is told: the playhead and every cue that is not idle.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    playhead: (String, String),
    paused: bool,
    /// Cue number, state and progress in per cent (0 to 100, or None with no length).
    busy: BTreeMap<CueId, (String, State, Option<u8>)>,
    /// What GO will do, and what comes after it on a timeline (`go_text`).
    go: (String, String),
    /// Each timeline told about, by its address part: its state word and
    /// where its clock is, in tenths of a second.
    timelines: BTreeMap<String, (&'static str, i64)>,
}

pub type Message = (String, Vec<OscArg>);

/// One word for what a timeline's run is doing, for a controller to show or
/// test: `stopped`, `stopping` (a Stop's fade), `at_end` (a list that played
/// to its end), `cueing`, `running` (GO does nothing to a list then),
/// `looping`, `last_pass` (GO was pressed in a loop), `paused` or `parked`.
pub fn state_word(v: &RunView) -> &'static str {
    match v.state {
        RunState::Stopped if v.fading => "stopping",
        RunState::Stopped if v.at_end => "at_end",
        RunState::Stopped => "stopped",
        RunState::Cueing => "cueing",
        RunState::Running => match v.looping {
            Some(l) if l.last_pass => "last_pass",
            Some(_) => "looping",
            None => "running",
        },
        RunState::Paused { .. } if v.parked => "parked",
        RunState::Paused { .. } => "paused",
    }
}

/// The same in words for people, as the editor's toolbar says it.
pub fn state_text(v: &RunView) -> &'static str {
    match v.state {
        RunState::Stopped if v.fading => "Stopping · fading out",
        RunState::Stopped if v.at_end => "At its end",
        RunState::Stopped => "Stopped",
        RunState::Cueing => "Cueing",
        RunState::Running => match v.looping {
            Some(l) if l.last_pass => "Looping · last pass, then on",
            Some(l) if l.at_once => "Looping · GO leaves the loop",
            Some(_) => "Looping · GO finishes this pass",
            None => "Running",
        },
        RunState::Paused { .. } if v.parked => "Parked · GO plays from here",
        RunState::Paused {
            waits_for_go: true, ..
        } => "Paused · GO continues",
        RunState::Paused { .. } => "Paused",
    }
}

/// What GO will do now, in words, and what comes after it (or nothing): the
/// transports' read-out (`GoPlan::describe`), or the cue it fires.
pub fn go_text(show: &Show, engine: &Engine) -> (String, String) {
    let plan = engine.go_plan(show);
    if let Some((what, then)) = plan.describe(show) {
        return (what, then.unwrap_or_default());
    }
    match plan {
        GoPlan::Cue(Some(id)) => show
            .cue(id)
            .map(|c| (format!("GO {} · {}", c.number, c.name), String::new()))
            .unwrap_or_default(),
        _ => ("End of the cue list".into(), String::new()),
    }
}

/// The timelines a controller is told about: every timeline list, and every
/// timeline that is doing something (a group's while it runs, pauses or fades).
pub fn timelines_to_tell(show: &Show, engine: &Engine) -> Vec<TimelineKey> {
    let mut keys: Vec<TimelineKey> = show
        .lists
        .iter()
        .map(|l| TimelineKey::List(l.id))
        .filter(|k| model::is_timeline(show, *k))
        .collect();
    for k in engine.timelines() {
        if !keys.contains(&k) && model::is_timeline(show, k) {
            keys.push(k);
        }
    }
    keys
}

pub fn snapshot(show: &Show, engine: &mut Engine) -> Snapshot {
    let mut timelines = BTreeMap::new();
    for key in timelines_to_tell(show, engine) {
        let v = engine.timeline(show, key);
        // Its name, or its reference where the name is another's too.
        let name = crate::remote::timeline_address(show, key);
        timelines.insert(name, (state_word(&v), (v.pos * 10.0).round() as i64));
    }
    let engine = &*engine;
    let mut busy = BTreeMap::new();
    for p in &engine.pending {
        if let Some(c) = show.cue(p.cue_id) {
            busy.insert(c.id, (c.number.clone(), State::PreWait, None));
        }
    }
    for a in &engine.active {
        let Some(c) = show.cue(a.cue_id) else {
            continue;
        };
        // The newest instance of a cue says how it is.
        let progress = match engine.status(c.id) {
            CueStatus::Playing {
                elapsed,
                duration: Some(d),
            } => Some(((elapsed / d.max(1e-6)).clamp(0.0, 1.0) * 50.0).round() as u8 * 2),
            _ => None,
        };
        busy.insert(c.id, (c.number.clone(), State::Playing, progress));
    }
    let playhead = engine
        .playhead
        .and_then(|id| show.cue(id))
        .map(|c| (c.number.clone(), c.name.clone()))
        .unwrap_or_default();
    Snapshot {
        playhead,
        paused: engine.paused,
        busy,
        go: go_text(show, engine),
        timelines,
    }
}

fn text(s: &str) -> OscArg {
    OscArg::Text(s.to_string())
}

/// The cue number as an address part: no slashes or spaces, which would split it.
fn part(number: &str) -> String {
    let s: String = number
        .chars()
        .map(|c| {
            if c.is_whitespace() || "/#*,?[]{}".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    if s.is_empty() { "_".into() } else { s }
}

fn status_message(number: &str, word: &str) -> Message {
    (
        format!("/newq/cue/{}/status", part(number)),
        vec![text(word)],
    )
}

fn progress_message(number: &str, percent: u8) -> Message {
    (
        format!("/newq/cue/{}/progress", part(number)),
        vec![OscArg::Float(f32::from(percent) / 100.0)],
    )
}

fn go_message(now: &Snapshot) -> Message {
    ("/newq/go".into(), vec![text(&now.go.0), text(&now.go.1)])
}

fn timeline_state(name: &str, word: &str) -> Message {
    (format!("/newq/timeline/{name}/state"), vec![text(word)])
}

fn timeline_position(name: &str, tenths: i64) -> Message {
    (
        format!("/newq/timeline/{name}/position"),
        vec![OscArg::Float(tenths as f32 / 10.0)],
    )
}

/// What to say of the timelines: each one's state and clock when they changed
/// (all of them with no `before`), and "stopped" for one no longer told about.
fn timeline_changes(before: Option<&Snapshot>, now: &Snapshot, out: &mut Vec<Message>) {
    for (name, (word, tenths)) in &now.timelines {
        let was = before.and_then(|b| b.timelines.get(name));
        if was.is_none_or(|w| w.0 != *word) {
            out.push(timeline_state(name, word));
        }
        if was.is_none_or(|w| w.1 != *tenths) {
            out.push(timeline_position(name, *tenths));
        }
    }
    for name in before.map(|b| b.timelines.keys()).into_iter().flatten() {
        if !now.timelines.contains_key(name) {
            out.push(timeline_state(name, "stopped"));
        }
    }
}

fn summary(now: &Snapshot) -> Vec<Message> {
    vec![
        (
            "/newq/playhead".into(),
            vec![text(&now.playhead.0), text(&now.playhead.1)],
        ),
        (
            "/newq/paused".into(),
            vec![OscArg::Int(i32::from(now.paused))],
        ),
        (
            "/newq/running".into(),
            vec![OscArg::Int(now.busy.len() as i32)],
        ),
    ]
}

/// The messages that bring a controller that knew `old` up to `now`. With no
/// `old` (a controller that has just been given the address) everything.
pub fn changes(old: Option<&Snapshot>, now: &Snapshot) -> Vec<Message> {
    let mut out = Vec::new();
    let empty = Snapshot::default();
    let before = old.unwrap_or(&empty);
    if old.is_none() || before.playhead != now.playhead {
        out.push(summary(now).remove(0));
    }
    if old.is_none() || before.paused != now.paused {
        out.push(summary(now).remove(1));
    }
    if old.is_none() || before.busy.len() != now.busy.len() {
        out.push(summary(now).remove(2));
    }
    if old.is_none() || before.go != now.go {
        out.push(go_message(now));
    }
    timeline_changes(old, now, &mut out);
    for (id, (number, state, progress)) in &now.busy {
        let was = before.busy.get(id);
        if was.is_none_or(|w| w.1 != *state) {
            out.push(status_message(number, state.word()));
        }
        if let Some(p) = progress
            && was.is_none_or(|w| w.2 != Some(*p))
        {
            out.push(progress_message(number, *p));
        }
    }
    for (id, (number, _, progress)) in &before.busy {
        if !now.busy.contains_key(id) {
            if progress.is_some() {
                out.push(progress_message(number, 100));
            }
            out.push(status_message(number, "idle"));
        }
    }
    out
}

/// Everything a late controller needs to know.
pub fn repeat(now: &Snapshot) -> Vec<Message> {
    let mut out = summary(now);
    out.push(go_message(now));
    timeline_changes(None, now, &mut out);
    for (number, state, progress) in now.busy.values() {
        out.push(status_message(number, state.word()));
        if let Some(p) = progress {
            out.push(progress_message(number, *p));
        }
    }
    out
}

/// Sends the show's state to the address in the settings.
pub struct Feedback {
    wanted: String,
    /// Filled in by the thread that looks the address up (a name may take a while).
    addr: Arc<parking_lot::Mutex<Option<SocketAddr>>>,
    socket: Option<UdpSocket>,
    last: Option<Snapshot>,
    at: Instant,
    repeated: Instant,
    /// The last problem, for the settings page.
    pub error: Option<String>,
}

impl Feedback {
    pub fn new() -> Feedback {
        let now = Instant::now();
        Feedback {
            wanted: String::new(),
            addr: Arc::new(parking_lot::Mutex::new(None)),
            socket: None,
            last: None,
            at: now,
            repeated: now,
            error: None,
        }
    }

    /// Follow the setting (`host:port`, empty = off).
    pub fn set_target(&mut self, wanted: &str) {
        let wanted = wanted.trim();
        if wanted == self.wanted {
            return;
        }
        self.wanted = wanted.to_string();
        self.last = None;
        self.error = None;
        *self.addr.lock() = None;
        self.socket = None;
        if wanted.is_empty() {
            return;
        }
        match UdpSocket::bind("0.0.0.0:0").and_then(|s| s.set_nonblocking(true).map(|_| s)) {
            Ok(s) => self.socket = Some(s),
            Err(e) => {
                self.error = Some(e.to_string());
                return;
            }
        }
        let (slot, name) = (self.addr.clone(), wanted.to_string());
        let spawned = std::thread::Builder::new()
            .name("osc-feedback-lookup".into())
            .spawn(move || {
                let found = name.to_socket_addrs().ok().and_then(|mut a| a.next());
                *slot.lock() = found;
            });
        if spawned.is_err() {
            self.error = Some("could not look the address up".into());
        }
    }

    /// Send what has changed, at most ten times a second.
    pub fn update(&mut self, show: &Show, engine: &mut Engine, now: Instant) {
        if self.socket.is_none() || now.duration_since(self.at) < INTERVAL {
            return;
        }
        let Some(addr) = *self.addr.lock() else {
            return;
        };
        self.at = now;
        let snap = snapshot(show, engine);
        let messages = if now.duration_since(self.repeated) >= REPEAT || self.last.is_none() {
            self.repeated = now;
            repeat(&snap)
        } else {
            changes(self.last.as_ref(), &snap)
        };
        self.last = Some(snap);
        let Some(socket) = &self.socket else {
            return;
        };
        for (address, args) in messages {
            // A full buffer or a controller that is not there loses a packet; fine.
            let _ = socket.send_to(&osc_message(&address, &args), addr);
        }
    }
}

impl Default for Feedback {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::parse_osc;

    fn busy(entries: &[(u64, &str, State, Option<u8>)]) -> Snapshot {
        Snapshot {
            busy: entries
                .iter()
                .map(|(id, n, s, p)| (CueId(*id), (n.to_string(), *s, *p)))
                .collect(),
            ..Snapshot::default()
        }
    }

    fn addresses(m: &[Message]) -> Vec<&str> {
        m.iter().map(|m| m.0.as_str()).collect()
    }

    #[test]
    fn a_new_controller_is_told_everything_and_then_only_what_changes() {
        let a = busy(&[(1, "1", State::Playing, Some(10))]);
        let first = changes(None, &a);
        assert_eq!(
            addresses(&first),
            [
                "/newq/playhead",
                "/newq/paused",
                "/newq/running",
                "/newq/go",
                "/newq/cue/1/status",
                "/newq/cue/1/progress"
            ]
        );
        assert!(changes(Some(&a), &a).is_empty(), "nothing changed");
        let b = busy(&[(1, "1", State::Playing, Some(12))]);
        assert_eq!(addresses(&changes(Some(&a), &b)), ["/newq/cue/1/progress"]);
    }

    #[test]
    fn a_cue_that_ends_is_reported_idle_and_complete() {
        let a = busy(&[(1, "7.5", State::Playing, Some(98))]);
        let b = busy(&[]);
        let m = changes(Some(&a), &b);
        assert_eq!(
            addresses(&m),
            [
                "/newq/running",
                "/newq/cue/7.5/progress",
                "/newq/cue/7.5/status"
            ]
        );
        assert_eq!(m[1].1, [OscArg::Float(1.0)]);
        assert_eq!(m[2].1, [OscArg::Text("idle".into())]);
    }

    #[test]
    fn cue_numbers_cannot_break_the_address() {
        assert_eq!(part("Q 1/2"), "Q_1_2");
        assert_eq!(part(""), "_");
        assert_eq!(part("12.5a"), "12.5a");
    }

    #[test]
    fn a_waiting_cue_is_prewait_and_the_repeat_covers_everything() {
        let a = busy(&[
            (1, "1", State::PreWait, None),
            (2, "2", State::Playing, None),
        ]);
        let r = repeat(&a);
        assert_eq!(
            addresses(&r),
            [
                "/newq/playhead",
                "/newq/paused",
                "/newq/running",
                "/newq/go",
                "/newq/cue/1/status",
                "/newq/cue/2/status"
            ]
        );
        assert_eq!(r[4].1, [OscArg::Text("prewait".into())]);
    }

    #[test]
    fn the_engines_state_is_read_and_sent_over_udp() {
        let mut show = Show::default();
        let id = show.make_cue(crate::show::CueType::Wait, None, 0);
        let (cue, number) = (id.id, id.number.clone());
        show.cues.push(id);
        let mut e = Engine::for_tests();
        e.playhead = Some(cue);
        let snap = snapshot(&show, &mut e);
        assert_eq!(snap.playhead.0, number);
        assert!(snap.busy.is_empty());
        e.go(&show);
        e.tick(&show, 0.1);
        let snap = snapshot(&show, &mut e);
        assert_eq!(snap.busy.len(), 1);

        // A listener on loopback gets the messages, which our own parser reads.
        let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
        listener
            .set_read_timeout(Some(Duration::from_millis(30)))
            .unwrap();
        let mut f = Feedback::new();
        f.set_target(&listener.local_addr().unwrap().to_string());
        assert!(f.socket.is_some());
        let start = Instant::now();
        let mut got = Vec::new();
        let mut buf = [0u8; 1024];
        while got.is_empty() && start.elapsed() < Duration::from_secs(5) {
            f.update(&show, &mut e, Instant::now());
            std::thread::sleep(Duration::from_millis(20));
            if let Ok((n, _)) = listener.recv_from(&mut buf) {
                got.push(buf[..n].to_vec());
            }
        }
        assert!(!got.is_empty(), "nothing arrived");
        assert!(got[0].starts_with(b"/newq/"), "{:?}", got[0]);
        let _ = parse_osc(&got[0]);
        f.set_target("");
        assert!(f.socket.is_none());
    }

    fn with_timeline(word: &'static str, tenths: i64) -> Snapshot {
        let mut s = Snapshot::default();
        s.timelines.insert("Act_1".into(), (word, tenths));
        s
    }

    #[test]
    fn timelines_are_told_their_state_and_clock_and_when_they_stop() {
        let a = with_timeline("running", 12);
        let first = changes(None, &a);
        assert!(
            addresses(&first).ends_with(&[
                "/newq/timeline/Act_1/state",
                "/newq/timeline/Act_1/position"
            ]),
            "{:?}",
            addresses(&first)
        );
        assert!(changes(Some(&a), &a).is_empty());
        // The clock moving is its position only.
        let b = with_timeline("running", 13);
        let m = changes(Some(&a), &b);
        assert_eq!(addresses(&m), ["/newq/timeline/Act_1/position"]);
        assert_eq!(m[0].1, [OscArg::Float(1.3)]);
        // A pause is its state only.
        let c = with_timeline("paused", 13);
        let m = changes(Some(&b), &c);
        assert_eq!(addresses(&m), ["/newq/timeline/Act_1/state"]);
        assert_eq!(m[0].1, [OscArg::Text("paused".into())]);
        // One no longer told about (a group that stopped) is said to be stopped.
        let m = changes(Some(&c), &Snapshot::default());
        assert_eq!(addresses(&m), ["/newq/timeline/Act_1/state"]);
        assert_eq!(m[0].1, [OscArg::Text("stopped".into())]);
        // What GO does is said when it changes, and the repeat has it all.
        let mut d = c.clone();
        d.go = ("Continue Act 1".into(), "then bring in 3 · Song".into());
        let m = changes(Some(&c), &d);
        assert_eq!(addresses(&m), ["/newq/go"]);
        assert_eq!(
            m[0].1,
            [
                OscArg::Text("Continue Act 1".into()),
                OscArg::Text("then bring in 3 · Song".into())
            ]
        );
        let r = repeat(&d);
        for a in [
            "/newq/go",
            "/newq/timeline/Act_1/state",
            "/newq/timeline/Act_1/position",
        ] {
            assert!(addresses(&r).contains(&a), "{a}");
        }
    }

    #[test]
    fn timelines_that_share_a_name_are_told_at_addresses_of_their_own() {
        use crate::remote::fixtures;
        let (mut show, l, _) = fixtures::show();
        let l2 = show.add_list("Act 1");
        crate::timeline::edit::set_list_timeline(&mut show, l2, true).unwrap();
        let mut e = fixtures::engine(&show);
        let snap = snapshot(&show, &mut e);
        let names: Vec<&String> = snap.timelines.keys().collect();
        assert_eq!(names, vec![&format!("list:{l}"), &format!("list:{l2}")]);
        let m = changes(None, &snap);
        for n in [l, l2] {
            assert!(
                addresses(&m).contains(&format!("/newq/timeline/list:{n}/state").as_str()),
                "{:?}",
                addresses(&m)
            );
        }
    }

    #[test]
    fn the_feedback_of_a_timeline_list_says_running_and_that_go_does_nothing() {
        use crate::remote::{TimelineDo, fixtures, run_timeline};
        let (show, l, g) = fixtures::show();
        let k = TimelineKey::List(l);
        let mut e = fixtures::engine(&show);
        let snap = snapshot(&show, &mut e);
        // Every timeline list is told about, a group only once it does something.
        assert_eq!(snap.timelines.len(), 1);
        assert_eq!(snap.timelines["Act_1"], ("stopped", 0));
        e.switch_list(&show, l);
        assert_eq!(snapshot(&show, &mut e).go.0, "Run Act 1");
        e.go(&show);
        fixtures::run_while(&mut e, &show, k, 0.05, |e| e.timeline(&show, k).pos < 0.5);
        let running = snapshot(&show, &mut e);
        assert_eq!(running.timelines["Act_1"].0, "running");
        assert!(running.timelines["Act_1"].1 >= 5);
        assert_eq!(
            running.go,
            (
                "Act 1 is running".to_string(),
                "GO does nothing until it pauses".to_string()
            )
        );
        // A remote's GO while it runs changes nothing it is told.
        let tenths = running.timelines["Act_1"].1;
        run_timeline(&show, &mut e, "Act_1", &TimelineDo::Go);
        e.go(&show);
        let again = snapshot(&show, &mut e);
        assert_eq!(again.timelines["Act_1"], ("running", tenths));
        assert!(changes(Some(&running), &again).is_empty());
        // The group, while it plays, by its number.
        e.fire(&show, g);
        e.tick(&show, 0.0);
        let both = snapshot(&show, &mut e);
        assert_eq!(both.timelines.get("5").map(|t| t.0), Some("running"));
        let m = changes(Some(&again), &both);
        assert!(addresses(&m).contains(&"/newq/timeline/5/state"));
        e.timeline_stop(&show, TimelineKey::Group(g), 0.0);
        let gone = snapshot(&show, &mut e);
        assert!(!gone.timelines.contains_key("5"));
        let m = changes(Some(&both), &gone);
        assert!(m.contains(&timeline_state("5", "stopped")), "{m:?}");
        // Paused at its pause, then stopping with a fade.
        fixtures::run_while(&mut e, &show, k, 0.05, |e| {
            !matches!(e.timeline(&show, k).state, RunState::Paused { .. })
        });
        let paused = snapshot(&show, &mut e);
        assert_eq!(paused.timelines["Act_1"], ("paused", 30));
        assert_eq!(paused.go.0, "Continue Act 1");
        run_timeline(&show, &mut e, "Act_1", &TimelineDo::Stop(None));
        assert_eq!(snapshot(&show, &mut e).timelines["Act_1"].0, "stopping");
    }
}
