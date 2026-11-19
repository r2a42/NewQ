//! The computers playing together (stage 4a of `.claude/notes/network-plan.md`,
//! D6): what the primary tells its computers and how each keeps to it.
//!
//! - Every operator input that changes playback on the primary (GO, a cue
//!   fired, stopped or faded, Stop All, pause, each timeline verb, a remote's,
//!   a hotkey's) is caught by the engine's gate (`Engine::net_take`) as a
//!   `Command`, given a shared time `at` a lead ahead, sent to the followers
//!   (`Timed`, acknowledged, sent again until it is) and done by the primary
//!   itself at the same time (`Leader`).
//! - A follower does each at its time by its shared clock (`Follower::step`):
//!   it ticks its engine to that time, does it, then ticks on. One that came
//!   late is done at once and placed as though it had been done in time.
//! - The primary sends what it is playing a few times a second (`PlayState`,
//!   the heartbeat): cues with the shared time their own time was 0, each
//!   timeline's run and clock, the playheads. A follower that differs corrects
//!   itself (`Engine::net_correct`), which is also how a computer that joins or
//!   restarts mid-show comes into step. A difference is acted on only when it
//!   is seen twice in a row and no command is still to be done, so a correction
//!   never fights a command in flight.
//!
//! Nothing here touches a socket: the app (`app.rs::pass_net`, `pass_show`)
//! moves the messages, and the tests (`play_tests.rs`) move them through a
//! simulated network.

use crate::engine::{Engine, NetCorrection};
use crate::net::node::NodeId;
use crate::show::{ControlAction, CueId, Show};
use crate::timeline::{PauseFreeze, TimelineKey};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};

/// An operator's input that changes playback, as it is done on every computer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Command {
    /// GO on `list` with its playhead at `playhead` (as they were when pressed).
    Go {
        list: u32,
        playhead: Option<CueId>,
    },
    /// The cue at the playhead, whatever slide show is on.
    GoPlayhead {
        list: u32,
        playhead: Option<CueId>,
    },
    GoList {
        list: u32,
    },
    Fire {
        cue: CueId,
    },
    StopCue {
        cue: CueId,
        fade: f64,
    },
    FadeOutCue {
        cue: CueId,
    },
    StopAll {
        fade: f64,
    },
    Panic,
    ResetAll,
    SetPaused {
        paused: bool,
    },
    /// A playing cue's instance, as the `nth` (by start) playing of that cue.
    StopInstance {
        cue: CueId,
        nth: usize,
        fade: f64,
    },
    FadeOutInstance {
        cue: CueId,
        nth: usize,
    },
    Seek {
        cue: CueId,
        nth: usize,
        to: f64,
    },
    /// A control action on a cue from outside the show (a remote, the menu).
    Control {
        /// (An action a later NewQ adds reads as `Memo`: nothing done.)
        #[serde(deserialize_with = "lenient_action")]
        action: ControlAction,
        target: CueId,
        from: String,
    },
    TimelineRun {
        key: TimelineKey,
    },
    TimelineSeek {
        key: TimelineKey,
        pos: f64,
        picture: bool,
    },
    TimelineRunFrom {
        key: TimelineKey,
        pos: f64,
    },
    TimelinePause {
        key: TimelineKey,
        freeze: PauseFreeze,
        clips: Vec<CueId>,
        live: bool,
        waits_for_go: bool,
    },
    TimelineStop {
        key: TimelineKey,
        fade: f64,
    },
    TimelineGoOne {
        key: TimelineKey,
    },
    TimelineContinue {
        key: TimelineKey,
    },
    TimelineLeave {
        key: TimelineKey,
    },
    /// A command a later NewQ sends that this one doesn't know: acknowledged
    /// and not done (a frame that can't be read would close the session).
    #[serde(other)]
    Unknown,
}

/// A control action, or `Memo` (which does nothing) for one a later NewQ adds:
/// the command is still acknowledged and the session stays up.
fn lenient_action<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ControlAction, D::Error> {
    use serde::de::IntoDeserializer;
    let word = String::deserialize(d)?;
    let known: Result<ControlAction, serde::de::value::Error> =
        ControlAction::deserialize(word.as_str().into_deserializer());
    Ok(known.unwrap_or(ControlAction::Memo))
}

