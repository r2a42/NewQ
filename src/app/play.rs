//! The app's side of computers playing together (stage 4a; `net::play`): which
//! way the engine is ticked, the commands and heartbeats moved through the hub,
//! the show sent to the followers and taken from the primary, and what this
//! computer tells the others of itself. Called from `App::pass_net` (messages)
//! and `App::pass_show` (the tick). Nothing here waits on the network.

use super::App;
use crate::engine::NetMode;
use crate::net::hub::{NetView, SyncFacts};
use crate::net::node::{Identity, NodeId, Role};
use crate::net::play::{
    Follower, Leader, Out, ShowAssembly, Standing, lead_for, show_hash, show_parts,
};
use crate::net::proto::{Link, Msg};
use crate::net::time::local_now;
// ---- Take control (stage 5) ----
use crate::net::hub::Hub;

/// The shared time when this computer runs the show: its own clock, or the
/// one it carried on when it took control (`Hub::primary_now`).
fn primary_now(net: &Hub) -> f64 {
    net.primary_now()
}
// ---- end of take control's ----
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A show edit is sent this long after the last change of a burst.
const SETTLE: Duration = Duration::from_millis(150);
/// The show is looked at for changes this often.
const LOOK: Duration = Duration::from_millis(100);
/// A computer whose show isn't the primary's is sent it again this often.
const RESEND_SHOW: Duration = Duration::from_secs(3);
/// While in a show network the loop comes round at least this often (the
/// heartbeat, the clock).
const PACE: Duration = Duration::from_millis(50);
/// What a computer's status says of itself is told to the hub this often.
const TELL: Duration = Duration::from_millis(500);

/// The file a follower keeps the last show it was given in (beside `node.json`),
/// so it comes up with it after a restart.
const SYNCED: &str = "synced-show.newq";

#[derive(Default)]
pub(super) struct Playing {
    mode: NetMode,
    /// The shared clock's `jumps` last seen (a jump of more than
    /// `clock::Settings::big_step`: the engine's shared times move with it).
    clock_jumps: u32,
    leader: Option<Leader>,
    follower: Follower,
    // The primary's show, as sent.
    version: u64,
    hash: Option<u64>,
    seen: Option<(u64, Instant)>,
    looked: Option<Instant>,
    resent: HashMap<NodeId, Instant>,
    followers: HashSet<NodeId>,
    // A follower's show, as taken.
    assembly: ShowAssembly,
    have: Option<u64>,
    told: Option<Instant>,
    /// The primary asked for NewQ to start again.
    pub restart: bool,
    /// This computer's address and when it was looked at (a display card's).
    address: Option<(Instant, Option<String>)>,
    /// Started as a display computer (`--display`, NewQ Display): one from
    /// the first moment, before any primary adopts it (`display_computer`).
    pub display: bool,
    /// The waiting computers this primary added because its show names them
    /// (`vouched_for`), each once a run.
    pub vouched: HashSet<NodeId>,
    /// The card drawn on each monitor (its words and texture).
    pub cards: HashMap<usize, (Card, std::sync::Arc<crate::gpu::LayerTexture>)>,
    /// Since when no computer has had a session with this one (a display
    /// computer's card: `not_reached_words`).
    unreached_since: Option<Instant>,
    /// The show has started (`show_started`): this display computer's
    /// start-up card is gone for the rest of the run (`card_shows`).
    started: bool,
    // ---- The network test (stage 6) ----
    measure: crate::net::measure::Measurer,
    /// This computer's log since the test began, being read for the primary.
    tail: Option<(u64, std::sync::mpsc::Receiver<String>)>,
    /// Where in the log the test began (`LogMark`).
    log_mark: crate::net::measure::LogMark,
    /// When the measuring began, or was last asked for again (it ends by
    /// itself if never told to).
    measuring_since: Option<Instant>,
    /// Since when the primary has been out of reach while measuring.
    primary_away: Option<Instant>,
    /// `NEWQ_NET_TEST_AUTOSTART` was looked at (and acted on).
    autostarted: bool,
    auto_ready: Option<Instant>,
    // ---- Every test across the network (`net::measure_local`) ----
    /// What this follower measures of itself during a test.
    local: crate::net::measure_local::LocalMeter,
    /// Its rows of measurements (on a thread of their own).
    local_stats: Option<crate::stats::StatsLog>,
    /// The primary's test asked for its sound muted here.
    test_muted: bool,
    /// The test's network load with the primary (`net::load`), while measuring.
    load: Option<crate::net::load::LoadClient>,
}

impl Playing {
    /// Where a follower stands, for its card; None elsewhere.
    pub fn standing(&self, clock_locked: bool) -> Option<Standing> {
        (self.mode == NetMode::Follower).then(|| self.follower.standing(clock_locked, self.have))
    }
}

/// What this computer is in the show network now. A primary with no computers
/// adopted is alone: everything as before networks.
pub(super) fn mode_of(v: &NetView) -> NetMode {
    if !v.me.on {
        NetMode::Off
    } else if v.is_primary() {
        if v.peers.iter().any(|p| p.ours) {
            NetMode::Primary
        } else {
            NetMode::Off
        }
    } else if v.me.adopted() {
        NetMode::Follower
    } else {
        NetMode::Off
    }
}

/// The computers of the network a primary sends to (those with a session up).
fn followers(v: &NetView) -> Vec<NodeId> {
    v.peers
        .iter()
        .filter(|p| p.ours && p.connected)
        .map(|p| p.id)
        .collect()
}

impl App {
    /// The engine's tick, by the shared clock while in a show network.
    pub(super) fn play_tick(&mut self, dt: f64) {
        let mode = mode_of(&self.ui.net);
        if mode != self.play.mode {
            self.play_switch(mode);
        }
        match mode {
            NetMode::Off => self.engine.tick(&self.show, dt),
            NetMode::Primary => self.play_primary(),
            NetMode::Follower => {
                let clock = self.net.clock().now();
                if let Some(n) = clock
                    && n.jumps != self.play.clock_jumps
                {
                    // The clock jumped (put back, rather than chased for
                    // days): the engine's shared times move with it rather
                    // than standing still that long (or jumping), so each cue
                    // and timeline carries on exactly where it is
                    // (`a_clock_put_back_moves_nothing_that_plays`), and a
                    // command timed by the primary's clock lands where it
                    // belongs (`a_command_after_the_clock_was_put_back_starts_near_zero`).
                    self.play.clock_jumps = n.jumps;
                    let by = self
                        .engine
                        .net_shared_time()
                        .map_or(0.0, |was| n.time - (was + dt.max(0.0)));
                    log::info!(
                        "show network: the shared clock jumped to the primary's ({by:+.3} s)"
                    );
                    self.engine.net_shift_shared(by);
                }
                let shared = clock
                    .filter(|n| n.locked || self.engine.net_shared_time().is_some())
                    .map(|n| n.time);
                self.play
                    .follower
                    .step(&mut self.engine, &self.show, shared, dt, self.play.have);
            }
        }
        self.play_tell();
    }

