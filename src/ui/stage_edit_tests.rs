use super::*;

#[test]
fn a_new_screen_shows_the_first_display_and_stands_apart() {
    let mut show = Show::default();
    let d = show.outputs[0].id;
    let a = add_surface(&mut show, Preset::Flat);
    let b = add_surface(&mut show, Preset::Flat);
    let j = add_surface(&mut show, Preset::Jumbotron);
    let sa = show.stage3d.surface(a).unwrap();
    let sb = show.stage3d.surface(b).unwrap();
    let sj = show.stage3d.surface(j).unwrap();
    assert_eq!(sa.name, "Screen 1");
    assert_eq!(sb.name, "Screen 2");
    assert_eq!(sj.name, "Jumbotron 1");
    assert_eq!(sa.feed.output(), Some(d));
    // On the floor, side by side; the jumbotron hangs.
    assert!((sa.placement.position[1] - 2.25 / 2.0).abs() < 1e-4);
    assert_ne!(sa.placement.position, sb.placement.position);
    assert!(sj.placement.position[1] > 4.0);
    // Every preset makes something to draw.
    for p in Preset::ALL {
        assert!(!geometry::surface_mesh(&p.shape()).is_empty(), "{p:?}");
    }
}

#[test]
fn a_display_that_shows_the_stage_is_not_offered_to_a_screen() {
    let mut show = Show::default();
    let a = show.outputs[0].id;
    let b = show.add_display();
    assert_eq!(feed_choices(&show).len(), 2);
    show.outputs.iter_mut().find(|o| o.id == a).unwrap().source = Some(StageSource::default());
    let choices = feed_choices(&show);
    assert_eq!(choices.iter().map(|(o, _)| *o).collect::<Vec<_>>(), vec![b]);
    // A new screen shows the first display that doesn't.
    assert_eq!(first_feed(&show).output(), Some(b));
    // A projector is offered with what it shows.
    let (_wide, left, _right) = show.add_blended_pair(1920, 1080, 0.1);
    let (_, label) = feed_choices(&show)
        .into_iter()
        .find(|(o, _)| *o == left)
        .unwrap();
    assert!(label.contains("whole picture"), "{label}");
}

#[test]
fn edits_from_a_view_go_through_the_show_edit_path() {
    let mut show = Show::default();
    let mut st = UiState::default();
    let mut e = crate::engine::Engine::for_tests();
    apply(
        StageEdit::AddSurface(Preset::Curved),
        &mut show,
        &mut e,
        &mut st,
    );
    assert_eq!(show.stage3d.surfaces.len(), 1);
    assert_eq!(st.stage.picked, Some(show.stage3d.surfaces[0].id));
    let vp = Viewpoint {
        name: "FOH".into(),
        eye: [0.0, 2.0, 20.0],
        target: [0.0, 2.0, 0.0],
        fov_deg: 50.0,
    };
    apply(
        StageEdit::AddViewpoint(vp.clone()),
        &mut show,
        &mut e,
        &mut st,
    );
    apply(StageEdit::AddViewpoint(vp), &mut show, &mut e, &mut st);
    let names: Vec<&str> = show
        .stage3d
        .viewpoints
        .iter()
        .map(|v| v.name.as_str())
        .collect();
    assert_eq!(names, vec!["FOH", "FOH 2"]);
    // A new venue file keeps the old one's settings.
    apply(
        StageEdit::SetVenue(PathBuf::from("/venues/a.glb")),
        &mut show,
        &mut e,
        &mut st,
    );
    show.stage3d.venue.as_mut().unwrap().units = LengthUnit::Centimetres;
    apply(
        StageEdit::SetVenue(PathBuf::from("/venues/b.obj")),
        &mut show,
        &mut e,
        &mut st,
    );
    let v = show.stage3d.venue.as_ref().unwrap();
    assert_eq!(v.path, PathBuf::from("/venues/b.obj"));
    assert_eq!(v.units, LengthUnit::Centimetres);
    // They are edits of the show: refused in show mode like any other.
    assert!(Action::Stage(StageEdit::AddSurface(Preset::Flat)).edits_the_show());
    st.show_mode = true;
    crate::ui::apply(
        Action::Stage(StageEdit::AddSurface(Preset::Flat)),
        &mut show,
        &mut e,
        &mut st,
    );
    assert_eq!(
        show.stage3d.surfaces.len(),
        1,
        "show mode keeps it as it was"
    );
}

#[test]
fn a_sample_venue_comes_with_its_screens_and_a_file_takes_its_place() {
    let mut show = Show::default();
    let d = show.outputs[0].id;
    let mut st = UiState::default();
    let mut e = crate::engine::Engine::for_tests();
    apply(
        StageEdit::UseSample {
            kind: SampleVenue::Arena,
            usual: true,
        },
        &mut show,
        &mut e,
        &mut st,
    );
    let v = show.stage3d.venue.clone().unwrap();
    assert_eq!(v.sample, Some(SampleVenue::Arena));
    assert!(!show.stage3d.surfaces.is_empty());
    assert!(
        show.stage3d
            .surfaces
            .iter()
            .all(|s| s.feed.output() == Some(d))
    );
    assert!(!show.stage3d.viewpoints.is_empty());
    // Another sample without its screens keeps the screens there are.
    let n = show.stage3d.surfaces.len();
    show.stage3d.venue.as_mut().unwrap().brightness = 0.5;
    assert!(use_sample(&mut show, SampleVenue::Club, false).is_empty());
    assert_eq!(show.stage3d.surfaces.len(), n);
    let v = show.stage3d.venue.as_ref().unwrap();
    assert_eq!((v.sample, v.brightness), (Some(SampleVenue::Club), 0.5));
    // A model file in its place: no sample any more.
    apply(
        StageEdit::SetVenue(PathBuf::from("/venues/hall.glb")),
        &mut show,
        &mut e,
        &mut st,
    );
    let v = show.stage3d.venue.as_ref().unwrap();
    assert_eq!(v.sample, None);
    assert_eq!(v.path, PathBuf::from("/venues/hall.glb"));
    // Writing a sample out is not an edit of the show (allowed in show mode).
    let path = std::env::temp_dir().join(format!("newq-sample-{}.glb", std::process::id()));
    let x = StageEdit::ExportGlb(SampleVenue::BlackBox, path.clone());
    assert!(!Action::Stage(x.clone()).edits_the_show());
    apply(x, &mut show, &mut e, &mut st);
    let rx = st.stage.exports.pop().unwrap();
    let written = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap()
        .unwrap();
    assert!(std::fs::metadata(&written).unwrap().len() > 1000);
    let _ = std::fs::remove_file(&path);
}