impl Command {
    /// The cue it will start, to be got ready on arrival (a cold FFmpeg start
    /// is the real floor of the lead, as the trial found).
    pub fn will_start(&self, show: &Show) -> Option<CueId> {
        match self {
            Command::Fire { cue } => Some(*cue),
            Command::Go { playhead, .. } | Command::GoPlayhead { playhead, .. } => playhead
                .and_then(|p| show.cue_index(p))
                .map(|i| show.top_index(i))
                .and_then(|i| show.armed_top_from(i))
                .map(|i| show.cues[i].id),
            _ => None,
        }
    }
}

/// A command and the shared time it is to be done at. `epoch` is the
/// primary's run (a new one each time it starts), `seq` counts its commands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Timed {
    pub epoch: u64,
    pub seq: u64,
    pub at: f64,
    pub cmd: Command,
}

/// A cue playing, as the heartbeat has it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CueNow {
    pub cue: CueId,
    /// The shared time its own time was 0 (its time is `now - anchor`).
    pub anchor: f64,
    /// Fading out to its end.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stopping: bool,
    /// Its sound, playing on this computer: the shared time at which the
    /// sound's own time was 0 by what its device has been sent (it keeps to
    /// `anchor` within `audio::SOUND_SLACK`). For the network test; a NewQ
    /// before 0.24.2 neither sends nor reads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sound: Option<f64>,
}

/// Where a timeline's run is, as the heartbeat has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunWord {
    Stopped,
    Cueing,
    Running,
    Paused,
    /// A word a later NewQ sends that this one doesn't know: nothing to
    /// compare with (as while cueing).
    #[serde(other)]
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunNow {
    pub key: TimelineKey,
    pub state: RunWord,
    /// Its clock at the heartbeat's time.
    pub pos: f64,
    /// While it runs: the shared time its clock was at 0.
    #[serde(default)]
    pub anchor: Option<f64>,
}

/// The heartbeat: what the primary is playing at shared time `time`, after
/// its command `seq` of run `epoch`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayState {
    pub epoch: u64,
    pub seq: u64,
    pub time: f64,
    /// The hash of the show it plays (`show_hash`): a follower compares only
    /// with the same show.
    pub show: u64,
    pub paused: bool,
    pub list: u32,
    pub playhead: Option<CueId>,
    /// The other lists' playheads.
    #[serde(default)]
    pub playheads: Vec<(u32, Option<CueId>)>,
    pub cues: Vec<CueNow>,
    pub runs: Vec<RunNow>,
}

/// A piece of the show (its JSON, cut so that a large show never makes one
/// large frame that commands would wait behind).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShowPart {
    pub version: u64,
    /// Of the whole JSON (`show_hash`).
    pub hash: u64,
    pub part: u32,
    pub parts: u32,
    pub data: String,
}

/// The size of a show piece (characters of JSON).
pub const SHOW_PART: usize = 256 * 1024;

