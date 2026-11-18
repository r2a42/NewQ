//! Hubs on this computer only (loopback), each with its own folder and the
//! same small range of free ports, as several NewQs on one computer would be.
//! Every wait is on a condition with a generous limit: nothing assumes a fast
//! machine.

use super::*;

/// A range of ports nothing else uses: `span` free ports in a row, found by
/// binding them.
fn free_range(span: u16) -> u16 {
    for _ in 0..200 {
        let base = 20_000 + (node::random_u64() % 40_000) as u16;
        let ok = discover::ports(base, span).all(|p| {
            UdpSocket::bind(("127.0.0.1", p)).is_ok() && TcpListener::bind(("127.0.0.1", p)).is_ok()
        });
        if ok && discover::ports(base, span).count() == span as usize {
            return base;
        }
    }
    panic!("no free ports");
}

struct Rig {
    base: u16,
    dirs: Vec<PathBuf>,
}

impl Rig {
    fn new() -> Rig {
        Rig {
            base: free_range(4),
            dirs: Vec::new(),
        }
    }

    fn cfg(&mut self, name: &str) -> HubConfig {
        let dir = std::env::temp_dir().join(format!(
            "newq-hub-{name}-{}-{}",
            std::process::id(),
            node::random_u64()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A name of its own rather than the computer's.
        let mut me = node::load(&dir);
        me.name = Some(name.to_string());
        node::save(&dir, &me).unwrap();
        self.dirs.push(dir.clone());
        HubConfig {
            dir,
            port_base: self.base,
            port_span: 4,
            lan: false,
        }
    }

    fn hub(&mut self, name: &str) -> Hub {
        Hub::start(self.cfg(name), || {})
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        for d in &self.dirs {
            let _ = std::fs::remove_dir_all(d);
        }
    }
}

/// Wait (up to a minute) for `f` to hold of `hub`'s view.
fn until(hub: &Hub, what: &str, f: impl Fn(&NetView) -> bool) -> Arc<NetView> {
    let began = Instant::now();
    loop {
        let v = hub.view();
        if f(&v) {
            return v;
        }
        assert!(
            began.elapsed() < Duration::from_secs(60),
            "waited a minute for {what}; the view: {v:#?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn id_of(h: &Hub) -> NodeId {
    h.view().me.id
}

fn peer(v: &NetView, id: NodeId) -> Option<&PeerView> {
    v.peer(id)
}

/// A primary and a computer adopted by it, both on.
fn primary_and_display(rig: &mut Rig) -> (Hub, Hub) {
    let p = rig.hub("Booth");
    let d = rig.hub("Left");
    p.ask(NetAction::BecomePrimary);
    d.ask(NetAction::SetOn(true));
    let did = id_of(&d);
    until(&p, "the display to be heard", |v| {
        peer(v, did).is_some_and(|x| x.waiting && x.connected)
    });
    p.ask(NetAction::Adopt(did));
    until(&p, "the display to be adopted and report", |v| {
        peer(v, did).is_some_and(|x| x.ours && x.status.is_some() && x.connected)
    });
    (p, d)
}

#[test]
fn a_computer_that_is_not_switched_on_opens_nothing() {
    let mut rig = Rig::new();
    let cfg = rig.cfg("Quiet");
    let h = Hub::start(cfg.clone(), || {});
    std::thread::sleep(Duration::from_millis(500));
    let v = h.view();
    assert!(v.port.is_none());
    assert!(!v.me.on);
    // Every port of the range is still free.
    for p in discover::ports(cfg.port_base, cfg.port_span) {
        assert!(
            TcpListener::bind(("127.0.0.1", p)).is_ok(),
            "port {p} is free"
        );
    }
}

#[test]
fn two_computers_find_each_other_and_the_primary_adopts_names_and_releases() {
    let mut rig = Rig::new();
    let (p, d) = primary_and_display(&mut rig);
    let (pid, did) = (id_of(&p), id_of(&d));
    // The display: adopted, following the primary, kept on disk.
    let dv = until(&d, "the display to follow the primary", |v| {
        v.following
            .as_ref()
            .is_some_and(|f| f.online && f.id == pid)
    });
    assert_eq!(dv.me.role, Role::Display);
    assert_eq!(dv.me.network, p.view().me.network);
    assert_eq!(dv.following.as_ref().unwrap().name, "Booth");
    let kept = node::load(&rig.dirs[1]);
    assert_eq!((kept.role, kept.network), (Role::Display, dv.me.network));
    // Its report: name, role, version and the placeholders of later stages.
    d.set_local(LocalFacts {
        monitors: vec![MonitorReport {
            identity: "DELL U2720".into(),
            raw: "DP-1".into(),
            hardware: None,
            size: (2560, 1440),
            refresh_hz: Some(60.0),
            ..Default::default()
        }],
        audio_devices: vec!["Speakers".into(), "USB interface".into()],
        software: None,
    });
    let pv = until(&p, "the display's monitors", |v| {
        peer(v, did)
            .and_then(|x| x.status.as_ref())
            .is_some_and(|s| s.monitors.len() == 1 && s.audio_devices.len() == 2)
    });
    let s = peer(&pv, did).unwrap().status.clone().unwrap();
    assert_eq!((s.name.as_str(), s.role), ("Left", Role::Display));
    assert_eq!(s.version, proto::VERSION);
    assert!(s.clock_error_ms.is_none() && s.show_version.is_none());
    assert_eq!(pv.computers().len(), 1);
    // The role, set from the primary, written there at once and reported back.
    p.ask(NetAction::SetRole(did, Role::Backup));
    until(&p, "the backup role to be reported", |v| {
        peer(v, did).is_some_and(|x| x.role == Role::Backup)
    });
    assert_eq!(node::load(&rig.dirs[1]).role, Role::Backup);
    // A primary can't be made from here.
    p.ask(NetAction::SetRole(did, Role::Primary));
    // Renamed from the primary.
    p.ask(NetAction::Rename(did, "Truss left".into()));
    until(&p, "the new name", |v| {
        peer(v, did).is_some_and(|x| x.name == "Truss left")
    });
    assert_eq!(d.view().name, "Truss left");
    assert_eq!(d.view().me.role, Role::Backup);
    // Identify: the display is told to show its name.
    let _ = d.take_events();
    p.ask(NetAction::Identify(did));
    until(&d, "the name to be shown", |v| v.identify_until.is_some());
    assert!(
        d.take_events()
            .iter()
            .any(|e| matches!(e, NetEvent::Identify { .. }))
    );
    // Released: back to waiting, still on, and the primary sees it so.
    p.ask(NetAction::Release(did));
    let dv = until(&d, "the release", |v| v.me.role == Role::Alone);
    assert!(dv.me.on && dv.me.network.is_none() && dv.following.is_none());
    until(&p, "the release to be seen", |v| {
        peer(v, did).is_some_and(|x| !x.ours && x.waiting)
    });
    assert_eq!(node::load(&rig.dirs[1]).network, None);
    let _ = pid;
}

#[test]
fn the_first_primary_wins_and_another_network_is_refused() {
    let mut rig = Rig::new();
    let (p, d) = primary_and_display(&mut rig);
    let did = id_of(&d);
    let net = p.view().me.network;
    // A second primary, another network: it sees the display belongs elsewhere
    // and can't adopt it.
    let q = rig.hub("Other");
    q.ask(NetAction::BecomePrimary);
    until(&q, "the display to be heard", |v| peer(v, did).is_some());
    q.ask(NetAction::Adopt(did));
    std::thread::sleep(Duration::from_secs(2));
    let qv = q.view();
    let x = peer(&qv, did).unwrap();
    assert!(!x.ours && !x.waiting && !x.connected, "{x:?}");
    // Reached by its address all the same, it says why not.
    let port = d.view().port.unwrap();
    q.ask(NetAction::AddAddress(format!("127.0.0.1:{port}")));
    until(&q, "the refusal", |v| {
        peer(v, did).is_some_and(|x| {
            x.problem
                .as_deref()
                .is_some_and(|t| t.contains("another show network"))
        })
    });
    assert_eq!(d.view().me.network, net, "still the first primary's");
    assert_eq!(d.view().me.role, Role::Display);
}

#[test]
fn a_computer_that_vanishes_is_offline_and_comes_back() {
    let mut rig = Rig::new();
    let (p, d) = primary_and_display(&mut rig);
    let did = id_of(&d);
    let dir = rig.dirs[1].clone();
    let base = rig.base;
    drop(d);
    let pv = until(&p, "the display to go offline", |v| {
        peer(v, did).is_some_and(|x| !x.online)
    });
    assert!(peer(&pv, did).unwrap().ours, "kept in the list, offline");
    // It starts again (same folder: same identity) and is back, still adopted.
    let d = Hub::start(
        HubConfig {
            dir,
            port_base: base,
            port_span: 4,
            lan: false,
        },
        || {},
    );
    assert_eq!(d.view().me.role, Role::Display);
    until(&p, "the display to be back", |v| {
        peer(v, did).is_some_and(|x| x.online && x.connected && x.ours)
    });
    until(&d, "the display to follow again", |v| {
        v.following.as_ref().is_some_and(|f| f.online)
    });
}

#[test]
fn three_computers_and_a_switch_turned_off() {
    let mut rig = Rig::new();
    let (p, d) = primary_and_display(&mut rig);
    let e = rig.hub("Right");
    e.ask(NetAction::SetOn(true));
    let eid = id_of(&e);
    until(&p, "the third to be heard", |v| {
        peer(v, eid).is_some_and(|x| x.waiting && x.connected)
    });
    p.ask(NetAction::Adopt(eid));
    until(&p, "both adopted", |v| v.computers().len() == 2);
    // The display switched off on itself: it leaves.
    d.ask(NetAction::SetOn(false));
    let dv = until(&d, "the display to be off", |v| v.port.is_none());
    assert_eq!((dv.me.role, dv.me.network), (Role::Alone, None));
    let did = id_of(&d);
    until(&p, "the display to be offline", |v| {
        peer(v, did).is_some_and(|x| !x.online)
    });
    assert!(e.view().following.as_ref().is_some_and(|f| f.online));
}

#[test]
fn an_address_added_by_hand_is_found_without_beacons() {
    let mut rig = Rig::new();
    // Two ranges: the beacons of one never reach the other.
    let mut far = Rig::new();
    let p = rig.hub("Booth");
    let mut cfg = far.cfg("Far");
    cfg.port_span = 1;
    let d = Hub::start(cfg, || {});
    p.ask(NetAction::BecomePrimary);
    d.ask(NetAction::SetOn(true));
    let dport = until(&d, "the far one to listen", |v| v.port.is_some())
        .port
        .unwrap();
    let did = id_of(&d);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(p.view().peer(did).is_none(), "not heard by beacon");
    p.ask(NetAction::AddAddress(format!("127.0.0.1:{dport}")));
    until(&p, "the address to be reached", |v| {
        peer(v, did).is_some_and(|x| x.connected && x.waiting)
    });
    assert!(
        p.view().manual[0]
            .note
            .as_deref()
            .is_some_and(|n| n.contains("Far"))
    );
    p.ask(NetAction::Adopt(did));
    until(&d, "adoption by address", |v| v.me.role == Role::Display);
    // An address that is nobody.
    p.ask(NetAction::AddAddress(
        "nonsense address that can't be".into(),
    ));
    until(&p, "the bad address to be said so", |v| {
        v.manual
            .iter()
            .any(|m| m.note.as_deref().is_some_and(|n| n.contains("isn't")))
    });
}

#[test]
fn the_passcode_is_needed_once_set() {
    let mut rig = Rig::new();
    let (p, d) = primary_and_display(&mut rig);
    let did = id_of(&d);
    p.ask(NetAction::SetPasscode(Some("opening night".into())));
    until(&d, "the key to reach the display", |v| {
        v.me.passcode.is_some()
    });
    assert_eq!(
        d.view().me.passcode,
        Some(Key::from_passcode("opening night"))
    );
    // The primary restarts without the passcode (another computer claiming
    // the network): the display refuses it.
    let pdir = rig.dirs[0].clone();
    drop(p);
    let mut me = node::load(&pdir);
    me.passcode = None;
    node::save(&pdir, &me).unwrap();
    let p = Hub::start(
        HubConfig {
            dir: pdir.clone(),
            port_base: rig.base,
            port_span: 4,
            lan: false,
        },
        || {},
    );
    let pv = until(&p, "the refusal", |v| {
        peer(v, did).is_some_and(|x| x.problem.as_deref().is_some_and(|t| t.contains("passcode")))
    });
    assert!(!peer(&pv, did).unwrap().connected);
    drop(p);
    // With the passcode again, it is obeyed.
    let mut me = node::load(&pdir);
    me.passcode = Some(Key::from_passcode("opening night"));
    node::save(&pdir, &me).unwrap();
    let p = Hub::start(
        HubConfig {
            dir: pdir,
            port_base: rig.base,
            port_span: 4,
            lan: false,
        },
        || {},
    );
    until(&p, "the display back with the passcode", |v| {
        peer(v, did).is_some_and(|x| x.connected && x.ours)
    });
    // A computer waiting with a passcode typed on it: only that primary adopts it.
    let w = rig.hub("Waiting");
    w.ask(NetAction::SetPasscode(Some("something else".into())));
    w.ask(NetAction::SetOn(true));
    let wid = id_of(&w);
    until(&p, "the refusal of the waiting one", |v| {
        peer(v, wid).is_some_and(|x| x.problem.is_some())
    });
    assert_eq!(w.view().me.role, Role::Alone);
}

#[test]
fn a_newer_primary_wins_and_the_old_one_becomes_the_backup() {
    let mut rig = Rig::new();
    let (p, d) = primary_and_display(&mut rig);
    let did = id_of(&d);
    // Another computer of the same network with a higher term (a backup that
    // took control: a later stage's button; here written into its file).
    let b = rig.cfg("Backup");
    let mut me = node::load(&b.dir);
    me.role = Role::Primary;
    me.on = true;
    me.network = p.view().me.network;
    me.term = p.view().me.term + 1;
    node::save(&b.dir, &me).unwrap();
    let b = Hub::start(b, || {});
    let bid = id_of(&b);
    until(&d, "the display to follow the newer primary", |v| {
        v.following
            .as_ref()
            .is_some_and(|f| f.id == bid && f.online)
    });
    until(&p, "the old primary to step down", |v| {
        v.me.role == Role::Backup
    });
    assert_eq!(d.view().me.term, me.term);
    until(&b, "the display under the new primary", |v| {
        peer(v, did).is_some_and(|x| x.connected && x.ours)
    });
}

#[test]
fn messages_of_later_stages_reach_the_app() {
    let mut rig = Rig::new();
    let (p, d) = primary_and_display(&mut rig);
    let (pid, did) = (id_of(&p), id_of(&d));
    let _ = d.take_events();
    // A message the hub itself does something with goes through send_to.
    p.send_to(did, Msg::Identify { seconds: 2.0 });
    until(&d, "identify sent directly", |v| v.identify_until.is_some());
    // And a broadcast reaches every computer.
    p.broadcast(Msg::Identify { seconds: 2.0 });
    let began = Instant::now();
    let mut seen = Vec::new();
    while began.elapsed() < Duration::from_secs(60) && seen.len() < 2 {
        seen.extend(d.take_events());
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(seen.len() >= 2, "{seen:?}");
    let _ = pid;
}

#[test]
fn a_newq_of_another_protocol_is_named_and_not_spoken_to() {
    let mut rig = Rig::new();
    let p = rig.hub("Booth");
    p.ask(NetAction::BecomePrimary);
    let port = until(&p, "listening", |v| v.port.is_some()).port.unwrap();
    // A pretend NewQ of a later protocol: it beacons and answers on TCP.
    let fake_tcp = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let fake_port = fake_tcp.local_addr().unwrap().port();
    let beacon = Beacon {
        proto: proto::PROTOCOL + 1,
        version: "9.1.0".into(),
        id: NodeId(0xfeed),
        name: "Future".into(),
        role: Role::Alone,
        network: None,
        tcp_port: fake_port,
        show_version: None,
        term: 0,
        on: true,
        system: String::new(),
    };
    let udp = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    let began = Instant::now();
    loop {
        udp.send_to(&proto::encode_beacon(&beacon), ("127.0.0.1", port))
            .unwrap();
        let v = p.view();
        if let Some(x) = v.peer(NodeId(0xfeed))
            && x.problem.is_some()
        {
            assert!(!x.compatible);
            assert_eq!(
                x.problem.as_deref(),
                Some(
                    format!(
                        "Future runs NewQ 9.1, this is {}: update one of them.",
                        proto::VERSION
                            .split('.')
                            .take(2)
                            .collect::<Vec<_>>()
                            .join(".")
                    )
                    .as_str()
                )
            );
            break;
        }
        assert!(began.elapsed() < Duration::from_secs(60), "{v:#?}");
        std::thread::sleep(Duration::from_millis(100));
    }
    // It isn't dialled.
    fake_tcp.set_nonblocking(true).unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    assert!(fake_tcp.accept().is_err(), "never connected to");
    // Added by hand, the session's hello tells it apart the same way.
    let fake = std::thread::spawn(move || {
        fake_tcp.set_nonblocking(false).unwrap();
        let (mut s, _) = fake_tcp.accept().unwrap();
        let mut v = serde_json::to_value(Msg::Hello(Hello {
            proto: proto::PROTOCOL,
            version: "x".into(),
            id: NodeId(0xfeed),
            name: "Future".into(),
            role: Role::Alone,
            network: None,
            term: 0,
            nonce: 1,
            tcp_port: 0,
        }))
        .unwrap();
        v["proto"] = (proto::PROTOCOL + 1).into();
        v["version"] = "9.1.0".into();
        let body = serde_json::to_vec(&v).unwrap();
        use std::io::Write;
        let _ = s.write_all(&(body.len() as u32).to_be_bytes());
        let _ = s.write_all(&body);
        std::thread::sleep(Duration::from_secs(1));
    });
    p.ask(NetAction::AddAddress(format!("127.0.0.1:{fake_port}")));
    until(&p, "the hello of another protocol", |v| {
        v.manual.iter().any(|m| {
            m.note
                .as_deref()
                .is_some_and(|n| n.contains("Future runs NewQ 9.1"))
        })
    });
    fake.join().unwrap();
}

#[test]
fn garbage_on_the_port_is_shrugged_off() {
    let mut rig = Rig::new();
    let p = rig.hub("Booth");
    p.ask(NetAction::BecomePrimary);
    let port = until(&p, "listening", |v| v.port.is_some()).port.unwrap();
    use std::io::Write;
    // A huge length, and plain nonsense.
    for junk in [
        vec![0xff, 0xff, 0xff, 0xff, 1, 2],
        b"GET / HTTP/1.0\r\n\r\n".to_vec(),
    ] {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let _ = s.write_all(&junk);
        s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
        // The hub closes the connection on the junk (after its hello, if
        // that got out first).
        let mut buf = Vec::new();
        use std::io::Read;
        let _ = s.read_to_end(&mut buf);
    }
    // And it still works.
    let d = rig.hub("Left");
    d.ask(NetAction::SetOn(true));
    let did = id_of(&d);
    until(&p, "a real computer after the junk", |v| {
        peer(v, did).is_some_and(|x| x.connected)
    });
}

// ---- Playing together (stage 4a) over loopback: the clock, the show, commands ----

/// A follower keeps the primary's clock over UDP (the beacon socket answers),
/// locks, and its report of the error and its show reaches the primary.
#[test]
fn a_follower_locks_to_the_primary_clock_and_reports_it() {
    let mut rig = Rig::new();
    let (p, d) = primary_and_display(&mut rig);
    let did = id_of(&d);
    let clock = d.clock();
    let began = Instant::now();
    while !clock.now().is_some_and(|n| n.locked) {
        assert!(began.elapsed() < Duration::from_secs(60), "never locked");
        std::thread::sleep(Duration::from_millis(50));
    }
    // One computer, one clock: the error is the loopback's.
    let n = clock.now().unwrap();
    assert!(
        (n.time - super::super::time::local_now()).abs() < 0.005,
        "{n:?}"
    );
    assert!(clock.worst_rtt().is_some_and(|r| r < 0.5));
    d.set_sync(SyncFacts {
        clock_error_ms: Some(n.error * 1000.0),
        clock_locked: true,
        show_hash: Some(42),
        in_step: true,
        ..Default::default()
    });
    until(&p, "the follower's clock to be reported", |v| {
        peer(v, did)
            .and_then(|x| x.status.as_ref())
            .is_some_and(|s| {
                s.clock_locked && s.clock_error_ms.is_some() && s.show_hash == Some(42)
            })
    });
}

/// Two engines and their app-side state, as `app/play.rs` keeps them.
struct Pair {
    show_p: crate::show::Show,
    show_d: crate::show::Show,
    ep: crate::engine::Engine,
    ed: crate::engine::Engine,
    leader: crate::net::play::Leader,
    follower: crate::net::play::Follower,
    assembly: crate::net::play::ShowAssembly,
    have: Option<u64>,
    hash: u64,
    version: u64,
}

impl Pair {
    fn send_show(&mut self, p: &Hub) {
        let json = serde_json::to_string(&self.show_p).unwrap();
        self.version += 1;
        for part in crate::net::play::show_parts(self.version, &json) {
            p.tell(None, Msg::ShowPart(Box::new(part)), false);
        }
        self.hash = crate::net::play::show_hash(&json);
    }

    /// One pass of both computers, as the app makes it.
    fn pass(&mut self, p: &Hub, d: &Hub, did: NodeId) {
        use crate::net::play::Out;
        let now = super::super::time::local_now();
        let outs = self.leader.issue(self.ep.net_take(), now, 0.05, &[did]);
        for o in outs.into_iter().chain(self.leader.resend(now)) {
            if let Out::Command(to, t) = o {
                p.tell(to, Msg::Play(Box::new(t)), true);
            }
        }
        self.leader.step(&mut self.ep, &self.show_p, now);
        if let Some(s) = self.leader.beat(&self.ep, self.hash, now) {
            p.tell(None, Msg::PlayState(Box::new(s)), true);
        }
        for ev in d.take_events() {
            if let NetEvent::Message { msg, .. } = ev {
                match msg {
                    Msg::Play(t) => {
                        let (epoch, seq) = self.follower.receive(*t, &mut self.ed, &self.show_d);
                        d.tell(None, Msg::PlayAck { epoch, seq }, true);
                    }
                    Msg::PlayState(s) => self.follower.heard(*s),
                    Msg::ShowPart(part) => {
                        if let Some((_, h, json)) = self.assembly.take(*part) {
                            self.show_d = serde_json::from_str(&json).unwrap();
                            self.have = Some(h);
                        }
                    }
                    _ => {}
                }
            }
        }
        for ev in p.take_events() {
            if let NetEvent::Message {
                from,
                msg: Msg::PlayAck { epoch, seq },
            } = ev
            {
                self.leader.ack(from, epoch, seq);
            }
        }
        let shared = d.clock().now().filter(|n| n.locked).map(|n| n.time);
        self.follower
            .step(&mut self.ed, &self.show_d, shared, 0.01, self.have);
        std::thread::sleep(Duration::from_millis(10));
    }

    /// Passes until `f` holds, for up to a minute.
    fn until(&mut self, p: &Hub, d: &Hub, did: NodeId, what: &str, f: impl Fn(&Pair) -> bool) {
        let began = Instant::now();
        while !f(self) {
            assert!(
                began.elapsed() < Duration::from_secs(60),
                "waited a minute for {what}"
            );
            self.pass(p, d, did);
        }
    }
}

/// Two engines on two hubs over loopback: the show goes over, GO on the
/// primary plays on both at the same shared time, an edit reaches the follower
/// without stopping what plays there, and the heartbeat finds both alike.
#[test]
fn the_show_and_commands_reach_a_follower_over_loopback() {
    use crate::engine::{Engine, NetMode};
    let mut rig = Rig::new();
    let (p, d) = primary_and_display(&mut rig);
    let did = id_of(&d);
    let ts = crate::net::trial::trial_show();
    let mut ep = Engine::for_tests();
    ep.net_set_mode(NetMode::Primary);
    let mut ed = Engine::for_tests();
    ed.net_set_mode(NetMode::Follower);
    let mut pair = Pair {
        show_p: ts.show.clone(),
        show_d: crate::show::Show::default(),
        ep,
        ed,
        leader: crate::net::play::Leader::new(5),
        follower: Default::default(),
        assembly: Default::default(),
        have: None,
        hash: 0,
        version: 0,
    };
    pair.send_show(&p);
    pair.until(&p, &d, did, "the show and the clock", |x| {
        x.have == Some(x.hash) && x.ed.net_shared_time().is_some()
    });
    assert_eq!(pair.show_d.cues.len(), pair.show_p.cues.len());
    // The operator fires the cue on the primary: both play it.
    let show_p = pair.show_p.clone();
    pair.ep.fire(&show_p, ts.cue);
    pair.until(&p, &d, did, "the cue on the follower", |x| {
        x.ed.active.iter().any(|a| a.cue_id == ts.cue)
    });
    // An edit: the cue renamed. It reaches the follower, and the cue plays on.
    pair.show_p.cues[0].name = "Renamed".into();
    // And the media manager's show settings with it.
    pair.show_p.no_transfer_cards = true;
    pair.show_p.library_folders = vec!["/media/library".into()];
    pair.send_show(&p);
    pair.until(&p, &d, did, "the edit and a heartbeat in step", |x| {
        x.have == Some(x.hash) && x.follower.in_step
    });
    assert_eq!(pair.show_d.cues[0].name, "Renamed");
    assert!(pair.show_d.no_transfer_cards);
    assert_eq!(pair.show_d.library_folders, pair.show_p.library_folders);
    assert!(
        pair.ed.active.iter().any(|a| a.cue_id == ts.cue),
        "it plays on"
    );
    // Where the cue's time was 0, by the shared clock, on each: within a frame.
    let zero = |e: &Engine| {
        let a = e.active.iter().find(|a| a.cue_id == ts.cue).unwrap();
        e.net_shared_time().unwrap() - a.elapsed
    };
    let (zp, zd) = (zero(&pair.ep), zero(&pair.ed));
    assert!((zp - zd).abs() < 1.0 / 60.0, "{zp} {zd}");
    assert_eq!(
        pair.follower.corrections, 0,
        "{:?}",
        pair.follower.corrected
    );
}

// ---- Take control (stage 5) ----

/// A primary, a display and a backup, all reporting, the followers' clocks
/// locked to the primary's.
fn primary_display_backup(rig: &mut Rig) -> (Hub, Hub, Hub) {
    let (p, d) = primary_and_display(rig);
    let b = rig.hub("Backup");
    b.ask(NetAction::SetOn(true));
    let bid = id_of(&b);
    until(&p, "the backup to be heard", |v| {
        peer(v, bid).is_some_and(|x| x.waiting && x.connected)
    });
    p.ask(NetAction::Adopt(bid));
    until(&p, "the backup adopted", |v| {
        peer(v, bid).is_some_and(|x| x.ours && x.connected && x.status.is_some())
    });
    p.ask(NetAction::SetRole(bid, Role::Backup));
    until(&b, "the backup role", |v| v.me.role == Role::Backup);
    for (h, what) in [(&d, "the display's clock"), (&b, "the backup's clock")] {
        let began = Instant::now();
        while !h.clock().now().is_some_and(|n| n.locked) {
            assert!(
                began.elapsed() < Duration::from_secs(120),
                "{what} never locked"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    (p, d, b)
}

fn restart(dir: PathBuf, base: u16) -> Hub {
    Hub::start(
        HubConfig {
            dir,
            port_base: base,
            port_span: 4,
            lan: false,
        },
        || {},
    )
}

/// The primary stops; the backup says so, takes control, and the display
/// follows it with its clock carried on; the old primary, started again,
/// steps down to be the backup and follows the new one.
#[test]
fn the_backup_takes_control_and_the_old_primary_comes_back_as_the_backup() {
    let mut rig = Rig::new();
    let (p, d, b) = primary_display_backup(&mut rig);
    let (pid, did, bid) = (id_of(&p), id_of(&d), id_of(&b));
    let term = p.view().me.term;
    let pdir = rig.dirs[0].clone();
    // Take control is refused on the display.
    d.ask(NetAction::TakeControl);
    drop(p);
    let bv = until(&b, "the primary to be said quiet", |v| {
        v.following
            .as_ref()
            .is_some_and(|f| f.id == pid && !f.online && f.quiet_secs.is_some())
    });
    assert!(bv.following.as_ref().unwrap().quiet_secs.unwrap() >= 2);
    assert_eq!(
        d.view().me.role,
        Role::Display,
        "a display can't take control"
    );
    b.ask(NetAction::TakeControl);
    let bv = until(&b, "the backup to run the show", |v| v.is_primary());
    assert_eq!(bv.me.term, term + 1);
    assert_eq!(node::load(&rig.dirs[2]).role, Role::Primary, "kept");
    until(&d, "the display to follow the new primary", |v| {
        v.following
            .as_ref()
            .is_some_and(|f| f.id == bid && f.online)
    });
    until(&b, "the display under the new primary", |v| {
        peer(v, did).is_some_and(|x| x.connected && x.ours && x.status.is_some())
    });
    assert_eq!(d.view().me.term, term + 1);
    // The shared clock carried on: the display's agrees with the new
    // primary's (one computer: within the loopback's noise).
    let began = Instant::now();
    let mut best = f64::INFINITY;
    while began.elapsed() < Duration::from_secs(60) {
        if let Some(n) = d.clock().now().filter(|n| n.locked) {
            best = best.min((n.time - b.primary_now()).abs());
            if best < 0.005 {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(best < 0.005, "display {best} s from the new primary");
    // The old primary starts again: it hears the newer one and steps down.
    let p = restart(pdir.clone(), rig.base);
    until(&p, "the old primary to step down", |v| {
        v.me.role == Role::Backup
            && v.following
                .as_ref()
                .is_some_and(|f| f.id == bid && f.online)
    });
    assert_eq!(node::load(&pdir).role, Role::Backup);
    until(&b, "the old primary as the backup", |v| {
        peer(v, pid).is_some_and(|x| x.connected && x.role == Role::Backup)
    });
    assert!(
        d.view().following.as_ref().is_some_and(|f| f.id == bid),
        "the display never went back"
    );
}

/// The backup takes control while the primary is still there: the primary
/// hears it and steps down; the display moves over.
#[test]
fn a_primary_still_there_steps_down_when_the_backup_takes_control() {
    let mut rig = Rig::new();
    let (p, d, b) = primary_display_backup(&mut rig);
    let bid = id_of(&b);
    b.ask(NetAction::TakeControl);
    until(&b, "the backup to run the show", |v| v.is_primary());
    until(&p, "the old primary to step down and follow", |v| {
        v.me.role == Role::Backup
            && v.following
                .as_ref()
                .is_some_and(|f| f.id == bid && f.online)
    });
    until(&d, "the display to follow the new primary", |v| {
        v.following
            .as_ref()
            .is_some_and(|f| f.id == bid && f.online)
    });
    // Only one primary: the old one runs nothing now.
    assert!(!p.view().is_primary());
    until(&b, "everyone under the new primary", |v| {
        v.computers().len() == 2
    });
}

/// A primary remembers its computers in node.json: started again where no
/// beacon reaches, it dials them at their last address; one gone for good is
/// forgotten.
#[test]
fn a_restarted_primary_finds_its_computers_again() {
    let mut rig = Rig::new();
    let mut far = Rig::new();
    let p = rig.hub("Booth");
    let mut cfg = far.cfg("Far");
    cfg.port_span = 1;
    let d = Hub::start(cfg, || {});
    p.ask(NetAction::BecomePrimary);
    d.ask(NetAction::SetOn(true));
    let dport = until(&d, "the far one to listen", |v| v.port.is_some())
        .port
        .unwrap();
    let did = id_of(&d);
    p.ask(NetAction::AddAddress(format!("127.0.0.1:{dport}")));
    until(&p, "the address to be reached", |v| {
        peer(v, did).is_some_and(|x| x.connected && x.waiting)
    });
    p.ask(NetAction::Adopt(did));
    until(&p, "adopted", |v| {
        peer(v, did).is_some_and(|x| x.connected && x.ours)
    });
    let pdir = rig.dirs[0].clone();
    let began = Instant::now();
    while !node::load(&pdir)
        .known
        .iter()
        .any(|k| k.id == did && k.port == dport)
    {
        assert!(
            began.elapsed() < Duration::from_secs(60),
            "never remembered"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // Started again: no address added by hand this time, and no beacon.
    drop(p);
    let p = restart(pdir.clone(), rig.base);
    let pv = p.view();
    assert!(
        peer(&pv, did).is_some_and(|x| x.ours),
        "listed at once, before it is reached"
    );
    until(&p, "the display found again", |v| {
        peer(v, did).is_some_and(|x| x.connected && x.ours)
    });
    until(&d, "the display following again", |v| {
        v.following.as_ref().is_some_and(|f| f.online)
    });
    // Gone for good: forgotten.
    drop(d);
    until(&p, "the display offline", |v| {
        peer(v, did).is_some_and(|x| !x.online && !x.connected)
    });
    p.ask(NetAction::Forget(did));
    until(&p, "forgotten", |v| peer(v, did).is_none());
    assert!(node::load(&pdir).known.is_empty());
}

// ---- The network test (stage 6, `net::measure`) ----

#[test]
fn the_network_tests_measurements_reach_the_primary_from_two_computers() {
    use crate::net::measure::{NetTest, Report, Row};
    let mut rig = Rig::new();
    let (p, d) = primary_and_display(&mut rig);
    let e = rig.hub("Right");
    e.ask(NetAction::SetOn(true));
    let eid = id_of(&e);
    until(&p, "the third to be heard", |v| {
        peer(v, eid).is_some_and(|x| x.waiting && x.connected)
    });
    p.ask(NetAction::Adopt(eid));
    until(&p, "both adopted and connected", |v| {
        v.computers().len() == 2 && v.peers.iter().all(|x| !x.ours || x.connected)
    });
    let (pid, did) = (id_of(&p), id_of(&d));
    let _ = (d.take_events(), e.take_events(), p.take_events());
    // The primary tells them to measure; each hears it.
    p.broadcast(Msg::NetTest(Box::new(NetTest::Begin {
        run: 9,
        start: 1.0,
        load: None,
    })));
    for h in [&d, &e] {
        let began = Instant::now();
        let mut got = false;
        while !got && began.elapsed() < Duration::from_secs(60) {
            got = h.take_events().into_iter().any(|ev| {
                matches!(ev, NetEvent::Message { from, msg: Msg::NetTest(t) }
                    if from == pid && *t == NetTest::Begin { run: 9, start: 1.0, load: None })
            });
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(got, "Begin reached a computer");
    }
    // Each reports back.
    for (h, n) in [(&d, 1u32), (&e, 2)] {
        let r = Report {
            run: 9,
            seq: n,
            rows: vec![Row {
                t: 0.5,
                apart_ms: Some(f64::from(n)),
                ..Default::default()
            }],
            ..Default::default()
        };
        h.send_to(pid, Msg::NetTest(Box::new(NetTest::Report(Box::new(r)))));
    }
    let began = Instant::now();
    let mut from = Vec::new();
    while from.len() < 2 && began.elapsed() < Duration::from_secs(60) {
        for ev in p.take_events() {
            if let NetEvent::Message {
                from: f,
                msg: Msg::NetTest(t),
            } = ev
                && let NetTest::Report(r) = *t
            {
                assert_eq!(r.rows[0].apart_ms, Some(f64::from(r.seq)));
                from.push(f);
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    from.sort();
    let mut want = vec![did, eid];
    want.sort();
    assert_eq!(from, want);
}

#[test]
fn the_tests_network_load_goes_both_ways_on_its_own_connection_and_stops() {
    use crate::net::load::{LoadClient, LoadLevel, LoadServer};
    use crate::net::measure::NetTest;
    let mut rig = Rig::new();
    let (p, d) = primary_and_display(&mut rig);
    let (pid, did) = (id_of(&p), id_of(&d));
    let _ = (d.take_events(), p.take_events());
    // The primary starts the load as playing begins and says where in Begin.
    let server = LoadServer::start("127.0.0.1:0".parse().unwrap(), LoadLevel::Light).unwrap();
    p.broadcast(Msg::NetTest(Box::new(NetTest::Begin {
        run: 4,
        start: 1.0,
        load: Some(server.ask()),
    })));
    // The computer dials the primary's address, as the app does.
    let began = Instant::now();
    let mut client = None;
    while client.is_none() && began.elapsed() < Duration::from_secs(60) {
        for ev in d.take_events() {
            if let NetEvent::Message {
                from,
                msg: Msg::NetTest(t),
            } = ev
                && let NetTest::Begin {
                    run,
                    load: Some(ask),
                    ..
                } = *t
                && from == pid
            {
                let addr = d
                    .view()
                    .peer(pid)
                    .and_then(|x| x.addr)
                    .expect("its address");
                client = Some(LoadClient::start(
                    std::net::SocketAddr::new(addr, ask.port),
                    did,
                    run,
                    &ask,
                ));
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let client = client.expect("Begin with the load reached the computer");
    let going = |what: &str, f: &dyn Fn(&crate::net::load::Totals) -> bool| {
        let t = Instant::now();
        while !server.totals().get(&did).is_some_and(f) {
            assert!(t.elapsed() < Duration::from_secs(30), "{what}");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    going("bytes both ways", &|t| {
        t.connected && t.sent > 2_000_000 && t.received > 2_000_000
    });
    // The hubs' own sessions carry on meanwhile.
    until(&p, "the computer still connected", |v| {
        peer(v, did).is_some_and(|x| x.connected && x.status.is_some())
    });
    // The test ends: the primary stops, the computer lets go by itself.
    server.stop();
    let t = Instant::now();
    while !client.ended() {
        assert!(t.elapsed() < Duration::from_secs(30), "the computer let go");
        std::thread::sleep(Duration::from_millis(50));
    }
    going("closed", &|t| !t.connected);
    let t = server.totals()[&did].clone();
    let line = crate::net::load::report_lines(LoadLevel::Light, &[("Left".into(), Some(t))], None);
    assert!(line[1].starts_with("  Left: to it "), "{line:?}");
}

// ---- NewQ Display (`--display`) ----

/// A computer started as a display (`--display`): its folder set up as
/// `apply_startup` does at start-up.
fn display_cfg(rig: &mut Rig, name: &str) -> HubConfig {
    let cfg = rig.cfg(name);
    assert!(node::apply_startup(node::Startup::Display, &cfg.dir).display);
    cfg
}

/// Started as a display, it is seen waiting by a primary, is added with one
/// click there and is a display without its role being set.
#[test]
fn a_computer_started_as_a_display_waits_and_is_added_as_a_display() {
    let mut rig = Rig::new();
    let p = rig.hub("Booth");
    p.ask(NetAction::BecomePrimary);
    let d = Hub::start(display_cfg(&mut rig, "Left"), || {});
    let did = id_of(&d);
    let pid = id_of(&p);
    until(&p, "the display waiting", |v| {
        peer(v, did).is_some_and(|x| x.waiting && !x.ours)
    });
    p.ask(NetAction::Adopt(did));
    until(&d, "the display following", |v| {
        v.me.role == Role::Display
            && v.following
                .as_ref()
                .is_some_and(|f| f.id == pid && f.online)
    });
    until(&p, "the display reporting as a display", |v| {
        peer(v, did).is_some_and(|x| {
            x.ours && x.connected && x.status.as_ref().is_some_and(|s| s.role == Role::Display)
        })
    });
}

/// A display added once comes back to its primary with no click anywhere,
/// whichever of them starts again, and in either order when both do; and
/// `--display` at each start never undoes the pairing.
#[test]
fn a_paired_display_comes_back_by_itself_after_either_or_both_start_again() {
    let mut rig = Rig::new();
    let p = rig.hub("Booth");
    p.ask(NetAction::BecomePrimary);
    let dcfg = display_cfg(&mut rig, "Left");
    let (pdir, ddir) = (rig.dirs[0].clone(), dcfg.dir.clone());
    let d = Hub::start(dcfg, || {});
    let (pid, did) = (id_of(&p), id_of(&d));
    until(&p, "the display waiting", |v| {
        peer(v, did).is_some_and(|x| x.waiting)
    });
    p.ask(NetAction::Adopt(did));
    // Paired: the display follows, and the primary remembers it.
    let paired = |p: &Hub, d: &Hub, what: &str| {
        until(d, &format!("{what}: the display following"), |v| {
            v.me.role == Role::Display
                && v.following
                    .as_ref()
                    .is_some_and(|f| f.id == pid && f.online)
        });
        until(p, &format!("{what}: the display connected"), |v| {
            peer(v, did).is_some_and(|x| x.ours && x.connected && x.status.is_some())
        });
    };
    paired(&p, &d, "added");
    let began = Instant::now();
    while !node::load(&pdir).known.iter().any(|k| k.id == did) {
        assert!(
            began.elapsed() < Duration::from_secs(60),
            "never remembered"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let network = node::load(&ddir).network;
    assert!(network.is_some());
    // As NewQ Display starts it: `--display` again, then the hub.
    let start_display = |dir: &PathBuf| {
        let r = node::apply_startup(node::Startup::Display, dir);
        assert!(r.display, "{r:?}");
        let me = node::load(dir);
        assert_eq!((me.role, me.network), (Role::Display, network), "kept");
        restart(dir.clone(), rig.base)
    };
    // (a) The display starts again.
    drop(d);
    let d = start_display(&ddir);
    paired(&p, &d, "display restarted");
    // (b) The primary starts again.
    drop(p);
    let p = restart(pdir.clone(), rig.base);
    paired(&p, &d, "primary restarted");
    // (c) Both, the display first.
    drop(p);
    drop(d);
    let d = start_display(&ddir);
    std::thread::sleep(Duration::from_millis(300));
    let p = restart(pdir.clone(), rig.base);
    paired(&p, &d, "both, display first");
    // (c) Both, the primary first.
    drop(p);
    drop(d);
    let p = restart(pdir.clone(), rig.base);
    std::thread::sleep(Duration::from_millis(300));
    let d = start_display(&ddir);
    paired(&p, &d, "both, primary first");
    assert_eq!(node::load(&ddir).network, network);
}

/// The link a computer reported to its primary is in its own row too (the
/// network test's report read "link unknown" for a computer whose status
/// said "Wired", 0.24.1).
#[test]
fn a_computers_own_status_says_its_link() {
    let mut rig = Rig::new();
    let (tx, _rx) = crossbeam_channel::unbounded();
    let (out, _events) = crossbeam_channel::unbounded();
    let mut st = State::new(rig.cfg("link"), Identity::default(), tx, out);
    assert_eq!(st.view().status.link, Link::Unknown);
    st.link = Link::Wired;
    assert_eq!(st.view().status.link, Link::Wired);
    assert_eq!(st.own_status().link, Link::Wired);
}

// ---- Second test's fixes: one identity twice, an adoption that can't connect ----

/// A second NewQ with the primary's identity (its `node.json` copied, as a
/// stray copy on the same computer shares it) on another port: every computer
/// says so, and the display keeps its clock with the one its session is with.
#[test]
fn a_second_newq_with_the_primarys_identity_is_noticed_and_never_followed() {
    let mut rig = Rig::new();
    let (p, d) = primary_and_display(&mut rig);
    let (pid, _did) = (id_of(&p), id_of(&d));
    let pport = p.view().port.unwrap();
    let dv = until(&d, "the display's clock to be kept", |v| {
        v.clock_with.is_some() && v.following.as_ref().is_some_and(|f| f.online)
    });
    assert_eq!(dv.clock_with.unwrap().port(), pport);
    // The stray copy: the same node.json in another folder.
    let twin_dir = rig.dirs[0].with_extension("twin");
    let _ = std::fs::remove_dir_all(&twin_dir);
    std::fs::create_dir_all(&twin_dir).unwrap();
    std::fs::copy(rig.dirs[0].join(node::FILE), twin_dir.join(node::FILE)).unwrap();
    rig.dirs.push(twin_dir.clone());
    let t = restart(twin_dir, rig.base);
    let tport = until(&t, "the copy listening", |v| v.port.is_some())
        .port
        .unwrap();
    assert_ne!(tport, pport);
    assert_eq!(id_of(&t), pid, "one identity");
    // Both copies say another NewQ has their identity; the display says two
    // answer as the primary, and keeps to the one it is connected to.
    let pv = until(&p, "the primary to see the copy", |v| v.twin.is_some());
    assert!(
        pv.twin.as_deref().unwrap().contains(&tport.to_string()),
        "{:?}",
        pv.twin
    );
    until(&t, "the copy to see the primary", |v| v.twin.is_some());
    let dv = until(&d, "the display to see two primaries", |v| {
        peer(v, pid).is_some_and(|x| x.twin.is_some())
    });
    let said = peer(&dv, pid).unwrap().twin.clone().unwrap();
    assert!(said.contains(&format!("port {pport}")), "{said}");
    // For a while beacons come from both: the clock never moves to the copy.
    let began = Instant::now();
    while began.elapsed() < Duration::from_secs(4) {
        let v = d.view();
        assert_eq!(v.clock_with.map(|a| a.port()), Some(pport), "{v:#?}");
        let reached = v
            .reached
            .iter()
            .find(|(id, _)| *id == pid)
            .map(|r| r.1.port());
        assert_eq!(reached, Some(pport));
        std::thread::sleep(Duration::from_millis(100));
    }
    // The copy closed: the warnings go.
    drop(t);
    until(&p, "the primary's warning to go", |v| v.twin.is_none());
    until(&d, "the display's warning to go", |v| {
        peer(v, pid).is_some_and(|x| x.twin.is_none())
    });
}

/// "Add to the show" on a computer that beacons but can't be connected to
/// (a firewall, a VPN address): after `ADOPT_PATIENCE` its row says why,
/// naming each address tried and the way it goes, and the firewall only for a
/// Windows computer.
#[test]
fn an_adoption_that_cant_connect_says_why_on_the_row() {
    let mut rig = Rig::new();
    let p = rig.hub("Booth");
    p.ask(NetAction::BecomePrimary);
    let port = until(&p, "listening", |v| v.port.is_some()).port.unwrap();
    // A port nothing listens on.
    let closed = TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let beacon = Beacon {
        proto: proto::PROTOCOL,
        version: proto::VERSION.into(),
        id: NodeId(0xbeef),
        name: "WNO-2".into(),
        role: Role::Alone,
        network: None,
        tcp_port: closed,
        show_version: None,
        term: 0,
        on: true,
        system: "windows".into(),
    };
    let stop = Arc::new(AtomicBool::new(false));
    let beaconing = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let udp = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
            while !stop.load(Ordering::Relaxed) {
                let _ = udp.send_to(&proto::encode_beacon(&beacon), ("127.0.0.1", port));
                std::thread::sleep(Duration::from_millis(200));
            }
        })
    };
    until(&p, "the computer to be heard waiting", |v| {
        peer(v, NodeId(0xbeef)).is_some_and(|x| x.waiting)
    });
    p.ask(NetAction::Adopt(NodeId(0xbeef)));
    let asked = Instant::now();
    let v = until(&p, "the row to say why", |v| {
        peer(v, NodeId(0xbeef)).is_some_and(|x| x.problem.is_some())
    });
    assert!(asked.elapsed() >= ADOPT_PATIENCE - Duration::from_millis(500));
    let x = peer(&v, NodeId(0xbeef)).unwrap();
    assert!(x.adopting, "still being tried");
    let said = x.problem.clone().unwrap();
    assert!(
        said.starts_with("Can't reach WNO-2: tried ")
            && said.contains(&format!("127.0.0.1:{closed} (this computer)"))
            && said.contains("Windows Firewall"),
        "{said}"
    );
    stop.store(true, Ordering::Relaxed);
    beaconing.join().unwrap();
}

#[test]
fn the_row_names_the_firewall_only_on_windows_and_says_when_nothing_was_tried() {
    let at: SocketAddr = "10.0.0.5:47820".parse().unwrap();
    let s = unreached_sentence("A", Some(at), None, true, "macos");
    assert_eq!(s, "A at 10.0.0.5:47820 hasn't answered yet.");
    let s = unreached_sentence("A", Some(at), None, false, "");
    assert_eq!(s, "A isn't heard on the network any more.");
    let s = unreached_sentence("A", None, Some("x (wired): refused"), true, "linux");
    assert_eq!(s, "Can't reach A: tried x (wired): refused.");
}

#[test]
fn a_port_changing_once_is_a_restart_back_and_forth_is_two_newqs() {
    let now = Instant::now();
    let seq =
        |ports: &[u16]| -> VecDeque<(u16, Instant)> { ports.iter().map(|p| (*p, now)).collect() };
    assert_eq!(twin_ports(&seq(&[1, 1, 1, 2, 2, 2]), now), None);
    assert_eq!(twin_ports(&seq(&[1, 2, 1, 2]), now), Some((1, 2)));
    assert_eq!(twin_ports(&seq(&[2, 1, 1, 2, 2, 1]), now), Some((1, 2)));
    // Old ones don't count.
    let old: VecDeque<(u16, Instant)> = [1, 2, 1, 2]
        .iter()
        .map(|p| (*p, now.checked_sub(TWIN_WINDOW * 2).unwrap_or(now)))
        .collect();
    if now.checked_sub(TWIN_WINDOW * 2).is_some() {
        assert_eq!(twin_ports(&old, now), None);
    }
}

/// Another NewQ with this computer's id, heard (as on a LAN) from each of its
/// addresses in turn: still noticed, counted by its port.
#[test]
fn this_computers_identity_from_another_port_is_noticed_whatever_address_it_comes_from() {
    let mut rig = Rig::new();
    let h = rig.hub("Booth");
    h.ask(NetAction::SetOn(true));
    let v = until(&h, "listening", |v| v.port.is_some());
    let port = v.port.unwrap();
    let other = if port == rig.base { port + 1 } else { port - 1 };
    let beacon = Beacon {
        proto: proto::PROTOCOL,
        version: proto::VERSION.into(),
        id: v.me.id,
        name: "Booth".into(),
        role: Role::Alone,
        network: None,
        tcp_port: other,
        show_version: None,
        term: 0,
        on: true,
        system: String::new(),
    };
    let data = proto::encode_beacon(&beacon);
    let socks: Vec<UdpSocket> = ["127.0.0.1", "127.0.0.2"]
        .iter()
        .filter_map(|a| UdpSocket::bind((*a, 0)).ok())
        .collect();
    assert!(!socks.is_empty());
    let began = Instant::now();
    let mut k = 0;
    loop {
        let _ = socks[k % socks.len()].send_to(&data, ("127.0.0.1", port));
        k += 1;
        let v = h.view();
        if let Some(t) = &v.twin {
            assert!(t.contains(&other.to_string()), "{t}");
            break;
        }
        assert!(began.elapsed() < Duration::from_secs(60), "{v:#?}");
        std::thread::sleep(Duration::from_millis(100));
    }
    // Its own beacons (its own port) never count.
    drop(socks);
    until(&h, "the warning to go once the other is quiet", |v| {
        v.twin.is_none()
    });
}