// The 3D stage chosen from Settings ▸ Displays, and the 3D stage page's list
// of the displays that show it, drawn in the whole window at 1280×760 in
// both interfaces and driven with the mouse.

use crate::dock::Pane;
use crate::ui::SettingsPage;
use egui::{Event, PointerButton, Pos2, RawInput, Rect, vec2};

const SIZE: egui::Vec2 = vec2(1280.0, 760.0);

struct Win {
    ctx: egui::Context,
    e: crate::engine::Engine,
    t: f64,
    texts: Vec<String>,
    /// Where each text was drawn, the last frame.
    spots: Vec<(String, Pos2)>,
    size: egui::Vec2,
}

impl Win {
    /// The window with the settings open at `page`.
    fn new(modern: bool, page: SettingsPage, show: &mut Show) -> (Win, UiState) {
        Win::sized(modern, page, show, SIZE)
    }

    /// The same in a window of `size`.
    fn sized(
        modern: bool,
        page: SettingsPage,
        show: &mut Show,
        size: egui::Vec2,
    ) -> (Win, UiState) {
        let mut st = UiState::default();
        st.prefs.new_layout = modern;
        st.layout.open(Pane::Settings);
        st.settings_page = page;
        let mut w = Win {
            ctx: egui::Context::default(),
            e: crate::engine::Engine::for_tests(),
            t: 1.0,
            texts: Vec::new(),
            spots: Vec::new(),
            size,
        };
        w.settle(show, &mut st);
        (w, st)
    }

    fn frame(&mut self, show: &mut Show, st: &mut UiState, events: Vec<Event>) {
        self.t += 1.0 / 60.0;
        self.ctx.data_mut(|d| {
            d.remove::<Vec<(String, Rect, bool)>>(egui::Id::new("stage_sources"));
            d.remove::<Vec<(u64, Rect)>>(egui::Id::new("display_frames"));
        });
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, self.size)),
            time: Some(self.t),
            events,
            ..Default::default()
        };
        let e = &mut self.e;
        let mut out = self
            .ctx
            .run_ui(input, |ui| crate::ui::draw(ui, show, e, st));
        out.textures_delta.clear();
        self.spots = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::epaint::Shape::Text(t) => Some((
                    t.galley.text().to_string(),
                    t.pos + t.galley.rect.center().to_vec2(),
                )),
                _ => None,
            })
            .collect();
        self.texts = self.spots.iter().map(|(t, _)| t.clone()).collect();
    }

    fn settle(&mut self, show: &mut Show, st: &mut UiState) {
        for _ in 0..3 {
            self.frame(show, st, vec![]);
        }
    }

    fn rects(&self) -> Vec<(String, Rect, bool)> {
        self.ctx
            .data(|d| d.get_temp::<Vec<(String, Rect, bool)>>(egui::Id::new("stage_sources")))
            .unwrap_or_default()
    }

    /// Where `name` was drawn, and whether it was in sight.
    fn find(&self, name: &str) -> Option<(Rect, bool)> {
        self.rects()
            .into_iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, r, seen)| (r, seen))
    }

    fn display_frame(&self, id: OutputId) -> Option<Rect> {
        self.ctx
            .data(|d| d.get_temp::<Vec<(u64, Rect)>>(egui::Id::new("display_frames")))
            .unwrap_or_default()
            .into_iter()
            .find(|(d, _)| *d == id.0)
            .map(|(_, r)| r)
    }

    fn click_at(&mut self, show: &mut Show, st: &mut UiState, at: Rect) {
        let p = at.center();
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

    /// Clicks the control `name`, scrolling the page down to it first if it
    /// is out of sight.
    fn press(&mut self, show: &mut Show, st: &mut UiState, name: &str) {
        let r = self.see(show, st, name);
        self.click_at(show, st, r);
    }

    /// Scrolls the control `name` into sight, and where it is.
    fn see(&mut self, show: &mut Show, st: &mut UiState, name: &str) -> Rect {
        for _ in 0..40 {
            match self.find(name) {
                Some((_, true)) | None => break,
                Some((r, false)) => {
                    // Sideways to one off the side of the 3D stage's tab
                    // (a cramped page), else down to one below, up to one
                    // above.
                    let side = self.find("3D stage ▸ tab content").map(|(c, _)| c);
                    match side {
                        Some(c) if r.max.x > c.max.x => self.wheel_by(show, st, vec2(-80.0, 0.0)),
                        Some(c) if r.min.x < c.min.x => self.wheel_by(show, st, vec2(80.0, 0.0)),
                        _ => {
                            let down = if r.center().y > self.size.y * 0.5 {
                                -80.0
                            } else {
                                80.0
                            };
                            self.wheel(show, st, down);
                        }
                    }
                }
            }
        }
        let r = self
            .find(name)
            .filter(|(_, seen)| *seen)
            .map(|(r, _)| r)
            .unwrap_or_else(|| panic!("{name} isn't in sight; drawn: {:?}", self.rects()));
        assert!(
            Rect::from_min_size(Pos2::ZERO, self.size).contains_rect(r),
            "{name} is in the window: {r:?}"
        );
        r
    }

    /// Turns the wheel over the page: on the 3D stage page, over its tab's
    /// content (over the pinned view the wheel moves the camera).
    fn wheel(&mut self, show: &mut Show, st: &mut UiState, down: f32) {
        self.wheel_by(show, st, vec2(0.0, down));
    }

    fn wheel_by(&mut self, show: &mut Show, st: &mut UiState, delta: egui::Vec2) {
        let at = match self.find("3D stage ▸ tab content") {
            Some((r, _)) => r.center(),
            None => Pos2::new(self.size.x * 0.6, self.size.y * 0.5),
        };
        self.frame(show, st, vec![Event::PointerMoved(at)]);
        self.frame(
            show,
            st,
            vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta,
                phase: egui::TouchPhase::Move,
                modifiers: Default::default(),
            }],
        );
        self.settle(show, st);
    }

    /// Clicks the text `words` where it was last drawn (the last of them:
    /// a menu's is over the page's).
    fn click_text(&mut self, show: &mut Show, st: &mut UiState, words: &str) {
        let at = self
            .spots
            .iter()
            .rev()
            .find(|(t, _)| t == words)
            .map(|(_, p)| *p)
            .unwrap_or_else(|| panic!("no {words:?} in {:?}", self.texts));
        self.click_at(show, st, Rect::from_center_size(at, vec2(1.0, 1.0)));
    }

    /// Clicks the 3D stage page's tab `label`.
    fn tab(&mut self, show: &mut Show, st: &mut UiState, label: &str) {
        self.press(show, st, &format!("3D stage ▸ tab ▸ {label}"));
    }
}