/// A hash of the show's JSON (FNV-1a, 64 bits): the same on every computer.
pub fn show_hash(json: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in json.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// The show's JSON cut into pieces.
pub fn show_parts(version: u64, json: &str) -> Vec<ShowPart> {
    let hash = show_hash(json);
    let mut pieces = Vec::new();
    let mut rest = json;
    while !rest.is_empty() {
        let mut cut = rest.len().min(SHOW_PART);
        while !rest.is_char_boundary(cut) {
            cut -= 1;
        }
        pieces.push(&rest[..cut]);
        rest = &rest[cut..];
    }
    if pieces.is_empty() {
        pieces.push("");
    }
    let parts = pieces.len() as u32;
    pieces
        .into_iter()
        .enumerate()
        .map(|(i, d)| ShowPart {
            version,
            hash,
            part: i as u32,
            parts,
            data: d.to_string(),
        })
        .collect()
}

/// A follower putting a show together from its pieces.
#[derive(Default)]
pub struct ShowAssembly {
    version: u64,
    hash: u64,
    got: BTreeMap<u32, String>,
    parts: u32,
}

impl ShowAssembly {
    /// Take a piece; the whole JSON (checked against its hash) once all are in.
    /// A piece of a newer version starts again; of an older one is dropped.
    pub fn take(&mut self, p: ShowPart) -> Option<(u64, u64, String)> {
        if p.version != self.version || p.hash != self.hash {
            if p.version < self.version && !self.got.is_empty() {
                return None;
            }
            self.version = p.version;
            self.hash = p.hash;
            self.parts = p.parts;
            self.got.clear();
        }
        if p.part >= p.parts {
            return None;
        }
        self.got.insert(p.part, p.data);
        if self.got.len() as u32 != self.parts {
            return None;
        }
        let json: String = std::mem::take(&mut self.got).into_values().collect();
        (show_hash(&json) == self.hash).then_some((self.version, self.hash, json))
    }
}

/// How far ahead a command is timed, from the worst recent round trip to the
/// followers (seconds): 50 ms on a show LAN (the network needs a few; the rest
/// is for a busy main loop and getting media ready), more as the round trip
/// grows, at most 400 ms. On Wi-Fi at least 300 ms (its holds of 100 ms and
/// more, with a packet lost, as the trial found).
pub fn lead_for(worst_rtt: Option<f64>, wifi: bool) -> f64 {
    let rtt = worst_rtt.filter(|r| r.is_finite()).unwrap_or(0.0).max(0.0);
    let lead = (0.04 + 3.0 * rtt).clamp(LEAD_MIN, LEAD_MAX);
    if wifi { lead.max(0.3) } else { lead }
}

pub const LEAD_MIN: f64 = 0.05;
pub const LEAD_MAX: f64 = 0.4;
/// A command not acknowledged is sent again this often (seconds of shared time).
pub const RESEND: f64 = 0.25;
/// And given up on this long after its time (the heartbeat sees to the rest).
pub const GIVE_UP: f64 = 10.0;
/// The heartbeat's pace (seconds).
pub const BEAT: f64 = 0.25;

/// The primary's side.
pub struct Leader {
    pub epoch: u64,
    seq: u64,
    last_at: f64,
    /// Sent and not yet acknowledged by everyone: the command, who still owes
    /// an ack, when it was last sent.
    out: BTreeMap<u64, (Timed, HashSet<NodeId>, f64)>,
    /// Its own commands, to be done at their time.
    own: Vec<Timed>,
    /// The last command done here.
    pub done: u64,
    last_beat: Option<f64>,
    /// A heartbeat is due at once (a command was just done).
    beat_now: bool,
}

/// A message for the computers: to one (`Some`) or all of the network.
#[derive(Clone, Debug, PartialEq)]
pub enum Out {
    Command(Option<NodeId>, Timed),
    State(PlayState),
}

impl Leader {
    pub fn new(epoch: u64) -> Leader {
        Leader {
            epoch,
            seq: 0,
            last_at: f64::NEG_INFINITY,
            out: BTreeMap::new(),
            own: Vec::new(),
            done: 0,
            last_beat: None,
            beat_now: false,
        }
    }

    /// Time the commands the gate took at `now`, `lead` ahead (never before
    /// one sent earlier, so they are done in the order given), to be done here
    /// and by `followers`.
    pub fn issue(
        &mut self,
        cmds: Vec<Command>,
        now: f64,
        lead: f64,
        followers: &[NodeId],
    ) -> Vec<Out> {
        let mut sent = Vec::new();
        for cmd in cmds {
            self.seq += 1;
            let at = (now + lead).max(self.last_at);
            self.last_at = at;
            let t = Timed {
                epoch: self.epoch,
                seq: self.seq,
                at,
                cmd,
            };
            self.own.push(t.clone());
            if !followers.is_empty() {
                self.out
                    .insert(t.seq, (t.clone(), followers.iter().copied().collect(), now));
                sent.push(Out::Command(None, t));
            }
        }
        sent
    }

    /// The earliest of its own commands still to be done.
    pub fn next_due(&self) -> Option<f64> {
        self.own.iter().map(|t| t.at).reduce(f64::min)
    }

    pub fn ack(&mut self, from: NodeId, epoch: u64, seq: u64) {
        if epoch != self.epoch {
            return;
        }
        if let Some((_, owed, _)) = self.out.get_mut(&seq) {
            owed.remove(&from);
            if owed.is_empty() {
                self.out.remove(&seq);
            }
        }
    }

    /// A computer left: it owes nothing.
    pub fn forget(&mut self, gone: NodeId) {
        self.out.retain(|_, (_, owed, _)| {
            owed.remove(&gone);
            !owed.is_empty()
        });
    }

    /// A computer joined (or came back): everything still to be done goes to
    /// it too.
    pub fn joined(&mut self, who: NodeId, now: f64) -> Vec<Out> {
        let mut v = Vec::new();
        for t in self.own.iter().filter(|t| t.at > now) {
            v.push(Out::Command(Some(who), t.clone()));
            self.out
                .entry(t.seq)
                .or_insert_with(|| (t.clone(), HashSet::new(), now))
                .1
                .insert(who);
        }
        v
    }

    /// What is to be sent again (to those that haven't acknowledged), and what
    /// is given up on.
    pub fn resend(&mut self, now: f64) -> Vec<Out> {
        let mut v = Vec::new();
        self.out.retain(|_, (t, owed, last)| {
            if now - t.at > GIVE_UP {
                return false;
            }
            if now - *last >= RESEND {
                *last = now;
                for who in owed.iter() {
                    v.push(Out::Command(Some(*who), t.clone()));
                }
            }
            true
        });
        v
    }

    /// Do its own commands that are due by `now` and tick its engine to `now`
    /// by the shared clock (its own).
    pub fn step(&mut self, engine: &mut Engine, show: &Show, now: f64) {
        self.own
            .sort_by(|a, b| a.at.total_cmp(&b.at).then(a.seq.cmp(&b.seq)));
        while self.own.first().is_some_and(|t| t.at <= now) {
            let t = self.own.remove(0);
            engine.tick_shared(show, t.at);
            engine.net_apply(show, &t.cmd, 0.0);
            self.done = t.seq;
            self.beat_now = true;
        }
        engine.tick_shared(show, now);
    }

    /// The heartbeat, when one is due.
    pub fn beat(&mut self, engine: &Engine, show_hash: u64, now: f64) -> Option<PlayState> {
        if !self.beat_now && self.last_beat.is_some_and(|t| now - t < BEAT) {
            return None;
        }
        self.beat_now = false;
        self.last_beat = Some(now);
        let mut s = engine.net_state(now);
        s.epoch = self.epoch;
        s.seq = self.done;
        s.show = show_hash;
        Some(s)
    }
}

/// How a follower stands, for its card, its status and the primary's page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Standing {
    /// No shared clock yet (or not locked): nothing new is played.
    WaitingForClock,
    /// Locked, without the show (or an old one).
    WaitingForShow,
    /// Locked, with the show, not yet agreeing with the primary.
    GettingReady,
    InStep,
}

