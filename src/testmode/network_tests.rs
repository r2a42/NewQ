//! The network test, without sockets: a primary with Test Mode and two
//! followers, each a whole `Engine` playing the test show's real clips, all on
//! one perfect shared clock. Commands and heartbeats go through `net::play`'s
//! `Leader` and `Follower` as in the app; show sync is a copy of the primary's
//! show to each connected follower; the network test's messages go between
//! `TestMode::net_out`/`net_in` and each follower's `Measurer`. A follower can
//! be made late (its commands delayed) or cut off.

use super::*;
use crate::engine::NetMode;
use crate::net::hub::NetView;
use crate::net::measure::{Measurer, NetTest, Sample};
use crate::net::media::MediaReport;
use crate::net::node::{Identity, NetworkId, Role};
use crate::net::play::{Follower, Leader, Out, PlayState, Timed, show_hash};
use crate::net::proto::{Link, MonitorReport, StatusReport};
use crate::testmode::TestMode;
use crate::testmode::assets::{Kind, Source};
use crate::testmode::session::View;

const DT: f64 = 1.0 / 60.0;
const LEAD: f64 = 0.05;
const PRIMARY: NodeId = NodeId(0x10);

fn media_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test_media")
}

fn ffmpeg() -> Option<Ffmpeg> {
    let ff = Ffmpeg::locate();
    let have = media_dir().join("testsrc_10s.mp4").is_file();
    if ff.is_none() || !have {
        assert!(
            std::env::var_os("NEWQ_REQUIRE_MEDIA").is_none(),
            "NEWQ_REQUIRE_MEDIA is set but ffmpeg or the test clips are missing"
        );
        return None;
    }
    ff
}

fn resolved(id: &'static str, file: &str, kind: Kind) -> (&'static str, Resolved) {
    let (duration, width, height, fps, ch) = match kind {
        Kind::Video => (8.0, 1280, 720, 30.0, 2),
        Kind::Image => (0.0, 640, 360, 0.0, 0),
        Kind::Audio => (5.0, 0, 0, 0.0, 2),
    };
    (
        id,
        Resolved {
            path: media_dir().join(file),
            source: Source::Downloaded,
            duration,
            width,
            height,
            fps,
            audio_channels: ch,
            codec: "h264".into(),
        },
    )
}

fn res() -> HashMap<&'static str, Resolved> {
    [
        resolved("bbb_60", "testsrc_10s.mp4", Kind::Video),
        resolved("sintel_24", "smpte_8s.mp4", Kind::Video),
        resolved("apollo11", "color_red.png", Kind::Image),
        resolved("sintel_sound", "tone_5s.wav", Kind::Audio),
    ]
    .into_iter()
    .collect()
}

fn monitor(name: &str) -> MonitorReport {
    MonitorReport {
        identity: name.into(),
        raw: name.into(),
        hardware: None,
        size: (1920, 1080),
        refresh_hz: Some(60.0),
        ..Default::default()
    }
}

fn peer(id: NodeId, name: &str, connected: bool, in_step: bool, media: Option<&str>) -> PeerView {
    PeerView {
        id,
        name: name.into(),
        role: Role::Display,
        version: crate::net::proto::VERSION.into(),
        compatible: true,
        network: Some(NetworkId(1)),
        ours: true,
        waiting: false,
        online: connected,
        connected,
        addr: None,
        status: Some(StatusReport {
            name: name.into(),
            role: Role::Display,
            link: Link::Wired,
            in_step,
            monitors: vec![monitor("Projector")],
            media_progress: media.map(|m| {
                let mut r = MediaReport::default();
                r.progress.manifest_id = Some(m.to_string());
                Box::new(r)
            }),
            ..Default::default()
        }),
        problem: None,
        adopting: false,
    }
}

/// The user's show: a display here, and a clip of their own.
fn user_show() -> Show {
    let mut show = Show::default();
    let c = show.make_cue(CueType::Video, Some(media_dir().join("own_clip.mov")), 0);
    show.cues.push(c);
    show.name = "Tonight".into();
    show
}