fn foh() -> Viewpoint {
    Viewpoint {
        name: "FOH".into(),
        eye: [0.0, 2.0, 20.0],
        target: [0.0, 2.0, 0.0],
        fov_deg: 50.0,
    }
}

#[test]
fn a_display_is_set_to_show_the_stage_on_the_displays_page() {
    for modern in [false, true] {
        let mut show = Show::default();
        show.stage3d.viewpoints.push(foh());
        let other = show.add_display();
        let (id, name) = (show.outputs[0].id, show.outputs[0].name.clone());
        let (mut w, mut st) = Win::new(modern, SettingsPage::Displays, &mut show);
        let menu = format!("Displays ▸ {name} ▸ Shows");
        w.press(&mut show, &mut st, &menu);
        w.press(
            &mut show,
            &mut st,
            &format!("{menu} ▸ The 3D stage, from FOH"),
        );
        assert_eq!(
            show.output(id).unwrap().source,
            Some(StageSource {
                viewpoint: Some("FOH".into())
            }),
            "modern {modern}"
        );
        assert!(show.shows_stage(id) && !show.shows_stage(other));
        assert!(
            w.texts.iter().any(|t| t == "The 3D stage, from FOH"),
            "modern {modern}: the row says so"
        );
        // All of it, from in front, is offered too.
        w.press(&mut show, &mut st, &menu);
        w.press(
            &mut show,
            &mut st,
            &format!("{menu} ▸ The 3D stage, all of it"),
        );
        assert_eq!(
            show.output(id).unwrap().source,
            Some(StageSource::default())
        );
        // And back to its cues.
        w.press(&mut show, &mut st, &menu);
        w.press(&mut show, &mut st, &format!("{menu} ▸ Its own cues"));
        assert_eq!(show.output(id).unwrap().source, None, "modern {modern}");
    }
}

#[test]
fn a_display_that_shows_part_of_another_says_why_it_cant_show_the_stage() {
    for modern in [false, true] {
        let mut show = Show::default();
        let (wide, left, right) = show.add_blended_pair(1920, 1080, 0.1);
        let name = |show: &Show, id| show.output(id).unwrap().name.clone();
        let (wide_name, left_name) = (name(&show, wide), name(&show, left));
        let (mut w, mut st) = Win::new(modern, SettingsPage::Displays, &mut show);
        let menu = format!("Displays ▸ {left_name} ▸ Shows");
        w.press(&mut show, &mut st, &menu);
        let why = show.stage_source_refused(left).unwrap();
        assert!(
            w.texts.iter().any(|t| t == why),
            "modern {modern}: the reason is on the menu"
        );
        // Greyed: a click does nothing.
        w.press(
            &mut show,
            &mut st,
            &format!("{menu} ▸ The 3D stage, all of it"),
        );
        assert_eq!(show.output(left).unwrap().source, None, "modern {modern}");
        assert!(show.output(left).unwrap().part_of.is_some());
        // The menu stays open over a greyed item: a click outside closes it.
        w.click_at(
            &mut show,
            &mut st,
            Rect::from_center_size(Pos2::new(SIZE.x - 40.0, SIZE.y - 60.0), vec2(1.0, 1.0)),
        );
        // The whole picture the pair shares may show it, and both
        // projectors then do.
        let menu = format!("Displays ▸ {wide_name} ▸ Shows");
        w.press(&mut show, &mut st, &menu);
        w.press(
            &mut show,
            &mut st,
            &format!("{menu} ▸ The 3D stage, all of it"),
        );
        assert_eq!(
            show.output(wide).unwrap().source,
            Some(StageSource::default()),
            "modern {modern}"
        );
        assert!(show.shows_stage(left) && show.shows_stage(right));
    }
}

