//! The show network's own thread: it owns this computer's identity, the beacon,
//! the listener and a session per peer, acts on what the peers say and what
//! the page asks, and publishes a `NetView` the UI reads without waiting.
//!
//! A computer that isn't switched on opens no socket at all (D1). Switched on,
//! it listens and announces itself. The primary connects to the others (D4):
//! to every computer of its network it hears, to every computer waiting to be
//! adopted, and to the addresses added by hand, again with a growing pause
//! while one can't be reached. Each side sends a `Hello` at once, then its
//! passcode proof; a computer then takes orders from a primary as
//! `node::judge` says, and says `Welcome`, or refuses with a sentence.
//!
//! For later stages: `Hub::send_to` and `Hub::broadcast` send a message to a
//! peer or to every computer of the network; messages the hub doesn't handle
//! itself come out of `Hub::take_events` as `NetEvent::Message`; a role change
//! of this computer comes out as `NetEvent::RoleChanged` and shows in
//! `NetView::me`.

use super::discover::{self, Beaconer, Heard};
use super::node::{self, Claim, Identity, Key, NodeId, Refusal, Role};
use super::proto::{self, Beacon, FrameError, Hello, Link, MonitorReport, Msg, StatusReport};
use super::session::{ConnId, Session, SessionEvent};
use crossbeam_channel::{Receiver, Sender};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// A peer not heard from (no beacon and no session) for this long is offline.
pub const OFFLINE_AFTER: Duration = Duration::from_secs(4);
/// A session with nothing from the other side for this long is dead.
pub const SILENT_FOR: Duration = Duration::from_secs(6);
/// A computer of another network, or waiting, not heard for this long is dropped from the list.
pub const FORGET_AFTER: Duration = Duration::from_secs(60);
/// Keep-alives and status reports.
pub const EVERY: Duration = Duration::from_secs(1);
/// The longest pause between tries at reaching a computer.
pub const MAX_BACKOFF: Duration = Duration::from_secs(10);
/// How long "Identify" shows the name.
pub const IDENTIFY_SECONDS: f32 = 8.0;

/// Where the hub keeps its file and which ports it uses.
#[derive(Clone, Debug)]
pub struct HubConfig {
    /// The folder of `node.json`.
    pub dir: PathBuf,
    /// The show network's first port, and how many it has.
    pub port_base: u16,
    pub port_span: u16,
    /// Listen and announce on every adapter. Off (tests), only this computer.
    pub lan: bool,
}

impl HubConfig {
    /// The folder from `NEWQ_NODE_DIR` (else the log folder) and the ports from
    /// `NEWQ_NET_PORT` (else `discover::DEFAULT_PORT`).
    pub fn from_env() -> HubConfig {
        HubConfig {
            dir: node::node_dir(),
            port_base: std::env::var("NEWQ_NET_PORT")
                .ok()
                .and_then(|p| p.trim().parse().ok())
                .unwrap_or(discover::DEFAULT_PORT),
            port_span: discover::PORTS,
            lan: true,
        }
    }
}

/// What the page (or the command line) asks of the show network.
#[derive(Clone, Debug, PartialEq)]
pub enum NetAction {
    /// The switch "This computer is part of a show network".
    SetOn(bool),
    /// "Run the show network from this computer".
    BecomePrimary,
    /// Primary: take a waiting computer into the network.
    Adopt(NodeId),
    /// Primary: send a computer back to waiting.
    Release(NodeId),
    SetRole(NodeId, Role),
    /// Rename a computer (this one or, on the primary, another); empty = its own name.
    Rename(NodeId, String),
    /// Show a computer's name big on it.
    Identify(NodeId),
    AddAddress(String),
    RemoveAddress(String),
    /// Primary: the network's passcode (None: none). Waiting: accept only a
    /// primary that has it.
    SetPasscode(Option<String>),
    /// Primary: start NewQ again on a computer (a role change that needs it).
    Restart(NodeId),
    /// Primary: a computer starts NewQ at login, or not.
    StartAtLogin(NodeId, bool),
    // ---- Take control (stage 5) ----
    /// The backup takes control: it becomes the primary (D7).
    TakeControl,
    /// Primary: forget a computer of the network that is gone for good.
    Forget(NodeId),
}

/// What this computer has that the others are told about (set by the app).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LocalFacts {
    pub monitors: Vec<MonitorReport>,
    pub audio_devices: Vec<String>,
}

/// How this computer is playing with the others, told to the hub by the app
/// (stage 4a) for its status report.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SyncFacts {
    pub clock_error_ms: Option<f64>,
    pub clock_locked: bool,
    pub rtt_ms: Option<f64>,
    pub show_version: Option<u64>,
    pub show_hash: Option<u64>,
    pub in_step: bool,
    pub standing: Option<String>,
    pub start_at_login: bool,
}

/// What the hub tells the app.
#[derive(Clone, Debug, PartialEq)]
pub enum NetEvent {
    /// This computer's role or the primary it follows changed.
    RoleChanged { role: Role, primary: Option<NodeId> },
    /// Show this computer's name big for this long.
    Identify { seconds: f32 },
    /// A message the hub doesn't handle itself, from a computer of the network.
    #[allow(dead_code)] // what later stages (clock, commands, show sync) receive
    Message { from: NodeId, msg: Msg },
}

/// A computer as the page shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct PeerView {
    pub id: NodeId,
    pub name: String,
    pub role: Role,
    pub version: String,
    /// It speaks this NewQ's protocol.
    pub compatible: bool,
    /// The network it belongs to, if any.
    pub network: Option<node::NetworkId>,
    /// In this computer's network.
    pub ours: bool,
    /// Switched on and not in any network: can be adopted.
    pub waiting: bool,
    pub online: bool,
    /// A session is up (it can be given orders).
    pub connected: bool,
    pub addr: Option<IpAddr>,
    pub status: Option<StatusReport>,
    /// A sentence for the page: why it can't be reached or refused.
    pub problem: Option<String>,
    /// An adoption asked for and not yet answered.
    pub adopting: bool,
}

/// An address added by hand, and what became of it.
#[derive(Clone, Debug, PartialEq)]
pub struct ManualView {
    pub text: String,
    pub note: Option<String>,
}

/// The primary this computer follows.
#[derive(Clone, Debug, PartialEq)]
pub struct Following {
    pub id: NodeId,
    pub name: String,
    pub online: bool,
    /// Seconds since it was last heard from, once that is `QUIET_AFTER` or
    /// more (stage 5: the backup's cue to take control).
    pub quiet_secs: Option<u64>,
}

/// The primary is said to have gone quiet after this long without a word.
pub const QUIET_AFTER: Duration = Duration::from_secs(2);

/// Everything the UI shows of the show network, published by the hub.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NetView {
    pub me: Identity,
    pub name: String,
    /// The port it listens on, while switched on.
    pub port: Option<u16>,
    /// Why it couldn't listen, or another trouble of this computer's.
    pub problem: Option<String>,
    pub peers: Vec<PeerView>,
    pub manual: Vec<ManualView>,
    pub following: Option<Following>,
    /// This computer's own report (its row on the primary's page).
    pub status: StatusReport,
    /// Its name is to be shown big until then.
    pub identify_until: Option<Instant>,
    /// This computer took control (stage 5) from the primary of this name.
    pub took_control_from: Option<String>,
}

impl NetView {
    pub fn is_primary(&self) -> bool {
        self.me.role == Role::Primary && self.me.on
    }

