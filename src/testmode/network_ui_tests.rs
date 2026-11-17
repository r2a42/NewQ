//! The Test Mode window's network test, drawn in the whole window (both
//! interfaces) at 1280×760: alone (what it needs), on a primary with a
//! computer in step (its choice), waiting for the media, and the results.

use super::*;
use crate::net::hub::NetView;
use crate::net::node::{Identity, NetworkId, Role};
use crate::net::proto::{Link, StatusReport};
use crate::testmode::session::{Done, End};
use crate::testmode::{Level, TestMode};
use egui::{Pos2, RawInput, Rect, vec2};
use std::time::Duration;

const SIZE: egui::Vec2 = vec2(1280.0, 760.0);
const PRIMARY: NodeId = NodeId(0x10);
const LEFT: NodeId = NodeId(0x20);

fn primary_view(in_step: bool) -> NetView {
    NetView {
        me: Identity {
            id: PRIMARY,
            role: Role::Primary,
            on: true,
            network: Some(NetworkId(1)),
            primary: Some(PRIMARY),
            ..Identity::fresh()
        },
        name: "Booth".into(),
        peers: vec![PeerView {
            id: LEFT,
            name: "Left".into(),
            role: Role::Display,
            version: crate::net::proto::VERSION.into(),
            compatible: true,
            network: Some(NetworkId(1)),
            ours: true,
            waiting: false,
            online: true,
            connected: true,
            addr: None,
            status: Some(StatusReport {
                link: Link::Wired,
                in_step,
                ..Default::default()
            }),
            problem: None,
            adopting: false,
        }],
        ..Default::default()
    }
}

/// Draw the whole window a few times; the network test's words and buttons.
fn draw(modern: bool, show: &mut Show, e: &mut Engine, st: &mut UiState) -> Vec<(String, Rect)> {
    st.prefs.new_layout = modern;
    st.test.open = true;
    let ctx = egui::Context::default();
    let mut notes = Vec::new();
    for i in 0..4 {
        ctx.data_mut(|d| d.remove::<Vec<(String, Rect)>>(egui::Id::new("test_net")));
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
            time: Some(1.0 + f64::from(i) / 60.0),
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| crate::ui::draw(ui, show, e, st));
        out.textures_delta.clear();
        notes = ctx
            .data(|d| d.get_temp::<Vec<(String, Rect)>>(egui::Id::new("test_net")))
            .unwrap_or_default();
    }
    notes
}

fn has(notes: &[(String, Rect)], text: &str) -> bool {
    notes.iter().any(|(n, _)| n.contains(text))
}

fn in_window(notes: &[(String, Rect)], name: &str) -> bool {
    notes.iter().find(|(n, _)| n == name).is_some_and(|(_, r)| {
        r.is_positive() && Rect::from_min_size(Pos2::ZERO, SIZE).contains_rect(*r)
    })
}

#[test]
fn alone_the_window_says_what_the_network_test_needs() {
    for modern in [false, true] {
        let (mut show, mut e, mut st) = (Show::default(), Engine::for_tests(), UiState::default());
        let notes = draw(modern, &mut show, &mut e, &mut st);
        assert!(
            has(&notes, "Several computers: the network test"),
            "{notes:?}"
        );
        assert!(has(
            &notes,
            "The network test runs on the computer that runs the show network"
        ));
        assert!(!st.test.busy());
    }
}

#[test]
fn a_primary_with_a_computer_in_step_is_offered_the_test_first() {
    for modern in [false, true] {
        let (mut show, mut e, mut st) = (Show::default(), Engine::for_tests(), UiState::default());
        st.net = Arc::new(primary_view(false));
        let notes = draw(modern, &mut show, &mut e, &mut st);
        assert!(
            has(&notes, "No computer of this show network is in step yet"),
            "{notes:?}"
        );
        st.net = Arc::new(primary_view(true));
        let notes = draw(modern, &mut show, &mut e, &mut st);
        assert!(has(&notes, "With Left (in step now)."), "{notes:?}");
        assert!(!has(&notes, "No computer of this show network"));
        // First in the window, in sight.
        assert!(in_window(&notes, "Start the network test"), "{notes:?}");
        assert!(in_window(&notes, "Network soak"));
        // The other tests run across the network too: Left is in step.
        assert!(st.test_refusal().is_none(), "{:?}", st.test_refusal());
        // The decoding comparison is one computer's own.
        assert!(
            st.decode_refusal()
                .unwrap()
                .contains("runs on a computer alone")
        );
    }
}

