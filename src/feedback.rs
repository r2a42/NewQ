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
//!
//! `snapshot` reads the state, `changes` is the difference between two of
//! them as messages (pure, so it is tested), and `Feedback` keeps the socket
//! and the pace (ten times a second at most, with the whole of the small
//! state repeated every few seconds so that a lost packet or a controller
//! started late catches up).

use crate::engine::{CueStatus, Engine};
use crate::output::{OscArg, osc_message};
use crate::show::{CueId, Show};
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
}

pub type Message = (String, Vec<OscArg>);

pub fn snapshot(show: &Show, engine: &Engine) -> Snapshot {
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
    pub fn update(&mut self, show: &Show, engine: &Engine, now: Instant) {
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
                "/newq/cue/1/status",
                "/newq/cue/2/status"
            ]
        );
        assert_eq!(r[3].1, [OscArg::Text("prewait".into())]);
    }

    #[test]
    fn the_engines_state_is_read_and_sent_over_udp() {
        let mut show = Show::default();
        let id = show.make_cue(crate::show::CueType::Wait, None, 0);
        let (cue, number) = (id.id, id.number.clone());
        show.cues.push(id);
        let mut e = Engine::for_tests();
        e.playhead = Some(cue);
        let snap = snapshot(&show, &e);
        assert_eq!(snap.playhead.0, number);
        assert!(snap.busy.is_empty());
        e.go(&show);
        e.tick(&show, 0.1);
        let snap = snapshot(&show, &e);
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
            f.update(&show, &e, Instant::now());
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
}