#[test]
fn a_display_that_takes_part_of_another_lets_its_own_stage_go() {
    let mut show = Show::default();
    let mut st = UiState::default();
    let mut e = crate::engine::Engine::for_tests();
    let a = show.outputs[0].id;
    let b = show.add_display();
    show.outputs[1].source = Some(StageSource::default());
    crate::ui::apply(Action::ShowPartOf(b, Some(a)), &mut show, &mut e, &mut st);
    assert_eq!(show.output(b).unwrap().source, None);
    assert!(!show.shows_stage(b));
}

#[test]
fn the_stage_page_lists_the_displays_that_show_it_and_goes_to_each() {
    for modern in [false, true] {
        let mut show = Show::default();
        show.stage3d.viewpoints.push(foh());
        for _ in 0..6 {
            show.add_display();
        }
        let last = show.outputs.last().unwrap().id;
        let name = show.outputs.last().unwrap().name.clone();
        let (mut w, mut st) = Win::new(modern, SettingsPage::Stage3d, &mut show);
        show.outputs.last_mut().unwrap().source = Some(StageSource {
            viewpoint: Some("FOH".into()),
        });
        w.settle(&mut show, &mut st);
        w.tab(&mut show, &mut st, "Displays");
        assert_eq!(st.stage.tab, StageTab::Displays);
        w.press(&mut show, &mut st, &format!("3D stage ▸ {name} ▸ Open"));
        assert_eq!(st.settings_page, SettingsPage::Displays, "modern {modern}");
        // It scrolls there smoothly, in a fraction of a second.
        for _ in 0..20 {
            w.frame(&mut show, &mut st, vec![]);
        }
        let at = w.display_frame(last).expect("the display is drawn");
        assert!(
            at.min.y > 0.0 && at.max.y < SIZE.y,
            "modern {modern}: the display at {at:?}"
        );
        assert!(st.reveal_display.get().is_none());
        assert!(
            w.texts.iter().any(|t| t == "The 3D stage, from FOH"),
            "modern {modern}: its Shows says the stage"
        );
    }
}

#[test]
fn with_no_display_on_the_stage_the_page_points_to_the_displays_page() {
    for modern in [false, true] {
        let mut show = Show::default();
        let (mut w, mut st) = Win::new(modern, SettingsPage::Stage3d, &mut show);
        w.tab(&mut show, &mut st, "Displays");
        w.press(&mut show, &mut st, "3D stage ▸ Go to Displays");
        assert!(
            w.find("3D stage ▸ Go to Displays").is_none(),
            "modern {modern}: the page has gone"
        );
        assert_eq!(st.settings_page, SettingsPage::Displays, "modern {modern}");
    }
}

/// A show whose Screens tab is longer than the page: six screens, each open.
fn many_screens() -> Show {
    let mut show = Show::default();
    show.stage3d.viewpoints.push(foh());
    for _ in 0..6 {
        add_surface(&mut show, Preset::Flat);
    }
    show
}

#[test]
fn the_tabs_switch_by_mouse_and_each_shows_its_own_part() {
    for modern in [false, true] {
        let mut show = many_screens();
        let (mut w, mut st) = Win::new(modern, SettingsPage::Stage3d, &mut show);
        assert_eq!(st.stage.tab, StageTab::Venue, "the first tab to start with");
        // What only each tab has on it.
        let only = [
            ("Venue", "Load a venue model…"),
            ("Screens", "+ Add a screen"),
            ("Projectors", "+ Add a projector"),
            ("Viewpoints", "FOH"),
            ("Displays", "No display shows the stage."),
        ];
        for (tab, _) in only {
            w.tab(&mut show, &mut st, tab);
            assert_eq!(st.stage.tab.section().name, tab, "modern {modern}");
            for (other, theirs) in only {
                let has = w.texts.iter().any(|t| t == theirs);
                assert_eq!(
                    has,
                    other == tab,
                    "modern {modern}: on {tab}, {theirs:?} {}",
                    if has { "is drawn" } else { "is missing" }
                );
            }
        }
        // The view stays on every tab.
        assert!(w.find("3D stage ▸ view").is_some_and(|(_, seen)| seen));
    }
}