    fn play_switch(&mut self, mode: NetMode) {
        log::info!(
            "show network: playing {:?} (was {:?})",
            mode,
            self.play.mode
        );
        // ---- Take control (stage 5) ----
        // A backup that takes control, and a primary that steps down to be
        // the backup, go straight from one to the other: the engine keeps
        // the shared times its cues are anchored to (the clock carries on),
        // so nothing that plays here jumps.
        let took_control = self.play.mode == NetMode::Follower && mode == NetMode::Primary;
        let stepped_down = self.play.mode == NetMode::Primary && mode == NetMode::Follower;
        if took_control {
            // The old primary's commands still to come are let go: if it is
            // still there it does them itself as it steps down (a send cue
            // never fires twice, R8).
            let let_go = self.play.follower.take_queue().len();
            if let_go > 0 {
                log::info!("show network: {let_go} of the old primary's commands let go");
            }
            self.ui.took_control();
            self.engine
                .log("Show network: this computer took control and runs the show now");
        }
        if stepped_down {
            // What it had still to do it does now (the new primary let its
            // copies go); inputs not yet timed are the new primary's to make.
            if let Some(mut l) = self.play.leader.take() {
                l.step(&mut self.engine, &self.show, f64::INFINITY);
            }
            let left = self.engine.net_take();
            if !left.is_empty() {
                log::info!(
                    "show network: {} inputs left to the new primary",
                    left.len()
                );
            }
            self.engine
                .log("Show network: another computer took control; this one is the backup now");
        }
        if took_control || stepped_down {
            self.engine.net_set_mode(mode);
        }
        // ---- end of take control's ----
        // Commands still to be done here are done now, as they would have
        // been alone.
        if let Some(mut l) = self.play.leader.take() {
            l.step(&mut self.engine, &self.show, f64::INFINITY);
        }
        // Inputs caught and not yet timed are done at once.
        let left = self.engine.net_take();
        if !(took_control || stepped_down) {
            self.engine.net_set_mode(NetMode::Off);
        }
        for c in left {
            self.engine.net_apply(&self.show, &c, 0.0);
        }
        self.engine.net_set_mode(mode);
        self.play.mode = mode;
        self.play.follower = Follower::default();
        self.play.followers.clear();
        self.play.resent.clear();
        match mode {
            NetMode::Primary => {
                self.play.leader = Some(Leader::new(crate::net::node::random_u64()));
                // Versions go on rising across restarts of the primary (and
                // past the old primary's, after taking control).
                self.play.version = (primary_now(&self.net) * 1000.0) as u64;
                self.play.hash = None;
                self.play.seen = None;
            }
            NetMode::Follower => {
                self.play.assembly = ShowAssembly::default();
            }
            NetMode::Off => {}
        }
    }

    fn play_primary(&mut self) {
        let Some(leader) = self.play.leader.as_mut() else {
            return;
        };
        let now = primary_now(&self.net);
        let cmds = self.engine.net_take();
        for c in &cmds {
            if let Some(id) = c.will_start(&self.show) {
                self.engine.load_ahead(&self.show, id);
            }
        }
        let v = self.ui.net.clone();
        let to = followers(&v);
        // Computers that came (again) are given what is still to be done; those
        // gone owe nothing.
        let mut outs = Vec::new();
        let now_set: HashSet<NodeId> = to.iter().copied().collect();
        for who in now_set.difference(&self.play.followers) {
            outs.extend(leader.joined(*who, now));
        }
        for gone in self.play.followers.difference(&now_set) {
            leader.forget(*gone);
        }
        self.play.followers = now_set;
        let ours = v.peers.iter().filter(|p| p.ours && p.connected);
        let rtt = ours
            .clone()
            .filter_map(|p| p.status.as_ref()?.rtt_ms)
            .reduce(f64::max)
            .map(|ms| ms / 1000.0);
        let wifi = ours
            .filter_map(|p| p.status.as_ref())
            .any(|s| s.link == Link::WiFi);
        outs.extend(leader.issue(cmds, now, lead_for(rtt, wifi), &to));
        outs.extend(leader.resend(now));
        leader.step(&mut self.engine, &self.show, now);
        if let Some(s) = leader.beat(&self.engine, self.play.hash.unwrap_or(0), now) {
            outs.push(Out::State(s));
        }
        for o in outs {
            match o {
                Out::Command(to, t) => self.net.tell(to, Msg::Play(Box::new(t)), true),
                Out::State(s) => self.net.tell(None, Msg::PlayState(Box::new(s)), true),
            }
        }
    }

    /// A message from a computer of the network (`NetEvent::Message`).
    pub(super) fn play_message(&mut self, from: NodeId, msg: Msg) {
        let primary =
            self.ui.net.me.primary == Some(from) && mode_of(&self.ui.net) == NetMode::Follower;
        match msg {
            Msg::Play(t) if primary => {
                // ---- The network test: how much lead it had left ----
                let shared = self.net.clock().now().map(|n| n.time);
                self.play.measure.command(shared, t.seq, t.at);
                let (epoch, seq) = self.play.follower.receive(*t, &mut self.engine, &self.show);
                self.net.tell(Some(from), Msg::PlayAck { epoch, seq }, true);
            }
            Msg::PlayState(s) if primary => {
                // ---- The network test: how far apart, at this heartbeat ----
                if self.play.measure.running().is_some() && self.play.follower.has_done(&s) {
                    let ours = self.engine.net_state(s.time);
                    // This computer's clock against the primary's, as this
                    // heartbeat shows it: what the screens show is compared.
                    let clock = self.net.clock();
                    let seen = clock.now().map(|n| {
                        crate::net::measure::seen_offset(n.time, s.time, clock.best_rtt())
                    });
                    self.play.measure.heartbeat(&s, &ours, seen);
                } else {
                    self.play.measure.unmatched(&s);
                }
                self.play.follower.heard(*s)
            }
            Msg::ShowPart(p) if primary => {
                if let Some((_, hash, json)) = self.play.assembly.take(*p) {
                    self.take_show(hash, json);
                }
            }
            Msg::PlayAck { epoch, seq } => {
                if let Some(l) = self.play.leader.as_mut() {
                    l.ack(from, epoch, seq);
                }
            }
            Msg::Restart if primary => {
                log::info!("show network: the primary asked NewQ to start again");
                self.play.restart = true;
            }
            Msg::StartAtLogin { on } if primary => self.set_start_at_login(on),
            Msg::NetTest(t) => self.net_test_message(from, *t, primary),
            _ => {}
        }
    }

    /// A follower takes the primary's show in place of its own.
    fn take_show(&mut self, hash: u64, json: String) {
        if self.play.have == Some(hash) {
            return;
        }
        let mut show: crate::show::Show = match serde_json::from_str(&json) {
            Ok(s) => s,
            Err(e) => {
                self.engine.problem(format!(
                    "Show network: the primary's show couldn't be read: {e}"
                ));
                return;
            }
        };
        // (The hash stays the bytes', so putting this right never puts the
        // computer out of step.)
        show.sanitize_stage();
        self.show = show;
        self.ui.took_synced_show(&self.show, &mut self.engine);
        self.ui.check_media(&self.show);
        self.play.have = Some(hash);
        // A show that isn't a test's while measuring: the test is over on
        // the primary (its `End` was lost on the way).
        if self.play.measure.running().is_some()
            && !crate::testmode::network::is_any_test_show(&self.show)
        {
            self.stop_measuring("the primary's own show came back");
        }
        log::info!(
            "show network: took the primary's show ({} bytes)",
            json.len()
        );
        // Kept for the next start (on a thread: a large show is a moment's write);
        // not the network test's show, which is the primary's for a few minutes.
        if crate::testmode::network::is_test_show(&self.show) {
            return;
        }
        let dir = crate::net::node::node_dir();
        let _ = std::thread::Builder::new()
            .name("net-keep-show".into())
            .spawn(move || {
                let path = dir.join(SYNCED);
                let tmp = path.with_extension("tmp");
                if std::fs::write(&tmp, json.as_bytes()).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
            });
    }

    /// A follower starting up takes the show it was last given, if it has one
    /// (the primary sends a newer one when it is reached).
    pub(super) fn play_load_kept(&mut self) -> bool {
        let path: PathBuf = crate::net::node::node_dir().join(SYNCED);
        let Ok(json) = std::fs::read_to_string(&path) else {
            return false;
        };
        let Ok(mut show) = serde_json::from_str::<crate::show::Show>(&json) else {
            return false;
        };
        show.sanitize_stage();
        self.show = show;
        self.engine.playhead = self.show.cues.first().map(|c| c.id);
        self.ui.took_synced_show(&self.show, &mut self.engine);
        self.play.have = Some(show_hash(&json));
        log::info!("show network: opened the show last given by the primary");
        true
    }

