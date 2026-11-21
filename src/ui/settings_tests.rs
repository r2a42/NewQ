//! The settings pages drawn in the whole window (both interfaces) at
//! 1280×760 and driven with the mouse: each setting that moved in 0.27 is on
//! its new page and saves to the field it always did, and nothing on disk
//! changes for the move.

use super::*;
use crate::prefs::Prefs;
use egui::{Event, PointerButton, Pos2, RawInput, Rect, vec2};

const SIZE: egui::Vec2 = vec2(1280.0, 760.0);

struct Win {
    ctx: egui::Context,
    e: Engine,
    t: f64,
}

impl Win {
    fn new() -> Win {
        Win {
            ctx: egui::Context::default(),
            e: Engine::for_tests(),
            t: 1.0,
        }
    }

    fn frame(&mut self, show: &mut Show, st: &mut UiState, events: Vec<Event>) {
        self.t += 1.0 / 60.0;
        self.ctx.data_mut(|d| {
            d.remove::<Vec<(String, Rect)>>(egui::Id::new("settings_rects"));
            d.remove::<Vec<String>>(egui::Id::new("settings_sections"));
        });
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
            time: Some(self.t),
            events,
            ..Default::default()
        };
        let e = &mut self.e;
        let mut out = self
            .ctx
            .run_ui(input, |ui| super::super::draw(ui, show, e, st));
        out.textures_delta.clear();
    }

    fn settle(&mut self, show: &mut Show, st: &mut UiState) {
        for _ in 0..3 {
            self.frame(show, st, vec![]);
        }
    }

    fn find(&self, name: &str) -> Option<Rect> {
        self.ctx
            .data(|d| d.get_temp::<Vec<(String, Rect)>>(egui::Id::new("settings_rects")))
            .unwrap_or_default()
            .into_iter()
            .find(|(n, _)| n == name)
            .map(|(_, r)| r)
    }

    /// A point over the sidebar: a row of it in sight.
    fn sidebar_spot(&self) -> Option<Pos2> {
        self.ctx
            .data(|d| d.get_temp::<Vec<(String, Rect)>>(egui::Id::new("settings_rects")))
            .unwrap_or_default()
            .into_iter()
            .find(|(n, r)| n.starts_with("page:") && r.is_positive())
            .map(|(_, r)| r.center())
    }

    fn sections(&self) -> Vec<String> {
        self.ctx
            .data(|d| d.get_temp::<Vec<String>>(egui::Id::new("settings_sections")))
            .unwrap_or_default()
    }

    fn click_at(&mut self, show: &mut Show, st: &mut UiState, p: Pos2) {
        let b = |pressed| Event::PointerButton {
            pos: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        self.frame(show, st, vec![Event::PointerMoved(p)]);
        self.frame(show, st, vec![b(true)]);
        self.frame(show, st, vec![b(false)]);
        self.settle(show, st);
    }

    /// Click the control `name`, scrolling the page (or the sidebar) to it
    /// if it is out of sight.
    fn press(&mut self, show: &mut Show, st: &mut UiState, name: &str) {
        // Down the page first, then back up (it may have been scrolled past).
        for i in 0..45 {
            match self.find(name) {
                Some(r) if r.is_positive() => break,
                Some(_) => {
                    // Over the sidebar for one of its rows, else over the page.
                    let at = if name.starts_with("page:") {
                        self.sidebar_spot()
                    } else {
                        None
                    }
                    .unwrap_or(Pos2::new(SIZE.x * 0.6, SIZE.y * 0.5));
                    self.frame(show, st, vec![Event::PointerMoved(at)]);
                    self.frame(
                        show,
                        st,
                        vec![Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: vec2(0.0, if i < 20 { -80.0 } else { 80.0 }),
                            phase: egui::TouchPhase::Move,
                            modifiers: Default::default(),
                        }],
                    );
                    self.settle(show, st);
                }
                None => break,
            }
        }
        let r = self
            .find(name)
            .filter(|r| r.is_positive())
            .unwrap_or_else(|| panic!("{name} isn't in sight"));
        assert!(
            Rect::from_min_size(Pos2::ZERO, SIZE).contains_rect(r),
            "{name} is in the window: {r:?}"
        );
        self.click_at(show, st, r.center());
    }

    /// Go to `page` the way someone would: its row in the sidebar.
    fn go_to(&mut self, show: &mut Show, st: &mut UiState, page: SettingsPage) {
        self.press(show, st, &format!("page:{}", page.label()));
        assert_eq!(st.settings_page, page);
    }
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("newq_settings_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// The window with the settings open, this computer's settings kept in `dir`.
fn window(modern: bool, prefs: Prefs, dir: &std::path::Path) -> (Win, Show, UiState) {
    let mut st = UiState {
        prefs,
        ..UiState::default()
    };
    st.prefs.new_layout = modern;
    st.layout_dir = Some(dir.to_path_buf());
    st.layout.open(Pane::Settings);
    let mut show = Show::default();
    let mut w = Win::new();
    w.settle(&mut show, &mut st);
    (w, show, st)
}

/// What is in `prefs.json`: exactly `want` as NewQ writes it.
fn saved_is(dir: &std::path::Path, want: &Prefs, what: &str) {
    assert_eq!(&Prefs::load(dir), want, "{what}");
    let text = std::fs::read_to_string(dir.join("prefs.json")).unwrap();
    assert_eq!(text, serde_json::to_string(want).unwrap(), "{what}");
}

#[test]
fn the_sidebar_says_what_is_saved_in_the_show_and_what_stays_here() {
    for modern in [false, true] {
        let dir = scratch(&format!("sidebar_{modern}"));
        let (mut w, mut show, mut st) = window(modern, Prefs::default(), &dir);
        // Every page has its row (the sidebar scrolls when the window is
        // short), and those in sight are in the order of `ALL`.
        let rows: Vec<Rect> = SettingsPage::ALL
            .iter()
            .map(|p| {
                w.find(&format!("page:{}", p.label()))
                    .unwrap_or_else(|| panic!("modern {modern}: {p:?} has a row"))
            })
            .filter(|r| r.is_positive())
            .collect();
        assert!(rows.len() > 6, "modern {modern}: {} in sight", rows.len());
        assert!(rows.windows(2).all(|r| r[0].top() < r[1].top()));
        // Each page opens from its row and draws its sections.
        for page in SettingsPage::ALL {
            w.go_to(&mut show, &mut st, page);
            for s in settings::SECTIONS.iter().filter(|s| s.page == page) {
                assert!(
                    w.sections().contains(&s.heading()),
                    "modern {modern}: {:?} on {page:?}",
                    s.name
                );
            }
        }
        // Clicking around changed nothing that is kept.
        assert!(!dir.join("prefs.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn the_timeline_editor_comes_first_on_the_interface_page() {
    let dir = scratch("timeline_first");
    let (mut w, mut show, mut st) = window(false, Prefs::default(), &dir);
    w.go_to(&mut show, &mut st, SettingsPage::Interface);
    let seen = w.sections();
    assert_eq!(
        seen.first(),
        Some(&settings::TIMELINE_EDITOR.heading()),
        "{seen:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn each_moved_setting_is_on_its_new_page_and_saves_to_its_own_field() {
    for modern in [false, true] {
        let dir = scratch(&format!("moved_{modern}"));
        // Programs and decoding start ticked: clicking them unticks, which
        // puts the process-wide switches where every other test expects them.
        let start = Prefs {
            allow_scripts: true,
            hw_decode: true,
            ..Prefs::default()
        };
        let (mut w, mut show, mut st) = window(modern, start.clone(), &dir);
        let mut want = st.prefs.clone();

        // This computer: Power, Decoding, Allowed here.
        w.go_to(&mut show, &mut st, SettingsPage::ThisComputer);
        w.press(&mut show, &mut st, "keep_awake");
        want.let_computer_sleep = true;
        saved_is(&dir, &want, "keep awake");
        w.press(&mut show, &mut st, "hw_decode");
        want.hw_decode = false;
        saved_is(&dir, &want, "decoding");
        w.press(&mut show, &mut st, "allow_hosting");
        want.allow_hosting = true;
        saved_is(&dir, &want, "streaming server");
        w.press(&mut show, &mut st, "allow_scripts");
        want.allow_scripts = false;
        saved_is(&dir, &want, "programs");
        // The logs are there too.
        assert!(
            w.sections()
                .contains(&settings::RECOVERY_AND_LOGS.heading())
        );

        // Interface: the previews and the rate written on the display (which
        // was a tick in every Preview pane's header).
        w.go_to(&mut show, &mut st, SettingsPage::Interface);
        w.press(&mut show, &mut st, "fps_on_output");
        want.fps_on_output = true;
        saved_is(&dir, &want, "fps on the display");
        w.press(&mut show, &mut st, "previews:off");
        want.previews_off = true;
        saved_is(&dir, &want, "previews off");
        w.press(&mut show, &mut st, "previews:on");
        want.previews_off = false;
        saved_is(&dir, &want, "previews on");

        // And the interface itself, which the page goes on showing.
        let other = if modern {
            "interface:classic"
        } else {
            "interface:new"
        };
        w.press(&mut show, &mut st, other);
        want.new_layout = !modern;
        saved_is(&dir, &want, "the interface");
        assert!(st.settings_open() && st.settings_page == SettingsPage::Interface);
        w.settle(&mut show, &mut st);
        assert!(
            w.find(if modern {
                "interface:new"
            } else {
                "interface:classic"
            })
            .is_some(),
            "the page is still drawn in the other interface"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// A `prefs.json` as 0.26 writes it, with every setting the pages show away
/// from its default.
const PREFS_0_26: &str = r#"{"hw_decode":true,"show_mode":false,"allow_scripts":true,"allow_hosting":true,"let_computer_sleep":true,"fps_on_output":true,"new_layout":true,"preview_cap":"Fps15","preview_size":"Half","preview_strictness":"Strict","previews_off":true,"timeline_layout":"Workspace","timeline_follow":"Centre"}"#;

#[test]
fn a_prefs_file_and_a_show_from_0_26_are_read_and_written_byte_for_byte() {
    // Prefs: read, every page drawn in both interfaces, written again.
    let dir = scratch("round_trip");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("prefs.json"), PREFS_0_26).unwrap();
    let prefs = Prefs::load(&dir);
    assert_ne!(prefs, Prefs::default(), "it was read");
    assert_eq!(serde_json::to_string(&prefs).unwrap(), PREFS_0_26);

    // A show with something on every page, saved as a file.
    let mut show = Show {
        name: "Opening night".into(),
        ..Show::default()
    };
    let d = show.outputs[0].id;
    show.add_sub_display(d);
    show.add_audio_output();
    show.remote.osc = true;
    show.remote.web = true;
    show.sync.audio_ms = 40;
    show.panic_fade = 2.5;
    show.library_folders = vec!["/Content/Looks".into()];
    let c = show.make_cue(CueType::Text, None, 0);
    show.cues.push(c);
    let file = dir.join("show.newq");
    show.save(&file).unwrap();
    let before = std::fs::read(&file).unwrap();
    let mut show = Show::load(&file).unwrap();

    for modern in [false, true] {
        let mut st = UiState {
            prefs: prefs.clone(),
            ..UiState::default()
        };
        st.prefs.new_layout = modern;
        st.layout_dir = Some(dir.clone());
        st.show_path = Some(file.clone());
        st.layout.open(Pane::Settings);
        let mut w = Win::new();
        w.settle(&mut show, &mut st);
        for page in SettingsPage::ALL {
            w.go_to(&mut show, &mut st, page);
        }
    }
    // Nothing was written by looking, and what is there is what was.
    assert_eq!(
        std::fs::read_to_string(dir.join("prefs.json")).unwrap(),
        PREFS_0_26
    );
    let again = dir.join("again.newq");
    show.save(&again).unwrap();
    assert_eq!(
        std::fs::read(&again).unwrap(),
        before,
        "the show is the same"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