struct Comp {
    id: NodeId,
    name: &'static str,
    show: Show,
    has: Option<u64>,
    engine: Engine,
    follower: Follower,
    measure: Measurer,
    connected: bool,
    /// Commands on their way: when they arrive, and the command.
    wire: Vec<(f64, Timed)>,
    /// Extra delay on its commands (s).
    delay: f64,
    /// It ran the end of the test (its last report and log were sent).
    ended: bool,
    // ---- Every test across the network: what it measures of itself ----
    local: crate::net::measure_local::LocalMeter,
    /// The frame rate its output reports.
    fps: f32,
    /// The most cues it had playing at once.
    played: usize,
    next_row: f64,
}

struct Rig {
    show: Show,
    engine: Engine,
    st: UiState,
    test: TestMode,
    leader: Leader,
    comps: Vec<Comp>,
    now: f64,
    dir: PathBuf,
    user: String,
    /// The media version the primary serves (the test's once it is made).
    current: &'static str,
    /// The computers have the test's media (else it never comes).
    media_comes: bool,
    /// Each step takes as long as it stands for: a test that keeps its own
    /// time by the computer's clock (the cue tests' laps) sees it pass.
    paced: bool,
}

impl Rig {
    fn new(ff: &Ffmpeg, name: &str) -> Rig {
        let dir = std::env::temp_dir().join(format!("newq_nettest_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut engine = Engine::for_tests();
        engine.ff = Some(ff.clone());
        engine.net_set_mode(NetMode::Primary);
        let show = user_show();
        let user = serde_json::to_string(&show).unwrap();
        let comps = [(NodeId(0x20), "Left"), (NodeId(0x30), "Right")]
            .into_iter()
            .map(|(id, name)| {
                let mut e = Engine::for_tests();
                e.ff = Some(ff.clone());
                e.net_set_mode(NetMode::Follower);
                Comp {
                    id,
                    name,
                    show: Show::default(),
                    has: None,
                    engine: e,
                    follower: Follower::default(),
                    measure: Measurer::default(),
                    connected: true,
                    wire: Vec::new(),
                    delay: 0.0,
                    ended: false,
                    local: Default::default(),
                    fps: 60.0,
                    played: 0,
                    next_row: 0.0,
                }
            })
            .collect();
        let mut st = UiState::default();
        st.net = Arc::new(NetView {
            me: Identity {
                id: PRIMARY,
                role: Role::Primary,
                on: true,
                network: Some(NetworkId(1)),
                primary: Some(PRIMARY),
                ..Identity::fresh()
            },
            name: "Booth".into(),
            ..Default::default()
        });
        let mut r = Rig {
            show,
            engine,
            st,
            test: TestMode::default(),
            leader: Leader::new(5),
            comps,
            now: 1000.0,
            dir,
            user,
            current: "user",
            media_comes: true,
            paced: false,
        };
        // Into step before the test.
        for _ in 0..60 {
            r.step();
        }
        r
    }

    fn view(&mut self) {
        let peers = self
            .comps
            .iter()
            .map(|c| {
                peer(
                    c.id,
                    c.name,
                    c.connected,
                    c.follower.in_step,
                    (self.media_comes || self.current == "user").then_some(self.current),
                )
            })
            .collect();
        let mut v = (*self.st.net).clone();
        v.peers = peers;
        self.st.net = Arc::new(v);
        let mut m = (*self.st.media).clone();
        m.current = Some(self.current.to_string());
        self.st.media = Arc::new(m);
    }

    fn step(&mut self) {
        if self.paced {
            std::thread::sleep(std::time::Duration::from_secs_f64(DT));
        }
        self.now += DT;
        let now = self.now;
        self.view();
        // The primary.
        let json = serde_json::to_string(&self.show).unwrap();
        let hash = show_hash(&json);
        let to: Vec<NodeId> = self
            .comps
            .iter()
            .filter(|c| c.connected)
            .map(|c| c.id)
            .collect();
        let cmds = self.engine.net_take();
        let mut outs = self.leader.issue(cmds, now, LEAD, &to);
        outs.extend(self.leader.resend(now));
        self.leader.step(&mut self.engine, &self.show, now);
        if let Some(s) = self.leader.beat(&self.engine, hash, now) {
            outs.push(Out::State(s));
        }
        let mut beats: Vec<PlayState> = Vec::new();
        for o in outs {
            match o {
                Out::Command(who, t) => {
                    for c in self.comps.iter_mut().filter(|c| c.connected) {
                        if who.is_none_or(|w| w == c.id) {
                            c.wire.push((now + DT + c.delay, t.clone()));
                        }
                    }
                }
                Out::State(s) => beats.push(s),
            }
        }
        // Show sync, the test's messages and the followers.
        let out = std::mem::take(&mut self.test.net_out);
        for c in self.comps.iter_mut().filter(|c| c.connected) {
            if c.has != Some(hash) {
                c.show = serde_json::from_str(&json).unwrap();
                c.has = Some(hash);
            }
            for (who, m) in &out {
                if who.is_some_and(|w| w != c.id) {
                    continue;
                }
                match m {
                    NetTest::Begin { run, start } => {
                        c.measure.begin(*run, *start);
                        c.local.begin(*run, *start);
                    }
                    NetTest::End { run } => {
                        if let Some(r) = c.measure.end(*run, now, Link::Wired, Some(60.0)) {
                            self.test.net_in.push((c.id, NetTest::Report(Box::new(r))));
                        }
                        if let Some(r) = c.local.end(*run, now, Vec::new(), None, 8192) {
                            self.test.net_in.push((c.id, NetTest::Local(Box::new(r))));
                        }
                        self.test.net_in.push((
                            c.id,
                            NetTest::LogTail {
                                run: *run,
                                text: format!("{}'s log\n", c.name),
                            },
                        ));
                        c.ended = true;
                    }
                    _ => {}
                }
            }
            for s in &beats {
                if c.measure.running().is_some() && c.follower.has_done(s) {
                    let ours = c.engine.net_state(s.time);
                    c.measure.heartbeat(s, &ours);
                }
                c.follower.heard(s.clone());
            }
            let (due, later): (Vec<_>, Vec<_>) = c.wire.drain(..).partition(|(at, _)| *at <= now);
            c.wire = later;
            for (_, t) in due {
                c.measure.command(Some(now), t.seq, t.at);
                let (epoch, seq) = c.follower.receive(t, &mut c.engine, &c.show);
                self.leader.ack(c.id, epoch, seq);
            }
            c.follower
                .step(&mut c.engine, &c.show, Some(now), DT, c.has);
            c.measure
                .corrections(now, c.follower.corrections, &c.follower.corrected);
            c.measure.sample(
                now,
                Sample {
                    clock_error_ms: Some(0.05),
                    rtt_ms: Some(0.4),
                    locked: true,
                    in_step: c.follower.in_step,
                },
            );
            // Its screen draws at `fps` (as `App::update_outputs` measures).
            c.measure.drawing(&[crate::net::measure::MonitorRate {
                index: 0,
                name: "Projector".into(),
                refresh_hz: Some(60.0),
                fps: c.fps,
                late_permille: 0,
            }]);
            if let Some(r) = c
                .measure
                .report(now, Link::Wired, Some(60.0), Some(0), false)
            {
                self.test.net_in.push((c.id, NetTest::Report(Box::new(r))));
            }
            // What it measures of itself (made up but for what plays).
            c.played = c.played.max(c.engine.active.len());
            if c.local.running().is_some() && now >= c.next_row {
                c.next_row = now + 0.5;
                let row = crate::stats::Sample {
                    rss_mb: 300,
                    cpu_pct: 20.0,
                    ffmpeg: 2,
                    inputs: crate::stats::Inputs {
                        playing: c.engine.active.len(),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                c.local.sample(now, &row);
            }
            let rates = vec![crate::net::measure_local::OutputRate::new(
                &format!("{} 1", c.name),
                c.fps,
                Some(60.0),
            )];
            if let Some(r) = c
                .local
                .report(now, rates, Some("Speakers".into()), 8192, false)
            {
                self.test.net_in.push((c.id, NetTest::Local(Box::new(r))));
            }
        }
        // The test's own look.
        self.test
            .update_at(&mut self.show, &mut self.engine, &mut self.st, now);
    }

    /// Start the test with a short script and wait for it to be playing.
    fn start(&mut self) {
        let peers = self.st.net.peers.clone();
        let flash = media_dir().join("testsrc_10s.mp4");
        let built = build(&self.show, &res(), Some(&flash), &peers, &[]);
        let dir = self.dir.clone();
        self.test
            .launch_network(built, &dir, &mut self.show, &mut self.engine, &mut self.st)
            .expect("the network test starts");
        self.test.network_run_mut().unwrap().shorten(
            vec![
                (0.0, Act::Begin),
                (1.0, Act::Fire(Which::Film)),
                (1.0, Act::Fire(Which::Sound)),
                (2.0, Act::Fire(Which::Timeline)),
                (3.0, Act::Seek(14.0)),
                (4.0, Act::Pause(true)),
                (4.5, Act::Pause(false)),
                (6.0, Act::StopAll(0.5)),
                (6.5, Act::Fire(Which::FlashAgain)),
                (8.0, Act::StopAll(0.5)),
            ],
            9.0,
        );
        assert!(is_test_show(&self.show));
        // The media manager makes the test's version.
        self.current = "test";
    }

    fn run_until(&mut self, what: &str, secs: f64, mut f: impl FnMut(&Rig) -> bool) {
        let end = self.now + secs;
        while self.now < end {
            self.step();
            if f(self) {
                return;
            }
        }
        let lines = self.test.network_run().map(|n| n.media_lines());
        let steps: Vec<(bool, bool)> = self
            .st
            .net
            .peers
            .iter()
            .map(|p| (p.connected, p.status.as_ref().is_some_and(|s| s.in_step)))
            .collect();
        panic!("{what}: not within {secs} s of shared time; {lines:?} {steps:?}");
    }

    fn done(&self) -> Option<&Done> {
        match self.test.view() {
            View::Done(d) => Some(d),
            _ => None,
        }
    }

    /// The user's show is back here, and (with show sync) on every connected
    /// computer.
    fn assert_restored(&mut self) {
        assert_eq!(serde_json::to_string(&self.show).unwrap(), self.user);
        assert!(!self.st.test_locked);
        for _ in 0..10 {
            self.step();
        }
        for c in self.comps.iter().filter(|c| c.connected) {
            assert_eq!(
                serde_json::to_string(&c.show).unwrap(),
                self.user,
                "{}",
                c.name
            );
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn the_test_show_keeps_the_displays_adds_one_per_bare_monitor_and_keeps_the_users_media() {
    let mut show = user_show();
    let left = NodeId(0x20);
    // A display of the user's on Left's first monitor; Left has two.
    let d = show.add_display();
    let o = show.outputs.iter_mut().find(|o| o.id == d).unwrap();
    o.computer = Some(left);
    o.monitor = Some(monitor("Projector").key().reference(0));
    let mut p = peer(left, "Left", true, true, None);
    p.status.as_mut().unwrap().monitors.push(monitor("Side"));
    let flash = PathBuf::from("/m/flash.mp4");
    let b = build(&show, &res(), Some(&flash), &[p], &[]);
    assert!(is_test_show(&b.show) && b.show.own_layers);
    // The user's displays, and one for Left's second monitor.
    assert_eq!(b.show.outputs.len(), show.outputs.len() + 1);
    let added = b.show.outputs.last().unwrap();
    assert_eq!(added.name, "Left monitor 2 (test)");
    assert_eq!(added.computer, Some(left));
    assert_eq!(added.monitor.as_ref().unwrap().index, 1);
    assert_eq!(b.displays.len(), b.show.outputs.len());
    assert!(
        b.notes
            .iter()
            .any(|n| n.contains("Left's monitor 2 (Side)")),
        "{:?}",
        b.notes
    );
    // The flash is on every display.
    let flash_cue = b.show.cue(b.cues.flash).unwrap();
    let m = flash_cue.kind.file_media().unwrap();
    assert_eq!(m.file, flash);
    assert!(m.loop_media);
    assert_eq!(m.displays(&b.show).len(), b.show.outputs.len());
    // The main list: flash, film, sound, the timeline (three clips), the flash again.
    let main: Vec<&Cue> = b.show.cues.iter().filter(|c| c.list == 0).collect();
    assert_eq!(main.len(), 8);
    assert_eq!(
        b.show
            .cues
            .iter()
            .filter(|c| c.parent == Some(b.cues.timeline))
            .count(),
        3
    );
    // The user's clip, in a list that never plays.
    let kept: Vec<&Cue> = b.show.cues.iter().filter(|c| c.list != 0).collect();
    assert_eq!(kept.len(), 1);
    assert!(!kept[0].armed);
    assert_eq!(
        crate::net::store::manifest::media_paths(&b.show).len(),
        5 + 1,
        "the test's five files and the user's"
    );
    assert_eq!(b.files.len(), 5);
    // The original isn't touched, and a show of the user's isn't the test's.
    assert!(!is_test_show(&show));
    // No flash: the film in its place, said.
    let b = build(&show, &res(), None, &[], &[]);
    assert!(
        b.notes
            .iter()
            .any(|n| n.contains("Sync flash clip couldn't be made"))
    );
}

#[test]
fn a_lap_is_about_two_minutes_and_the_soak_about_twenty() {
    let one = script(false);
    assert!(one.iter().all(|(t, _)| *t < LAP_SECS));
    assert!((100.0..=130.0).contains(&LAP_SECS));
    let soak = script(true);
    assert_eq!(soak.len(), one.len() * SOAK_LAPS as usize);
    let total = f64::from(SOAK_LAPS) * LAP_SECS;
    assert!((17.0 * 60.0..=22.0 * 60.0).contains(&total));
    // There is a seek and a pause, and every lap begins with the flash.
    assert!(one.iter().any(|(_, a)| matches!(a, Act::Seek(_))));
    assert!(one.iter().any(|(_, a)| *a == Act::Pause(true)));
    assert_eq!(
        soak.iter().filter(|(_, a)| *a == Act::Begin).count(),
        SOAK_LAPS as usize
    );
}

#[test]
fn a_computers_media_is_here_when_its_version_is_the_primarys_and_lacks_none_of_the_tests() {
    use crate::net::media::manager::Lack;
    let files = vec![("/m/a.mp4".to_string(), 100), ("/m/b.png".to_string(), 50)];
    assert_eq!(media_state(None, Some("v2"), &files), MediaState::Waiting);
    let mut r = MediaReport::default();
    r.progress.manifest_id = Some("v1".into());
    assert_eq!(
        media_state(Some(&r), Some("v2"), &files),
        MediaState::Waiting
    );
    r.progress.manifest_id = Some("v2".into());
    assert_eq!(media_state(Some(&r), None, &files), MediaState::Waiting);
    r.lacking = vec![
        Lack {
            path: "/m/a.mp4".into(),
            done: 40,
            total: 100,
        },
        // Not one of the test's: the user's own, still coming.
        Lack {
            path: "/show/big.mov".into(),
            done: 0,
            total: 1 << 30,
        },
    ];
    assert_eq!(
        media_state(Some(&r), Some("v2"), &files),
        MediaState::Coming {
            left: 60,
            total: 150
        }
    );
    r.lacking.remove(0);
    assert_eq!(media_state(Some(&r), Some("v2"), &files), MediaState::Here);
}

#[test]
fn the_measurements_of_two_followers_reach_the_primary_and_the_show_comes_back_everywhere() {
    let _heavy = crate::testmode::heavy();
    let Some(ff) = ffmpeg() else { return };
    let mut r = Rig::new(&ff, "full");
    r.start();
    // The test's media version is made a moment later.
    r.run_until("the test plays", 30.0, |r| {
        r.test
            .network_run()
            .is_some_and(|n| n.stage == Stage::Playing)
    });
    r.run_until("the test ends", 60.0, |r| r.done().is_some());
    let d = r.done().unwrap();
    let n = d.network.as_ref().expect("a network test's result");
    assert_eq!(n.computers.len(), 2, "{}", d.report);
    // (What plays is real clips decoded by FFmpeg: on a loaded machine a
    // start can lag, so the verdict itself isn't asserted; the commands'
    // timing is the simulation's own and is.)
    for c in &n.computers {
        assert!(
            !c.findings.iter().any(|f| f.text.contains("Late commands")),
            "{}: {:?}\n{}",
            c.name,
            c.findings,
            d.report
        );
        assert!(
            c.lines.iter().any(|l| l.contains(" commands, 0 late")),
            "{:?}",
            c.lines
        );
    }
    let csv = std::fs::read_to_string(d.folder.join("network.csv")).unwrap();
    assert!(
        csv.lines().filter(|l| l.starts_with("Left,")).count() >= 10,
        "{csv}"
    );
    assert!(csv.lines().filter(|l| l.starts_with("Right,")).count() >= 10);
    // Commands reached both, each with its lead.
    let report = std::fs::read_to_string(d.folder.join("report.txt")).unwrap();
    assert!(
        report.contains("Left — ") && report.contains("Right — "),
        "{report}"
    );
    assert!(report.contains(" 0 late"), "{report}");
    assert!(d.folder.join("computer-Left-newq.log").is_file());
    assert!(d.folder.join("events.log").is_file());
    let events = std::fs::read_to_string(d.folder.join("events.log")).unwrap();
    assert!(
        events.contains("All here") && events.contains("Fire the film"),
        "{events}"
    );
    r.assert_restored();
}

#[test]
fn a_follower_whose_commands_come_late_is_reported_late() {
    let _heavy = crate::testmode::heavy();
    let Some(ff) = ffmpeg() else { return };
    let mut r = Rig::new(&ff, "late");
    r.comps[1].delay = 0.12;
    r.start();
    r.run_until("the test ends", 90.0, |r| r.done().is_some());
    let d = r.done().unwrap();
    let n = d.network.as_ref().unwrap();
    let left = n.computers.iter().find(|c| c.name == "Left").unwrap();
    let right = n.computers.iter().find(|c| c.name == "Right").unwrap();
    assert!(
        !left
            .findings
            .iter()
            .any(|f| f.text.contains("Late commands")),
        "{:?}",
        left.findings
    );
    assert_eq!(
        right.verdict,
        Verdict::ProblemsFound,
        "{:?}",
        right.findings
    );
    assert!(
        right.headline.contains("arrived after their time"),
        "{}",
        right.headline
    );
    assert_eq!(d.verdict, Verdict::ProblemsFound);
    r.assert_restored();
}

#[test]
fn cancelling_while_the_media_is_on_its_way_puts_the_show_back_everywhere() {
    let _heavy = crate::testmode::heavy();
    let Some(ff) = ffmpeg() else { return };
    let mut r = Rig::new(&ff, "cancel");
    r.media_comes = false;
    r.start();
    for _ in 0..120 {
        r.step();
    }
    assert!(
        r.test
            .network_run()
            .is_some_and(|n| n.stage == Stage::WaitingForMedia)
    );
    // The computers have the test show meanwhile.
    assert!(r.comps.iter().all(|c| is_test_show(&c.show)));
    let lines = r.test.network_run().unwrap().media_lines();
    assert!(
        lines
            .iter()
            .all(|l| l.ends_with("getting the list of media")),
        "{lines:?}"
    );
    r.test.stop();
    r.run_until("it stops", 5.0, |r| r.done().is_some());
    let d = r.done().unwrap();
    assert_eq!(d.ended, End::Stopped);
    assert!(
        d.headline.contains("Stopped before it played"),
        "{}",
        d.headline
    );
    r.assert_restored();
}

#[test]
fn a_follower_that_drops_out_is_reported_and_gets_the_show_back_when_it_returns() {
    let _heavy = crate::testmode::heavy();
    let Some(ff) = ffmpeg() else { return };
    let mut r = Rig::new(&ff, "drop");
    r.start();
    r.run_until("the test plays", 30.0, |r| {
        r.test
            .network_run()
            .is_some_and(|n| n.stage == Stage::Playing)
    });
    for _ in 0..(3.0 / DT) as usize {
        r.step();
    }
    r.comps[1].connected = false;
    r.run_until("the test ends", 60.0, |r| r.done().is_some());
    let d = r.done().unwrap();
    let n = d.network.as_ref().unwrap();
    let right = n.computers.iter().find(|c| c.name == "Right").unwrap();
    assert_eq!(right.verdict, Verdict::ProblemsFound);
    assert!(
        right
            .findings
            .iter()
            .any(|f| f.text.contains("Right dropped out of the network")),
        "{:?}",
        right.findings
    );
    let left = n.computers.iter().find(|c| c.name == "Left").unwrap();
    assert!(
        !left.findings.iter().any(|f| f.text.contains("dropped out")),
        "{:?}",
        left.findings
    );
    r.assert_restored();
    // Right was away with the test show; back, it is given the user's.
    assert!(is_test_show(&r.comps[1].show));
    r.comps[1].connected = true;
    for _ in 0..5 {
        r.step();
    }
    assert_eq!(serde_json::to_string(&r.comps[1].show).unwrap(), r.user);
}

// ---- Every test across the network (`fleet`) ----

/// A short cue test: a film, a picture and a sound, one after another.
fn short_lap(user: &Show) -> crate::testmode::plan::Built {
    let mut show = user.clone();
    show.cues.clear();
    show.lists.clear();
    show.name = "Test Mode: Short".into();
    show.own_layers = true;
    let mut ids = Vec::new();
    for (ty, file, secs) in [
        (CueType::Video, "testsrc_10s.mp4", 3.0),
        (CueType::Image, "color_red.png", 2.0),
        (CueType::Audio, "tone_5s.wav", 2.0),
    ] {
        let mut c = show.make_cue(ty, Some(media_dir().join(file)), show.cues.len());
        c.continue_mode = ContinueMode::AutoFollow;
        if let Some(m) = c.kind.media_mut() {
            m.end = Some(secs);
        }
        ids.push(c.id);
        show.cues.push(c);
    }
    if let Some(c) = show.cues.last_mut() {
        c.continue_mode = ContinueMode::DoNotContinue;
    }
    crate::testmode::plan::spread_pictures(&mut show);
    crate::testmode::plan::Built {
        cues: show.cues.len(),
        show,
        first: ids[0],
        repeats: false,
        lap_secs: 7.0,
        notes: Vec::new(),
        used: Vec::new(),
        timeline: None,
    }
}

impl Rig {
    /// Start a Short cue test across the network.
    fn start_short(&mut self, ff: &Ffmpeg) {
        let built = short_lap(&self.show);
        let dir = self.dir.clone();
        self.test.launch_across(
            ff.clone(),
            built,
            &dir,
            crate::testmode::Level::Short,
            &mut self.show,
            &mut self.engine,
            &mut self.st,
        );
        assert!(!is_test_show(&self.show) && self.show.name == "Test Mode: Short");
        self.current = "test";
        self.paced = true;
    }

    fn spreading(&self) -> bool {
        matches!(self.test.view(), View::Spreading(..))
    }
}

#[test]
fn a_short_test_plays_on_every_computer_and_each_ones_own_measurements_come_back() {
    let _heavy = crate::testmode::heavy();
    let Some(ff) = ffmpeg() else { return };
    let mut r = Rig::new(&ff, "short");
    r.start_short(&ff);
    // The computers get the test show first; nothing plays meanwhile.
    for _ in 0..30 {
        r.step();
    }
    assert!(r.spreading() || matches!(r.test.view(), View::Running(_)));
    assert!(r.comps.iter().all(|c| c.show.name == "Test Mode: Short"));
    r.run_until("the test plays", 30.0, |r| {
        matches!(r.test.view(), View::Running(_))
    });
    r.run_until("the test ends", 90.0, |r| r.done().is_some());
    let d = r.done().unwrap();
    assert_eq!(d.computers.len(), 2, "{}", d.report);
    for c in &r.comps {
        assert!(c.played > 0, "{} played nothing", c.name);
    }
    for c in &d.computers {
        assert!(
            c.lines.iter().any(|l| l.contains("memory up to 300 MB")),
            "{}: {:?}",
            c.name,
            c.lines
        );
        assert!(
            c.lines
                .iter()
                .any(|l| l.contains("Monitor 1 (Projector): drew at least 60 frames a second")),
            "{:?}",
            c.lines
        );
        assert!(
            !c.findings
                .iter()
                .any(|f| f.text.contains("No measurements")),
            "{:?}",
            c.findings
        );
    }
    let report = std::fs::read_to_string(d.folder.join("report.txt")).unwrap();
    assert!(
        report.contains("The other computers")
            && report.contains("  Left — ")
            && report.contains("  Right — "),
        "{report}"
    );
    assert!(report.contains("What the test adds on each computer"));
    let csv = std::fs::read_to_string(d.folder.join("computers.csv")).unwrap();
    assert!(
        csv.lines().filter(|l| l.starts_with("Left,")).count() >= 4,
        "{csv}"
    );
    assert!(d.folder.join("outputs.csv").is_file());
    assert!(d.folder.join("computer-Right-newq.log").is_file());
    r.assert_restored();
}

#[test]
fn a_computer_whose_output_stutters_fails_the_test_and_says_which() {
    let _heavy = crate::testmode::heavy();
    let Some(ff) = ffmpeg() else { return };
    let mut r = Rig::new(&ff, "stutter");
    r.comps[1].fps = 4.0;
    r.start_short(&ff);
    r.run_until("the test ends", 120.0, |r| r.done().is_some());
    let d = r.done().unwrap();
    assert_eq!(d.verdict, Verdict::ProblemsFound, "{}", d.report);
    let right = d.computers.iter().find(|c| c.name == "Right").unwrap();
    assert_eq!(right.verdict, Verdict::ProblemsFound);
    assert!(
        right.findings.iter().any(|f| f
            .text
            .contains("Right's monitor 1 (Projector) drew about 4 frames a second")),
        "{:?}",
        right.findings
    );
    let left = d.computers.iter().find(|c| c.name == "Left").unwrap();
    assert!(
        !left
            .findings
            .iter()
            .any(|f| f.text.contains("frames a second for"))
    );
    r.assert_restored();
}

#[test]
fn a_computer_that_drops_out_of_a_short_test_is_reported_and_gets_the_show_back() {
    let _heavy = crate::testmode::heavy();
    let Some(ff) = ffmpeg() else { return };
    let mut r = Rig::new(&ff, "shortdrop");
    r.start_short(&ff);
    r.run_until("the test plays", 30.0, |r| {
        matches!(r.test.view(), View::Running(_))
    });
    for _ in 0..(1.0 / DT) as usize {
        r.step();
    }
    r.comps[1].connected = false;
    r.run_until("the test ends", 90.0, |r| r.done().is_some());
    let d = r.done().unwrap();
    let right = d.computers.iter().find(|c| c.name == "Right").unwrap();
    assert_eq!(
        right.verdict,
        Verdict::ProblemsFound,
        "{:?}",
        right.findings
    );
    assert!(
        right
            .findings
            .iter()
            .any(|f| f.text.contains("Right dropped out")),
        "{:?}",
        right.findings
    );
    r.assert_restored();
    assert!(!r.comps[1].show.name.contains("Tonight"));
    r.comps[1].connected = true;
    for _ in 0..5 {
        r.step();
    }
    assert_eq!(serde_json::to_string(&r.comps[1].show).unwrap(), r.user);
}

#[test]
fn cancelling_a_short_test_while_its_media_travels_puts_the_show_back_everywhere() {
    let _heavy = crate::testmode::heavy();
    let Some(ff) = ffmpeg() else { return };
    let mut r = Rig::new(&ff, "shortcancel");
    r.media_comes = false;
    r.start_short(&ff);
    for _ in 0..120 {
        r.step();
    }
    assert!(r.spreading());
    let lines = r.test.fleet().unwrap().media_lines();
    assert!(
        lines
            .iter()
            .all(|l| l.ends_with("getting the list of media")),
        "{lines:?}"
    );
    r.test.stop();
    for _ in 0..3 {
        r.step();
    }
    assert!(matches!(r.test.view(), View::Idle));
    assert!(
        r.test
            .notice
            .as_deref()
            .unwrap_or("")
            .starts_with("Cancelled"),
        "{:?}",
        r.test.notice
    );
    r.assert_restored();
    // And quitting NewQ mid-test, alone or across: the show is back too.
    r.media_comes = true;
    r.start_short(&ff);
    for _ in 0..30 {
        r.step();
    }
    r.test.shutdown(&mut r.show, &mut r.engine, &mut r.st);
    r.assert_restored();
}