    /// The primary: send the show when it has changed and settled, and to any
    /// computer whose show isn't this one (one that joined, or missed it).
    pub(super) fn play_send_show(&mut self) {
        self.net_test_pass();
        if self.play.mode != NetMode::Primary {
            return;
        }
        let now = Instant::now();
        if self
            .play
            .looked
            .is_some_and(|t| now.duration_since(t) < LOOK)
        {
            return;
        }
        self.play.looked = Some(now);
        let json = match self.ui.show_json() {
            Some(j) => j.to_string(),
            None => serde_json::to_string(&self.show).unwrap_or_default(),
        };
        let h = show_hash(&json);
        if self.play.seen.is_none_or(|(s, _)| s != h) {
            self.play.seen = Some((h, now));
        }
        let settled = self
            .play
            .seen
            .is_some_and(|(_, at)| now.duration_since(at) >= SETTLE);
        if self.play.hash != Some(h) && (settled || self.play.hash.is_none()) {
            self.play.version += 1;
            self.play.hash = Some(h);
            for p in show_parts(self.play.version, &json) {
                self.net.tell(None, Msg::ShowPart(Box::new(p)), false);
            }
            self.play.resent.clear();
            return;
        }
        let behind: Vec<NodeId> = self
            .ui
            .net
            .peers
            .iter()
            .filter(|p| p.ours && p.connected)
            .filter(|p| {
                p.status
                    .as_ref()
                    .is_none_or(|s| s.show_hash != self.play.hash)
            })
            .map(|p| p.id)
            .collect();
        for who in behind {
            if self
                .play
                .resent
                .get(&who)
                .is_some_and(|t| now.duration_since(*t) < RESEND_SHOW)
            {
                continue;
            }
            self.play.resent.insert(who, now);
            for p in show_parts(self.play.version, &json) {
                self.net.tell(Some(who), Msg::ShowPart(Box::new(p)), false);
            }
        }
    }

    /// Tell the hub how this computer is playing, for its status report.
    fn play_tell(&mut self) {
        let now = Instant::now();
        if self.play.told.is_some_and(|t| now.duration_since(t) < TELL) {
            return;
        }
        self.play.told = Some(now);
        let clock = self.net.clock();
        let mut facts = match self.play.mode {
            NetMode::Follower => {
                let n = clock.now();
                let locked = n.is_some_and(|n| n.locked);
                SyncFacts {
                    // None while not locked (the page says so), capped.
                    clock_error_ms: n
                        .and_then(|n| n.report_ms())
                        .map(|e| (e * 100.0).round() / 100.0),
                    clock_locked: locked,
                    rtt_ms: clock
                        .worst_rtt()
                        .map(|r| (r * 1000.0 * 100.0).round() / 100.0),
                    show_version: None,
                    show_hash: self.play.have,
                    in_step: self.play.follower.in_step,
                    standing: self.play.standing(locked).map(|s| s.words().to_string()),
                    start_at_login: false,
                }
            }
            NetMode::Primary => SyncFacts {
                clock_error_ms: Some(0.0),
                clock_locked: true,
                show_version: Some(self.play.version),
                show_hash: self.play.hash,
                in_step: true,
                standing: Some("Runs the show".into()),
                ..Default::default()
            },
            NetMode::Off => SyncFacts::default(),
        };
        facts.start_at_login = self.ui.start_at_login;
        self.net.set_sync(facts);
    }

    /// When the loop must come round next for the show network: its pace, or
    /// a command's time sooner.
    pub(super) fn play_wake(&self, now: Instant) -> Option<Instant> {
        let due = match self.play.mode {
            NetMode::Off => return None,
            NetMode::Primary => self
                .play
                .leader
                .as_ref()
                .and_then(Leader::next_due)
                .map(|at| at - primary_now(&self.net)),
            NetMode::Follower => {
                let shared = self.net.clock().now().map(|n| n.time);
                self.play
                    .follower
                    .next_due()
                    .zip(shared)
                    .map(|(at, now)| at - now)
            }
        };
        let pace = now + PACE;
        Some(match due {
            Some(d) if d.is_finite() => pace.min(now + Duration::from_secs_f64(d.max(0.0))),
            _ => pace,
        })
    }

    /// The primary only sends, listens and records (R8): a follower's remote
    /// inputs, clock triggers, streams and feedback stay off.
    pub(super) fn sends_here(&self) -> bool {
        self.play.mode != NetMode::Follower
    }

    /// Start at login on this computer (`autostart.rs`), as the primary asked
    /// or the page ticked: only ever on someone's word, never by itself (a
    /// computer made a display is not set to start at login).
    pub(super) fn set_start_at_login(&mut self, on: bool) {
        // A display computer comes back as one (`--display`, display mode):
        // no splash, nothing asked, its cards at once.
        let display = self.play.display || self.ui.net.me.role == Role::Display;
        match crate::autostart::set(on, display) {
            Ok(()) => log::info!(
                "start at login {} ({})",
                if on { "switched on" } else { "switched off" },
                crate::autostart::name(display)
            ),
            Err(e) => self
                .engine
                .problem(format!("Start at login couldn't be changed: {e}")),
        }
        self.ui.start_at_login = crate::autostart::is_set();
    }

    /// NewQ opened normally on a display computer of a show network (its
    /// banner's *Open as a display computer*): display mode from now on, as if
    /// NewQ Display had started it. The control window hides and every monitor
    /// gets its window (`keep_control_hidden`, `reconcile_outputs`).
    pub(super) fn open_as_display(&mut self) {
        if !self.play.display {
            log::info!("show network: opened as a display computer from the control window");
            self.play.display = true;
        }
    }
}

/// Whether NewQ, started again (`restart_self`), needs `--display` added to
/// its arguments: it is in display mode now but wasn't started so (the
/// banner's *Open as a display computer*). Pure, for the tests.
pub(super) fn restart_adds_display(display: bool, args: &[std::ffi::OsString]) -> bool {
    display && !args.iter().any(|a| a == "--display")
}

// ---- Display computers (D8) ----

/// What a display computer's card says (`card_text`), and whether it is the
/// Identify flash.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Card {
    pub text: String,
    pub big: bool,
    /// Small, dim words at the foot of the card (how to add it, how to quit).
    pub hint: Option<String>,
}

/// The key that quits a display computer, as the card says it (`quits`).
pub(super) fn quit_words() -> &'static str {
    if cfg!(target_os = "macos") {
        "Command-Q quits NewQ on this computer"
    } else {
        "Ctrl+Q quits NewQ on this computer"
    }
}

/// Whether a key pressed in an output window quits a display computer:
/// Ctrl+Q, or Command-Q on a Mac (`egui::Modifiers::command`).
pub(super) fn quits(key: egui::Key, mods: egui::Modifiers) -> bool {
    key == egui::Key::Q && mods.command && !mods.alt && !mods.shift
}

/// Display mode (no control window, every monitor an output): only a NewQ
/// started as a display (`--display`: NewQ Display, or start at login on a
/// display computer) that is waiting for a primary or is its display. NewQ
/// started normally opens its control window even on a computer a primary
/// made a display (it follows, locked, and its banner offers display mode or
/// leaving: `paired_display`). Given another role (the backup) it is not: its
/// control window shows, as for any backup. Pure, for the tests.
pub(super) fn display_computer(me: &Identity, started_as_display: bool) -> bool {
    started_as_display && matches!(me.role, Role::Display | Role::Alone)
}

/// A display computer of a show network opened as NewQ, not as NewQ Display:
/// its control window shows (locked, as any follower's) and its banner says
/// what it is and offers display mode or leaving the network. Pure, for the
/// tests.
pub(super) fn paired_display(me: &Identity, started_as_display: bool) -> bool {
    me.on && me.adopted() && me.role == Role::Display && !started_as_display
}

/// How long a display computer goes without any computer reaching it (no
/// session up) before its card says so (`not_reached_words`).
pub(super) const NOT_REACHED_AFTER: Duration = Duration::from_secs(20);

/// Since when nothing has reached this computer, after this look: reset by a
/// session up, started by the first look without one. Pure, for the tests.
pub(super) fn unreached_since(
    was: Option<Instant>,
    reached: bool,
    now: Instant,
) -> Option<Instant> {
    if reached {
        None
    } else {
        Some(was.unwrap_or(now))
    }
}