#[test]
fn the_view_stays_put_while_the_tab_scrolls() {
    for modern in [false, true] {
        // Forty viewpoints: the tab is longer than the page.
        let mut show = many_screens();
        let names: Vec<String> = (1..=40).map(|i| format!("Seat {i}")).collect();
        for n in &names {
            show.stage3d.viewpoints.push(Viewpoint {
                name: n.clone(),
                ..foh()
            });
        }
        let (mut w, mut st) = Win::new(modern, SettingsPage::Stage3d, &mut show);
        w.tab(&mut show, &mut st, "Viewpoints");
        let (view, seen) = w.find("3D stage ▸ view").expect("the view is drawn");
        assert!(seen && view.height() >= 90.0, "modern {modern}: {view:?}");
        let tabs = w.find("3D stage ▸ tab ▸ Viewpoints").unwrap().0;
        let before = w.texts.clone();
        for _ in 0..6 {
            w.wheel(&mut show, &mut st, -200.0);
        }
        let (after, seen) = w.find("3D stage ▸ view").unwrap();
        assert!(seen, "modern {modern}");
        assert_eq!(view, after, "modern {modern}: the view didn't move");
        assert_eq!(
            tabs,
            w.find("3D stage ▸ tab ▸ Viewpoints").unwrap().0,
            "modern {modern}: nor the tabs"
        );
        // The content did: the first seat has gone out of sight and the
        // last has come into it.
        let last = names.last().unwrap();
        assert!(
            before.contains(&names[0]) && !before.contains(last),
            "modern {modern}: {before:?}"
        );
        assert!(
            !w.texts.contains(&names[0]) && w.texts.contains(last),
            "modern {modern}: the tab's content scrolled: {:?}",
            w.texts
        );
        // The wheel over the view turns nothing on the page.
        let at = view.center();
        w.frame(&mut show, &mut st, vec![Event::PointerMoved(at)]);
        w.frame(
            &mut show,
            &mut st,
            vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, 200.0),
                phase: egui::TouchPhase::Move,
                modifiers: Default::default(),
            }],
        );
        w.settle(&mut show, &mut st);
        assert_eq!(
            view,
            w.find("3D stage ▸ view").unwrap().0,
            "modern {modern}"
        );
    }
}

#[test]
fn the_view_is_made_bigger_by_its_handle_and_never_hides_the_tabs() {
    for modern in [false, true] {
        let mut show = many_screens();
        let (mut w, mut st) = Win::new(modern, SettingsPage::Stage3d, &mut show);
        let before = w.find("3D stage ▸ view").unwrap().0;
        let tabs_before = w.find("3D stage ▸ tab ▸ Venue").unwrap().0;
        // The new interface has the view on top of the tabs; the classic
        // window's settings, short under the transport, have it beside them.
        assert_eq!(
            tabs_before.min.y > before.max.y,
            modern,
            "modern {modern}: {before:?} and the tabs at {tabs_before:?}"
        );
        let grip = w.find("3D stage ▸ view size").unwrap().0.center();
        let way = if modern {
            vec2(0.0, 1.0)
        } else {
            vec2(1.0, 0.0)
        };
        let b = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        w.frame(&mut show, &mut st, vec![Event::PointerMoved(grip)]);
        w.frame(&mut show, &mut st, vec![b(grip, true)]);
        for i in 1..=10 {
            let at = grip + way * (1000.0 * i as f32 / 10.0);
            w.frame(&mut show, &mut st, vec![Event::PointerMoved(at)]);
        }
        let end = grip + way * 1000.0;
        w.frame(&mut show, &mut st, vec![b(end, false)]);
        w.settle(&mut show, &mut st);
        let after = w.find("3D stage ▸ view").unwrap().0;
        let grown = if modern {
            after.height() - before.height()
        } else {
            after.width() - before.width()
        };
        // (The classic page is narrow: the view can't take much from the tab.)
        let least = if modern { 50.0 } else { 20.0 };
        assert!(grown > least, "modern {modern}: {before:?} to {after:?}");
        // As far as it goes, the tabs and some of the content stay in sight.
        let (tabs, seen) = w.find("3D stage ▸ tab ▸ Displays").unwrap();
        assert!(seen, "modern {modern}: {tabs:?}");
        let (content, _) = w.find("3D stage ▸ tab content").unwrap();
        assert!(
            content.height() >= 80.0 && content.width() >= 300.0,
            "modern {modern}: {content:?}"
        );
    }
}

#[test]
fn a_screen_or_projector_picked_in_the_view_opens_its_tab() {
    let mut show = many_screens();
    let id = show.stage3d.surfaces[2].id;
    let p = show.add_projector();
    let (mut w, mut st) = Win::new(false, SettingsPage::Stage3d, &mut show);
    w.tab(&mut show, &mut st, "Viewpoints");
    // As a click in the view does.
    st.stage.picked = Some(id);
    w.settle(&mut show, &mut st);
    assert_eq!(st.stage.tab, StageTab::Screens);
    // It stays where it is taken to afterwards.
    w.tab(&mut show, &mut st, "Venue");
    assert_eq!(st.stage.tab, StageTab::Venue);
    st.stage.picked = None;
    st.stage.projectors.picked = Some(p);
    w.settle(&mut show, &mut st);
    assert_eq!(st.stage.tab, StageTab::Projectors);
    w.tab(&mut show, &mut st, "Viewpoints");
    assert_eq!(st.stage.tab, StageTab::Viewpoints);
}

#[test]
fn a_narrow_classic_settings_pane_keeps_every_tab() {
    // The classic window, small: the settings have little room beside the
    // sidebar, cramped but whole.
    let mut show = many_screens();
    show.add_projector();
    let size = vec2(720.0, 480.0);
    let (mut w, mut st) = Win::sized(false, SettingsPage::Stage3d, &mut show, size);
    for tab in StageTab::ALL {
        let label = tab.section().name;
        w.tab(&mut show, &mut st, label);
        assert_eq!(st.stage.tab, tab);
        // Cramped: the view scrolls with the rest, at its least.
        let view = w.find("3D stage ▸ view").unwrap().0;
        assert!(view.height() >= 90.0, "{label}: {view:?}");
        for _ in 0..3 {
            w.wheel(&mut show, &mut st, -300.0);
        }
    }
    // The Screens tab's add button can still be reached: scrolled to.
    w.tab(&mut show, &mut st, "Screens");
    let mut seen = false;
    for _ in 0..20 {
        seen = w.texts.iter().any(|t| t == "+ Add a screen");
        if seen {
            break;
        }
        w.wheel(&mut show, &mut st, -40.0);
    }
    assert!(seen, "{:?}", w.texts);
}