    /// The computers of this network that have reported (for choosing a
    /// display's monitor or an audio output's computer), by name.
    pub fn computers(&self) -> Vec<&PeerView> {
        let mut v: Vec<&PeerView> = self
            .peers
            .iter()
            .filter(|p| p.ours && p.status.is_some())
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn peer(&self, id: NodeId) -> Option<&PeerView> {
        self.peers.iter().find(|p| p.id == id)
    }

    /// What to call computer `id`: this one, a peer, or its id.
    pub fn name_of(&self, id: NodeId) -> String {
        if id == self.me.id {
            return self.name.clone();
        }
        self.peer(id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| format!("computer {}", id.short()))
    }
}

/// Everything that reaches the hub's thread.
pub enum Event {
    Heard(Heard),
    Session(SessionEvent),
    Accepted(TcpStream),
    Dialled(Target, Result<TcpStream, String>),
    Do(NetAction),
    Local(LocalFacts),
    /// This computer's media report (the media manager's; sent with the
    /// next status, not at once: it changes every second while copying).
    Media(Option<Box<crate::net::media::MediaReport>>),
    Send(NodeId, Msg),
    Broadcast(Msg),
    /// To the computers of this network (or one), ahead of other frames when
    /// `urgent` (stage 4a).
    Tell {
        to: Option<NodeId>,
        msg: Msg,
        urgent: bool,
    },
    Sync(SyncFacts),
    Quit,
}

/// What a connection was made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Peer(NodeId),
    Manual(SocketAddr),
}