/// The card's words for a display computer that no primary has reached for
/// `NOT_REACHED_AFTER` (it announces itself, but no session came): plain,
/// with the firewall named as one possibility, Windows' own only on Windows.
/// `primary`: the one it belongs to, if any. Pure, for the tests.
pub(super) fn not_reached_words(primary: Option<&str>, windows: bool) -> String {
    let first = match primary {
        Some(p) => format!("Not reached by the primary ({p}) for a while."),
        None => "Not reached by a primary yet.".to_string(),
    };
    let wall = if windows {
        "Windows Firewall on this computer may be blocking NewQ: look for its \"allow access\" \
         question (press Alt+Tab), or allow NewQ in Windows Security, Firewall."
    } else {
        "the firewall on this computer may be blocking NewQ."
    };
    format!("{first}\nIf the primary lists this computer but can't add it, {wall}")
}

/// The windows a display computer has: one per connected monitor, whatever
/// the show gives them (`reconcile_outputs`). None for any other computer, or
/// with every output kept in the main window. Pure, for the tests.
pub(super) fn display_windows(
    display: bool,
    main_window_only: bool,
    monitors: usize,
) -> Vec<usize> {
    if display && !main_window_only {
        (0..monitors).collect()
    } else {
        Vec::new()
    }
}

/// Whether the show has started, as a display computer sees it, after this
/// look (`Playing::started`): once it has, it has for the rest of the run,
/// whatever comes and goes (out of step, the clock lost, a new show sent, a
/// takeover). `heard`: the primary said so or was seen starting something
/// (`Follower::saw_start`); `started_here`: this computer's engine has started
/// a cue. Pure, for the tests.
pub(super) fn show_started(was: bool, heard: bool, started_here: bool) -> bool {
    was || heard || started_here
}

/// Whether a display computer's screen shows its start-up card: always while
/// it waits for a primary to add it (else nobody could find it); once added,
/// until the show starts (the first cue the primary plays), as the show's
/// setting says, and never again after that, so nothing but the show (or
/// Identify) is ever seen between cues. `AlsoUnused` keeps it on the screens
/// the show gives no display, which nothing is ever played on. Pure, for the
/// tests.
pub(super) fn card_shows(
    setting: crate::show::WaitCards,
    adopted: bool,
    started: bool,
    has_display: bool,
) -> bool {
    use crate::show::WaitCards;
    if !adopted {
        return true;
    }
    match setting {
        WaitCards::Never => false,
        WaitCards::UntilReady => !started,
        WaitCards::AlsoUnused => !started || !has_display,
    }
}

/// Which screen of its computer a monitor is, as the primary numbers them (its
/// Displays menu, Test Mode's *Show on*: the computer's report lists the
/// monitors in this order): "Assistant · Screen 1", even with one. Pure, for
/// the tests.
pub(super) fn screen_name(name: &str, monitor: usize) -> String {
    format!("{name} · Screen {}", monitor + 1)
}

/// What Identify shows on each screen of the computer, big, on black: its
/// name, which of its screens this is ("Screen 2", as `screen_name` numbers
/// them) and its address on the network. Pure, for the tests.
pub(super) fn identify_text(name: &str, monitor: usize, address: Option<&str>) -> String {
    let mut lines = vec![name.to_string(), format!("Screen {}", monitor + 1)];
    if let Some(a) = address {
        lines.push(a.to_string());
    }
    lines.join("\n")
}

/// The address a card shows: the best of this computer's adapters, as the
/// show network ranks them (`paths::rank`: a wired one on our subnet before
/// Wi-Fi, a VPN last; never link-local), else None. Pure, for the tests.
pub(super) fn card_address_of(adapters: &[crate::net::paths::Adapter]) -> Option<String> {
    adapters
        .iter()
        .filter(|a| !a.ip.is_link_local() && !a.ip.is_loopback())
        .min_by_key(|a| crate::net::paths::rank(a.ip.into(), adapters))
        .map(|a| a.ip.to_string())
}

/// The card's words: the computer's name and which of its screens this is,
/// its address, who runs the show and where it stands. Pure, for the tests.
pub(super) fn card_text(
    name: &str,
    monitor: usize,
    address: Option<&str>,
    primary: Option<(&str, bool)>,
    standing: Option<Standing>,
    clock_ms: Option<f64>,
) -> String {
    let mut lines = vec![screen_name(name, monitor)];
    if let Some(a) = address {
        lines.push(a.to_string());
    }
    lines.push(String::new());
    if primary.is_none() && standing.is_none() {
        // Not adopted yet (started as a display): any primary may add it.
        lines.push("Waiting for a NewQ primary on this network".into());
        return lines.join("\n");
    }
    match primary {
        Some((p, true)) => lines.push(format!("Run from {p}")),
        Some((p, false)) => lines.push(format!("Waiting for the primary ({p})")),
        None => lines.push("Waiting for the primary".into()),
    }
    let mut state = standing.map_or("Waiting for the primary".to_string(), |s| {
        s.words().to_string()
    });
    if let Some(e) = clock_ms.filter(|_| standing != Some(Standing::WaitingForClock)) {
        state.push_str(&format!(" · clock ±{e:.1} ms"));
    }
    lines.push(state);
    lines.join("\n")
}

/// The waiting computers the open show vouches for: switched on, in no show
/// network, speaking this NewQ's protocol, and named by one of the show's
/// displays or sound outputs (it was this show's computer on another primary,
/// or before this primary forgot it). A primary adds them by itself, once a run
/// each (`asked`), so a computer released by hand stays released. A computer
/// that belongs to any network, another primary's included, is never taken.
/// Pure, for the tests.
pub(super) fn vouched_for(
    show: &crate::show::Show,
    v: &NetView,
    asked: &HashSet<NodeId>,
) -> Vec<NodeId> {
    if !v.is_primary() {
        return Vec::new();
    }
    let named = |id: NodeId| {
        show.outputs.iter().any(|o| o.computer == Some(id))
            || show.audio_outputs.iter().any(|a| a.computers.contains(&id))
    };
    v.peers
        .iter()
        .filter(|p| {
            p.waiting
                && !p.ours
                && p.network.is_none()
                && p.compatible
                && !p.adopting
                && p.problem.is_none()
                && !asked.contains(&p.id)
                && named(p.id)
        })
        .map(|p| p.id)
        .collect()
}

impl App {
    /// A primary adds, with no click, a waiting computer its open show names
    /// (`vouched_for`).
    pub(super) fn add_vouched(&mut self) {
        for id in vouched_for(&self.show, &self.ui.net, &self.play.vouched) {
            self.play.vouched.insert(id);
            let name = self.ui.net.name_of(id);
            self.engine.log(format!(
                "Show network: {name} is one of this show's computers: added to the show"
            ));
            self.net.ask(crate::net::hub::NetAction::Adopt(id));
        }
    }

    /// A display computer: no control window; every monitor is an output.
    pub(super) fn is_display_computer(&self) -> bool {
        display_computer(&self.ui.net.me, self.play.display)
    }

    /// Whether any computer has a session with this one now (once a pass,
    /// before the cards are made: `not_reached_words`).
    pub(super) fn note_reached(&mut self) {
        let v = &self.ui.net;
        let reached = !v.me.on || v.peers.iter().any(|p| p.connected);
        self.play.unreached_since =
            unreached_since(self.play.unreached_since, reached, Instant::now());
    }

    /// This computer's address on the network, for its card (looked at now
    /// and then: a cable plugged in, an address given).
    pub(super) fn own_address(&mut self) -> Option<String> {
        let now = Instant::now();
        if self
            .play
            .address
            .as_ref()
            .is_none_or(|(t, _)| now.duration_since(*t) > Duration::from_secs(10))
        {
            let found = if_addrs::get_if_addrs()
                .unwrap_or_default()
                .into_iter()
                .filter(|i| !i.is_loopback())
                .find(|i| i.ip().is_ipv4())
                .map(|i| i.ip().to_string());
            self.play.address = Some((now, found));
        }
        self.play.address.as_ref().and_then(|(_, a)| a.clone())
    }

    /// The address the cards and Identify show: the one the show network
    /// would use (`card_address_of`), else the first the system lists.
    pub(super) fn card_address(&mut self) -> Option<String> {
        card_address_of(&self.ui.net.adapters).or_else(|| self.own_address())
    }

