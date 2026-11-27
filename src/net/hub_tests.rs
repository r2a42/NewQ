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
    assert_eq!(
        bv.took_control_from_id,
        Some(pid),
        "who from, for the sound left with it"
    );
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

#[test]
fn kinds_a_later_newq_adds_are_passed_by_and_the_session_stays_up() {
    use crate::net::measure::NetTest;
    use crate::net::play::{Command, PlayState, RunNow, RunWord, Timed};
    let mut rig = Rig::new();
    let (p, d) = primary_and_display(&mut rig);
    let (pid, did) = (id_of(&p), id_of(&d));
    let _ = (d.take_events(), p.take_events());
    // A network test message, a command and a heartbeat's run of kinds this
    // NewQ doesn't know (written as a later one would).
    p.broadcast_json(r#"{"t":"net_test","nt":"something_later","run":3,"extra":[1,2]}"#);
    let play = Msg::Play(Box::new(Timed {
        epoch: 1,
        seq: 5,
        at: 2.0,
        cmd: Command::Panic,
    }));
    let play = serde_json::to_string(&play)
        .unwrap()
        .replace(r#""do":"panic""#, r#""do":"a_later_command","n":4"#);
    assert!(play.contains("a_later_command"), "{play}");
    p.broadcast_json(&play);
    let state_msg = |state: RunWord| {
        Msg::PlayState(Box::new(PlayState {
            runs: vec![RunNow {
                key: crate::timeline::TimelineKey::List(1),
                state,
                pos: 1.0,
                anchor: None,
            }],
            ..PlayState::default()
        }))
    };
    let state = serde_json::to_string(&state_msg(RunWord::Running))
        .unwrap()
        .replace(r#""state":"running""#, r#""state":"a_later_state""#);
    assert!(state.contains("a_later_state"), "{state}");
    p.broadcast_json(&state);
    // A known command and heartbeat carrying a later kind of timeline and of
    // freeze (`TimelineKey::UNKNOWN`, `PauseFreeze::Nothing`).
    let pause = Msg::Play(Box::new(Timed {
        epoch: 1,
        seq: 6,
        at: 2.5,
        cmd: Command::TimelinePause {
            key: crate::timeline::TimelineKey::List(1),
            freeze: crate::timeline::PauseFreeze::Clips,
            clips: vec![],
            live: false,
            waits_for_go: false,
        },
    }));
    let later_key = |json: String| json.replace(r#"{"List":1}"#, r#"{"Reel":{"n":1}}"#);
    let pause = later_key(serde_json::to_string(&pause).unwrap()).replace("Clips", "Melt");
    assert!(pause.contains("Reel") && pause.contains("Melt"), "{pause}");
    p.broadcast_json(&pause);
    let state2 = later_key(serde_json::to_string(&state_msg(RunWord::Paused)).unwrap());
    assert!(state2.contains("Reel"), "{state2}");
    p.broadcast_json(&state2);
    // Then a message it knows, on the same session: a frame that couldn't be
    // read would have closed it, and this one would be lost with it.
    p.broadcast(Msg::NetTest(Box::new(NetTest::End { run: 3 })));
    let began = Instant::now();
    let mut got = Vec::new();
    while began.elapsed() < Duration::from_secs(60)
        && !got
            .iter()
            .any(|m| matches!(m, Msg::NetTest(t) if **t == NetTest::End { run: 3 }))
    {
        for ev in d.take_events() {
            if let NetEvent::Message { from, msg } = ev {
                assert_eq!(from, pid);
                got.push(msg);
            }
        }
        assert!(
            peer(&p.view(), did).is_some_and(|x| x.connected),
            "the session stayed up"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    let unknown = Msg::NetTest(Box::new(NetTest::Unknown));
    let i = got
        .iter()
        .position(|m| *m == unknown)
        .expect("passed on, unknown");
    let Some(Msg::Play(t)) = got.get(i + 1) else {
        panic!("the command after it: {got:?}");
    };
    assert_eq!((t.seq, &t.cmd), (5, &Command::Unknown));
    let Some(Msg::PlayState(s)) = got.get(i + 2) else {
        panic!("the heartbeat after it: {got:?}");
    };
    assert_eq!(s.runs[0].state, RunWord::Unknown);
    let Some(Msg::Play(t)) = got.get(i + 3) else {
        panic!("the pause after it: {got:?}");
    };
    assert!(matches!(
        t.cmd,
        Command::TimelinePause {
            key: crate::timeline::TimelineKey::UNKNOWN,
            freeze: crate::timeline::PauseFreeze::Nothing,
            ..
        }
    ));
    let Some(Msg::PlayState(s)) = got.get(i + 4) else {
        panic!("the heartbeat after it: {got:?}");
    };
    assert_eq!(s.runs[0].key, crate::timeline::TimelineKey::UNKNOWN);
    assert_eq!(
        got.get(i + 5),
        Some(&Msg::NetTest(Box::new(NetTest::End { run: 3 })))
    );
    // Done by a follower's engine, the unknown command changes nothing.
    let show = crate::show::Show::default();
    let mut e = crate::engine::Engine::for_tests();
    e.net_apply(&show, &t.cmd, 0.0);
    assert!(!e.is_active());
}

/// This computer's `node.json` copied to another computer: the copy beacons
/// with this id and the same port, but from an address that isn't this
/// computer's, and is told apart from this computer's own beacons coming back
/// (from loopback or one of its addresses).
#[test]
fn this_computers_identity_from_another_computer_on_the_same_port_is_a_twin() {
    let mut rig = Rig::new();
    let (tx, _rx) = crossbeam_channel::unbounded();
    let (out, _events) = crossbeam_channel::unbounded();
    let me = Identity {
        on: true,
        ..Identity::default()
    };
    let mut st = State::new(rig.cfg("clone"), me, tx, out);
    st.open(Instant::now());
    let port = st.net.as_ref().expect("listening").port;
    let ours = Ipv4Addr::new(10, 1, 2, 3);
    st.adapters = vec![super::super::paths::Adapter::new(
        "eth0",
        ours,
        Ipv4Addr::new(255, 255, 255, 0),
    )];
    let beacon = Beacon {
        proto: proto::PROTOCOL,
        version: proto::VERSION.into(),
        id: st.me.id,
        name: "Booth".into(),
        role: Role::Alone,
        network: None,
        tcp_port: port,
        show_version: None,
        term: 0,
        on: true,
        system: String::new(),
    };
    let hear = |st: &mut State, ip: Ipv4Addr| {
        for _ in 0..5 {
            st.heard(
                discover::Heard {
                    beacon: beacon.clone(),
                    from: SocketAddr::new(IpAddr::V4(ip), port),
                },
                Instant::now(),
            );
        }
    };
    // Its own beacons come back from loopback and from its own address.
    hear(&mut st, Ipv4Addr::LOCALHOST);
    hear(&mut st, ours);
    assert_eq!(st.view().twin, None);
    // The copy, elsewhere.
    hear(&mut st, Ipv4Addr::new(192, 0, 2, 7));
    let twin = st.view().twin.expect("the copy is noticed");
    assert!(twin.contains("192.0.2.7"), "{twin}");
    assert!(from_here(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)), &[]));
    assert!(!from_here(IpAddr::V4(ours), &[]));
}

// ---- Connection limits (review 2026-11-26: Fable R9, Opus R6) ----

/// The frames that come on `s` until it closes (or `limit` passes with
/// nothing more).
fn frames_until_closed(s: &mut TcpStream, limit: Duration) -> Vec<Msg> {
    s.set_read_timeout(Some(limit)).unwrap();
    let mut seen = Vec::new();
    while let Ok(body) = proto::read_frame(s) {
        match proto::decode(&body) {
            Ok(m) => seen.push(m),
            Err(_) => break,
        }
    }
    seen
}

/// A hello from a program that isn't a NewQ, as `id`.
fn raw_hello(id: NodeId, role: Role, network: Option<node::NetworkId>, name: String) -> Msg {
    Msg::Hello(Hello {
        proto: proto::PROTOCOL,
        version: proto::VERSION.into(),
        id,
        name,
        role,
        network,
        term: 0,
        nonce: 1,
        tcp_port: 0,
    })
}

/// A first frame over `HELLO_MAX` (a real hello padded with a long name)
/// closes the connection unread: no passcode proof comes back, as it does for
/// the same hello a little shorter.
#[test]
fn an_oversized_first_frame_closes_the_connection_unread() {
    let mut rig = Rig::new();
    let p = rig.hub("Booth");
    p.ask(NetAction::BecomePrimary);
    let port = until(&p, "listening", |v| v.port.is_some()).port.unwrap();
    let say = |name: String| {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let hello = raw_hello(NodeId(node::random_u64()), Role::Alone, None, name);
        proto::write_frame(&mut s, &hello).unwrap();
        s
    };
    // Just under: it is answered (the hub's hello, then its proof).
    let mut s = say("x".repeat(proto::HELLO_MAX - 400));
    s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let mut answered = false;
    while let Ok(body) = proto::read_frame(&mut s) {
        if matches!(proto::decode(&body), Ok(Msg::Prove { .. })) {
            answered = true;
            break;
        }
    }
    assert!(answered, "a hello under the limit is answered");
    drop(s);
    // Over: closed with nothing but the hub's own hello sent.
    let mut s = say("x".repeat(proto::HELLO_MAX + 1));
    let seen = frames_until_closed(&mut s, Duration::from_secs(60));
    assert!(
        seen.iter().all(|m| matches!(m, Msg::Hello(_))),
        "nothing but the hub's hello: {seen:?}"
    );
    // A huge length first is closed the same way, and the hub works on.
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    use std::io::Write;
    s.write_all(&((proto::MAX_FRAME - 1) as u32).to_be_bytes())
        .unwrap();
    let seen = frames_until_closed(&mut s, Duration::from_secs(60));
    assert!(seen.iter().all(|m| matches!(m, Msg::Hello(_))), "{seen:?}");
    let d = rig.hub("Left");
    d.ask(NetAction::SetOn(true));
    let did = id_of(&d);
    until(&p, "a real computer after the oversized frames", |v| {
        peer(v, did).is_some_and(|x| x.connected)
    });
}

/// The pure rule: at most `UNNAMED_MAX` connections that haven't said hello,
/// `UNNAMED_PER_ADDRESS` from one address.
#[test]
fn connections_that_havent_said_hello_are_capped_in_all_and_per_address() {
    let ip = |n: u8| IpAddr::V4(Ipv4Addr::new(10, 0, 0, n));
    assert!(admits(std::iter::empty(), ip(1)));
    let same = [ip(1); UNNAMED_PER_ADDRESS];
    assert!(!admits(same.iter().copied(), ip(1)));
    assert!(admits(same.iter().copied(), ip(2)));
    assert!(admits(same[1..].iter().copied(), ip(1)));
    let spread: Vec<IpAddr> = (0..UNNAMED_MAX as u8).map(|n| ip(n + 10)).collect();
    assert!(!admits(spread.iter().copied(), ip(1)));
    assert!(admits(spread[1..].iter().copied(), ip(1)));
}

/// Many silent connections from one address: only `UNNAMED_PER_ADDRESS` are
/// let in (sessions started); the rest are shut as they come, with nothing
/// sent and no threads. The hub's own state is driven here, so nothing
/// depends on how quickly the connections were made.
#[test]
fn many_silent_connections_are_capped() {
    let mut rig = Rig::new();
    let (tx, rx) = crossbeam_channel::unbounded();
    let (out, _events) = crossbeam_channel::unbounded();
    let me = Identity {
        on: true,
        ..Identity::default()
    };
    let mut st = State::new(rig.cfg("capped"), me, tx, out);
    st.open(Instant::now());
    let port = st.net.as_ref().expect("listening").port;
    let n = UNNAMED_PER_ADDRESS + 8;
    let mut socks: Vec<TcpStream> = (0..n)
        .map(|_| TcpStream::connect(("127.0.0.1", port)).unwrap())
        .collect();
    let mut accepted = 0;
    let began = Instant::now();
    while accepted < n {
        assert!(
            began.elapsed() < Duration::from_secs(60),
            "accepted {accepted}"
        );
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(100))
            && matches!(ev, Event::Accepted(_))
        {
            accepted += 1;
            st.handle(ev);
        }
    }
    assert_eq!(st.conns.len(), UNNAMED_PER_ADDRESS);
    // Those let in were sent the hub's hello; the others nothing, and closed.
    let greeted = socks
        .iter_mut()
        .map(|s| {
            s.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
            proto::read_frame(s).is_ok()
        })
        .filter(|g| *g)
        .count();
    assert_eq!(greeted, UNNAMED_PER_ADDRESS);
    // One that said hello no longer counts: another is let in.
    let first = *st.conns.keys().min().unwrap();
    st.conns.get_mut(&first).unwrap().hello =
        match raw_hello(NodeId(3), Role::Alone, None, "Left".into()) {
            Msg::Hello(h) => Some(h),
            _ => unreachable!(),
        };
    let _more = TcpStream::connect(("127.0.0.1", port)).unwrap();
    loop {
        assert!(began.elapsed() < Duration::from_secs(60));
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(100))
            && matches!(ev, Event::Accepted(_))
        {
            st.handle(ev);
            break;
        }
    }
    assert_eq!(st.conns.len(), UNNAMED_PER_ADDRESS + 1);
}

// ---- Who is heard (review 2026-11-26: Fable R3, Opus R1) ----

/// A program on the LAN that isn't a NewQ, speaking the show network's
/// protocol by hand: what comes to it is collected; it keeps its session
/// alive with pings until it is closed.
struct Stranger {
    stream: TcpStream,
    seen: Arc<Mutex<Vec<Msg>>>,
    closed: Arc<AtomicBool>,
}

impl Stranger {
    fn connect(port: u16, hello: Msg) -> Stranger {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        proto::write_frame(&mut stream, &hello).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let closed = Arc::new(AtomicBool::new(false));
        {
            let mut r = stream.try_clone().unwrap();
            let (seen, closed) = (seen.clone(), closed.clone());
            std::thread::spawn(move || {
                while let Ok(body) = proto::read_frame(&mut r) {
                    match proto::decode(&body) {
                        Ok(m) => seen.lock().push(m),
                        Err(_) => break,
                    }
                }
                closed.store(true, Ordering::Relaxed);
            });
        }
        {
            let mut w = stream.try_clone().unwrap();
            let closed = closed.clone();
            std::thread::spawn(move || {
                let mut n = 1_000_000;
                while !closed.load(Ordering::Relaxed) {
                    n += 1;
                    if proto::write_frame(&mut w, &Msg::Ping { n }).is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
            });
        }
        Stranger {
            stream,
            seen,
            closed,
        }
    }

    fn say(&mut self, m: &Msg) {
        let _ = proto::write_frame(&mut self.stream, m);
    }

    /// Wait until `f` holds of what came (a minute at most).
    fn until(&self, what: &str, f: impl Fn(&[Msg], bool) -> bool) {
        let began = Instant::now();
        loop {
            if f(&self.seen.lock(), self.closed.load(Ordering::Relaxed)) {
                return;
            }
            assert!(
                began.elapsed() < Duration::from_secs(60),
                "waited a minute for {what}; it saw {:?}",
                self.seen.lock()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Every order a primary gives a follower, then a ping: once its pong
    /// comes, the hub has read everything before it.
    fn give_orders_and_wait(&mut self) {
        let part = crate::net::play::ShowPart {
            version: 99,
            hash: 1,
            part: 0,
            parts: 1,
            data: "{}".into(),
        };
        self.say(&Msg::ShowPart(Box::new(part)));
        self.say(&Msg::Restart);
        self.say(&Msg::StartAtLogin { on: true });
        self.say(&Msg::MediaVerify);
        self.say(&Msg::Ping { n: 7 });
        self.until("the pong after the orders", |seen, closed| {
            closed || seen.iter().any(|m| matches!(m, Msg::Pong { n: 7 }))
        });
    }
}

/// What the app was given from `from` (none but those of `from`).
fn messages_from(h: &Hub, from: NodeId) -> Vec<Msg> {
    h.take_events()
        .into_iter()
        .filter_map(|e| match e {
            NetEvent::Message { from: f, msg } if f == from => Some(msg),
            _ => None,
        })
        .collect()
}

/// A display whose primary has gone: a stranger says hello with the primary's
/// id (from its beacons), says `Welcome`, and gives orders. None reaches the
/// app, with or without a passcode, whether it says it is a display or (with
/// a passcode, and no proof) the primary. The real primary, back, is heard
/// again at once: the stranger never held its place.
#[test]
fn a_stranger_with_the_primarys_id_is_not_heard_and_the_real_primary_is() {
    for passcode in [false, true] {
        let mut rig = Rig::new();
        let (p, d) = primary_and_display(&mut rig);
        let (pid, did) = (id_of(&p), id_of(&d));
        if passcode {
            p.ask(NetAction::SetPasscode(Some("opening night".into())));
            until(&d, "the key", |v| v.me.passcode.is_some());
        }
        let network = p.view().me.network;
        let dport = d.view().port.unwrap();
        let pdir = rig.dirs[0].clone();
        drop(p);
        until(&d, "the primary gone", |v| {
            v.following.as_ref().is_some_and(|f| !f.online)
        });
        let _ = d.take_events();
        let mut claims = vec![Role::Display, Role::Alone];
        if passcode {
            // With a passcode, claiming to be the primary needs its proof.
            claims.push(Role::Primary);
        }
        let mut strangers = Vec::new();
        for role in claims {
            let mut s = Stranger::connect(dport, raw_hello(pid, role, network, "Booth".into()));
            // (With a passcode the display keeps its proof back until the
            // other side has proved it.)
            s.until("the display's hello", |seen, _| {
                seen.iter().any(|m| matches!(m, Msg::Hello(_)))
            });
            s.say(&Msg::Prove { mac: None });
            s.say(&Msg::Welcome);
            s.give_orders_and_wait();
            let heard = messages_from(&d, pid);
            assert!(
                heard.is_empty(),
                "passcode {passcode}, claimed {role:?}: the app heard {heard:?}"
            );
            let v = d.view();
            assert_eq!(v.me.primary, Some(pid));
            assert_eq!(v.me.role, Role::Display);
            assert!(v.following.as_ref().is_some_and(|f| !f.online), "{v:#?}");
            strangers.push(s);
        }
        // The primary starts again: it reaches the display though strangers
        // hold sessions as it, they are closed, and its orders are heard.
        let p = restart(pdir, rig.base);
        until(&d, "the real primary back", |v| {
            v.following.as_ref().is_some_and(|f| f.online)
        });
        for s in &strangers {
            s.until("the stranger closed", |_, closed| closed);
        }
        until(&p, "the display back", |v| {
            peer(v, did).is_some_and(|x| x.connected && x.ours)
        });
        let _ = d.take_events();
        p.ask(NetAction::Restart(did));
        let began = Instant::now();
        loop {
            let heard = messages_from(&d, pid);
            if heard.iter().any(|m| matches!(m, Msg::Restart)) {
                break;
            }
            assert!(heard.is_empty(), "{heard:?}");
            assert!(
                began.elapsed() < Duration::from_secs(60),
                "the real primary's order never came (passcode {passcode})"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// A primary with a passcode: a stranger says it is one of its displays (the
/// id from the beacons) and `Welcome`, with no proof. It is sent no show, no
/// command, no order, and is closed with the passcode refusal.
#[test]
fn a_stranger_with_a_displays_id_is_sent_nothing_by_a_primary_with_a_passcode() {
    let mut rig = Rig::new();
    let (p, d) = primary_and_display(&mut rig);
    let did = id_of(&d);
    p.ask(NetAction::SetPasscode(Some("opening night".into())));
    until(&d, "the key", |v| v.me.passcode.is_some());
    let network = p.view().me.network;
    let pport = p.view().port.unwrap();
    drop(d);
    until(&p, "the display gone", |v| {
        peer(v, did).is_some_and(|x| !x.connected)
    });
    let mut s = Stranger::connect(pport, raw_hello(did, Role::Display, network, "Left".into()));
    s.until("the primary's proof request", |seen, _| {
        seen.iter().any(|m| matches!(m, Msg::Prove { .. }))
    });
    s.say(&Msg::Welcome);
    s.say(&Msg::Ping { n: 7 });
    s.until("the welcome read", |seen, _| {
        seen.iter().any(|m| matches!(m, Msg::Pong { n: 7 }))
    });
    let part = crate::net::play::ShowPart {
        version: 1,
        hash: 2,
        part: 0,
        parts: 1,
        data: "{}".into(),
    };
    p.tell(None, Msg::ShowPart(Box::new(part.clone())), false);
    p.tell(Some(did), Msg::ShowPart(Box::new(part)), true);
    p.send_to(did, Msg::MediaVerify);
    p.broadcast(Msg::MediaVerify);
    p.ask(NetAction::Restart(did));
    p.ask(NetAction::Identify(did));
    // Not proved within `PROVE_WITHIN`: refused, and closed.
    s.until("the refusal", |_, closed| closed);
    let seen = s.seen.lock().clone();
    assert!(
        seen.iter().any(|m| matches!(
            m,
            Msg::Refused {
                why: Refusal::Passcode,
                ..
            }
        )),
        "{seen:?}"
    );
    let sent: Vec<&Msg> = seen
        .iter()
        .filter(|m| {
            !matches!(
                m,
                Msg::Hello(_)
                    | Msg::Prove { .. }
                    | Msg::Ping { .. }
                    | Msg::Pong { .. }
                    | Msg::Refused { .. }
            )
        })
        .collect();
    assert!(sent.is_empty(), "the stranger was sent {sent:?}");
    assert!(
        peer(&p.view(), did).is_some_and(|x| !x.connected),
        "never shown connected"
    );
}

/// The passcode set before anything is added: the display and the backup are
/// added, the primary's orders and a show piece of 256 KB of JSON (quotes
/// that double when sent) reach the display; the backup takes control and is
/// heard in turn; the old primary comes back as the backup of the new one.
#[test]
fn with_a_passcode_from_the_start_adding_orders_and_taking_control_work() {
    let mut rig = Rig::new();
    let p = rig.hub("Booth");
    p.ask(NetAction::BecomePrimary);
    until(&p, "primary", |v| v.me.role == Role::Primary);
    p.ask(NetAction::SetPasscode(Some("opening night".into())));
    until(&p, "the passcode", |v| v.me.passcode.is_some());
    let d = rig.hub("Left");
    let b = rig.hub("Backup");
    let (pid, did, bid) = (id_of(&p), id_of(&d), id_of(&b));
    for (h, id) in [(&d, did), (&b, bid)] {
        h.ask(NetAction::SetOn(true));
        until(&p, "heard", |v| {
            peer(v, id).is_some_and(|x| x.waiting && x.connected)
        });
        p.ask(NetAction::Adopt(id));
        until(&p, "added and reporting", |v| {
            peer(v, id).is_some_and(|x| x.ours && x.connected && x.status.is_some())
        });
    }
    p.ask(NetAction::SetRole(bid, Role::Backup));
    until(&b, "the backup role", |v| v.me.role == Role::Backup);
    let heard = |from: &Hub, to: &Hub, fid: NodeId, tid: NodeId, what: &str| {
        let _ = to.take_events();
        from.ask(NetAction::Restart(tid));
        let began = Instant::now();
        loop {
            if messages_from(to, fid)
                .iter()
                .any(|m| matches!(m, Msg::Restart))
            {
                break;
            }
            assert!(began.elapsed() < Duration::from_secs(60), "{what}");
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    heard(&p, &d, pid, did, "the primary's order to the display");
    // A big piece, as `show_parts` sends.
    let part = crate::net::play::ShowPart {
        version: 1,
        hash: 2,
        part: 0,
        parts: 1,
        data: "\"".repeat(crate::net::play::SHOW_PART),
    };
    let _ = d.take_events();
    p.tell(Some(did), Msg::ShowPart(Box::new(part.clone())), false);
    let began = Instant::now();
    loop {
        if messages_from(&d, pid)
            .iter()
            .any(|m| matches!(m, Msg::ShowPart(x) if **x == part))
        {
            break;
        }
        assert!(began.elapsed() < Duration::from_secs(60), "the big piece");
        std::thread::sleep(Duration::from_millis(20));
    }
    // The primary goes; the backup takes control.
    let pdir = rig.dirs[0].clone();
    drop(p);
    until(&b, "the primary gone", |v| {
        v.following.as_ref().is_some_and(|f| !f.online)
    });
    b.ask(NetAction::TakeControl);
    until(&d, "the display following the backup", |v| {
        v.following
            .as_ref()
            .is_some_and(|f| f.id == bid && f.online)
    });
    until(&b, "the display under the new primary", |v| {
        peer(v, did).is_some_and(|x| x.connected && x.ours)
    });
    heard(&b, &d, bid, did, "the new primary's order");
    let p = restart(pdir, rig.base);
    until(&p, "the old primary as the backup", |v| {
        v.me.role == Role::Backup
            && v.following
                .as_ref()
                .is_some_and(|f| f.id == bid && f.online)
    });
    until(&b, "the old primary under the new", |v| {
        peer(v, pid).is_some_and(|x| x.connected && x.role == Role::Backup)
    });
    heard(&b, &p, bid, pid, "the new primary's order to the old");
}

/// A relay: a stranger takes the display's nonce from its hello and says
/// hello to the backup with it, to have the backup sign it. The backup (a
/// computer with the key that isn't a primary) proves the passcode only to a
/// peer that proved it first: the stranger gets no proof to pass on.
#[test]
fn a_computer_with_the_key_proves_it_only_to_a_peer_that_proved_it_first() {
    let mut rig = Rig::new();
    let (p, d, b) = primary_display_backup(&mut rig);
    p.ask(NetAction::SetPasscode(Some("opening night".into())));
    until(&d, "the key", |v| v.me.passcode.is_some());
    until(&b, "the key", |v| v.me.passcode.is_some());
    let network = p.view().me.network;
    let (dport, bport) = (d.view().port.unwrap(), b.view().port.unwrap());
    let sd = Stranger::connect(
        dport,
        raw_hello(
            NodeId(node::random_u64()),
            Role::Display,
            network,
            "X".into(),
        ),
    );
    sd.until("the display's hello", |seen, _| {
        seen.iter().any(|m| matches!(m, Msg::Hello(_)))
    });
    let nonce = sd
        .seen
        .lock()
        .iter()
        .find_map(|m| match m {
            Msg::Hello(h) => Some(h.nonce),
            _ => None,
        })
        .unwrap();
    let Msg::Hello(mut h) = raw_hello(
        NodeId(node::random_u64()),
        Role::Display,
        network,
        "Y".into(),
    ) else {
        unreachable!()
    };
    h.nonce = nonce;
    let mut sb = Stranger::connect(bport, Msg::Hello(h));
    sb.say(&Msg::Ping { n: 7 });
    sb.until("the backup's pong", |seen, closed| {
        closed || seen.iter().any(|m| matches!(m, Msg::Pong { n: 7 }))
    });
    let seen = sb.seen.lock().clone();
    assert!(
        !seen
            .iter()
            .any(|m| matches!(m, Msg::Prove { mac: Some(_) })),
        "the backup signed a stranger's nonce: {seen:?}"
    );
    // The real ones still prove to each other: the display and the backup
    // follow the primary as before.
    assert!(d.view().following.as_ref().is_some_and(|f| f.online));
    assert!(b.view().following.as_ref().is_some_and(|f| f.online));
}

// ---- Line-up across the network (mapping plan 4.4, `net::lineup`) ----

mod line_up {
    use super::*;
    use crate::net::lineup::{
        LEASE, LineUpMsg, Link, Mark, Overlay, Route, SEEN_WITHIN, Sender, Shown, Source, Target,
        Under, route,
    };
    use crate::show::OutputId;

    const D: OutputId = OutputId(3);
    const MONITOR: (u32, u32) = (1920, 1080);
    const REPEAT_AFTER: Duration = Duration::from_secs(2);

    fn want(x: f32) -> Overlay {
        Overlay {
            session: 0,
            seq: 0,
            display: D,
            pixels: MONITOR,
            marks: vec![Mark {
                n: 1,
                at: [x, 200.0],
                on: true,
            }],
            active: Some(0),
            outline: None,
            under: Under::Black,
        }
    }

    /// The follower's side, as `app/lineup.rs` has it: each line-up message
    /// the hub hands on, routed by `lineup::route` with `play_message`'s
    /// `primary` (from the primary it follows), taken by its `Shown` at `now`
    /// (a clock the test owns), and the `Seen` told back.
    fn follower_takes(d: &Hub, shown: &mut Shown, now: Instant) {
        for ev in d.take_events() {
            let NetEvent::Message {
                from,
                msg: Msg::LineUp(m),
            } = ev
            else {
                continue;
            };
            let v = d.view();
            let primary = v.me.primary == Some(from) && v.me.adopted() && !v.is_primary();
            if route(&m, primary, v.is_primary()) == Route::Shown
                && let Some(seen) = shown.take(Source::Primary(from), *m, |_| Some(MONITOR), now)
            {
                d.tell(Some(from), Msg::LineUp(Box::new(seen)), true);
            }
        }
    }

    /// The primary's side: the `Seen`s that came back, heard by its sender.
    fn primary_hears(p: &Hub, sender: &mut Sender, now: Instant) -> usize {
        let mut n = 0;
        for ev in p.take_events() {
            if let NetEvent::Message {
                from,
                msg: Msg::LineUp(m),
            } = ev
                && route(&m, false, p.view().is_primary()) == Route::Sender
                && sender.heard(Target::Computer(from), &m, now)
            {
                n += 1;
            }
        }
        n
    }

    fn until_ok(what: &str, mut f: impl FnMut() -> bool) {
        let began = Instant::now();
        while !f() {
            assert!(
                began.elapsed() < Duration::from_secs(60),
                "waited a minute for {what}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Two hubs on loopback with a passcode: the primary's nudges reach the
    /// follower and its crosses follow; its `Seen` reaches the primary.
    #[test]
    fn the_crosses_move_on_the_follower_and_seen_comes_back() {
        let mut rig = Rig::new();
        let (p, d) = primary_and_display(&mut rig);
        let did = id_of(&d);
        p.ask(NetAction::SetPasscode(Some("opening night".into())));
        until(&d, "the key", |v| v.me.passcode.is_some());
        until(&p, "the display connected, proved", |v| {
            peer(v, did).is_some_and(|x| x.connected && x.ours)
        });
        let _ = (p.take_events(), d.take_events());
        let t0 = Instant::now();
        let mut sender = Sender::new(41, D, Target::Computer(did));
        let mut shown = Shown::default();
        for (k, x) in [100.0, 101.0, 102.5].into_iter().enumerate() {
            let now = t0 + Duration::from_millis(16 * k as u64);
            let m = sender.due(&want(x), None, 0, now).expect("a nudge is sent");
            p.tell(Some(did), Msg::LineUp(Box::new(m)), true);
            until_ok("the nudge on the follower", || {
                follower_takes(&d, &mut shown, now);
                shown.overlay().is_some_and(|o| o.marks[0].at[0] == x)
            });
        }
        let held = shown.overlay().unwrap();
        assert_eq!((held.session, held.seq, held.display), (41, 3, D));
        until_ok("the follower's Seen", || {
            primary_hears(&p, &mut sender, t0);
            sender.link(true, t0) == Link::Seen { pixels: MONITOR }
        });
        // `End`: the crosses go.
        p.tell(Some(did), Msg::LineUp(Box::new(sender.end())), true);
        until_ok("the end", || {
            follower_takes(&d, &mut shown, t0);
            shown.overlay().is_none()
        });
    }

    /// A stranger with the primary's id (the primary gone) sends line-up
    /// crosses: the hub hands none of it on, with or without a passcode. And
    /// the app's own rule passes by anything not from its primary.
    #[test]
    fn a_strangers_line_up_is_never_heard() {
        for passcode in [false, true] {
            let mut rig = Rig::new();
            let (p, d) = primary_and_display(&mut rig);
            let pid = id_of(&p);
            if passcode {
                p.ask(NetAction::SetPasscode(Some("opening night".into())));
                until(&d, "the key", |v| v.me.passcode.is_some());
            }
            let network = p.view().me.network;
            let dport = d.view().port.unwrap();
            drop(p);
            until(&d, "the primary gone", |v| {
                v.following.as_ref().is_some_and(|f| !f.online)
            });
            let _ = d.take_events();
            let mut roles = vec![Role::Display, Role::Alone];
            if passcode {
                roles.push(Role::Primary);
            }
            for role in roles {
                let mut s = Stranger::connect(dport, raw_hello(pid, role, network, "Booth".into()));
                s.until("the display's hello", |seen, _| {
                    seen.iter().any(|m| matches!(m, Msg::Hello(_)))
                });
                s.say(&Msg::Prove { mac: None });
                s.say(&Msg::Welcome);
                let mut o = want(5.0);
                o.session = 66;
                o.seq = 1;
                s.say(&Msg::LineUp(Box::new(LineUpMsg::Show(Box::new(o)))));
                s.say(&Msg::LineUp(Box::new(LineUpMsg::End { session: 66 })));
                s.say(&Msg::Ping { n: 7 });
                s.until("the pong after the crosses", |seen, closed| {
                    closed || seen.iter().any(|m| matches!(m, Msg::Pong { n: 7 }))
                });
                let heard = messages_from(&d, pid);
                assert!(
                    heard.is_empty(),
                    "passcode {passcode}, claimed {role:?}: the app heard {heard:?}"
                );
            }
        }
        let show = LineUpMsg::Show(Box::new(want(1.0)));
        assert_eq!(route(&show, false, false), Route::Pass);
        assert_eq!(route(&show, false, true), Route::Pass);
    }

    /// A stranger claiming to be the primary's display, with no proof, says
    /// `Seen`: the primary with a passcode hands none of it on; and its
    /// sender takes a `Seen` only from the computer it is for.
    #[test]
    fn a_strangers_seen_is_never_taken_by_the_primary() {
        let mut rig = Rig::new();
        let (p, d) = primary_and_display(&mut rig);
        let did = id_of(&d);
        p.ask(NetAction::SetPasscode(Some("opening night".into())));
        until(&d, "the key", |v| v.me.passcode.is_some());
        let network = p.view().me.network;
        let pport = p.view().port.unwrap();
        drop(d);
        until(&p, "the display gone", |v| {
            peer(v, did).is_some_and(|x| !x.connected)
        });
        let _ = p.take_events();
        let mut s = Stranger::connect(pport, raw_hello(did, Role::Display, network, "Left".into()));
        s.until("the primary's proof request", |seen, _| {
            seen.iter().any(|m| matches!(m, Msg::Prove { .. }))
        });
        s.say(&Msg::Welcome);
        let seen = LineUpMsg::Seen {
            session: 41,
            seq: 1,
            pixels: MONITOR,
        };
        s.say(&Msg::LineUp(Box::new(seen.clone())));
        s.say(&Msg::Ping { n: 7 });
        s.until("the ping read", |seen, closed| {
            closed || seen.iter().any(|m| matches!(m, Msg::Pong { n: 7 }))
        });
        let heard = messages_from(&p, did);
        assert!(heard.is_empty(), "the primary heard {heard:?}");
        let t0 = Instant::now();
        let mut sender = Sender::new(41, D, Target::Computer(did));
        sender.due(&want(1.0), None, 0, t0);
        assert!(!sender.heard(Target::Computer(NodeId(did.0 ^ 1)), &seen, t0));
        assert!(sender.heard(Target::Computer(did), &seen, t0));
    }

    /// The primary goes mid line-up: on the follower's own clock the crosses
    /// go within the lease. The primary back, the next `Show` puts them up
    /// again; meanwhile the primary's sender says the link is lost.
    #[test]
    fn a_dropped_link_stops_the_crosses_within_the_lease() {
        let mut rig = Rig::new();
        let (p, d) = primary_and_display(&mut rig);
        let did = id_of(&d);
        let _ = (p.take_events(), d.take_events());
        let t0 = Instant::now();
        let mut sender = Sender::new(8, D, Target::Computer(did));
        let mut shown = Shown::default();
        let m = sender.due(&want(10.0), None, 0, t0).unwrap();
        p.tell(Some(did), Msg::LineUp(Box::new(m)), true);
        until_ok("the crosses up", || {
            follower_takes(&d, &mut shown, t0);
            shown.overlay().is_some()
        });
        let pdir = rig.dirs[0].clone();
        drop(p);
        until(&d, "the primary gone", |v| {
            v.following.as_ref().is_some_and(|f| !f.online)
        });
        follower_takes(&d, &mut shown, t0);
        assert!(!shown.tick(t0 + LEASE - Duration::from_millis(100)));
        assert!(shown.tick(t0 + LEASE + Duration::from_millis(1)));
        assert!(shown.overlay().is_none(), "never left on the projector");
        assert_eq!(sender.link(false, t0 + LEASE), Link::Lost);
        let p = restart(pdir, rig.base);
        until(&p, "the display back", |v| {
            peer(v, did).is_some_and(|x| x.connected && x.ours)
        });
        until(&d, "the primary back", |v| {
            v.following.as_ref().is_some_and(|f| f.online)
        });
        let _ = d.take_events();
        let later = t0 + LEASE + REPEAT_AFTER;
        let m = sender.due(&want(10.0), None, 0, later).expect("the repeat");
        p.tell(Some(did), Msg::LineUp(Box::new(m)), true);
        until_ok("the crosses back", || {
            follower_takes(&d, &mut shown, later);
            shown.overlay().is_some()
        });
    }

    /// An older peer: a later NewQ's line-up kind and a message kind this
    /// NewQ doesn't know are read and passed by, the session stays up, and the
    /// message after them arrives. A computer that doesn't answer line-up
    /// (one that reads `Msg::LineUp` as `Unknown`) is told apart from a lost
    /// link: connected, no `Seen`, it is `Older`, in words.
    #[test]
    fn an_older_peer_neither_closes_the_session_nor_leaves_the_primary_guessing() {
        let mut rig = Rig::new();
        let (p, d) = primary_and_display(&mut rig);
        let (pid, did) = (id_of(&p), id_of(&d));
        let _ = (p.take_events(), d.take_events());
        p.broadcast_json(r#"{"t":"line_up","k":"something_new","n":[1,2]}"#);
        p.broadcast_json(r#"{"t":"something_newer","line_up":true}"#);
        let mut o = want(3.0);
        o.session = 2;
        o.seq = 1;
        p.broadcast(Msg::LineUp(Box::new(LineUpMsg::Show(Box::new(o)))));
        let mut got = Vec::new();
        until_ok("the known one after them", || {
            got.extend(messages_from(&d, pid));
            got.iter()
                .any(|m| matches!(m, Msg::LineUp(l) if matches!(**l, LineUpMsg::Show(_))))
        });
        assert!(
            got.contains(&Msg::LineUp(Box::new(LineUpMsg::Unknown))),
            "{got:?}"
        );
        // (A message kind this NewQ doesn't know at all goes no further than the hub.)
        assert!(peer(&p.view(), did).is_some_and(|x| x.connected));
        // That display never answers: connected, nothing back after
        // `SEEN_WITHIN`, the primary says so.
        let t0 = Instant::now();
        let mut sender = Sender::new(2, D, Target::Computer(did));
        let m = sender.due(&want(3.0), None, 0, t0).unwrap();
        p.tell(Some(did), Msg::LineUp(Box::new(m)), true);
        let later = t0 + SEEN_WITHIN + Duration::from_millis(1);
        assert_eq!(primary_hears(&p, &mut sender, later), 0);
        let v = p.view();
        let x = peer(&v, did).unwrap();
        let link = sender.link(x.connected, later);
        assert_eq!(link, Link::Older);
        let words = crate::net::lineup::link_words(link, &x.name, "0.27.2", MONITOR).unwrap();
        assert!(words.contains("Left runs NewQ 0.27.2"), "{words}");
    }

    /// *Done* is one ordinary show edit: the line-up kept on the projector
    /// reaches the follower in the show the primary sends (its parts), read
    /// whole and still current there.
    #[test]
    fn done_arrives_as_a_show_edit() {
        use crate::net::play::{ShowAssembly, read_show, show_parts};
        use crate::stage3d::projector::{LinePoint, LineUp, Solved};
        let mut rig = Rig::new();
        let (p, d) = primary_and_display(&mut rig);
        let pid = id_of(&p);
        let _ = (p.take_events(), d.take_events());
        let mut show = crate::show::Show::default();
        let screen = show.add_surface(None);
        let id = show.add_projector();
        let display = show.outputs[0].id;
        let pr = show.projector_mut(id).unwrap();
        pr.display = Some(display);
        pr.mapped = true;
        pr.placement.position = [1.0, 2.0, 9.0];
        let mut line_up = LineUp {
            points: (0..4)
                .map(|i| LinePoint::on_screen(screen, [i as f32, 0.5, 0.0], [10.0 * i as f32, 5.0]))
                .collect(),
            pixels: MONITOR,
            ..LineUp::default()
        };
        line_up.solved = Some(Solved {
            points_key: line_up.points_key(),
            pixels: MONITOR,
            ..Solved::default()
        });
        pr.line_up = Some(line_up.clone());
        let json = serde_json::to_string(&show).unwrap();
        for part in show_parts(1, &json) {
            p.tell(None, Msg::ShowPart(Box::new(part)), false);
        }
        let mut asm = ShowAssembly::default();
        let mut whole = None;
        until_ok("the show on the follower", || {
            for m in messages_from(&d, pid) {
                if let Msg::ShowPart(part) = m
                    && let Some((_, _, json)) = asm.take(*part)
                {
                    whole = Some(json);
                }
            }
            whole.is_some()
        });
        let (there, _) = read_show(&whole.unwrap()).unwrap();
        let kept = there.stage3d.projectors[0].line_up.clone().unwrap();
        assert_eq!(kept, line_up);
        assert!(
            kept.is_current(),
            "the answer is for these points there too"
        );
        assert_eq!(
            there.stage3d.projectors[0].placement.position,
            [1.0, 2.0, 9.0]
        );
    }
}