#[test]
fn a_pick_opens_its_tab_once_even_with_nothing_on_it() {
    // A screen picked in a Preview pane's view, then gone (undone): the page
    // opens on Screens once and lets another tab be chosen.
    let mut show = Show::default();
    let (mut w, mut st) = Win::new(false, SettingsPage::Stage3d, &mut show);
    st.stage.picked = Some(SurfaceId(999));
    w.settle(&mut show, &mut st);
    assert_eq!(st.stage.tab, StageTab::Screens);
    w.tab(&mut show, &mut st, "Venue");
    w.settle(&mut show, &mut st);
    assert_eq!(st.stage.tab, StageTab::Venue, "it stays");
    // Likewise a projector.
    st.stage.projectors.picked = Some(crate::stage3d::ProjectorId(998));
    w.settle(&mut show, &mut st);
    assert_eq!(st.stage.tab, StageTab::Projectors);
    w.tab(&mut show, &mut st, "Viewpoints");
    w.settle(&mut show, &mut st);
    assert_eq!(st.stage.tab, StageTab::Viewpoints, "it stays");
}

#[test]
fn a_screen_and_a_projector_are_added_through_their_tabs() {
    for modern in [false, true] {
        let mut show = Show::default();
        let (mut w, mut st) = Win::new(modern, SettingsPage::Stage3d, &mut show);
        w.tab(&mut show, &mut st, "Screens");
        w.click_text(&mut show, &mut st, "+ Add a screen");
        w.click_text(&mut show, &mut st, Preset::Flat.label());
        assert_eq!(show.stage3d.surfaces.len(), 1, "modern {modern}");
        assert_eq!(st.stage.tab, StageTab::Screens, "modern {modern}");
        // Its rows opened by themselves: its name is there to be changed.
        assert!(
            w.texts.iter().any(|t| t == "Name"),
            "modern {modern}: {:?}",
            w.texts
        );
        w.tab(&mut show, &mut st, "Projectors");
        w.click_text(&mut show, &mut st, "+ Add a projector");
        w.click_text(&mut show, &mut st, "Aimed at Screen 1");
        assert_eq!(show.stage3d.projectors.len(), 1, "modern {modern}");
        assert_eq!(st.stage.tab, StageTab::Projectors, "modern {modern}");
        assert!(
            w.texts.iter().any(|t| t == "Projector 1"),
            "modern {modern}: {:?}",
            w.texts
        );
    }
}

#[test]
fn the_page_is_laid_out_by_the_room_it_has() {
    use Arrangement::*;
    // Tall enough: the view on top.
    assert_eq!(arrangement(vec2(1000.0, 600.0)), Stacked);
    assert_eq!(arrangement(vec2(300.0, STACKED_FROM)), Stacked);
    // Short and wide (the classic window): beside the tabs.
    assert_eq!(arrangement(vec2(600.0, 245.0)), SideBySide);
    assert_eq!(arrangement(vec2(SIDE_BY_SIDE_FROM, VIEW_LEAST)), SideBySide);
    // Short and narrow: stacked while it fits, else all of it scrolls.
    assert_eq!(arrangement(vec2(400.0, SHORT_PAGE)), Stacked);
    assert_eq!(arrangement(vec2(400.0, SHORT_PAGE - 1.0)), Cramped);
    assert_eq!(arrangement(vec2(1000.0, VIEW_LEAST - 1.0)), Cramped);
    // The view never leaves the content less than its least.
    assert_eq!(
        view_height(600.0, Some(10_000.0)),
        600.0 - CHROME - CONTENT_LEAST
    );
    assert_eq!(view_height(600.0, Some(0.0)), VIEW_LEAST);
    assert_eq!(
        view_width(602.0, Some(10_000.0)),
        602.0 - SIDE_CONTENT_LEAST
    );
    assert_eq!(view_width(602.0, Some(0.0)), SIDE_VIEW_LEAST);
}

impl Win {
    /// Clicks an entry of a page's *+ Add* menu (noted as "page_adds").
    fn press_add(&mut self, show: &mut Show, st: &mut UiState, name: &str) {
        let r = self
            .ctx
            .data(|d| d.get_temp::<Vec<(String, Rect)>>(egui::Id::new("page_adds")))
            .unwrap_or_default()
            .into_iter()
            .rev()
            .find(|(n, _)| n == name)
            .map(|(_, r)| r)
            .unwrap_or_else(|| panic!("no {name}"));
        self.click_at(show, st, r);
    }

    fn has(&self, words: &str) -> bool {
        self.texts.iter().any(|t| t == words)
    }
}

/// A show put right as a loaded one is equals itself: what the page lets
/// be made is what `sanitize_stage` keeps.
fn kept_by_sanitize(show: &Show) -> bool {
    let mut again = show.clone();
    again.sanitize_stage();
    serde_json::to_value(&again).ok() == serde_json::to_value(show).ok()
}