    /// The card on one monitor of this computer, if any: the name big while
    /// the primary asks it to Identify (on every output, whatever the role);
    /// on a display computer, the quiet card until it is in step, and always on
    /// a monitor the show gives no display. `monitor`: which of this
    /// computer's monitors (the card numbers it: `screen_name`).
    pub(super) fn card(&mut self, monitor: usize, has_display: bool) -> Option<Card> {
        let v = self.ui.net.clone();
        if let Some(until) = v.identify_until
            && Instant::now() < until
        {
            // Flashing: on half a second, dimmer the next.
            let on = (local_now() * 2.0) as i64 % 2 == 0;
            let address = self.card_address();
            let text = identify_text(&v.name, monitor, address.as_deref());
            return Some(Card {
                text: if on { text } else { format!("{text} ") },
                big: true,
                hint: None,
            });
        }
        if !self.is_display_computer() {
            return None;
        }
        let clock = self.net.clock().now();
        let standing = self.play.standing(clock.is_some_and(|n| n.locked));
        // The card is for start-up: from the first cue the primary plays, a
        // screen with nothing playing is black for the rest of the run (out
        // of step for a heartbeat, a clock lost, a new show: nothing brings
        // the card back in front of an audience).
        let adopted = v.me.adopted();
        self.play.started = show_started(
            self.play.started,
            self.play.follower.saw_start,
            self.engine.started_total() > 0,
        );
        if !card_shows(
            self.show.wait_cards,
            adopted,
            self.play.started,
            has_display,
        ) {
            return None;
        }
        let address = self.card_address();
        let primary = v.following.as_ref().map(|f| (f.name.as_str(), f.online));
        let mut text = card_text(
            &v.name,
            monitor,
            address.as_deref(),
            primary,
            standing,
            clock
                .and_then(|n| n.report_ms())
                .map(|e| (e * 10.0).round() / 10.0),
        );
        // The media manager's progress while files are still coming.
        if let Some(own) = &self.ui.media.own {
            let used = &own.progress.used;
            if used.bytes_wanted > 0 && used.bytes_here < used.bytes_wanted {
                text.push_str(&format!("\nMedia {:.0} % here", used.percent()));
            }
        }
        // Announcing itself, but no primary has come for a while: say so
        // (a firewall's question can hide behind this full-screen card).
        if v.port.is_some()
            && self
                .play
                .unreached_since
                .is_some_and(|t| t.elapsed() >= NOT_REACHED_AFTER)
        {
            let primary = v.following.as_ref().map(|f| f.name.as_str());
            text.push_str("\n\n");
            text.push_str(&not_reached_words(primary, cfg!(windows)));
        }
        let waiting = !v.me.adopted();
        let mut hint = Vec::new();
        if waiting {
            hint.push(
                "To add it: on the computer that runs the show, open Settings, then Show network, \
                 and press Add to the show."
                    .to_string(),
            );
        }
        if let Some(p) = &v.problem {
            hint.push(p.clone());
        }
        // The one-time start-at-login clean-up removed an older NewQ's entry.
        if let Some(n) = crate::ui::login_note(&self.ui, Instant::now()) {
            hint.push(n.to_string());
        }
        hint.push(quit_words().to_string());
        Some(Card {
            text,
            big: false,
            hint: Some(hint.join("\n")),
        })
    }

    /// The control window is hidden on a display computer, and shown again
    /// when it is given another role (no restart needed).
    pub(super) fn keep_control_hidden(&mut self) {
        let hide = self.is_display_computer();
        if let Some(ctl) = &mut self.control
            && hide
            && ctl.shown
        {
            log::info!("show network: a display computer: the control window is hidden");
            ctl.shown = false;
            ctl.window.set_visible(false);
        }
    }
}

// ---- The network test (stage 6, `net::measure`, `testmode::network`) ----

/// A follower measures for at most this long without being told it is over,
/// or again to measure (the primary says `Begin` again every
/// `fleet::BEGIN_AGAIN`; one before 0.24.4 never did, hence the hours).
const MEASURE_AT_MOST: Duration = Duration::from_secs(3 * 3600);
/// A follower whose primary has been out of reach this long stops measuring
/// (unmuted, the network load let go). A blip is shorter.
const PRIMARY_AWAY_AT_MOST: Duration = Duration::from_secs(20);

impl App {
    /// The test is over here without an `End` (`why`): nothing is measured
    /// or sent, the sound is back and the network load let go.
    fn stop_measuring(&mut self, why: &str) {
        log::info!("network test: measuring stopped: {why}");
        self.play.measure = Default::default();
        self.play.measuring_since = None;
        self.play.primary_away = None;
        self.play.local = Default::default();
        self.play.local_stats = None;
        self.play.load = None;
        self.unmute_test();
    }

    /// The primary's Test Mode said what it has for the computers as NewQ
    /// closes (`TestMode::shutdown`: the test is over): sent now, with a
    /// moment for the frames to leave before the sessions close.
    pub(super) fn net_test_flush(&mut self) {
        if self.play.mode != NetMode::Primary || self.ui.test.net_out.is_empty() {
            return;
        }
        for (to, m) in std::mem::take(&mut self.ui.test.net_out) {
            self.net.tell(to, Msg::NetTest(Box::new(m)), true);
        }
        std::thread::sleep(Duration::from_millis(300));
    }

    /// A network test message: from the primary to this follower (`primary`),
    /// or from a computer to this primary's Test Mode.
    fn net_test_message(&mut self, from: NodeId, t: crate::net::measure::NetTest, primary: bool) {
        use crate::net::measure::NetTest;
        if t == NetTest::Unknown {
            log::debug!("network test: a message from a later NewQ, not known here");
            return;
        }
        if !primary {
            if self.play.mode == NetMode::Primary {
                self.ui.test.net_in.push((from, t));
            }
            return;
        }
        match t {
            NetTest::Begin { run, start, load } => {
                self.net_load(from, run, load);
                if self.play.measure.running() != Some(run) {
                    // Stalls from before the test aren't its.
                    let _ = crate::crumbs::take_stalls();
                    // Where its log for the test begins (once a run: the
                    // primary says `Begin` again every minute).
                    crate::logging::flush();
                    let log = crate::crash::log_dir().join("newq.log");
                    self.play
                        .log_mark
                        .begin(run, crate::net::measure::LogAt::of(&log));
                }
                // The primary says it again now and then while the test
                // plays (`Fleet::keep_up`): the measuring goes on as long.
                self.play.measuring_since = Some(Instant::now());
                self.play.measure.begin(run, start);
                // ---- and how it copes itself ----
                if self.play.local.running() != Some(run) {
                    self.play.local_stats = Some(crate::stats::StatsLog::in_memory(
                        Duration::from_secs(crate::net::measure_local::ROW_EVERY),
                        crate::crash::log_dir().join("newq.log"),
                    ));
                }
                self.play.local.begin(run, start);
            }
            NetTest::Mute { run: _, muted } => {
                self.play.test_muted = muted;
                crate::media::audio::set_muted(muted);
            }
            NetTest::Whole { run, test } => self.play.log_mark.whole(run, test),
            NetTest::End { run } => {
                // Only the run measured here (an `End` said again for an
                // earlier part must not end the next one).
                if self.play.measure.running() != Some(run)
                    && self.play.local.running() != Some(run)
                {
                    return;
                }
                let now = self.net.clock().now().map_or(local_now(), |n| n.time);
                let (link, refresh) = self.net_test_facts();
                if let Some(r) = self.play.measure.end(run, now, link, refresh) {
                    self.net.tell(
                        Some(from),
                        Msg::NetTest(Box::new(NetTest::Report(Box::new(r)))),
                        false,
                    );
                }
                let (outputs, device, ram) = self.local_facts();
                if let Some(r) = self.play.local.end(run, now, outputs, device, ram) {
                    self.net.tell(
                        Some(from),
                        Msg::NetTest(Box::new(NetTest::Local(Box::new(r)))),
                        false,
                    );
                }
                self.play.local_stats = None;
                self.unmute_test();
                self.play.measuring_since = None;
                self.play.load = None;
                // Its log since the test began, read on a thread (it can be
                // large).
                let (tx, rx) = std::sync::mpsc::channel();
                let dir = crate::crash::log_dir();
                let from = self.play.log_mark.from(run);
                crate::logging::flush();
                let _ = std::thread::Builder::new()
                    .name("net-test-log".into())
                    .spawn(move || {
                        let _ = tx.send(crate::net::measure::log_since(
                            &dir,
                            from,
                            crate::net::measure::LOG_WHOLE,
                        ));
                    });
                self.play.tail = Some((run, rx));
            }
            _ => {}
        }
    }