/// The handle the app holds: start, ask, read.
pub struct Hub {
    tx: Sender<Event>,
    /// A follower's shared clock (stage 4a): kept by the clock thread the hub
    /// starts while this computer follows a primary.
    clock: super::time::ClockHandle,
    primary_clock: super::time::PrimaryClock,
    view: Arc<Mutex<Arc<NetView>>>,
    events: Receiver<NetEvent>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Hub {
    /// Read this computer's identity and start the hub's thread. Nothing is
    /// opened on the network unless the identity is switched on. `wake` is
    /// called when the view changes or there are events (to rouse the UI).
    pub fn start(cfg: HubConfig, wake: impl Fn() + Send + Sync + 'static) -> Hub {
        let me = node::load(&cfg.dir);
        let (tx, rx) = crossbeam_channel::unbounded();
        let (ev_tx, ev_rx) = crossbeam_channel::unbounded();
        let mut state = State::new(cfg, me, tx.clone(), ev_tx);
        state.wake = Arc::new(wake);
        let clock = state.clock.clone();
        let primary_clock = state.primary_clock.clone();
        state.recall_known(Instant::now());
        let view = Arc::new(Mutex::new(Arc::new(state.view())));
        let published = view.clone();
        let thread = std::thread::Builder::new()
            .name("net-hub".into())
            .spawn(move || {
                let _said = crate::crash::OnPanic(|| {
                    log::error!("show network: its thread stopped after a fault");
                });
                state.run(rx, &published);
            })
            .ok();
        Hub {
            tx,
            clock,
            primary_clock,
            view,
            events: ev_rx,
            thread,
        }
    }

    /// What the hub last published (never waits on the network).
    pub fn view(&self) -> Arc<NetView> {
        self.view.lock().clone()
    }

    pub fn ask(&self, a: NetAction) {
        let _ = self.tx.send(Event::Do(a));
    }

    pub fn set_local(&self, facts: LocalFacts) {
        let _ = self.tx.send(Event::Local(facts));
    }

    /// This computer's media report, sent with its status (`net::media`).
    pub fn set_media(&self, report: Option<crate::net::media::MediaReport>) {
        let _ = self.tx.send(Event::Media(report.map(Box::new)));
    }

    /// Send `msg` to computer `to` if a session with it is up (else dropped).
    #[allow(dead_code)] // for later stages
    pub fn send_to(&self, to: NodeId, msg: Msg) {
        let _ = self.tx.send(Event::Send(to, msg));
    }

    /// Send `msg` to every computer of the network with a session up.
    #[allow(dead_code)] // for later stages
    pub fn broadcast(&self, msg: Msg) {
        let _ = self.tx.send(Event::Broadcast(msg));
    }

    // ---- Playing together (stage 4a) ----

    /// Send `msg` to computer `to` of this network, or (None) to all of them,
    /// ahead of other frames when `urgent`. Dropped where no session is up.
    pub fn tell(&self, to: Option<NodeId>, msg: Msg, urgent: bool) {
        let _ = self.tx.send(Event::Tell { to, msg, urgent });
    }

    /// How this computer plays with the others, for its report.
    pub fn set_sync(&self, facts: SyncFacts) {
        let _ = self.tx.send(Event::Sync(facts));
    }

    /// A follower's shared clock.
    pub fn clock(&self) -> super::time::ClockHandle {
        self.clock.clone()
    }

    /// The shared time when this computer is the primary: its own clock, or
    /// the one it carried on when it took control (stage 5).
    pub fn primary_now(&self) -> f64 {
        self.primary_clock.now()
    }

    /// What happened since the last call.
    pub fn take_events(&self) -> Vec<NetEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        let _ = self.tx.send(Event::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A computer the hub knows of.
struct Peer {
    id: NodeId,
    name: String,
    role: Role,
    version: String,
    proto: u32,
    network: Option<node::NetworkId>,
    term: u64,
    on: bool,
    tcp_port: u16,
    addr: Option<IpAddr>,
    heard: Option<Instant>,
    /// The session that is up with it.
    conn: Option<ConnId>,
    status: Option<StatusReport>,
    problem: Option<String>,
    next_dial: Instant,
    backoff: Duration,
    dialling: bool,
    /// Primary: orders waiting for the session (an adoption).
    pending: Vec<Msg>,
    /// Remembered in `node.json` (stage 5): dialled at its last address even
    /// when its beacon isn't heard.
    known: bool,
}

impl Peer {
    fn new(id: NodeId) -> Peer {
        Peer {
            id,
            name: format!("computer {}", id.short()),
            role: Role::Alone,
            version: String::new(),
            proto: proto::PROTOCOL,
            network: None,
            term: 0,
            on: false,
            tcp_port: 0,
            addr: None,
            heard: None,
            conn: None,
            status: None,
            problem: None,
            next_dial: Instant::now(),
            backoff: Duration::from_millis(500),
            dialling: false,
            pending: Vec::new(),
            known: false,
        }
    }

    fn heard_lately(&self, now: Instant) -> bool {
        self.heard
            .is_some_and(|t| now.saturating_duration_since(t) < OFFLINE_AFTER)
    }

    fn failed(&mut self, now: Instant) {
        self.dialling = false;
        self.next_dial = now + self.backoff;
        self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
    }
}

struct Conn {
    session: Session,
    target: Option<Target>,
    peer: Option<NodeId>,
    hello: Option<Hello>,
    /// What our proof request was made over.
    nonce: u64,
    /// Their proof checked against our key: None when we have no key; unset
    /// until it arrives.
    proof: Option<Option<bool>>,
    /// They take orders from us (or will be adopted by us).
    welcomed: bool,
    /// We take orders from them.
    obeying: bool,
    closing: bool,
    last_rx: Instant,
    last_ping: Instant,
    ping: u64,
    last_status: Option<Instant>,
}

/// The sockets that exist only while switched on.
struct Net {
    /// Kept for as long as it is to announce (it stops when dropped).
    _beacon: Beaconer,
    beacon_content: Arc<Mutex<Beacon>>,
    stop_listening: Arc<AtomicBool>,
    port: u16,
}

impl Drop for Net {
    fn drop(&mut self) {
        self.stop_listening.store(true, Ordering::Relaxed);
    }
}

struct Manual {
    text: String,
    addr: Option<SocketAddr>,
    note: Option<String>,
    /// A connection is being tried or is up.
    busy: bool,
    next: Instant,
    backoff: Duration,
}

struct State {
    cfg: HubConfig,
    me: Identity,
    local: LocalFacts,
    tx: Sender<Event>,
    out: Sender<NetEvent>,
    net: Option<Net>,
    problem: Option<String>,
    next_try_listen: Instant,
    peers: HashMap<NodeId, Peer>,
    conns: HashMap<ConnId, Conn>,
    next_conn: ConnId,
    manual: Vec<Manual>,
    manual_addrs: Arc<Mutex<Vec<SocketAddr>>>,
    identify_until: Option<Instant>,
    disk_free: Option<u64>,
    disk_looked: Option<Instant>,
    /// The media manager's report, sent in the status.
    media: Option<Box<crate::net::media::MediaReport>>,
    quit: bool,
    wake: Arc<dyn Fn() + Send + Sync>,
    // ---- Playing together (stage 4a) ----
    clock: super::time::ClockHandle,
    keeper: Option<(NodeId, super::time::Keeper)>,
    sync: SyncFacts,
    // ---- Take control (stage 5) ----
    primary_clock: super::time::PrimaryClock,
    started: Instant,
    /// The network the follower's clock was kept in.
    clock_network: Option<node::NetworkId>,
    /// The primary this computer took control from.
    took_from: Option<String>,
}

impl State {
    fn new(cfg: HubConfig, me: Identity, tx: Sender<Event>, out: Sender<NetEvent>) -> State {
        State {
            cfg,
            me,
            local: LocalFacts::default(),
            tx,
            out,
            net: None,
            problem: None,
            next_try_listen: Instant::now(),
            peers: HashMap::new(),
            conns: HashMap::new(),
            next_conn: 1,
            manual: Vec::new(),
            manual_addrs: Arc::new(Mutex::new(Vec::new())),
            identify_until: None,
            disk_free: None,
            disk_looked: None,
            media: None,
            quit: false,
            wake: Arc::new(|| {}),
            clock: Default::default(),
            keeper: None,
            sync: SyncFacts::default(),
            primary_clock: Default::default(),
            started: Instant::now(),
            clock_network: None,
            took_from: None,
        }
    }

    fn run(&mut self, rx: Receiver<Event>, view: &Mutex<Arc<NetView>>) {
        let mut last_tick: Option<Instant> = None;
        while !self.quit {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(ev) => self.handle(ev),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
            // Whatever else is waiting, before the view is published.
            for ev in rx.try_iter().take(256) {
                self.handle(ev);
                if self.quit {
                    break;
                }
            }
            if last_tick.is_none_or(|t| t.elapsed() >= Duration::from_millis(100)) {
                last_tick = Some(Instant::now());
                self.tick(Instant::now());
            }
            // Published (and the UI roused) only when something changed.
            let now = self.view();
            if now != **view.lock() {
                *view.lock() = Arc::new(now);
                (self.wake)();
            }
        }
        self.close_everything();
    }

    fn name(&self) -> String {
        self.me.display_name()
    }

    /// Keep the identity, on disk and for the beacon; tell the app when the
    /// role or the primary changed.
    fn set_me(&mut self, next: Identity) {
        if next == self.me {
            return;
        }
        let told = (self.me.role, self.me.primary) != (next.role, next.primary);
        if let Err(e) = node::save(&self.cfg.dir, &next) {
            log::warn!("show network: could not keep this computer's role: {e}");
        }
        // Only the computers it remembers changed: nothing to say.
        let only_known = node::Identity {
            known: self.me.known.clone(),
            ..next.clone()
        } == self.me;
        log::log!(
            if only_known {
                log::Level::Debug
            } else {
                log::Level::Info
            },
            "show network: this computer is {} ({}{})",
            next.role.described(),
            if next.on { "on" } else { "off" },
            next.primary_name
                .as_ref()
                .filter(|_| next.adopted())
                .map(|p| format!(", follows {p}"))
                .unwrap_or_default()
        );
        self.me = next;
        if told {
            let _ = self.out.send(NetEvent::RoleChanged {
                role: self.me.role,
                primary: self.me.primary.filter(|_| self.me.adopted()),
            });
        }
        self.refresh_beacon();
    }

    fn beacon(&self) -> Beacon {
        Beacon {
            proto: proto::PROTOCOL,
            version: proto::VERSION.into(),
            id: self.me.id,
            name: self.name(),
            role: self.me.role,
            network: self.me.network,
            tcp_port: self.net.as_ref().map_or(0, |n| n.port),
            show_version: self.sync.show_version,
            term: self.me.term,
            on: self.me.on,
        }
    }

    fn refresh_beacon(&self) {
        if let Some(n) = &self.net {
            *n.beacon_content.lock() = self.beacon();
        }
    }

    fn handle(&mut self, ev: Event) {
        let now = Instant::now();
        match ev {
            Event::Quit => self.quit = true,
            Event::Heard(h) => self.heard(h, now),
            Event::Accepted(stream) => {
                if self.net.is_some() {
                    self.add_conn(stream, None, now);
                }
            }
            Event::Dialled(target, result) => self.dialled(target, result, now),
            Event::Session(SessionEvent::Hello(c, h)) => self.hello(c, h, now),
            Event::Session(SessionEvent::Msg(c, m)) => self.message(c, m, now),
            Event::Session(SessionEvent::Closed(c, why)) => self.closed(c, why, now),
            Event::Do(a) => self.act(a, now),
            Event::Local(f) => {
                if f != self.local {
                    self.local = f;
                    // Reported at once.
                    for c in self.conns.values_mut() {
                        c.last_status = None;
                    }
                }
            }
            Event::Media(m) => self.media = m,
            Event::Send(to, msg) => {
                if let Some(c) = self.peers.get(&to).and_then(|p| p.conn) {
                    self.send(c, &msg);
                }
            }
            Event::Tell { to, msg, urgent } => self.tell(to, &msg, urgent),
            Event::Sync(f) => self.sync = f,
            Event::Broadcast(msg) => {
                let frame = Arc::new(proto::encode(&msg));
                let to: Vec<ConnId> = self
                    .conns
                    .iter()
                    .filter(|(_, c)| (c.welcomed || c.obeying) && !c.closing)
                    .map(|(id, _)| *id)
                    .collect();
                for c in to {
                    if let Some(conn) = self.conns.get(&c)
                        && !conn.session.send_bytes(frame.clone())
                    {
                        self.drop_slow(c);
                    }
                }
            }
        }
    }

    // ---- the network, opened and closed with the switch ----

    fn open(&mut self, now: Instant) {
        if self.net.is_some() || now < self.next_try_listen {
            return;
        }
        let ip = if self.cfg.lan {
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        } else {
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        };
        let mut bound = None;
        for p in discover::ports(self.cfg.port_base, self.cfg.port_span) {
            let Ok(udp) = UdpSocket::bind(SocketAddr::new(ip, p)) else {
                continue;
            };
            let Ok(tcp) = TcpListener::bind(SocketAddr::new(ip, p)) else {
                continue;
            };
            bound = Some((udp, tcp, p));
            break;
        }
        let Some((udp, tcp, port)) = bound else {
            self.problem = Some(format!(
                "No port free between {} and {}: is another program using them?",
                self.cfg.port_base,
                self.cfg.port_base.saturating_add(self.cfg.port_span - 1)
            ));
            self.next_try_listen = now + Duration::from_secs(5);
            return;
        };
        let content = Arc::new(Mutex::new(Beacon::default()));
        let beacon = match Beaconer::start(
            udp,
            self.cfg.port_base,
            self.cfg.port_span,
            self.cfg.lan,
            content.clone(),
            self.manual_addrs.clone(),
            self.tx.clone(),
            self.primary_clock.clone(),
        ) {
            Ok(b) => b,
            Err(e) => {
                self.problem = Some(format!("Couldn't announce this computer: {e}"));
                self.next_try_listen = now + Duration::from_secs(5);
                return;
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        if let Err(e) = listen(tcp, stop.clone(), self.tx.clone()) {
            self.problem = Some(format!("Couldn't listen for the other computers: {e}"));
            self.next_try_listen = now + Duration::from_secs(5);
            return;
        }
        log::info!("show network: listening on port {port}");
        self.problem = None;
        self.net = Some(Net {
            _beacon: beacon,
            beacon_content: content,
            stop_listening: stop,
            port,
        });
        self.refresh_beacon();
    }

    fn close_everything(&mut self) {
        if self.net.take().is_some() {
            log::info!("show network: stopped");
        }
        for (_, c) in self.conns.drain() {
            c.session.close();
        }
        for p in self.peers.values_mut() {
            p.conn = None;
            p.dialling = false;
        }
        self.peers
            .retain(|_, p| p.network.is_some() && p.network == self.me.network);
        for m in &mut self.manual {
            m.busy = false;
        }
    }

    // ---- what is heard and said ----

    fn heard(&mut self, h: Heard, now: Instant) {
        let b = h.beacon;
        if b.id == self.me.id || self.net.is_none() {
            return;
        }
        let p = self.peers.entry(b.id).or_insert_with(|| Peer::new(b.id));
        // Heard again after a silence (a computer started again): dialled at
        // once rather than after the pause its silence grew (stage 5).
        if !p.heard_lately(now) && p.conn.is_none() && !p.dialling {
            p.next_dial = now;
            p.backoff = Duration::from_millis(500);
        }
        p.name = b.name;
        p.role = b.role;
        p.version = b.version;
        p.proto = b.proto;
        p.network = b.network;
        p.term = b.term;
        p.on = b.on;
        p.tcp_port = b.tcp_port;
        p.addr = Some(h.from.ip());
        p.heard = Some(now);
        if b.proto != proto::PROTOCOL {
            p.problem = Some(proto::mismatch_sentence(
                &p.name,
                &p.version,
                proto::VERSION,
            ));
        } else if p
            .problem
            .as_deref()
            .is_some_and(|s| s.ends_with("update one of them."))
        {
            p.problem = None;
        }
    }

    fn add_conn(&mut self, stream: TcpStream, target: Option<Target>, now: Instant) {
        let id = self.next_conn;
        self.next_conn += 1;
        let session = match Session::start(id, stream, self.tx.clone()) {
            Ok(s) => s,
            Err(e) => {
                log::debug!("show network: a connection failed to start: {e}");
                return;
            }
        };
        let nonce = node::random_u64();
        let conn = Conn {
            session,
            target,
            peer: match target {
                Some(Target::Peer(p)) => Some(p),
                _ => None,
            },
            hello: None,
            nonce,
            proof: None,
            welcomed: false,
            obeying: false,
            closing: false,
            last_rx: now,
            last_ping: now,
            ping: 0,
            last_status: None,
        };
        let hello = Msg::Hello(Hello {
            proto: proto::PROTOCOL,
            version: proto::VERSION.into(),
            id: self.me.id,
            name: self.name(),
            role: self.me.role,
            network: self.me.network,
            term: self.me.term,
            nonce,
        });
        conn.session.send(&hello);
        self.conns.insert(id, conn);
    }

    fn dialled(&mut self, target: Target, result: Result<TcpStream, String>, now: Instant) {
        match result {
            Ok(stream) if self.net.is_some() => self.add_conn(stream, Some(target), now),
            Ok(_) => {}
            Err(e) => match target {
                Target::Peer(id) => {
                    if let Some(p) = self.peers.get_mut(&id) {
                        p.failed(now);
                        log::debug!("show network: {} can't be reached: {e}", p.name);
                    }
                }
                Target::Manual(a) => {
                    if let Some(m) = self.manual.iter_mut().find(|m| m.addr == Some(a)) {
                        m.busy = false;
                        m.note = Some(format!("Nothing answers at {}.", m.text));
                        m.next = now + m.backoff;
                        m.backoff = (m.backoff * 2).min(MAX_BACKOFF);
                    }
                }
            },
        }
    }

    fn send(&mut self, c: ConnId, msg: &Msg) {
        if let Some(conn) = self.conns.get(&c)
            && !conn.session.send(msg)
        {
            self.drop_slow(c);
        }
    }

    /// A peer too far behind (its queue full) or gone: its session ends.
    fn drop_slow(&mut self, c: ConnId) {
        if let Some(conn) = self.conns.get_mut(&c)
            && !conn.closing
        {
            conn.closing = true;
            conn.session.close();
        }
    }

    fn refuse(&mut self, c: ConnId, why: Refusal) {
        let text = why.sentence(&self.name());
        self.send(c, &Msg::Refused { why, text });
        if let Some(conn) = self.conns.get_mut(&c) {
            conn.closing = true;
            conn.session.close_after_queue();
        }
    }

    fn hello(&mut self, c: ConnId, h: Hello, now: Instant) {
        let Some(conn) = self.conns.get_mut(&c) else {
            return;
        };
        conn.last_rx = now;
        if h.id == self.me.id {
            // An address added by hand that is this computer.
            if let Some(Target::Manual(a)) = conn.target
                && let Some(m) = self.manual.iter_mut().find(|m| m.addr == Some(a))
            {
                m.note = Some("That is this computer.".into());
            }
            conn.closing = true;
            conn.session.close();
            return;
        }
        // One session per computer: a second one (found twice) is let go.
        if let Some(p) = self.peers.get(&h.id)
            && let Some(other) = p.conn
            && other != c
            && self.conns.get(&other).is_some_and(|o| !o.closing)
        {
            if let Some(conn) = self.conns.get_mut(&c) {
                conn.closing = true;
                conn.session.close();
            }
            return;
        }
        let Some(conn) = self.conns.get_mut(&c) else {
            return;
        };
        conn.peer = Some(h.id);
        let manual = match conn.target {
            Some(Target::Manual(a)) => Some(a),
            _ => None,
        };
        let ip = conn.session.peer.ip();
        let their_port = conn.session.peer.port();
        let mac = self.me.passcode.map(|k| k.proof(h.nonce, self.me.id));
        let p = self.peers.entry(h.id).or_insert_with(|| Peer::new(h.id));
        p.name = h.name.clone();
        p.role = h.role;
        p.version = h.version.clone();
        p.proto = h.proto;
        p.network = h.network;
        p.term = h.term;
        p.conn = Some(c);
        p.dialling = false;
        // It answered on its port: it is switched on.
        p.on = true;
        if p.addr.is_none() {
            p.addr = Some(ip);
        }
        // Dialled by this computer: the port it answered on is the one it
        // listens on (kept for a restart, stage 5; a beacon may never come).
        if manual.is_some() && p.tcp_port == 0 {
            p.addr = Some(ip);
            p.tcp_port = their_port;
        }
        if let Some(a) = manual
            && let Some(m) = self.manual.iter_mut().find(|m| m.addr == Some(a))
        {
            m.note = Some(format!("Found {}.", h.name));
        }
        if let Some(conn) = self.conns.get_mut(&c) {
            conn.hello = Some(h);
        }
        self.send(c, &Msg::Prove { mac });
    }

    /// The claim a primary's session makes.
    fn claim_of(&self, conn: &Conn) -> Option<Claim> {
        let h = conn.hello.as_ref()?;
        if h.role != Role::Primary {
            return None;
        }
        Some(Claim {
            from: h.id,
            from_name: h.name.clone(),
            network: h.network?,
            term: h.term,
            proof: conn.proof.flatten().map(Some).unwrap_or_else(|| {
                // We have a key and they sent no proof.
                self.me.passcode.map(|_| false)
            }),
        })
    }

    fn proved(&mut self, c: ConnId, mac: Option<String>) {
        let Some(conn) = self.conns.get_mut(&c) else {
            return;
        };
        let Some(h) = &conn.hello else {
            return;
        };
        let checked = self.me.passcode.map(|k| {
            mac.as_deref()
                .is_some_and(|m| k.checks(m, conn.nonce, h.id))
        });
        conn.proof = Some(checked);
        let (role, network) = (h.role, h.network);
        let conn = &self.conns[&c];
        if let Some(claim) = self.claim_of(conn) {
            match node::judge(&self.me, &claim) {
                Ok(()) => {
                    if self.me.network.is_some() {
                        let was_primary = self.me.role == Role::Primary;
                        let next = node::obey(&self.me, &claim);
                        if was_primary {
                            // A newer primary (a backup that took control):
                            // this one steps down to be the backup (stage 5),
                            // and lets go of the computers it ran.
                            log::info!(
                                "show network: {} took control (turn {}): this computer is \
                                 the backup now",
                                claim.from_name,
                                claim.term
                            );
                            for (id, conn) in self.conns.iter_mut() {
                                if *id != c && !conn.closing {
                                    conn.closing = true;
                                    conn.session.close();
                                }
                            }
                        }
                        self.set_me(next);
                        if let Some(conn) = self.conns.get_mut(&c) {
                            conn.obeying = true;
                            conn.last_status = None;
                        }
                        self.leave_other_primaries(c);
                    }
                    self.send(c, &Msg::Welcome);
                }
                Err(why) => self.refuse(c, why),
            }
            return;
        }
        // A computer of this network that should know the passcode and doesn't.
        if self.me.role == Role::Primary
            && network.is_some()
            && network == self.me.network
            && role != Role::Primary
            && checked == Some(false)
        {
            self.refuse(c, Refusal::Passcode);
        }
    }

    /// Now that this computer follows the primary on `keep`, the sessions with
    /// any other primary end.
    fn leave_other_primaries(&mut self, keep: ConnId) {
        let others: Vec<ConnId> = self
            .conns
            .iter()
            .filter(|(id, c)| {
                **id != keep
                    && !c.closing
                    && c.hello.as_ref().is_some_and(|h| {
                        h.role == Role::Primary
                            && (h.network != self.me.network || h.term < self.me.term)
                    })
            })
            .map(|(id, _)| *id)
            .collect();
        for c in others {
            let why = match self.conns[&c].hello.as_ref() {
                Some(h) if h.network == self.me.network => Refusal::OldTerm { seen: self.me.term },
                _ => Refusal::OtherNetwork,
            };
            self.refuse(c, why);
        }
        for c in self.conns.values_mut() {
            if c.session.id != keep {
                c.obeying = false;
            }
        }
    }

    fn message(&mut self, c: ConnId, m: Msg, now: Instant) {
        let Some(conn) = self.conns.get_mut(&c) else {
            return;
        };
        conn.last_rx = now;
        if conn.closing {
            return;
        }
        let obeying = conn.obeying;
        let from = conn.peer;
        let from_primary = conn.hello.as_ref().is_some_and(|h| h.role == Role::Primary);
        match m {
            Msg::Hello(_) | Msg::Unknown => {}
            Msg::Prove { mac } => self.proved(c, mac),
            Msg::Ping { n } => self.send(c, &Msg::Pong { n }),
            Msg::Pong { .. } => {}
            Msg::Welcome => {
                conn.welcomed = true;
                if let Some(p) = from.and_then(|id| self.peers.get_mut(&id)) {
                    p.backoff = Duration::from_millis(500);
                    p.problem = None;
                    let pending = std::mem::take(&mut p.pending);
                    for msg in pending {
                        self.send(c, &msg);
                    }
                }
            }
            Msg::Refused { text, .. } => {
                conn.closing = true;
                if let Some(p) = from.and_then(|id| self.peers.get_mut(&id)) {
                    p.problem = Some(text);
                    p.pending.clear();
                    p.next_dial = now + MAX_BACKOFF;
                }
            }
            Msg::Status(s) => {
                if let Some(p) = from.and_then(|id| self.peers.get_mut(&id)) {
                    p.name = s.name.clone();
                    p.role = s.role;
                    p.network = s.network;
                    p.term = s.term;
                    p.status = Some(*s);
                }
            }
            Msg::Adopt {
                network,
                term,
                key,
                role,
                primary_name,
            } => {
                if !from_primary {
                    return;
                }
                if obeying && self.me.network == Some(network) {
                    // The passcode changed (or was given again).
                    let mut next = self.me.clone();
                    next.passcode = key;
                    next.primary_name = Some(primary_name);
                    next.term = next.term.max(term);
                    self.set_me(next);
                } else {
                    let Some(mut claim) = self.claim_of(&self.conns[&c]) else {
                        return;
                    };
                    claim.network = network;
                    claim.term = term;
                    claim.from_name = primary_name;
                    match node::adopt(&self.me, &claim, key, role) {
                        Ok(next) => {
                            log::info!("show network: adopted by {}", claim.from_name);
                            self.set_me(next);
                            if let Some(conn) = self.conns.get_mut(&c) {
                                conn.obeying = true;
                            }
                            self.leave_other_primaries(c);
                        }
                        Err(why) => {
                            self.refuse(c, why);
                            return;
                        }
                    }
                }
                self.report(c, now);
            }
            Msg::Release => {
                if obeying {
                    let next = node::release(&self.me);
                    self.set_me(next);
                    if let Some(conn) = self.conns.get_mut(&c) {
                        conn.obeying = false;
                    }
                    self.report(c, now);
                }
            }
            Msg::SetRole { role } => {
                if obeying && let Some(next) = node::set_role(&self.me, role) {
                    self.set_me(next);
                    self.report(c, now);
                }
            }
            Msg::Rename { name } => {
                if obeying {
                    let next = node::rename(&self.me, &name);
                    self.set_me(next);
                    self.report(c, now);
                }
            }
            Msg::Identify { seconds } => {
                if obeying {
                    self.identify(seconds, now);
                }
            }
            #[allow(unreachable_patterns)] // none yet: later stages add them
            other => {
                // A message of a later stage: for the app, from a computer of
                // the network.
                if (obeying || conn.welcomed)
                    && let Some(from) = from
                {
                    let _ = self.out.send(NetEvent::Message { from, msg: other });
                    // Commands are timed: the app is roused at once.
                    (self.wake)();
                }
            }
        }
    }

    fn identify(&mut self, seconds: f32, now: Instant) {
        let seconds = seconds.clamp(1.0, 60.0);
        self.identify_until = Some(now + Duration::from_secs_f32(seconds));
        log::info!("show network: asked to show this computer's name");
        let _ = self.out.send(NetEvent::Identify { seconds });
    }

    fn closed(&mut self, c: ConnId, why: FrameError, now: Instant) {
        let Some(conn) = self.conns.remove(&c) else {
            return;
        };
        let peer = conn.peer;
        if let FrameError::Mismatch { name, version, .. } = &why {
            let text = proto::mismatch_sentence(name, version, proto::VERSION);
            match conn.target {
                Some(Target::Manual(a)) => {
                    if let Some(m) = self.manual.iter_mut().find(|m| m.addr == Some(a)) {
                        m.note = Some(text.clone());
                        // Not tried again until it is added again.
                        m.next = now + Duration::from_secs(3600);
                    }
                }
                _ => {
                    if let Some(p) = peer.and_then(|id| self.peers.get_mut(&id)) {
                        p.problem = Some(text.clone());
                    }
                }
            }
            log::info!("show network: {text}");
        } else if conn.obeying {
            log::info!("show network: the connection to the primary ended ({why})");
        }
        if let Some(Target::Manual(a)) = conn.target
            && let Some(m) = self.manual.iter_mut().find(|m| m.addr == Some(a))
        {
            m.busy = false;
            if !matches!(why, FrameError::Mismatch { .. }) {
                m.next = now + m.backoff;
                m.backoff = (m.backoff * 2).min(MAX_BACKOFF);
            }
        }
        if let Some(p) = peer.and_then(|id| self.peers.get_mut(&id))
            && p.conn == Some(c)
        {
            p.conn = None;
            p.failed(now);
        }
        if let Some(Target::Peer(id)) = conn.target
            && let Some(p) = self.peers.get_mut(&id)
            && p.dialling
        {
            // Connected, but it never said hello.
            p.failed(now);
        }
    }

    // ---- what the page asks ----

    fn act(&mut self, a: NetAction, now: Instant) {
        log::info!("show network: {a:?}");
        match a {
            NetAction::SetOn(true) => {
                let next = node::switch_on(&self.me);
                self.set_me(next);
            }
            NetAction::SetOn(false) => {
                let next = node::switch_off(&self.me);
                self.set_me(next);
                self.close_everything();
            }
            NetAction::BecomePrimary => {
                if self.me.adopted() {
                    return;
                }
                let next = node::become_primary(&self.me);
                self.set_me(next);
                // Sessions made while waiting were the other way round.
                for c in self.conns.values() {
                    c.session.close();
                }
            }
            NetAction::Adopt(id) => {
                if self.me.role != Role::Primary {
                    return;
                }
                let Some(network) = self.me.network else {
                    return;
                };
                let msg = Msg::Adopt {
                    network,
                    term: self.me.term,
                    key: self.me.passcode,
                    role: Role::Display,
                    primary_name: self.name(),
                };
                let Some(p) = self.peers.get_mut(&id) else {
                    return;
                };
                p.problem = None;
                let up = p
                    .conn
                    .filter(|c| self.conns.get(c).is_some_and(|c| c.welcomed));
                match up {
                    Some(c) => self.send(c, &msg),
                    None => {
                        p.pending = vec![msg];
                        p.next_dial = now;
                        p.backoff = Duration::from_millis(500);
                    }
                }
            }
            NetAction::Release(id) => self.order(id, Msg::Release),
            NetAction::Restart(id) => self.order(id, Msg::Restart),
            NetAction::StartAtLogin(id, on) => self.order(id, Msg::StartAtLogin { on }),
            NetAction::TakeControl => self.take_control(now),
            NetAction::Forget(id) => {
                if self.me.role != Role::Primary {
                    return;
                }
                let up = self
                    .peers
                    .get(&id)
                    .and_then(|p| p.conn)
                    .is_some_and(|c| self.conns.get(&c).is_some_and(|c| !c.closing));
                if up {
                    // Still here: release it rather.
                    return;
                }
                self.peers.remove(&id);
                let next = node::forget(&self.me, id);
                self.set_me(next);
            }
            NetAction::SetRole(id, role) => {
                if Role::GIVEN.contains(&role) {
                    self.order(id, Msg::SetRole { role });
                }
            }
            NetAction::Rename(id, name) => {
                if id == self.me.id {
                    let next = node::rename(&self.me, &name);
                    self.set_me(next);
                } else {
                    self.order(id, Msg::Rename { name });
                }
            }
            NetAction::Identify(id) => {
                if id == self.me.id {
                    self.identify(IDENTIFY_SECONDS, now);
                } else {
                    self.order(
                        id,
                        Msg::Identify {
                            seconds: IDENTIFY_SECONDS,
                        },
                    );
                }
            }
            NetAction::AddAddress(text) => {
                let text = text.trim().to_string();
                if self.manual.iter().any(|m| m.text == text) {
                    return;
                }
                let (addr, note) = match discover::parse_address(&text) {
                    Ok(a) => (Some(a), Some("Looking…".to_string())),
                    Err(e) => (None, Some(e)),
                };
                self.manual.push(Manual {
                    text,
                    addr,
                    note,
                    busy: false,
                    next: now,
                    backoff: Duration::from_millis(500),
                });
                self.sync_manual();
            }
            NetAction::RemoveAddress(text) => {
                self.manual.retain(|m| m.text != text);
                self.sync_manual();
            }
            NetAction::SetPasscode(pass) => {
                let key = pass
                    .as_deref()
                    .map(str::trim)
                    .filter(|p| !p.is_empty())
                    .map(Key::from_passcode);
                if self.me.adopted() {
                    return;
                }
                let mut next = self.me.clone();
                next.passcode = key;
                self.set_me(next);
                // The computers of the network are given the new key over
                // the sessions they already trust.
                if self.me.role == Role::Primary
                    && let Some(network) = self.me.network
                {
                    let ours: Vec<(ConnId, Role)> = self
                        .peers
                        .values()
                        .filter(|p| p.network == Some(network))
                        .filter_map(|p| Some((p.conn?, p.role)))
                        .collect();
                    for (c, role) in ours {
                        let msg = Msg::Adopt {
                            network,
                            term: self.me.term,
                            key,
                            role,
                            primary_name: self.name(),
                        };
                        self.send(c, &msg);
                    }
                }
            }
        }
    }

    /// An order from the primary to a computer of its network, if a session
    /// with it is up.
    fn order(&mut self, id: NodeId, msg: Msg) {
        if self.me.role != Role::Primary {
            return;
        }
        let up = self
            .peers
            .get(&id)
            .filter(|p| p.network.is_some() && p.network == self.me.network)
            .and_then(|p| p.conn)
            .filter(|c| self.conns.get(c).is_some_and(|c| c.welcomed));
        if let Some(c) = up {
            self.send(c, &msg);
        }
    }

    fn sync_manual(&mut self) {
        *self.manual_addrs.lock() = self.manual.iter().filter_map(|m| m.addr).collect();
    }

    // ---- every 100 ms ----

    fn tick(&mut self, now: Instant) {
        if self.me.on {
            self.open(now);
        } else if self.net.is_some() {
            self.close_everything();
        }
        if self.identify_until.is_some_and(|t| now >= t) {
            self.identify_until = None;
        }
        self.look_at_disk(now);
        if self.net.is_none() {
            return;
        }
        // Keep-alives, and the silent let go.
        let ids: Vec<ConnId> = self.conns.keys().copied().collect();
        for c in ids {
            let Some(conn) = self.conns.get_mut(&c) else {
                continue;
            };
            if conn.closing || conn.session.is_closed() {
                continue;
            }
            if now.saturating_duration_since(conn.last_rx) > SILENT_FOR {
                log::info!(
                    "show network: nothing from {} for {} s: the connection is closed",
                    conn.hello
                        .as_ref()
                        .map_or("a computer", |h| h.name.as_str()),
                    SILENT_FOR.as_secs()
                );
                conn.closing = true;
                conn.session.close();
                continue;
            }
            if conn.hello.is_some() && now.saturating_duration_since(conn.last_ping) >= EVERY {
                conn.last_ping = now;
                conn.ping += 1;
                let n = conn.ping;
                self.send(c, &Msg::Ping { n });
            }
            // A computer that follows this primary reports to it.
            if let Some(conn) = self.conns.get(&c)
                && conn.obeying
                && conn
                    .last_status
                    .is_none_or(|t| now.saturating_duration_since(t) >= EVERY)
            {
                self.report(c, now);
            }
        }
        if self.me.role == Role::Primary {
            self.dial(now);
            self.keep_known();
        }
        self.keep_clock();
        // Computers of other networks, or waiting, long gone are forgotten.
        let mine = self.me.network;
        self.peers.retain(|_, p| {
            p.conn.is_some()
                || (p.network.is_some() && p.network == mine)
                || p.heard
                    .is_some_and(|t| now.saturating_duration_since(t) < FORGET_AFTER)
        });
    }

    /// Primary: connect to the computers of its network and those waiting,
    /// and to the addresses added by hand.
    fn dial(&mut self, now: Instant) {
        let mine = self.me.network;
        let mut wanted = Vec::new();
        for p in self.peers.values_mut() {
            let want = p.proto == proto::PROTOCOL
                && p.conn.is_none()
                && !p.dialling
                && now >= p.next_dial
                && (p.heard_lately(now) || p.known)
                && p.tcp_port != 0
                && (p.network == mine || (p.network.is_none() && p.on));
            if let (true, Some(ip)) = (want, p.addr) {
                p.dialling = true;
                wanted.push((Target::Peer(p.id), vec![SocketAddr::new(ip, p.tcp_port)]));
            }
        }
        // Addresses added by hand that no session has reached yet.
        let reached: Vec<IpAddr> = self
            .conns
            .values()
            .filter(|c| c.hello.is_some())
            .map(|c| c.session.peer.ip())
            .collect();
        for m in &mut self.manual {
            let Some(a) = m.addr else { continue };
            if m.busy || now < m.next {
                continue;
            }
            let found = reached
                .iter()
                .any(|ip| *ip == a.ip() || (ip.is_loopback() && a.ip().is_loopback()))
                && a.port() == 0;
            if found {
                continue;
            }
            m.busy = true;
            let tries = if a.port() == 0 {
                discover::ports(self.cfg.port_base, self.cfg.port_span)
                    .map(|p| SocketAddr::new(a.ip(), p))
                    .collect()
            } else {
                vec![a]
            };
            wanted.push((Target::Manual(a), tries));
        }
        for (target, addrs) in wanted {
            let tx = self.tx.clone();
            let me_port = self.net.as_ref().map(|n| n.port);
            let _ = std::thread::Builder::new()
                .name("net-dial".into())
                .spawn(move || {
                    let mut last = String::from("no address");
                    for a in addrs {
                        // Not this NewQ's own listener.
                        if a.ip().is_loopback() && Some(a.port()) == me_port {
                            continue;
                        }
                        match TcpStream::connect_timeout(&a, Duration::from_secs(3)) {
                            Ok(s) => {
                                let _ = tx.send(Event::Dialled(target, Ok(s)));
                                return;
                            }
                            Err(e) => last = e.to_string(),
                        }
                    }
                    let _ = tx.send(Event::Dialled(target, Err(last)));
                });
        }
    }

    /// Send this computer's report on session `c`.
    fn report(&mut self, c: ConnId, now: Instant) {
        let Some(conn) = self.conns.get_mut(&c) else {
            return;
        };
        conn.last_status = Some(now);
        let link = conn
            .session
            .local
            .map(|a| a.ip())
            .filter(|ip| !ip.is_loopback())
            .and_then(discover::adapter_with)
            .map_or(Link::Unknown, |n| Link::from_adapter_name(&n));
        let mut s = self.own_status();
        s.link = link;
        self.send(c, &Msg::Status(Box::new(s)));
    }

    fn own_status(&self) -> StatusReport {
        StatusReport {
            name: self.name(),
            role: self.me.role,
            version: proto::VERSION.into(),
            network: self.me.network,
            primary: self.me.primary,
            term: self.me.term,
            monitors: self.local.monitors.clone(),
            audio_devices: self.local.audio_devices.clone(),
            disk_free: self.disk_free,
            link: Link::Unknown,
            clock_error_ms: self.sync.clock_error_ms,
            show_version: self.sync.show_version,
            media_progress: self.media.clone(),
            clock_locked: self.sync.clock_locked,
            rtt_ms: self.sync.rtt_ms,
            show_hash: self.sync.show_hash,
            in_step: self.sync.in_step,
            standing: self.sync.standing.clone(),
            start_at_login: self.sync.start_at_login,
        }
    }

    // ---- Playing together (stage 4a) ----

    /// Send to the computers of this network that have a session up (`to`:
    /// one of them).
    fn tell(&mut self, to: Option<NodeId>, msg: &Msg, urgent: bool) {
        let mine = self.me.network;
        let conns: Vec<ConnId> = self
            .peers
            .values()
            .filter(|p| to.is_none_or(|t| t == p.id))
            .filter(|p| p.network.is_some() && p.network == mine)
            .filter_map(|p| p.conn)
            .filter(|c| {
                self.conns
                    .get(c)
                    .is_some_and(|c| (c.welcomed || c.obeying) && !c.closing)
            })
            .collect();
        if conns.is_empty() {
            return;
        }
        let frame = Arc::new(proto::encode(msg));
        for c in conns {
            let ok = self.conns.get(&c).is_some_and(|conn| {
                if urgent {
                    conn.session.send_urgent(frame.clone())
                } else {
                    conn.session.send_bytes(frame.clone())
                }
            });
            if !ok {
                self.drop_slow(c);
            }
        }
    }

    /// A follower keeps a shared clock with its primary over UDP (`time.rs`),
    /// asking at the primary's port where its beacon was heard; anyone else
    /// keeps none. A new primary starts the clock afresh; the same one found
    /// at another address keeps it.
    fn keep_clock(&mut self) {
        let primary = self
            .me
            .primary
            .filter(|_| self.me.adopted() && self.me.role != Role::Primary);
        let target = primary.and_then(|id| {
            let p = self.peers.get(&id)?;
            let ip = p.addr?;
            (p.tcp_port != 0).then(|| SocketAddr::new(ip, p.tcp_port))
        });
        match (primary, target) {
            (Some(id), Some(to)) => match &self.keeper {
                Some((was, k)) if *was == id => {
                    if k.target() != Some(to) {
                        k.aim(to);
                    }
                }
                _ => {
                    // Another primary of another network starts the clock
                    // afresh; one of the same network (a backup that took
                    // control, carrying the shared clock on) keeps it, so
                    // what plays here doesn't jump (stage 5).
                    if self.keeper.as_ref().is_some_and(|(was, _)| *was != id)
                        && self.clock_network != self.me.network
                    {
                        self.clock.reset();
                    }
                    self.clock_network = self.me.network;
                    match super::time::Keeper::start(self.clock.clone(), to) {
                        Ok(k) => {
                            log::info!("show network: keeping the shared clock with {to}");
                            self.keeper = Some((id, k));
                        }
                        Err(e) => log::warn!("show network: the clock couldn't start: {e}"),
                    }
                }
            },
            // Following, the primary not heard yet: the clock runs on as it was.
            (Some(_), None) => {}
            (None, _) => {
                if self.keeper.take().is_some() {
                    self.clock.reset();
                }
            }
        }
    }

    // ---- Take control (stage 5) ----

    /// The backup takes control (D7): the shared clock it kept as a follower
    /// carries on as the primary's, the term goes up, and every session is
    /// made again the other way round (the primary dials). The computers of
    /// the network obey it as soon as they hear it; the old primary, if it is
    /// there, steps down to be the backup when it does.
    fn take_control(&mut self, now: Instant) {
        let seen = self
            .peers
            .values()
            .filter(|p| p.network.is_some() && p.network == self.me.network)
            .map(|p| p.term)
            .max()
            .unwrap_or(0);
        let Some(next) = node::take_control(&self.me, seen) else {
            log::info!("show network: only the backup can take control");
            return;
        };
        // The time the followers have, carried on (before the keeper stops).
        let carried = self.clock.continued();
        if carried.is_none() {
            log::warn!(
                "show network: taking control without a shared clock: the other computers' \
                 pictures may jump"
            );
        }
        self.primary_clock.carry_on(carried);
        self.took_from = Some(
            self.me
                .primary
                .and_then(|p| self.peers.get(&p))
                .map(|p| p.name.clone())
                .or_else(|| self.me.primary_name.clone())
                .unwrap_or_else(|| "the primary".into()),
        );
        log::info!(
            "show network: this computer takes control (turn {}){}",
            next.term,
            self.me
                .primary_name
                .as_ref()
                .map(|p| format!(" from {p}"))
                .unwrap_or_default()
        );
        self.set_me(next);
        for c in self.conns.values_mut() {
            c.closing = true;
            c.obeying = false;
            c.session.close();
        }
        for p in self.peers.values_mut() {
            p.dialling = false;
            p.next_dial = now;
            p.backoff = Duration::from_millis(500);
            p.problem = None;
        }
        self.keep_clock();
    }

    /// A primary starting: the computers it remembers, to be dialled at their
    /// last address before their beacons are heard (or where none is).
    fn recall_known(&mut self, now: Instant) {
        if self.me.role != Role::Primary {
            return;
        }
        let network = self.me.network;
        for k in self.me.known.clone() {
            if k.id == self.me.id {
                continue;
            }
            let p = self.peers.entry(k.id).or_insert_with(|| Peer::new(k.id));
            p.known = true;
            p.name = k.name.clone();
            p.role = k.role;
            p.network = network;
            if let Some(a) = k.socket() {
                p.addr = Some(a.ip());
                p.tcp_port = a.port();
            }
            p.next_dial = now;
        }
    }

    /// A primary keeps the computers of its network in `node.json`.
    fn keep_known(&mut self) {
        let mine = self.me.network;
        let now: Vec<node::Known> = self
            .peers
            .values()
            .filter(|p| p.network.is_some() && p.network == mine && p.id != self.me.id)
            .map(|p| node::Known {
                id: p.id,
                name: p.name.clone(),
                role: p.role,
                // The address it was reached at, when this computer dialled
                // it (a beacon can be heard on several adapters in turn).
                addr: p
                    .conn
                    .and_then(|c| self.conns.get(&c))
                    .filter(|c| c.target.is_some() && c.hello.is_some())
                    .map(|c| c.session.peer.ip())
                    .or(p.addr)
                    .map(|a| a.to_string()),
                port: p.tcp_port,
            })
            .collect();
        let next = node::remember(&self.me.known, &now);
        if next != self.me.known {
            for p in self.peers.values_mut() {
                if next.iter().any(|k| k.id == p.id) {
                    p.known = true;
                }
            }
            let mut me = self.me.clone();
            me.known = next;
            self.set_me(me);
        }
    }

    /// How long the primary `id` has been quiet, once it is `QUIET_AFTER`.
    fn quiet_for(&self, id: NodeId, now: Instant) -> Option<u64> {
        let rx = self
            .conns
            .values()
            .filter(|c| c.peer == Some(id) && c.obeying)
            .map(|c| c.last_rx)
            .max();
        let heard = self.peers.get(&id).and_then(|p| p.heard);
        let last = rx.max(heard).unwrap_or(self.started);
        let quiet = now.saturating_duration_since(last);
        (quiet >= QUIET_AFTER).then_some(quiet.as_secs())
    }

    /// Free space on the log folder's disk, now and then (it can take a moment).
    fn look_at_disk(&mut self, now: Instant) {
        if self
            .disk_looked
            .is_some_and(|t| now.saturating_duration_since(t) < Duration::from_secs(15))
        {
            return;
        }
        self.disk_looked = Some(now);
        self.disk_free = free_space(&self.cfg.dir);
    }

    fn view(&self) -> NetView {
        let now = Instant::now();
        let mine = self.me.network;
        let mut peers: Vec<PeerView> = self
            .peers
            .values()
            .map(|p| {
                let conn = p.conn.and_then(|c| self.conns.get(&c));
                let connected = conn.is_some_and(|c| !c.closing && (c.welcomed || c.obeying));
                PeerView {
                    id: p.id,
                    name: p.name.clone(),
                    role: p.role,
                    version: p.version.clone(),
                    compatible: p.proto == proto::PROTOCOL,
                    network: p.network,
                    ours: p.network.is_some() && p.network == mine,
                    waiting: p.network.is_none() && p.on,
                    online: conn.is_some_and(|c| !c.closing) || p.heard_lately(now),
                    connected,
                    addr: p.addr,
                    status: p.status.clone(),
                    problem: p.problem.clone(),
                    adopting: !p.pending.is_empty(),
                }
            })
            .collect();
        peers.sort_by(|a, b| {
            (!a.ours, !a.waiting, a.name.to_lowercase(), a.id).cmp(&(
                !b.ours,
                !b.waiting,
                b.name.to_lowercase(),
                b.id,
            ))
        });
        let following = self
            .me
            .primary
            .filter(|_| self.me.adopted())
            .map(|id| Following {
                id,
                name: self
                    .peers
                    .get(&id)
                    .map(|p| p.name.clone())
                    .or_else(|| self.me.primary_name.clone())
                    .unwrap_or_else(|| "the primary".into()),
                online: self
                    .conns
                    .values()
                    .any(|c| c.obeying && !c.closing && c.peer == Some(id)),
                quiet_secs: self.quiet_for(id, now),
            });
        NetView {
            me: self.me.clone(),
            name: self.name(),
            port: self.net.as_ref().map(|n| n.port),
            problem: self.problem.clone(),
            peers,
            manual: self
                .manual
                .iter()
                .map(|m| ManualView {
                    text: m.text.clone(),
                    note: m.note.clone(),
                })
                .collect(),
            following,
            status: self.own_status(),
            identify_until: self.identify_until,
            took_control_from: self
                .took_from
                .clone()
                .filter(|_| self.me.role == Role::Primary),
        }
    }
}

/// The listener's thread: accepted connections go to the hub until `stop`.
fn listen(tcp: TcpListener, stop: Arc<AtomicBool>, tx: Sender<Event>) -> std::io::Result<()> {
    tcp.set_nonblocking(true)?;
    std::thread::Builder::new()
        .name("net-listen".into())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match tcp.accept() {
                    Ok((s, _)) => {
                        let _ = s.set_nonblocking(false);
                        if tx.send(Event::Accepted(s)).is_err() {
                            break;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            }
        })?;
    Ok(())
}

/// Bytes free on the disk that holds `dir` (the one whose mount point is the
/// longest that contains it).
fn free_space(dir: &std::path::Path) -> Option<u64> {
    let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .filter(|d| dir.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.available_space())
}

#[cfg(test)]
#[path = "hub_tests.rs"]
mod tests;
