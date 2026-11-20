use super::*;
use crate::preview_pace::PreviewId;
use crate::ui::stage_edit::{Preset, add_surface};
use crate::ui::stage_view::{self, StageViewState};
use egui::{Pos2, Rect, vec2};

const HD: f32 = 16.0 / 9.0;

/// A show with two flat screens side by side, and the UI's scene of it.
fn two_screens() -> (Show, UiState, SurfaceId, SurfaceId) {
    let mut show = Show::default();
    let a = add_surface(&mut show, Preset::Flat);
    let b = add_surface(&mut show, Preset::Flat);
    let mut st = UiState::default();
    st.stage.projectors.library_dir = Some(scratch("lib"));
    refresh(&show, &mut st);
    (show, st, a, b)
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "newq-projectors-ui-{name}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// The scene as the app would make it (`StageLive::begin` does in the app).
fn refresh(show: &Show, st: &mut UiState) {
    st.stage.scene = Arc::new(show.stage3d.scene(show, None));
    st.stage.scene_key = show.stage3d.shape_key();
    st.stage.bounds = render3d::scene_bounds(&st.stage.scene);
}

/// Where each text was drawn on the last frame.
type Texts = Vec<(String, Pos2)>;

fn texts(out: &egui::FullOutput) -> Texts {
    out.shapes
        .iter()
        .filter_map(|c| match &c.shape {
            egui::Shape::Text(t) => Some((
                t.galley.text().to_string(),
                t.pos + t.galley.rect.center().to_vec2(),
            )),
            _ => None,
        })
        .collect()
}

fn click_events(at: Pos2) -> Vec<egui::Event> {
    let b = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    vec![egui::Event::PointerMoved(at), b(true), b(false)]
}

/// Draws the Projectors section a frame at a time, clicking where asked.
struct Page {
    ctx: egui::Context,
    t: f64,
    last: Texts,
}

impl Page {
    fn new() -> Page {
        Page {
            ctx: egui::Context::default(),
            t: 1.0,
            last: Vec::new(),
        }
    }

    fn frame(&mut self, show: &mut Show, st: &mut UiState, events: Vec<egui::Event>) {
        self.t += 0.1;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1100.0, 1400.0))),
            time: Some(self.t),
            events,
            ..Default::default()
        };
        let mut out = self.ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| section_ui(ui, show, st));
            });
        });
        out.textures_delta.clear();
        self.last = texts(&out);
    }

    fn at(&self, text: &str) -> Option<Pos2> {
        self.last.iter().find(|(t, _)| t == text).map(|(_, p)| *p)
    }

    /// Click `text` (drawn on the last frame), then draw a frame more.
    fn click(&mut self, show: &mut Show, st: &mut UiState, text: &str) {
        let at = self
            .at(text)
            .unwrap_or_else(|| panic!("no {text:?} in {:?}", self.words()));
        self.frame(show, st, vec![egui::Event::PointerMoved(at)]);
        self.frame(show, st, click_events(at));
        self.frame(show, st, Vec::new());
    }

    fn words(&self) -> Vec<&str> {
        self.last.iter().map(|(t, _)| t.as_str()).collect()
    }
}