    /// For developers (the end-to-end test, `scripts/net_demo.sh`):
    /// `NEWQ_NET_TEST_AUTOSTART=<n>` starts the network test, with media made
    /// here, once `n` computers are in step (once a run).
    fn net_test_autostart(&mut self) {
        if self.play.autostarted {
            return;
        }
        let Some(n) = std::env::var("NEWQ_NET_TEST_AUTOSTART")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
        else {
            self.play.autostarted = true;
            return;
        };
        // The window shows meanwhile (what it says before the test), and the
        // test starts once they have been in step for a few seconds.
        self.ui.test.open = true;
        let ready = crate::testmode::network::in_step(&self.ui.net).len() >= n
            && crate::testmode::network::refusal(&self.ui).is_none();
        if !ready {
            self.play.auto_ready = None;
            return;
        }
        let since = *self.play.auto_ready.get_or_insert_with(Instant::now);
        if since.elapsed() < Duration::from_secs(8) {
            return;
        }
        // `NEWQ_TEST_PARTS` (say `cue,network`) ticks more than the network.
        let parts = crate::testmode::choice::parse_parts(
            &std::env::var("NEWQ_TEST_PARTS").unwrap_or_default(),
        );
        if (parts.cue_list || parts.timeline) && self.ui.test_refusal().is_some() {
            return;
        }
        if let Some(ff) = self.engine.ff.clone() {
            self.play.autostarted = true;
            log::info!("NEWQ_NET_TEST_AUTOSTART: the test starts ({parts:?})");
            self.ui.test.download = false;
            let plan = crate::testmode::choice::plan(crate::testmode::Level::Short, parts, true);
            self.ui.test.start_plan(
                ff,
                &crate::crash::log_dir(),
                plan,
                Vec::new(),
                Default::default(),
            );
        }
    }

    /// This computer's link and its monitors' slowest refresh (frames apart
    /// are counted at the slowest screen: a difference of less than one of
    /// its refreshes can't be seen there).
    fn net_test_facts(&self) -> (Link, Option<f32>) {
        (
            self.ui.net.status.link,
            self.ui
                .monitors
                .iter()
                .filter_map(|m| m.refresh_hz)
                .reduce(f32::min),
        )
    }

    /// Each pass: a primary sends what Test Mode has for the computers; a
    /// follower that is measuring notes and reports. Never waits.
    fn net_test_pass(&mut self) {
        use crate::net::measure::{NetTest, Sample};
        let out = std::mem::take(&mut self.ui.test.net_out);
        if self.play.mode == NetMode::Primary {
            for (to, m) in out {
                self.net.tell(to, Msg::NetTest(Box::new(m)), false);
            }
            self.net_test_autostart();
        }
        let primary = self.ui.net.me.primary;
        self.ui.following_test =
            self.play.mode == NetMode::Follower && self.play.measure.running().is_some();
        if let Some((run, rx)) = &self.play.tail
            && let Ok(text) = rx.try_recv()
        {
            let run = *run;
            self.play.tail = None;
            if let Some(p) = primary {
                // In pieces: one frame of megabytes holds the session for
                // longer than the primary waits to hear (`hub::SILENT_FOR`)
                // on a slow link, and the computer is dropped with its log.
                for piece in crate::net::measure::log_pieces(run, text) {
                    self.net.tell(Some(p), Msg::NetTest(Box::new(piece)), false);
                }
            }
        }
        if self.play.measure.running().is_none() {
            self.play.load = None;
            self.play.primary_away = None;
            return;
        }
        // The primary out of reach for a while (its session closed: it quit,
        // or its `End` was lost with it) ends the measuring here; a blip
        // doesn't (it says `Begin` again when the computer is heard again).
        let reached =
            primary.is_some_and(|p| self.ui.net.peers.iter().any(|v| v.id == p && v.connected));
        if reached {
            self.play.primary_away = None;
        } else if self.play.primary_away.is_none() {
            self.play.primary_away = Some(Instant::now());
        }
        let why = if self.play.mode != NetMode::Follower {
            Some("this computer no longer follows")
        } else if self
            .play
            .primary_away
            .is_some_and(|t| t.elapsed() > PRIMARY_AWAY_AT_MOST)
        {
            Some("the primary has been out of reach")
        } else if self
            .play
            .measuring_since
            .is_some_and(|t| t.elapsed() > MEASURE_AT_MOST)
        {
            Some("it was never told the test is over")
        } else {
            None
        };
        if let Some(why) = why {
            self.stop_measuring(why);
            return;
        }
        let clock = self.net.clock();
        let Some(n) = clock.now() else {
            return;
        };
        let now = n.time;
        let words = self.play.follower.corrected.clone();
        self.play
            .measure
            .corrections(now, self.play.follower.corrections, &words);
        for s in crate::crumbs::take_stalls() {
            self.play.measure.stall(now, s.ms, s.part);
        }
        self.play.measure.sample(
            now,
            Sample {
                clock_error_ms: n.report_ms(),
                rtt_ms: clock.worst_rtt().map(|r| r * 1000.0),
                locked: n.locked,
                in_step: self.play.follower.in_step,
            },
        );
        self.play.measure.drawing(&self.ui.drawing);
        let (link, refresh) = self.net_test_facts();
        let late = Some(crate::render::late_frames_permille());
        if let (Some(p), Some(r)) = (
            primary,
            self.play.measure.report(now, link, refresh, late, false),
        ) {
            self.net.tell(
                Some(p),
                Msg::NetTest(Box::new(NetTest::Report(Box::new(r)))),
                false,
            );
        }
        // ---- How it copes itself (Test Mode's own measurements) ----
        let engine = &self.engine;
        if let Some(stats) = &mut self.play.local_stats
            && let Some(row) = stats.frame(Instant::now(), || crate::stats::Inputs {
                playing: engine.active.len(),
                started_total: engine.started_total(),
                layers: engine.picture_layers(),
                late_permille: crate::render::late_frames_permille(),
                audio_permille: engine
                    .audio
                    .device_status(None)
                    .map_or(0, |d| d.load_permille),
                audio_errors: crate::media::audio::stream_errors(),
                previews: crate::stats::Previews::now(),
            })
        {
            self.play.local.sample(now, &row);
        }
        self.play.local.errors(
            now,
            crate::media::audio::stream_errors(),
            crate::media::audio::stream_errors_late(),
        );
        let (outputs, device, ram) = self.local_facts();
        if let (Some(p), Some(r)) = (
            primary,
            self.play.local.report(now, outputs, device, ram, false),
        ) {
            self.net.tell(
                Some(p),
                Msg::NetTest(Box::new(NetTest::Local(Box::new(r)))),
                false,
            );
        }
    }

    /// The primary's test asks for (or no longer for) the network load: dial
    /// it, on a thread, once a run (again if the last connection ended).
    fn net_load(&mut self, from: NodeId, run: u64, ask: Option<crate::net::load::LoadAsk>) {
        let Some(ask) = ask else {
            self.play.load = None;
            return;
        };
        if self
            .play
            .load
            .as_ref()
            .is_some_and(|c| c.run() == run && !c.ended())
        {
            return;
        }
        let Some(addr) = self.ui.net.peer(from).and_then(|p| p.addr) else {
            log::info!("network load: the primary's address isn't known; none here");
            return;
        };
        self.play.load = Some(crate::net::load::LoadClient::start(
            std::net::SocketAddr::new(addr, ask.port),
            self.ui.net.me.id,
            run,
            &ask,
        ));
    }