#[test]
fn a_canvas_is_added_a_screen_shows_it_and_a_mapped_projector_reads_as_one_everywhere() {
    for modern in [false, true] {
        let mut show = Show::default();
        let d = show.outputs[0].id;
        show.outputs[0].name = "Proj".into();
        let screen = add_surface(&mut show, Preset::Flat);
        // A projector aimed at the screen, whose display is Proj (which the
        // new screen shows, as the first display).
        let st0 = UiState::default();
        let p = super::super::stage_projectors::add_aimed_at(&mut show, &st0, screen);
        show.projector_mut(p).unwrap().display = Some(d);
        // A video cue on Proj, to be told it isn't seen once Proj is mapped.
        let mut cue = show.make_cue(crate::show::CueType::Video, None, 0);
        if let Some(m) = cue.kind.media_mut() {
            m.output = Some(d);
        }
        let cue_id = cue.id;
        show.cues.push(cue);
        let (mut w, mut st) = Win::new(modern, SettingsPage::Displays, &mut show);

        // + Add ▸ Canvas: a display with no monitor, a picture of its own.
        w.press_add(&mut show, &mut st, "Displays ▸ + Add");
        w.press_add(&mut show, &mut st, "Displays ▸ + Add ▸ Canvas");
        let canvas = show.outputs.last().unwrap().id;
        let c = show.output(canvas).unwrap();
        assert_eq!(c.name, "Canvas 1", "modern {modern}");
        assert!(c.monitor.is_none() && c.picture == Some((1920, 1080)));
        // Not a canvas until a screen shows it; its row offers a screen.
        assert!(!show.is_canvas(canvas));
        assert!(w.find("Displays ▸ Canvas 1 ▸ Put it on a screen").is_some());

        // Point the screen at it, on the 3D stage's Screens tab.
        st.settings_page = SettingsPage::Stage3d;
        st.stage.picked = Some(screen);
        w.settle(&mut show, &mut st);
        w.tab(&mut show, &mut st, "Screens");
        let menu = "3D stage ▸ Screen 1 ▸ Shows";
        w.press(&mut show, &mut st, menu);
        w.press(&mut show, &mut st, &format!("{menu} ▸ Canvas 1"));
        assert_eq!(
            show.stage3d.surface(screen).unwrap().feed.output(),
            Some(canvas)
        );
        assert!(show.is_canvas(canvas), "modern {modern}");
        // Worked out: a projector reaches it, so it is projection.
        assert!(
            w.texts
                .iter()
                .any(|t| t == "Works it out: projection, front"),
            "modern {modern}: {:?}",
            w.texts
        );

        // Tick Mapped on the projector.
        w.tab(&mut show, &mut st, "Projectors");
        st.stage.projectors.picked = Some(p);
        w.settle(&mut show, &mut st);
        w.press(&mut show, &mut st, "Projectors ▸ Projector 1 ▸ Mapped");
        assert!(show.projector(p).unwrap().mapped, "modern {modern}");
        assert!(w.has(super::super::stage_projectors::MAPPED_WORDS));
        // One flat screen: the small theatre's line.
        assert!(w.has(super::super::stage_projectors::ONE_FLAT_SCREEN_WORDS));
        assert!(w.has("Maps Canvas 1 onto Screen 1."), "{:?}", w.texts);
        assert!(kept_by_sanitize(&show), "modern {modern}");

        // The Displays page: the canvas's row and the projector's.
        st.settings_page = SettingsPage::Displays;
        w.settle(&mut show, &mut st);
        w.see(&mut show, &mut st, "Displays ▸ Canvas 1 ▸ Canvas");
        for words in [
            "Canvas",
            "Shown on:",
            "No monitor (a canvas for the stage's screens)",
        ] {
            assert!(w.has(words), "modern {modern}: {words:?} in {:?}", w.texts);
        }
        w.see(&mut show, &mut st, "Displays ▸ Proj ▸ Mapped");
        assert!(w.has("Mapped"), "modern {modern}: {:?}", w.texts);
        w.see(&mut show, &mut st, "Displays ▸ Proj ▸ Open Projector 1");
        let words = "The stage's screens, through Projector 1";
        assert!(w.has(words), "modern {modern}: {words:?} in {:?}", w.texts);
        assert!(
            w.texts
                .iter()
                .any(|t| t.starts_with("1 cue is sent to this display.")),
            "modern {modern}: {:?}",
            w.texts
        );
        // Its screen button goes to the screen.
        w.press(
            &mut show,
            &mut st,
            "Displays ▸ Canvas 1 ▸ Shown on: Screen 1",
        );
        assert_eq!(st.settings_page, SettingsPage::Stage3d);
        assert_eq!(st.stage.tab, StageTab::Screens);
        assert_eq!(st.stage.picked, Some(screen));

        // The cue inspector's Display menu.
        st.layout.close(super::super::Pane::Settings);
        st.selected = Some(cue_id);
        if modern {
            st.modern_view = super::super::modern::View::Editor;
        }
        w.settle(&mut show, &mut st);
        let mapped = format!("Proj ({})", super::super::MAPPED_LABEL);
        assert!(w.has(&mapped), "modern {modern}: {:?}", w.texts);
        w.click_text(&mut show, &mut st, &mapped);
        assert!(w.has("Canvas 1 (canvas)"), "modern {modern}: {:?}", w.texts);
        // The mapped display is greyed: clicking it changes nothing.
        w.click_text(&mut show, &mut st, &mapped);
        let on = show
            .cue(cue_id)
            .and_then(|c| c.kind.media())
            .and_then(|m| m.output);
        assert_eq!(on, Some(d), "modern {modern}: still where it was");
        // Choose the canvas instead.
        if !w.has("Canvas 1 (canvas)") {
            w.click_text(&mut show, &mut st, &mapped);
        }
        w.click_text(&mut show, &mut st, "Canvas 1 (canvas)");
        let on = show
            .cue(cue_id)
            .and_then(|c| c.kind.media())
            .and_then(|m| m.output);
        assert_eq!(on, Some(canvas), "modern {modern}");

        // The Patch view (the new interface's).
        if modern {
            st.modern_view = super::super::modern::View::Patch;
            w.settle(&mut show, &mut st);
            let rects = w
                .ctx
                .data(|d| {
                    d.get_temp::<super::super::modern::PatchRects>(egui::Id::new(
                        "modern_patch_rects",
                    ))
                })
                .expect("the patch view was drawn");
            let sub = |t: &str| {
                rects
                    .subs
                    .iter()
                    .find(|(n, _)| n == t)
                    .map(|(_, s)| s.clone())
                    .unwrap_or_default()
            };
            assert_eq!(sub("Canvas 1"), "canvas → Screen 1");
            assert_eq!(sub("Proj"), super::super::MAPPED_LABEL);
        }
        assert!(kept_by_sanitize(&show), "modern {modern}");
    }
}