#[test]
fn a_projector_is_added_aimed_at_a_screen_and_pointed_at_another_with_the_mouse() {
    let (mut show, mut st, a, b) = two_screens();
    let mut page = Page::new();
    page.frame(&mut show, &mut st, Vec::new());
    assert!(
        page.words().contains(&CORNER_PIN_WORDS),
        "{:?}",
        page.words()
    );
    page.click(&mut show, &mut st, "+ Add a projector");
    page.click(&mut show, &mut st, "Aimed at Screen 1");
    assert_eq!(show.stage3d.projectors.len(), 1);
    let p = show.stage3d.projectors[0].clone();
    assert_eq!(st.stage.projectors.picked, Some(p.id));
    assert_eq!(p.display, Some(show.outputs[0].id), "the first display");
    assert_eq!(LensPreset::of(&p.lens), Some(LensPreset::StandardZoom));
    // Stood where its picture fills Screen 1.
    let screen_a = show.stage3d.surface(a).unwrap().clone();
    let on = p.on_flat(&screen_a, HD).unwrap();
    assert!(on.fills > 0.99, "{}", on.fills);
    // Its row opened by itself, with what it gives in words.
    refresh(&show, &mut st);
    page.frame(&mut show, &mut st, Vec::new());
    let words = page.words().join("\n");
    assert!(words.contains("Picture on Screen 1: 4.0 m by"), "{words}");
    assert!(
        words.contains("fills 99% of it") || words.contains("fills 100% of it"),
        "{words}"
    );
    assert!(words.contains("lux there"), "{words}");
    assert!(words.contains("One pixel is about 2.1 mm"), "{words}");
    // Point it at Screen 2 from where it stands.
    page.click(&mut show, &mut st, "Choose a screen…");
    let last_screen_2 = page
        .last
        .iter()
        .rev()
        .find(|(t, _)| t == "Screen 2")
        .map(|(_, p)| *p)
        .expect("Screen 2 in the menu");
    page.frame(
        &mut show,
        &mut st,
        vec![egui::Event::PointerMoved(last_screen_2)],
    );
    page.frame(&mut show, &mut st, click_events(last_screen_2));
    page.frame(&mut show, &mut st, Vec::new());
    let p = show.stage3d.projectors[0].clone();
    assert_eq!(
        p.placement.position,
        show.stage3d.projectors[0].placement.position
    );
    refresh(&show, &mut st);
    let scene = st.stage.scene.clone();
    let from = show.stage3d.projectors_from(&scene);
    let hit = p
        .aimed_at(&scene, &|i, part| !part.lit && i < from, HD)
        .unwrap();
    assert_eq!(show.stage3d.part_surface(&scene, hit.part), Some(b));
    let middle = screen_middle(show.stage3d.surface(b).unwrap());
    assert!(
        (0..3).all(|i| (hit.point[i] - middle[i]).abs() < 1e-3),
        "{:?} {middle:?}",
        hit.point
    );
    // Which lens from here, for Screen 2.
    page.frame(&mut show, &mut st, Vec::new());
    page.click(&mut show, &mut st, "Which lens from here?");
    let words = page.words().join("\n");
    assert!(
        words.contains("From here Screen 2 needs a throw ratio"),
        "{words}"
    );
    // Kept in My projectors, and offered when adding.
    page.click(&mut show, &mut st, "Keep in My projectors");
    let dir = st.stage.projectors.library_dir.clone().unwrap();
    let kept = load_library(&dir);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].lumens, 10_000.0);
    page.click(&mut show, &mut st, "+ Add a projector");
    assert!(
        page.at("From My projectors").is_some(),
        "{:?}",
        page.words()
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_read_outs_say_what_spills_and_what_a_lens_cant_do() {
    let (mut show, st, a, _) = two_screens();
    let id = add_aimed_at(&mut show, &st, a);
    let p = show.projector_mut(id).unwrap();
    // Moved back, the zoom past its end: the picture is too big.
    p.placement.position[2] += 4.0;
    p.lens.throw = 9.0;
    let p = p.clone();
    let mut st = st;
    refresh(&show, &mut st);
    let (pixels, aspect) = shape_of(&show, &st, &p);
    let said = readout_words(&show, &st, &p, pixels, aspect);
    let warnings: Vec<&String> = said
        .iter()
        .filter_map(|s| match s {
            Said::Warning(w) => Some(w),
            Said::Fact(_) => None,
        })
        .collect();
    assert!(
        warnings.iter().any(|w| w.contains("can't zoom that far")),
        "{said:?}"
    );
    // Back in range but further away: it spills over every edge.
    let mut p = p;
    p.lens.throw = 2.0;
    let said = readout_words(&show, &st, &p, pixels, aspect);
    assert!(
        said.iter()
            .any(|s| matches!(s, Said::Warning(w) if w.contains("over the left") && w.contains("over the bottom"))),
        "{said:?}"
    );
    // Turned away from every screen.
    p.placement.yaw += 180.0;
    let said = readout_words(&show, &st, &p, pixels, aspect);
    assert!(matches!(&said[..], [Said::Warning(w)] if w.contains("lands on no screen")));
}

#[test]
fn words_for_lengths_and_light() {
    assert_eq!(round_words(493.8), "490");
    assert_eq!(round_words(157.2), "160");
    assert_eq!(round_words(8.4), "8");
    assert_eq!(round_words(12_345.0), "12000");
    assert_eq!(round_words(f32::NAN), "0");
    assert_eq!(length_words(5.83, LengthUnit::Metres), "5.8 m");
    assert_eq!(length_words(0.4, LengthUnit::Metres), "40 cm");
    assert_eq!(length_words(3.048, LengthUnit::Feet), "10.0 ft");
    assert_eq!(length_words(0.254, LengthUnit::Feet), "10 in");
    assert_eq!(pixel_words(0.003125, LengthUnit::Metres), "3.1 mm");
    assert_eq!(pixel_words(0.0254, LengthUnit::Feet), "1.00 in");
    let h = Here {
        key: 0,
        on: "Screen 1".into(),
        lit: vec![("Projector 1".into(), 300.0), ("Projector 2".into(), 110.0)],
        lux: 410.0,
        nits: 130.0,
        pixel: Some(0.003),
    };
    assert_eq!(
        here_words(&h, LengthUnit::Metres),
        "Projector 1 + Projector 2 · about 410 lux · about 130 nits · 1 pixel ≈ 3.0 mm"
    );
    let dark = Here {
        lit: Vec::new(),
        ..h
    };
    assert_eq!(
        here_words(&dark, LengthUnit::Metres),
        "No projector lights this spot on Screen 1"
    );
}

#[test]
fn my_projectors_are_kept_on_this_computer_and_a_bad_file_is_not_trusted() {
    let dir = scratch("keep");
    assert!(load_library(&dir).is_empty());
    let saved = Saved {
        name: "Hall's 10K".into(),
        lens: LensPreset::LongZoom.lens(Some(2.5)),
        lumens: 10_000.0,
        resolution: Some((1920, 1200)),
    };
    save_library(&dir, std::slice::from_ref(&saved)).unwrap();
    assert_eq!(load_library(&dir), vec![saved]);
    std::fs::write(dir.join(LIBRARY_FILE), "{ not json").unwrap();
    assert!(load_library(&dir).is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn outlines_and_the_light_under_the_pointer_are_worked_out_off_the_main_thread() {
    let (mut show, mut st, a, b) = two_screens();
    let one = add_aimed_at(&mut show, &st, a);
    let two = add_aimed_at(&mut show, &st, b);
    refresh(&show, &mut st);
    st.stage.projectors.picked = Some(one);
    // At first: the cones only (the outlines are being made).
    let first = view_lines(&show, &mut st, None);
    assert_eq!(first.len(), 16, "four edges and the far end, each");
    assert!(first[..8].iter().all(|l| l.colour == PICKED));
    assert!(first[8..].iter().all(|l| l.colour != PICKED));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut lines = first.clone();
    while std::time::Instant::now() < deadline {
        lines = view_lines(&show, &mut st, None);
        if st.stage.projectors.settled() && lines.len() != 16 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Each outline is a loop of four sides of OUTLINE_RAYS.
    assert_eq!(lines.len(), 8 + 2 * 4 * OUTLINE_RAYS, "{}", lines.len());
    // The same again: the same lines, not made anew.
    assert!(Arc::ptr_eq(&lines, &view_lines(&show, &mut st, None)));
    // Through projector two: its own lines are left out.
    let through = view_lines(&show, &mut st, Some(two));
    assert_eq!(through.len(), 4 + 4 * OUTLINE_RAYS);
    // The light in the middle of Screen 1, from a ray at it.
    let mid = screen_middle(show.stage3d.surface(a).unwrap());
    let ray = Ray {
        origin: [mid[0], mid[1], mid[2] + 5.0],
        dir: [0.0, 0.0, -1.0],
    };
    ask_here(&show, &mut st, ray);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while st.stage.projectors.here.is_none() && std::time::Instant::now() < deadline {
        st.stage.projectors.take_answers();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let here = st.stage.projectors.here.clone().expect("an answer");
    assert_eq!(here.on, "Screen 1");
    assert_eq!(here.lit.len(), 1, "{here:?}");
    assert_eq!(here.lit[0].0, "Projector 1");
    // 10,000 lumens on a 4 m by 2.25 m picture: about 1,100 lux.
    assert!((here.lux - 10_000.0 / 9.0).abs() < 2.0, "{}", here.lux);
    assert!(
        here.pixel
            .is_some_and(|px| (px - 4.0 / 1920.0).abs() < 1e-4)
    );
}

#[test]
fn a_projector_is_picked_in_the_view_and_looked_through() {
    let (mut show, mut st, a, _) = two_screens();
    let id = add_aimed_at(&mut show, &st, a);
    refresh(&show, &mut st);
    let mut view = StageViewState::default();
    let ctx = egui::Context::default();
    let mut t = 1.0;
    let mut frame = |events: Vec<egui::Event>, st: &mut UiState, view: &mut StageViewState| {
        t += 0.1;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 500.0))),
            time: Some(t),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                stage_view::draw(ui, &show, st, view, PreviewId::Stage(0));
            });
        });
        out.textures_delta.clear();
    };
    frame(Vec::new(), &mut st, &mut view);
    // Click where the projector's body is.
    let camera = view.orbit.camera();
    let p = show.projector(id).unwrap().clone();
    let body = [p.position()[0], p.position()[1], p.position()[2] + 0.25];
    let rect = Rect::from_min_size(Pos2::new(8.0, 8.0), vec2(784.0, 484.0));
    let at = camera
        .project(body, (rect.width() as u32, rect.height() as u32))
        .map(|[x, y]| rect.min + vec2(x, y))
        .expect("the projector is in view");
    frame(click_events(at), &mut st, &mut view);
    assert_eq!(st.stage.projectors.picked, Some(id), "picked by its body");
    assert_eq!(st.stage.picked, None);
    // The request carries its cone.
    st.stage.requests.clear();
    frame(Vec::new(), &mut st, &mut view);
    let r = st.stage.requests.last().expect("a picture asked for");
    assert!(r.lines.iter().any(|l| l.colour == PICKED));
    // Through its lens: the lens's camera, and its own lines left out.
    view.through = Some(id);
    st.stage.requests.clear();
    frame(Vec::new(), &mut st, &mut view);
    let r = st.stage.requests.last().unwrap();
    assert_eq!(r.camera, p.camera(HD));
    assert!(r.lines.iter().all(|l| l.colour != PICKED));
    // Escape leaves it, turning round from where the lens was.
    frame(
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }],
        &mut st,
        &mut view,
    );
    assert_eq!(view.through, None);
    let eye = view.orbit.camera().eye;
    assert!(
        (0..3).all(|i| (eye[i] - p.position()[i]).abs() < 0.01),
        "{eye:?}"
    );
}