    /// This computer's outputs' frame rates, its working sound device and
    /// its memory, for its own measurements.
    fn local_facts(
        &self,
    ) -> (
        Vec<crate::net::measure_local::OutputRate>,
        Option<String>,
        u64,
    ) {
        (
            crate::testmode::fleet::own_rates(&self.show, &self.ui),
            crate::testmode::hitches::working_device(&self.engine),
            self.play.ram_mb(),
        )
    }

    /// The sound back on, if a test muted it here.
    fn unmute_test(&mut self) {
        if std::mem::take(&mut self.play.test_muted) {
            crate::media::audio::set_muted(false);
        }
    }
}

impl Playing {
    /// This computer's memory in MB (looked at once).
    fn ram_mb(&self) -> u64 {
        static RAM: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        *RAM.get_or_init(|| {
            let mut sys = sysinfo::System::new();
            sys.refresh_memory();
            sys.total_memory() >> 20
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_start_up_card_shows_until_the_show_starts_and_never_again() {
        use crate::show::WaitCards;
        // Switched on, added, getting the show, in step: nothing has been
        // played yet, so the card shows on every screen (the projectors and
        // screens can be seen to be on and working).
        let mut started = false;
        for _ in 0..3 {
            started = show_started(started, false, false);
            assert!(!started);
            assert!(card_shows(WaitCards::UntilReady, true, started, true));
            assert!(card_shows(WaitCards::UntilReady, true, started, false));
        }
        // The primary plays its first cue: black from then on, whatever
        // happens (out of step, the clock lost, a new show, a takeover: the
        // follower is made afresh and has seen nothing).
        started = show_started(started, true, false);
        assert!(started);
        for _ in 0..5 {
            started = show_started(started, false, false);
            assert!(started);
            assert!(!card_shows(WaitCards::UntilReady, true, started, true));
            assert!(!card_shows(WaitCards::UntilReady, true, started, false));
        }
        // A cue this computer started itself counts as much.
        assert!(show_started(false, false, true));
        // "On screens the show doesn't use too": those keep it.
        assert!(card_shows(WaitCards::AlsoUnused, true, started, false));
        assert!(!card_shows(WaitCards::AlsoUnused, true, started, true));
        assert!(card_shows(WaitCards::AlsoUnused, true, false, true));
        // Never: black from the first moment once it is a show's computer...
        assert!(!card_shows(WaitCards::Never, true, false, true));
        assert!(!card_shows(WaitCards::Never, true, false, false));
        // ...but a computer no primary has added shows it, so it can be found.
        for w in WaitCards::ALL {
            assert!(card_shows(w, false, false, true));
            assert!(card_shows(w, false, true, true));
        }
    }

    #[test]
    fn the_follower_sees_the_show_start_from_a_command_or_a_heartbeat() {
        use crate::net::play::{Command, CueNow, PlayState, RunNow, RunWord, Timed};
        use crate::show::{CueId, TimelineKey};
        let show = crate::show::Show::default();
        let mut engine = crate::engine::Engine::for_tests();
        // Heartbeats of a primary that has played nothing: not started.
        let mut f = Follower::default();
        f.heard(PlayState::default());
        let stopped = RunNow {
            key: TimelineKey::List(1),
            state: RunWord::Stopped,
            pos: 0.0,
            anchor: None,
        };
        f.heard(PlayState {
            runs: vec![stopped.clone()],
            ..Default::default()
        });
        assert!(!f.saw_start);
        // A command that changes nothing playing: not started either.
        let timed = |seq, cmd| Timed {
            epoch: 1,
            seq,
            at: 0.0,
            cmd,
        };
        f.receive(
            timed(1, Command::SetPaused { paused: true }),
            &mut engine,
            &show,
        );
        f.receive(timed(2, Command::StopAll { fade: 0.0 }), &mut engine, &show);
        assert!(!f.saw_start);
        // A GO: started.
        f.receive(
            timed(
                3,
                Command::Go {
                    list: 0,
                    playhead: None,
                },
            ),
            &mut engine,
            &show,
        );
        assert!(f.saw_start);
        // A heartbeat that says so (0.24.5 on), or shows a cue or a run
        // going (an older primary): started.
        for s in [
            PlayState {
                started: true,
                ..Default::default()
            },
            PlayState {
                cues: vec![CueNow {
                    cue: CueId(4),
                    anchor: 0.0,
                    stopping: false,
                    sound: None,
                }],
                ..Default::default()
            },
            PlayState {
                runs: vec![RunNow {
                    state: RunWord::Paused,
                    ..stopped.clone()
                }],
                ..Default::default()
            },
        ] {
            let mut f = Follower::default();
            f.heard(s.clone());
            assert!(f.saw_start, "{s:?}");
        }
        // Said by a primary only once it has started a cue, and left out
        // of the message until then (an older NewQ reads it as before).
        let mut e = crate::engine::Engine::for_tests();
        let s = e.net_state(0.0);
        assert!(!s.started);
        assert!(!serde_json::to_string(&s).unwrap().contains("started"));
        let old: PlayState = serde_json::from_str(
            r#"{"epoch":1,"seq":0,"time":0.0,"show":0,"paused":false,"list":0,"playhead":null,"cues":[],"runs":[]}"#,
        )
        .unwrap();
        assert!(!old.started);
        let mut show = crate::show::Show::default();
        let c = show.make_cue(crate::show::CueType::Wait, None, 0);
        let id = c.id;
        show.cues.push(c);
        e.fire(&show, id);
        e.tick(&show, 0.01);
        let s = e.net_state(0.0);
        assert!(s.started);
        assert!(
            serde_json::to_string(&s)
                .unwrap()
                .contains("\"started\":true")
        );
        // Another show opened (or a test's show put back): not started until
        // a cue of it is.
        e.stop_all(0.0);
        e.show_opened();
        assert!(!e.net_state(0.0).started);
        e.fire(&show, id);
        e.tick(&show, 0.01);
        assert!(e.net_state(0.0).started);
    }

    #[test]
    fn identify_says_the_name_the_screen_and_the_address() {
        assert_eq!(
            identify_text("WNO-2", 1, Some("192.168.1.20")),
            "WNO-2\nScreen 2\n192.168.1.20"
        );
        assert_eq!(identify_text("Left", 0, None), "Left\nScreen 1");
        // The address the network would use: wired before Wi-Fi, a VPN last,
        // never link-local.
        use crate::net::paths::Adapter;
        let mask = std::net::Ipv4Addr::new(255, 255, 255, 0);
        let adapters = [
            Adapter::new("Tailscale", [100, 64, 0, 3].into(), mask),
            Adapter::new("Wi-Fi", [192, 168, 4, 7].into(), mask),
            Adapter::new("Ethernet 2", [169, 254, 3, 3].into(), mask),
            Adapter::new("Ethernet", [10, 0, 0, 5].into(), mask),
        ];
        assert_eq!(card_address_of(&adapters).as_deref(), Some("10.0.0.5"));
        assert_eq!(
            card_address_of(&adapters[..3]).as_deref(),
            Some("192.168.4.7")
        );
        assert_eq!(card_address_of(&[]), None);
    }

    #[test]
    fn a_display_card_says_who_and_where_and_how_it_stands() {
        let t = card_text(
            "Left",
            0,
            Some("10.0.0.2"),
            Some(("Booth", true)),
            Some(Standing::GettingReady),
            Some(0.2),
        );
        assert_eq!(
            t,
            "Left · Screen 1\n10.0.0.2\n\nRun from Booth\nGetting ready · clock ±0.2 ms"
        );
        let t = card_text(
            "Left",
            1,
            None,
            None,
            Some(Standing::WaitingForClock),
            Some(9.0),
        );
        assert_eq!(
            t,
            "Left · Screen 2\n\nWaiting for the primary\nWaiting for the primary's clock"
        );
        let t = card_text("Left", 0, None, Some(("Booth", false)), None, None);
        assert!(t.contains("Waiting for the primary (Booth)"), "{t}");
        // Started as a display and not added yet: any primary on the network.
        let t = card_text("Left", 0, Some("10.0.0.2"), None, None, None);
        assert_eq!(
            t,
            "Left · Screen 1\n10.0.0.2\n\nWaiting for a NewQ primary on this network"
        );
    }

    /// Every screen's card says which it is, numbered as the primary numbers
    /// that computer's monitors (from 1, in the order its report lists them),
    /// even when there is only one.
    #[test]
    fn each_screens_card_has_its_number() {
        assert_eq!(screen_name("Assistant", 0), "Assistant · Screen 1");
        assert_eq!(screen_name("Assistant", 2), "Assistant · Screen 3");
        let cards: Vec<String> = (0..3)
            .map(|m| card_text("Assistant", m, None, None, None, None))
            .collect();
        assert!(cards[0] != cards[1] && cards[1] != cards[2]);
        assert!(cards[1].starts_with("Assistant · Screen 2\n"));
    }

    #[test]
    fn a_display_computer_not_reached_for_a_while_says_so_calmly() {
        let t0 = Instant::now();
        // The first look without a session starts the count; a session ends it.
        let since = unreached_since(None, false, t0);
        assert_eq!(since, Some(t0));
        let later = t0 + Duration::from_secs(5);
        assert_eq!(unreached_since(since, false, later), Some(t0));
        assert_eq!(unreached_since(since, true, later), None);
        // Waiting for any primary, or for its own.
        let w = not_reached_words(None, false);
        assert!(w.starts_with("Not reached by a primary yet.\n"), "{w}");
        assert!(w.contains("the firewall on this computer may be blocking NewQ"));
        assert!(!w.contains("Windows"));
        let w = not_reached_words(Some("Booth"), true);
        assert!(w.starts_with("Not reached by the primary (Booth) for a while."));
        assert!(w.contains("Windows Firewall"), "{w}");
        assert!(w.contains("If the primary lists this computer but can't add it"));
    }

    /// Started again as the primary asked: display mode chosen from the
    /// banner comes back as display mode.
    #[test]
    fn a_restart_keeps_display_mode() {
        let plain: Vec<std::ffi::OsString> = vec!["show.newq".into()];
        assert!(restart_adds_display(true, &plain));
        assert!(!restart_adds_display(false, &plain));
        let shown: Vec<std::ffi::OsString> = vec!["--display".into()];
        assert!(!restart_adds_display(true, &shown));
    }

    #[test]
    fn started_as_a_display_it_is_one_before_any_primary_is_heard() {
        use crate::net::node::{self, Claim, NetworkId};
        let waiting = node::switch_on(&Identity::fresh());
        // Waiting: a display only when started as one (`--join` has controls).
        assert!(display_computer(&waiting, true));
        assert!(!display_computer(&waiting, false));
        // No control window, and a window on every monitor, before adoption.
        assert_eq!(display_windows(true, false, 3), [0, 1, 2]);
        assert_eq!(display_windows(true, true, 3), [] as [usize; 0]);
        assert_eq!(display_windows(false, false, 3), [] as [usize; 0]);
        // Adopted: a display, started either way (the role the primary gives).
        let claim = Claim {
            from: NodeId(1),
            from_name: "Booth".into(),
            network: NetworkId(10),
            term: 1,
            proof: None,
        };
        let adopted = node::adopt(&waiting, &claim, None, Role::Display).unwrap();
        assert!(display_computer(&adopted, true));
        assert!(!paired_display(&adopted, true));
        // ...but only display mode when started as one (NewQ Display, start at
        // login): opened as NewQ, its control window shows, and says so.
        assert!(!display_computer(&adopted, false));
        assert!(paired_display(&adopted, false));
        // Made the backup: its controls show, however it was started.
        let backup = node::set_role(&adopted, Role::Backup).unwrap();
        assert!(!display_computer(&backup, true));
        assert!(!paired_display(&backup, false));
        // Released by the primary: started as a display, it waits as one again.
        let released = node::release(&adopted);
        assert!(display_computer(&released, true));
        assert!(!display_computer(&released, false));
        // A primary never is.
        assert!(!display_computer(&node::become_primary(&waiting), true));
    }

    #[test]
    fn the_open_show_vouches_for_a_waiting_computer_it_names_and_never_steals_one() {
        use crate::net::hub::PeerView;
        use crate::net::node::{Identity, NetworkId};
        use crate::show::{AudioOutputConfig, OutputConfig, Show};
        let peer = |id: u64, network: Option<u64>| PeerView {
            id: NodeId(id),
            name: format!("C{id}"),
            role: Role::Alone,
            version: String::new(),
            compatible: true,
            network: network.map(NetworkId),
            ours: network == Some(1),
            waiting: network.is_none(),
            online: true,
            connected: true,
            addr: None,
            status: None,
            problem: None,
            adopting: false,
            twin: None,
        };
        let mut v = NetView {
            me: Identity {
                role: Role::Primary,
                on: true,
                network: Some(NetworkId(1)),
                ..Default::default()
            },
            ..Default::default()
        };
        // 2: waiting, named by a display. 3: waiting, named by a sound output.
        // 4: waiting, not named. 5: named, but another (absent) primary's.
        // 6: named and already ours.
        v.peers = vec![
            peer(2, None),
            peer(3, None),
            peer(4, None),
            peer(5, Some(7)),
            peer(6, Some(1)),
        ];
        let mut show = Show::default();
        for id in [2, 5, 6] {
            let mut o = OutputConfig::new(crate::show::OutputId(id), "D".into());
            o.computer = Some(NodeId(id));
            show.outputs.push(o);
        }
        let a: AudioOutputConfig = serde_json::from_str(
            r#"{"id": 9, "name": "Sound", "computers": ["0000000000000003"]}"#,
        )
        .unwrap();
        show.audio_outputs = vec![a];
        let none = HashSet::new();
        assert_eq!(vouched_for(&show, &v, &none), [NodeId(2), NodeId(3)]);
        // Each once a run: one released by hand stays released.
        let asked: HashSet<NodeId> = [NodeId(2)].into();
        assert_eq!(vouched_for(&show, &v, &asked), [NodeId(3)]);
        // Refused (a passcode of its own) or being added: left alone.
        v.peers[1].problem = Some("needs a passcode".into());
        v.peers[0].adopting = true;
        assert!(vouched_for(&show, &v, &none).is_empty());
        v.peers[1].problem = None;
        v.peers[0].adopting = false;
        // Another NewQ version: left alone.
        v.peers[0].compatible = false;
        assert_eq!(vouched_for(&show, &v, &none), [NodeId(3)]);
        // Only a primary adds anyone.
        v.me.role = Role::Display;
        assert!(vouched_for(&show, &v, &none).is_empty());
    }

    #[test]
    fn ctrl_or_command_q_quits_a_display_computer() {
        let cmd = egui::Modifiers::COMMAND;
        assert!(quits(egui::Key::Q, cmd));
        assert!(!quits(egui::Key::Q, egui::Modifiers::NONE));
        assert!(!quits(egui::Key::W, cmd));
        assert!(!quits(egui::Key::Q, cmd | egui::Modifiers::SHIFT));
        assert!(quit_words().contains("Q quits"));
    }

    #[test]
    fn a_primary_alone_is_not_in_a_show_network() {
        use crate::net::hub::PeerView;
        use crate::net::node::{Identity, NetworkId};
        let mut v = NetView {
            me: Identity {
                role: Role::Primary,
                on: true,
                network: Some(NetworkId(1)),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(mode_of(&v), NetMode::Off, "no computer adopted: alone");
        v.peers.push(PeerView {
            id: NodeId(2),
            name: "Left".into(),
            role: Role::Display,
            version: String::new(),
            compatible: true,
            network: Some(NetworkId(1)),
            ours: true,
            waiting: false,
            online: false,
            connected: false,
            addr: None,
            status: None,
            problem: None,
            adopting: false,
            twin: None,
        });
        assert_eq!(mode_of(&v), NetMode::Primary, "one adopted, even offline");
        v.me.role = Role::Display;
        v.me.primary = Some(NodeId(9));
        assert_eq!(mode_of(&v), NetMode::Follower);
        v.me.on = false;
        assert_eq!(mode_of(&v), NetMode::Off);
    }
}