impl Standing {
    /// For the card and the primary's page.
    pub fn words(self) -> &'static str {
        match self {
            Standing::WaitingForClock => "Waiting for the primary's clock",
            Standing::WaitingForShow => "Getting the show",
            Standing::GettingReady => "Getting ready",
            Standing::InStep => "In step",
        }
    }
}

/// A follower's side.
#[derive(Default)]
pub struct Follower {
    epoch: Option<u64>,
    /// Commands up to here are done or given up for this epoch.
    floor: u64,
    /// Done above the floor.
    done: BTreeSet<u64>,
    queue: Vec<Timed>,
    /// Differences seen at the last heartbeat (`Engine::net_correct`).
    pub doubts: NetCorrection,
    /// The last heartbeat, to be compared at the next step.
    state: Option<PlayState>,
    /// The last heartbeat agreed (nothing was corrected).
    pub in_step: bool,
    /// What the last corrections were (for the log and the tests).
    pub corrected: Vec<String>,
    /// Corrections made in all.
    pub corrections: u64,
    /// The show the primary said it plays at its last heartbeat (its hash).
    pub primary_show: Option<u64>,
}

impl Follower {
    fn new_epoch(&mut self, epoch: u64) {
        if self.epoch != Some(epoch) {
            self.epoch = Some(epoch);
            self.floor = 0;
            self.done.clear();
            self.queue.clear();
        }
    }

    /// A command arrived: the ack to send (always, so a repeat is acked too).
    /// It is got ready (`load_ahead`) at once.
    pub fn receive(&mut self, t: Timed, engine: &mut Engine, show: &Show) -> (u64, u64) {
        let ack = (t.epoch, t.seq);
        self.new_epoch(t.epoch);
        let known = t.seq <= self.floor
            || self.done.contains(&t.seq)
            || self.queue.iter().any(|q| q.seq == t.seq);
        if !known {
            if let Some(id) = t.cmd.will_start(show) {
                engine.load_ahead(show, id);
            }
            self.queue.push(t);
        }
        ack
    }

