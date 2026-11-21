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
}

impl Win {
    /// The window with the settings open at `page`.
    fn new(modern: bool, page: SettingsPage, show: &mut Show) -> (Win, UiState) {
        let mut st = UiState::default();
        st.prefs.new_layout = modern;
        st.layout.open(Pane::Settings);
        st.settings_page = page;
        let mut w = Win {
            ctx: egui::Context::default(),
            e: crate::engine::Engine::for_tests(),
            t: 1.0,
            texts: Vec::new(),
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
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
            time: Some(self.t),
            events,
            ..Default::default()
        };
        let e = &mut self.e;
        let mut out = self
            .ctx
            .run_ui(input, |ui| crate::ui::draw(ui, show, e, st));
        out.textures_delta.clear();
        self.texts = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect();
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
        for _ in 0..40 {
            match self.find(name) {
                Some((_, true)) | None => break,
                Some((r, false)) => {
                    // Down to one below, up to one above.
                    let down = if r.center().y > SIZE.y * 0.5 {
                        -80.0
                    } else {
                        80.0
                    };
                    let at = Pos2::new(SIZE.x * 0.6, SIZE.y * 0.5);
                    self.frame(show, st, vec![Event::PointerMoved(at)]);
                    self.frame(
                        show,
                        st,
                        vec![Event::MouseWheel {
                            unit: egui::MouseWheelUnit::Point,
                            delta: vec2(0.0, down),
                            phase: egui::TouchPhase::Move,
                            modifiers: Default::default(),
                        }],
                    );
                    self.settle(show, st);
                }
            }
        }
        let r = self
            .find(name)
            .filter(|(_, seen)| *seen)
            .map(|(r, _)| r)
            .unwrap_or_else(|| panic!("{name} isn't in sight; drawn: {:?}", self.rects()));
        assert!(
            Rect::from_min_size(Pos2::ZERO, SIZE).contains_rect(r),
            "{name} is in the window: {r:?}"
        );
        self.click_at(show, st, r);
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
        w.press(&mut show, &mut st, "3D stage ▸ Go to Displays");
        assert!(
            w.find("3D stage ▸ Go to Displays").is_none(),
            "modern {modern}: the page has gone"
        );
        assert_eq!(st.settings_page, SettingsPage::Displays, "modern {modern}");
    }
}