#[test]
fn a_projector_whose_display_a_screen_shows_is_refused_mapped_and_given_a_canvas_in_one_click() {
    for modern in [false, true] {
        // The usual start: a new screen shows the first display, which is
        // also the projector's.
        let mut show = Show::default();
        let d = show.outputs[0].id;
        let screen = add_surface(&mut show, Preset::Flat);
        let st0 = UiState::default();
        let p = super::super::stage_projectors::add_aimed_at(&mut show, &st0, screen);
        assert_eq!(show.projector(p).unwrap().display, Some(d));
        let (mut w, mut st) = Win::new(modern, SettingsPage::Stage3d, &mut show);
        w.tab(&mut show, &mut st, "Projectors");
        st.stage.projectors.picked = Some(p);
        w.settle(&mut show, &mut st);
        // The tick is greyed: clicking it does nothing, and the reason is said.
        w.press(&mut show, &mut st, "Projectors ▸ Projector 1 ▸ Mapped");
        assert!(!show.projector(p).unwrap().mapped, "modern {modern}");
        assert!(
            w.texts
                .iter()
                .any(|t| t.starts_with("Screen 1 shows this display's picture.")),
            "modern {modern}: {:?}",
            w.texts
        );
        // One click: the screen gets a canvas of its own, and it is mapped.
        w.press(
            &mut show,
            &mut st,
            "Projectors ▸ Projector 1 ▸ Give a canvas",
        );
        assert!(show.projector(p).unwrap().mapped, "modern {modern}");
        let feed = show.stage3d.surface(screen).unwrap().feed.output().unwrap();
        assert_ne!(feed, d);
        assert!(show.is_canvas(feed));
        assert_eq!(show.output(feed).unwrap().name, "Canvas 1");
        assert!(show.mapped_projector(d).is_some());
        assert!(kept_by_sanitize(&show), "modern {modern}");
    }
}

#[test]
fn a_movable_screen_says_where_it_is_now_and_is_put_at_its_places_by_mouse() {
    use super::super::screen_cue::place_words;
    for modern in [false, true] {
        let mut show = Show::default();
        let screen = add_surface(&mut show, Preset::Flat);
        let other = add_surface(&mut show, Preset::Flat);
        let mut cue = show.make_cue(crate::show::CueType::Screen, None, 0);
        if let crate::show::CueKind::Screen(c) = &mut cue.kind {
            c.screen = Some(screen);
            c.to.x = Some(3.0);
            c.to.yaw = Some(30.0);
        }
        let label = super::super::cue_label(&cue);
        show.cues.push(cue);
        let home = show.stage3d.surface(screen).unwrap().placement;
        let there = show.screen_places(screen)[0].1;
        assert_ne!(home, there);
        let (mut w, mut st) = Win::new(modern, SettingsPage::Stage3d, &mut show);
        w.tab(&mut show, &mut st, "Screens");
        st.stage.picked = Some(screen);
        w.settle(&mut show, &mut st);
        let name = show.stage3d.surface(screen).unwrap().name.clone();
        let now = format!("3D stage ▸ {name} ▸ {NOW_AT}");
        w.see(&mut show, &mut st, &now);
        let at_home = format!("{} (at home)", place_words(&home));
        assert!(
            w.has(&at_home),
            "modern {modern}: {at_home:?} in {:?}",
            w.texts
        );
        // A screen no cue moves has neither row.
        let other_name = show.stage3d.surface(other).unwrap().name.clone();
        assert!(
            w.find(&format!("3D stage ▸ {other_name} ▸ {NOW_AT}"))
                .is_none()
        );
        let saved = serde_json::to_value(&show).unwrap();
        // Put at ▸ the cue's place: the engine has it there, the show is
        // as it was.
        let menu = format!("3D stage ▸ {name} ▸ {PUT_AT}");
        w.press(&mut show, &mut st, &menu);
        w.press(&mut show, &mut st, &format!("{menu} ▸ {label}"));
        assert_eq!(w.e.screen_at(screen), Some(there), "modern {modern}");
        assert!(
            w.has(&place_words(&there)),
            "modern {modern}: {:?}",
            w.texts
        );
        assert!(!w.has(&at_home));
        assert_eq!(serde_json::to_value(&show).unwrap(), saved, "not an edit");
        // And home again.
        w.press(&mut show, &mut st, &menu);
        w.press(&mut show, &mut st, &format!("{menu} ▸ {PUT_HOME}"));
        assert_eq!(w.e.screen_at(screen), Some(home), "modern {modern}");
        // (Not in Show mode yet: `ui.rs` greys the whole settings pane
        // there, this menu with it.)
        assert!(w.has(&place_words(&home)), "modern {modern}: {:?}", w.texts);
    }
}