#[test]
fn waiting_for_the_media_says_how_far_each_computer_has_got_and_cancel_puts_the_show_back() {
    for modern in [false, true] {
        let (mut show, mut e, mut st) = (Show::default(), Engine::for_tests(), UiState::default());
        show.name = "Tonight".into();
        let user = serde_json::to_string(&show).unwrap();
        st.net = Arc::new(primary_view(true));
        let dir = std::env::temp_dir().join(format!("newq_netui_{}_{modern}", std::process::id()));
        let built = build(
            &show,
            &HashMap::new(),
            Some(Path::new("/m/flash.mp4")),
            &st.net.peers.clone(),
            &[],
        );
        let mut test = std::mem::take(&mut st.test);
        test.launch_network(built, &dir, &mut show, &mut e, &mut st)
            .unwrap();
        st.test = test;
        let notes = draw(modern, &mut show, &mut e, &mut st);
        assert!(
            has(
                &notes,
                "Network test: sending the test's media to the computers"
            ),
            "{notes:?}"
        );
        assert!(has(&notes, "Left: getting the list of media"), "{notes:?}");
        assert!(in_window(&notes, "Cancel"), "{notes:?}");
        // Cancel: the show is back.
        st.test.stop();
        let mut test = std::mem::take(&mut st.test);
        test.update_at(&mut show, &mut e, &mut st, 10.0);
        st.test = test;
        assert_eq!(serde_json::to_string(&show).unwrap(), user);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn the_results_give_each_computer_its_verdict_in_words() {
    use super::net_judge::{ComputerNotes, judge};
    let mut left = ComputerNotes::new("Left");
    left.refresh_hz = Some(60.0);
    left.reports = 10;
    left.rows = (0..20)
        .map(|i| crate::net::measure::Row {
            t: f64::from(i) * 0.5,
            clock_error_ms: Some(0.2),
            locked: true,
            in_step: true,
            apart_ms: Some(5.0),
            compared: 1,
            ..Default::default()
        })
        .collect();
    let right = ComputerNotes::new("Right");
    let j = judge(&[left, right], 120.0, Vec::new());
    for modern in [false, true] {
        let (mut show, mut e, mut st) = (Show::default(), Engine::for_tests(), UiState::default());
        let mut test = TestMode::default();
        test.show_done(Done {
            compare: None,
            full: None,
            computers: Vec::new(),
            network: Some(NetDone {
                soak: false,
                computers: j.computers.clone(),
            }),
            flavour: super::super::Flavour::Cues,
            bundle: Default::default(),
            level: Level::Short,
            verdict: j.verdict,
            headline: j.headline.clone(),
            steady: None,
            findings: j.findings.clone(),
            folder: PathBuf::from("/nowhere"),
            report: String::new(),
            laps: 0,
            elapsed: Duration::from_secs(130),
            ended: End::Finished,
        });
        st.test = test;
        let notes = draw(modern, &mut show, &mut e, &mut st);
        assert!(
            has(&notes, "Left: In step: worst 0.3 frames apart over 2 min"),
            "{notes:?}"
        );
        assert!(
            has(&notes, "Right: No measurements came from Right"),
            "{notes:?}"
        );
    }
}

// ---- Every test across the network: the screens of every computer ----

/// The window's choices (`gui::note_choice`): where each tick and button is.
fn choices(
    ctx: &egui::Context,
    modern: bool,
    show: &mut Show,
    e: &mut Engine,
    st: &mut UiState,
    events: Vec<Vec<egui::Event>>,
) -> Vec<(String, Rect)> {
    st.prefs.new_layout = modern;
    st.test.open = true;
    let mut notes = Vec::new();
    let mut frames = events;
    frames.extend([vec![], vec![], vec![]]);
    for (i, ev) in frames.into_iter().enumerate() {
        ctx.data_mut(|d| d.remove::<Vec<(String, Rect)>>(egui::Id::new("test_choices")));
        let t = ctx.input(|x| x.time) + 1.0 / 60.0 + if i == 0 { 1.0 } else { 0.0 };
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
            time: Some(t),
            events: ev,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| crate::ui::draw(ui, show, e, st));
        out.textures_delta.clear();
        notes = ctx
            .data(|d| d.get_temp::<Vec<(String, Rect)>>(egui::Id::new("test_choices")))
            .unwrap_or_default();
    }
    notes
}

fn two_monitors(in_step: bool) -> NetView {
    let mut v = primary_view(in_step);
    let m = |name: &str| crate::net::proto::MonitorReport {
        identity: name.into(),
        raw: name.into(),
        hardware: None,
        size: (1920, 1080),
        refresh_hz: Some(60.0),
    };
    if let Some(s) = v.peers[0].status.as_mut() {
        s.monitors = vec![m("Projector"), m("Side screen")];
    }
    v
}

#[test]
fn every_computers_screens_are_offered_grouped_and_ticked_all_at_once() {
    use crate::testmode::gui::remote_key;
    for modern in [false, true] {
        let (mut show, mut e, mut st) = (Show::default(), Engine::for_tests(), UiState::default());
        st.net = Arc::new(two_monitors(true));
        let ctx = egui::Context::default();
        let notes = choices(&ctx, modern, &mut show, &mut e, &mut st, vec![]);
        let tick = "screen:Left 1: Projector (1920×1080 @ 60 Hz)";
        assert!(has(&notes, tick), "{notes:?}");
        assert!(has(&notes, "screen:Left 2: Side screen"), "{notes:?}");
        // The whole group at once.
        let all = notes
            .iter()
            .find(|(n, _)| n == "All:Left")
            .map(|(_, r)| *r)
            .expect("All for Left");
        assert!(
            Rect::from_min_size(Pos2::ZERO, SIZE).contains_rect(all),
            "modern {modern}: All is in the window: {all:?}"
        );
        let at = all.center();
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        choices(
            &ctx,
            modern,
            &mut show,
            &mut e,
            &mut st,
            vec![
                vec![egui::Event::PointerMoved(at)],
                vec![press(true)],
                vec![press(false)],
            ],
        );
        let want = [
            remote_key(LEFT, 0, "Projector"),
            remote_key(LEFT, 1, "Side screen"),
        ];
        for k in &want {
            assert!(
                st.prefs.test_displays.contains(k),
                "modern {modern}: {:?}",
                st.prefs.test_displays
            );
        }
        // Left gone: its ticks are kept for when it comes back, and nothing
        // of it is offered or used meanwhile.
        st.net = Arc::new(primary_view(true));
        let notes = choices(&ctx, modern, &mut show, &mut e, &mut st, vec![]);
        assert!(!has(&notes, "screen:Left"), "{notes:?}");
        assert!(st.prefs.test_displays.contains(&want[0]));
    }
}
