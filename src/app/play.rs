//! The app's side of computers playing together (stage 4a; `net::play`): which
//! way the engine is ticked, the commands and heartbeats moved through the hub,
//! the show sent to the followers and taken from the primary, and what this
//! computer tells the others of itself. Called from `App::pass_net` (messages)
//! and `App::pass_show` (the tick). Nothing here waits on the network.

use super::App;
use crate::engine::NetMode;
use crate::net::hub::{NetView, SyncFacts};
use crate::net::node::{NodeId, Role};
use crate::net::play::{
    Follower, Leader, Out, ShowAssembly, Standing, lead_for, show_hash, show_parts,
};
use crate::net::proto::{Link, Msg};
use crate::net::time::local_now;
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
    /// The card drawn on each monitor (its words and texture).
    pub cards: HashMap<usize, (Card, std::sync::Arc<crate::gpu::LayerTexture>)>,
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
        // Commands still to be done here are done now, as they would have
        // been alone.
        if let Some(mut l) = self.play.leader.take() {
            l.step(&mut self.engine, &self.show, f64::INFINITY);
        }
        // Inputs caught and not yet timed are done at once.
        let left = self.engine.net_take();
        self.engine.net_set_mode(NetMode::Off);
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
                // Versions go on rising across restarts of the primary.
                self.play.version = (local_now() * 1000.0) as u64;
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
        let now = local_now();
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
                let (epoch, seq) = self.play.follower.receive(*t, &mut self.engine, &self.show);
                self.net.tell(Some(from), Msg::PlayAck { epoch, seq }, true);
            }
            Msg::PlayState(s) if primary => self.play.follower.heard(*s),
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
            _ => {}
        }
    }

    /// A follower takes the primary's show in place of its own.
    fn take_show(&mut self, hash: u64, json: String) {
        if self.play.have == Some(hash) {
            return;
        }
        let show: crate::show::Show = match serde_json::from_str(&json) {
            Ok(s) => s,
            Err(e) => {
                self.engine.problem(format!(
                    "Show network: the primary's show couldn't be read: {e}"
                ));
                return;
            }
        };
        self.show = show;
        self.ui.took_synced_show(&self.show, &mut self.engine);
        self.ui.check_media(&self.show);
        self.play.have = Some(hash);
        log::info!(
            "show network: took the primary's show ({} bytes)",
            json.len()
        );
        // Kept for the next start (on a thread: a large show is a moment's write).
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
        let Ok(show) = serde_json::from_str::<crate::show::Show>(&json) else {
            return false;
        };
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
                    clock_error_ms: n.map(|n| (n.error * 1000.0 * 100.0).round() / 100.0),
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
                .map(|at| at - local_now()),
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
    /// or the page ticked.
    pub(super) fn set_start_at_login(&mut self, on: bool) {
        match crate::autostart::set(on) {
            Ok(()) => log::info!(
                "start at login {}",
                if on { "switched on" } else { "switched off" }
            ),
            Err(e) => self
                .engine
                .problem(format!("Start at login couldn't be changed: {e}")),
        }
        self.ui.start_at_login = crate::autostart::is_set();
    }

    /// A role this computer was given that the primary might want it to have
    /// at login: a display computer starts at login unless told otherwise.
    pub(super) fn play_role_changed(&mut self, role: Role) {
        if role == Role::Display && !crate::autostart::is_set() {
            self.set_start_at_login(true);
        }
    }
}

// ---- Display computers (D8) ----

/// What a display computer's card says (`card_text`), and whether it is the
/// Identify flash.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Card {
    pub text: String,
    pub big: bool,
}

/// The card's words: the computer's name, its address, who runs the show and
/// where it stands. Pure, for the tests.
pub(super) fn card_text(
    name: &str,
    address: Option<&str>,
    primary: Option<(&str, bool)>,
    standing: Option<Standing>,
    clock_ms: Option<f64>,
) -> String {
    let mut lines = vec![name.to_string()];
    if let Some(a) = address {
        lines.push(a.to_string());
    }
    lines.push(String::new());
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

impl App {
    /// A display computer: no control window; every monitor is an output.
    pub(super) fn is_display_computer(&self) -> bool {
        let me = &self.ui.net.me;
        me.on && me.adopted() && me.role == Role::Display
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

    /// The card on one monitor of this computer, if any: the name big while
    /// the primary asks it to Identify (on every output, whatever the role);
    /// on a display computer, the quiet card until it is in step, and always on
    /// a monitor the show gives no display.
    pub(super) fn card(&mut self, has_display: bool) -> Option<Card> {
        let v = self.ui.net.clone();
        if let Some(until) = v.identify_until
            && Instant::now() < until
        {
            // Flashing: on half a second, dimmer the next.
            let on = (local_now() * 2.0) as i64 % 2 == 0;
            return Some(Card {
                text: if on {
                    v.name.clone()
                } else {
                    format!("{} ", v.name)
                },
                big: true,
            });
        }
        if !self.is_display_computer() {
            return None;
        }
        let clock = self.net.clock().now();
        let standing = self.play.standing(clock.is_some_and(|n| n.locked));
        if has_display && standing == Some(Standing::InStep) {
            return None;
        }
        let address = self.own_address();
        let primary = v.following.as_ref().map(|f| (f.name.as_str(), f.online));
        let mut text = card_text(
            &v.name,
            address.as_deref(),
            primary,
            standing,
            clock.map(|n| (n.error * 1000.0 * 10.0).round() / 10.0),
        );
        // The media manager's progress while files are still coming.
        if let Some(own) = &self.ui.media.own {
            let used = &own.progress.used;
            if used.bytes_wanted > 0 && used.bytes_here < used.bytes_wanted {
                text.push_str(&format!("\nMedia {:.0} % here", used.percent()));
            }
        }
        Some(Card { text, big: false })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_display_card_says_who_and_where_and_how_it_stands() {
        let t = card_text(
            "Left",
            Some("10.0.0.2"),
            Some(("Booth", true)),
            Some(Standing::GettingReady),
            Some(0.2),
        );
        assert_eq!(
            t,
            "Left\n10.0.0.2\n\nRun from Booth\nGetting ready · clock ±0.2 ms"
        );
        let t = card_text(
            "Left",
            None,
            None,
            Some(Standing::WaitingForClock),
            Some(9.0),
        );
        assert_eq!(
            t,
            "Left\n\nWaiting for the primary\nWaiting for the primary's clock"
        );
        let t = card_text("Left", None, Some(("Booth", false)), None, None);
        assert!(t.contains("Waiting for the primary (Booth)"), "{t}");
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
        });
        assert_eq!(mode_of(&v), NetMode::Primary, "one adopted, even offline");
        v.me.role = Role::Display;
        v.me.primary = Some(NodeId(9));
        assert_eq!(mode_of(&v), NetMode::Follower);
        v.me.on = false;
        assert_eq!(mode_of(&v), NetMode::Off);
    }
}
