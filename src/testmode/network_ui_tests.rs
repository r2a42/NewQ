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
            twin: None,
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
fn alone_the_network_tick_says_what_it_needs() {
    for modern in [false, true] {
        let (mut show, mut e, mut st) = (Show::default(), Engine::for_tests(), UiState::default());
        let notes = draw(modern, &mut show, &mut e, &mut st);
        assert!(
            has(
                &notes,
                "Network: Needs another computer in the show network."
            ),
            "{notes:?}"
        );
        assert!(has(&notes, "Cue list: about 1 minute"), "{notes:?}");
        assert!(!st.test.busy());
    }
}

#[test]
fn a_primary_with_a_computer_in_step_can_tick_the_network() {
    for modern in [false, true] {
        let (mut show, mut e, mut st) = (Show::default(), Engine::for_tests(), UiState::default());
        st.net = Arc::new(primary_view(false));
        let notes = draw(modern, &mut show, &mut e, &mut st);
        assert!(
            has(&notes, "Network: Waiting for Left to be in step."),
            "{notes:?}"
        );
        st.net = Arc::new(primary_view(true));
        let notes = draw(modern, &mut show, &mut e, &mut st);
        assert!(!has(&notes, "Network: "), "{notes:?}");
        assert!(
            has(
                &notes,
                "The cue list and timeline play on Left too, and each measures itself."
            ),
            "{notes:?}"
        );
        assert_eq!(
            crate::testmode::choice::and_list(&["Left", "Right", "Rear"]),
            "Left, Right and Rear"
        );
        assert!(has(&notes, "Cue list: about 1 minute"), "{notes:?}");
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
            parts: Vec::new(),
            compare: None,
            full: None,
            computers: Vec::new(),
            network: Some(NetDone {
                laps: 1,
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

#[test]
fn the_zip_note_says_which_computers_logs_are_in_it_and_which_did_not_come() {
    // 0.24.4's review (C9): it said every log was inside when one was missing.
    use super::net_judge::{ComputerNotes, judge};
    let j = judge(
        &[ComputerNotes::new("Left"), ComputerNotes::new("Right")],
        120.0,
        Vec::new(),
    );
    let done = |logs: [Option<&str>; 2]| {
        let mut computers = j.computers.clone();
        for (c, l) in computers.iter_mut().zip(logs) {
            c.log = l.map(str::to_string);
        }
        Done {
            parts: Vec::new(),
            compare: None,
            full: None,
            computers,
            network: None,
            flavour: super::super::Flavour::Cues,
            bundle: Default::default(),
            level: Level::Short,
            verdict: j.verdict,
            headline: j.headline.clone(),
            steady: None,
            findings: Vec::new(),
            folder: PathBuf::from("/nowhere"),
            report: String::new(),
            laps: 0,
            elapsed: Duration::from_secs(130),
            ended: End::Finished,
        }
    };
    let left = Some("computers/Left/newq.log");
    let right = Some("computers/Right/newq.log");
    let both = crate::testmode::gui::logs_inside(&done([left, right]));
    assert!(
        both.starts_with(
            "Every computer's own log is inside the zip too (computers/Left/newq.log, computers/Right/newq.log)"
        ),
        "{both}"
    );
    let one = crate::testmode::gui::logs_inside(&done([left, None]));
    assert!(one.contains("(computers/Left/newq.log)"), "{one}");
    assert!(
        one.contains("Right's own log did not come back") && !one.starts_with("Every"),
        "{one}"
    );
    let none = crate::testmode::gui::logs_inside(&done([None, None]));
    assert!(
        none.starts_with("The own logs of Left, Right did not come back"),
        "{none}"
    );
}

// ---- The choices, drawn and clicked in the whole window ----

fn click_at(at: Pos2) -> Vec<Vec<egui::Event>> {
    let press = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    vec![
        vec![egui::Event::PointerMoved(at)],
        vec![press(true)],
        vec![press(false)],
    ]
}

fn rect_of(notes: &[(String, Rect)], name: &str) -> Rect {
    notes
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, r)| *r)
        .unwrap_or_else(|| panic!("no {name} in {notes:?}"))
}

fn on_screen(r: Rect) -> bool {
    r.is_positive() && Rect::from_min_size(Pos2::ZERO, SIZE).contains_rect(r)
}

#[test]
fn start_is_in_sight_without_scrolling_alone_and_in_a_network() {
    for modern in [false, true] {
        for net in [false, true] {
            let (mut show, mut e, mut st) =
                (Show::default(), Engine::for_tests(), UiState::default());
            if net {
                st.net = Arc::new(two_monitors(true));
                st.test.parts.network = true;
                st.test.parts.timeline = true;
            }
            let ctx = egui::Context::default();
            let notes = choices(&ctx, modern, &mut show, &mut e, &mut st, vec![]);
            let start = rect_of(&notes, "Start");
            assert!(
                on_screen(start),
                "modern {modern}, network {net}: Start at {start:?}"
            );
            // Collapsed: More's contents aren't drawn.
            assert!(!has(&notes, "Check every feature"), "{notes:?}");
        }
    }
}

#[test]
fn alone_the_network_tick_cant_be_ticked() {
    for modern in [false, true] {
        let (mut show, mut e, mut st) = (Show::default(), Engine::for_tests(), UiState::default());
        let ctx = egui::Context::default();
        let notes = choices(&ctx, modern, &mut show, &mut e, &mut st, vec![]);
        let at = rect_of(&notes, "Network").center();
        choices(&ctx, modern, &mut show, &mut e, &mut st, click_at(at));
        assert!(!st.test.parts.network, "modern {modern}");
        // The cue list can be unticked: then there is nothing to start.
        let at = rect_of(&notes, "Cue list").center();
        choices(&ctx, modern, &mut show, &mut e, &mut st, click_at(at));
        assert!(!st.test.parts.cue_list);
        let notes = draw(modern, &mut show, &mut e, &mut st);
        assert!(has(&notes, "Tick at least one thing to test."), "{notes:?}");
    }
}

#[test]
fn in_a_network_all_three_can_be_ticked() {
    for modern in [false, true] {
        let (mut show, mut e, mut st) = (Show::default(), Engine::for_tests(), UiState::default());
        st.net = Arc::new(primary_view(true));
        let ctx = egui::Context::default();
        let notes = choices(&ctx, modern, &mut show, &mut e, &mut st, vec![]);
        for name in ["Timeline", "Network"] {
            let at = rect_of(&notes, name).center();
            choices(&ctx, modern, &mut show, &mut e, &mut st, click_at(at));
        }
        let p = st.test.parts;
        assert!(
            p.cue_list && p.timeline && p.network,
            "modern {modern}: {p:?}"
        );
        let notes = draw(modern, &mut show, &mut e, &mut st);
        assert!(
            has(
                &notes,
                "Cue list, then timeline, then the network check: about 4 minutes"
            ),
            "{notes:?}"
        );
        // Remembered for this computer.
        assert_eq!(st.prefs.test_pick.parts, p);
    }
}

#[test]
fn more_holds_the_feature_check_and_the_decoding_comparison() {
    for modern in [false, true] {
        let (mut show, mut e, mut st) = (Show::default(), Engine::for_tests(), UiState::default());
        let ctx = egui::Context::default();
        let notes = choices(&ctx, modern, &mut show, &mut e, &mut st, vec![]);
        assert!(!has(&notes, "Compare hardware decoding"));
        let at = rect_of(&notes, "More").center();
        let notes = choices(&ctx, modern, &mut show, &mut e, &mut st, click_at(at));
        for name in ["Check every feature", "Compare hardware decoding"] {
            assert!(has(&notes, name), "modern {modern}: {name} in {notes:?}");
        }
    }
}

#[test]
fn the_network_load_is_chosen_under_more_and_greyed_alone() {
    use crate::net::load::LoadLevel;
    for modern in [false, true] {
        for net in [false, true] {
            let (mut show, mut e, mut st) =
                (Show::default(), Engine::for_tests(), UiState::default());
            if net {
                st.net = Arc::new(primary_view(true));
            }
            let ctx = egui::Context::default();
            let notes = choices(&ctx, modern, &mut show, &mut e, &mut st, vec![]);
            let at = rect_of(&notes, "More").center();
            let notes = choices(&ctx, modern, &mut show, &mut e, &mut st, click_at(at));
            let combo = rect_of(&notes, "Load the network");
            assert!(
                on_screen(combo),
                "modern {modern}, network {net}: {combo:?}"
            );
            let notes = choices(
                &ctx,
                modern,
                &mut show,
                &mut e,
                &mut st,
                click_at(combo.center()),
            );
            if !net {
                // Greyed, with the reason said.
                assert!(
                    has(
                        &notes,
                        "Network load: Needs another computer in the show network."
                    ),
                    "{notes:?}"
                );
                assert!(!has(&notes, "load:Light"), "no list opens: {notes:?}");
                assert_eq!(st.prefs.test_load, LoadLevel::Off);
                continue;
            }
            let light = rect_of(&notes, "load:Light");
            assert!(on_screen(light), "modern {modern}: {light:?}");
            let notes = choices(
                &ctx,
                modern,
                &mut show,
                &mut e,
                &mut st,
                click_at(light.center()),
            );
            // Remembered for this computer, and said what it does.
            assert_eq!(st.prefs.test_load, LoadLevel::Light, "modern {modern}");
            assert!(
                has(
                    &notes,
                    "Each computer: Light, about 50 Mbit/s each way per computer."
                ),
                "{notes:?}"
            );
        }
    }
}

#[test]
fn a_run_of_several_parts_reads_overall_first_then_a_section_per_part() {
    use super::net_judge::{ComputerNotes, judge};
    use crate::testmode::analysis::Verdict;
    use crate::testmode::choice::{Part, PartPlan};
    use crate::testmode::sequence::PartDone;
    let j = judge(&[ComputerNotes::new("Right")], 120.0, Vec::new());
    let one = |verdict, network: Option<NetDone>, headline: &str| Done {
        parts: Vec::new(),
        compare: None,
        full: None,
        computers: Vec::new(),
        network,
        flavour: super::super::Flavour::Cues,
        bundle: Default::default(),
        level: Level::Short,
        verdict,
        headline: headline.into(),
        steady: None,
        findings: Vec::new(),
        folder: PathBuf::from("/nowhere"),
        report: String::new(),
        laps: 1,
        elapsed: Duration::from_secs(60),
        ended: End::Finished,
    };
    let plan = |part, laps| PartPlan {
        part,
        level: Level::Short,
        minutes: 0,
        laps,
    };
    let mut whole = one(
        Verdict::ProblemsFound,
        None,
        "Problems were found in: Network.",
    );
    whole.parts = vec![
        PartDone {
            part: plan(Part::CueList, 1),
            done: Some(Box::new(one(Verdict::Passed, None, "Everything held up."))),
            why_not: None,
        },
        PartDone {
            part: plan(Part::Network, 1),
            done: Some(Box::new(one(
                j.verdict,
                Some(NetDone {
                    laps: 1,
                    computers: j.computers.clone(),
                }),
                &j.headline,
            ))),
            why_not: None,
        },
    ];
    for modern in [false, true] {
        let (mut show, mut e, mut st) = (Show::default(), Engine::for_tests(), UiState::default());
        let mut test = TestMode::default();
        let mut d = one(whole.verdict, None, &whole.headline);
        d.parts = whole
            .parts
            .iter()
            .map(|p| PartDone {
                part: p.part,
                done: p.done.as_ref().map(|d| {
                    let mut c = one(d.verdict, d.network.clone(), &d.headline);
                    c.computers = d.computers.clone();
                    Box::new(c)
                }),
                why_not: None,
            })
            .collect();
        test.show_done(d);
        st.test = test;
        let notes = draw(modern, &mut show, &mut e, &mut st);
        let at = |text: &str| {
            notes
                .iter()
                .position(|(n, _)| n.contains(text))
                .unwrap_or_else(|| panic!("no {text} in {notes:?}"))
        };
        let (head, cue, net, right) = (
            at("Problems were found in: Network."),
            at("part:Cue list (Short): Passed"),
            at("part:Network test: Problems found"),
            at("Right: No measurements came from Right"),
        );
        assert!(head < cue && cue < net && net < right, "{notes:?}");
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
        ctx.data_mut(|d| {
            d.remove::<Vec<(String, Rect)>>(egui::Id::new("test_choices"));
            d.remove::<Vec<(String, Rect)>>(egui::Id::new("test_net"));
        });
        let t = ctx.input(|x| x.time) + 1.0 / 60.0 + if i == 0 { 1.0 } else { 0.0 };
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
            time: Some(t),
            events: ev,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| crate::ui::draw(ui, show, e, st));
        out.textures_delta.clear();
        // The ticks and buttons, then the words.
        notes = ["test_choices", "test_net"]
            .into_iter()
            .flat_map(|k| {
                ctx.data(|d| d.get_temp::<Vec<(String, Rect)>>(egui::Id::new(k)))
                    .unwrap_or_default()
            })
            .collect();
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
        ..Default::default()
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
        let tick = "screen:Left · screen 1 · 1920×1080";
        assert!(has(&notes, tick), "{notes:?}");
        assert!(
            has(&notes, "screen:Left · screen 2 · 1920×1080"),
            "{notes:?}"
        );
        let want = [
            remote_key(LEFT, 0, "Projector"),
            remote_key(LEFT, 1, "Side screen"),
        ];
        // On opening with none ticked, every screen is.
        for k in &want {
            assert!(st.prefs.test_displays.contains(k), "modern {modern}");
        }
        let none = rect_of(&notes, "None:Left").center();
        choices(&ctx, modern, &mut show, &mut e, &mut st, click_at(none));
        assert!(st.prefs.test_displays.is_empty(), "modern {modern}");
        let notes = choices(&ctx, modern, &mut show, &mut e, &mut st, vec![]);
        assert!(
            has(&notes, "Tick at least one screen to show the test on."),
            "{notes:?}"
        );
        let notes = choices(&ctx, modern, &mut show, &mut e, &mut st, vec![]);
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