    /// The commands still to be done, taken away. A backup taking control
    /// lets the old primary's go: that one, if it is still there, does them
    /// itself as it steps down (`app/play.rs::play_switch`), so a send cue
    /// never fires twice (R8); if it is gone, a GO pressed in its last
    /// fraction of a second is lost and is pressed again (stage 5).
    pub fn take_queue(&mut self) -> Vec<Timed> {
        std::mem::take(&mut self.queue)
    }

    /// The primary's heartbeat.
    pub fn heard(&mut self, s: PlayState) {
        self.new_epoch(s.epoch);
        self.primary_show = Some(s.show);
        self.state = Some(s);
    }

    /// The earliest command still to be done.
    pub fn next_due(&self) -> Option<f64> {
        self.queue.iter().map(|t| t.at).reduce(f64::min)
    }

    /// Every command the heartbeat `s` counts has been done here (the network
    /// test compares what plays only then: a command still on its way is a
    /// late command, not a difference).
    pub fn has_done(&self, s: &PlayState) -> bool {
        self.epoch == Some(s.epoch) && self.floor >= s.seq
    }

    /// Where it stands, with its clock (`locked`) and show (`hash`).
    pub fn standing(&self, locked: bool, hash: Option<u64>) -> Standing {
        if !locked {
            Standing::WaitingForClock
        } else if hash.is_none() || self.primary_show.is_some_and(|p| Some(p) != hash) {
            Standing::WaitingForShow
        } else if !self.in_step {
            Standing::GettingReady
        } else {
            Standing::InStep
        }
    }

    /// One pass: with the shared time `now` (None: no locked clock, so nothing
    /// new is done and the engine runs on by `dt` alone), do what is due, tick
    /// the engine there, and compare it with the last heartbeat. `show_hash`
    /// is the hash of the show this computer has; corrections are made only
    /// with the primary's.
    pub fn step(
        &mut self,
        engine: &mut Engine,
        show: &Show,
        now: Option<f64>,
        dt: f64,
        show_hash: Option<u64>,
    ) {
        let Some(now) = now else {
            // No shared clock (it started again): what plays goes on by this
            // computer's own (R21: never waits for the primary), from the
            // shared time it was at.
            match engine.net_shared_time() {
                Some(was) if dt.is_finite() && dt >= 0.0 => engine.tick_shared(show, was + dt),
                _ => engine.tick(show, dt),
            }
            self.in_step = false;
            return;
        };
        self.queue
            .sort_by(|a, b| a.at.total_cmp(&b.at).then(a.seq.cmp(&b.seq)));
        // Not with another show than the primary's (it is on its way): the
        // commands wait for it, and are done late, placed where they belong.
        let other_show = self.primary_show.is_some_and(|p| Some(p) != show_hash);
        while !other_show && self.queue.first().is_some_and(|t| t.at <= now) {
            let t = self.queue.remove(0);
            // Arrived after its time: done now, placed as if it had been then.
            let before = engine.net_shared_time().unwrap_or(t.at);
            engine.tick_shared(show, t.at);
            let late = (before - t.at).max(0.0);
            engine.net_apply(show, &t.cmd, late);
            self.done.insert(t.seq);
            while self.done.remove(&(self.floor + 1)) {
                self.floor += 1;
            }
        }
        engine.tick_shared(show, now);
        let Some(s) = self.state.take() else {
            return;
        };
        // Compared only when this computer has done exactly the commands the
        // primary had when it said this: none is waiting that it had done, and
        // none done here that it hadn't.
        let behind = self.queue.iter().any(|t| t.seq <= s.seq);
        let ahead = self.done.iter().any(|d| *d > s.seq) || self.floor > s.seq;
        if behind || ahead || show_hash != Some(s.show) {
            self.in_step = false;
            return;
        }
        let fixed = engine.net_correct(show, &s, now, &mut self.doubts);
        self.in_step = fixed.is_empty() && self.doubts.is_empty();
        if !fixed.is_empty() {
            // What it put right is the primary's state after its command
            // `s.seq`: one of those arriving now would be done twice.
            self.floor = self.floor.max(s.seq);
            self.done.retain(|d| *d > s.seq);
            self.queue.retain(|t| t.seq > s.seq);
            self.corrections += fixed.len() as u64;
            for f in &fixed {
                log::info!("show network: in step with the primary again: {f}");
            }
            self.corrected = fixed;
        }
    }
}

#[cfg(test)]
#[path = "play_tests.rs"]
mod tests;
