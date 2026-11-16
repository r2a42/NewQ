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
        }],
        audio_devices: vec!["Speakers".into(), "USB interface".into()],
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
