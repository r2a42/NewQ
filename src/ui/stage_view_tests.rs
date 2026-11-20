use super::*;
use crate::preview_pace::{Level, PreviewBudget};
use egui::{Pos2, Rect};

#[test]
fn pictures_are_asked_for_in_steps() {
    assert_eq!(request_size(vec2(1.0, 1.0)), (16, 16));
    assert_eq!(request_size(vec2(640.0, 360.0)), (640, 368));
    assert_eq!(request_size(vec2(641.0, 0.0)), (656, 16));
    assert_eq!(request_size(vec2(f32::NAN, -5.0)), (16, 16));
}

#[test]
fn a_view_draws_when_the_governor_says_or_when_it_has_nothing() {
    let paused = PreviewBudget {
        draw: false,
        scale: 0.25,
        every: std::time::Duration::from_secs(86_400),
        paused: true,
    };
    assert!(wants_picture(PreviewBudget::FULL, true));
    assert!(
        !wants_picture(paused, true),
        "paused keeps the last picture"
    );
    assert!(wants_picture(paused, false), "but a first one is drawn");
    // The governor's scale goes into the quality once; no multisampling below full.
    let view = StageViewState::default();
    let o = view_options(&view, 0.5, None);
    assert_eq!(o.quality.scale, 0.5);
    assert!(!o.quality.msaa);
    assert!(view_options(&view, 1.0, None).quality.msaa);
    assert!(o.grid, "the grid is on at first");
}

fn run(show: &Show, st: &mut UiState, view: &mut StageViewState) {
    let ctx = egui::Context::default();
    let input = egui::RawInput {
        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 500.0))),
        time: Some(1.0),
        ..Default::default()
    };
    let mut out = ctx.run_ui(input, |ui| {
        egui::CentralPanel::default().show(ui, |ui| {
            draw(ui, show, st, view, PreviewId::Stage(0));
        });
    });
    out.textures_delta.clear();
}

#[test]
fn a_view_asks_the_governor_and_only_then_for_a_picture() {
    let mut show = Show::default();
    let mut st = UiState::default();
    let mut view = StageViewState::default();
    // No stage: nothing asked for, nothing drawn.
    run(&show, &mut st, &mut view);
    assert!(st.stage.requests.is_empty());
    crate::ui::stage_edit::add_surface(&mut show, Preset::Flat);
    run(&show, &mut st, &mut view);
    let id = PreviewId::Stage(0);
    assert!(st.preview_governor.asked.borrow().contains(&id));
    assert_eq!(st.stage.requests.len(), 1);
    let r = &st.stage.requests[0];
    assert_eq!(r.id, id);
    assert!(r.size.0 >= 700 && r.size.1 >= 400, "{:?}", r.size);
    assert_eq!(r.options.quality.scale, 1.0);
    // Half size while the outputs need the room.
    st.stage.requests.clear();
    st.preview_governor.force(Level::Half);
    run(&show, &mut st, &mut view);
    assert_eq!(st.stage.requests.len(), 1);
    assert_eq!(st.stage.requests[0].options.quality.scale, 0.5);
}

#[test]
fn an_empty_stage_offers_a_start_and_a_view_asks_through_actions() {
    let show = Show::default();
    let mut st = UiState::default();
    let mut view = StageViewState::default();
    let ctx = egui::Context::default();
    let mut texts = Vec::new();
    for frame in 0..3 {
        let mut input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 500.0))),
            time: Some(1.0 + frame as f64 * 0.1),
            ..Default::default()
        };
        // Click "Add a screen" on the last frame, where it was drawn.
        if frame == 2
            && let Some(at) = ctx.data(|d| d.get_temp::<Pos2>(egui::Id::new("add-at")))
        {
            input.events = vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ];
        }
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                draw(ui, &show, &mut st, &mut view, PreviewId::Stage(0));
            });
        });
        out.textures_delta.clear();
        texts = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_string(), t.pos, t.galley.rect)),
                _ => None,
            })
            .collect();
        if let Some((_, pos, r)) = texts.iter().find(|(t, ..)| t == "Add a screen") {
            let at = *pos + r.center().to_vec2();
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("add-at"), at));
        }
    }
    let words: Vec<&str> = texts.iter().map(|(t, ..)| t.as_str()).collect();
    assert!(words.contains(&"No 3D stage yet"), "{words:?}");
    assert!(words.contains(&"Load a venue model…"), "{words:?}");
    assert!(
        st.stage
            .actions
            .iter()
            .any(|a| matches!(a, Action::Stage(StageEdit::AddSurface(Preset::Flat)))),
        "{:?}",
        st.stage.actions.len()
    );
}

#[test]
fn a_click_on_a_screen_picks_it_and_a_drag_turns_the_view() {
    let mut show = Show::default();
    let id = crate::ui::stage_edit::add_surface(&mut show, Preset::Flat);
    let mut st = UiState::default();
    st.stage.scene = Arc::new(show.stage3d.scene(&show, None));
    st.stage.bounds = render3d::scene_bounds(&st.stage.scene);
    let mut view = StageViewState::default();
    let ctx = egui::Context::default();
    let frame = |events: Vec<egui::Event>, st: &mut UiState, view: &mut StageViewState, t: f64| {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 500.0))),
            time: Some(t),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                draw(ui, &show, st, view, PreviewId::Stage(0));
            });
        });
        out.textures_delta.clear();
    };
    let button = |pos, pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    frame(Vec::new(), &mut st, &mut view, 1.0);
    // Framed on the screen at first: the middle of the view is the screen.
    let middle = Pos2::new(400.0, 270.0);
    frame(
        vec![egui::Event::PointerMoved(middle)],
        &mut st,
        &mut view,
        1.1,
    );
    frame(vec![button(middle, true)], &mut st, &mut view, 1.2);
    frame(vec![button(middle, false)], &mut st, &mut view, 1.25);
    assert_eq!(st.stage.picked, Some(id));
    // A click on nothing lets it go.
    let corner = Pos2::new(30.0, 480.0);
    frame(
        vec![egui::Event::PointerMoved(corner)],
        &mut st,
        &mut view,
        2.0,
    );
    frame(vec![button(corner, true)], &mut st, &mut view, 2.1);
    frame(vec![button(corner, false)], &mut st, &mut view, 2.15);
    assert_eq!(st.stage.picked, None);
    // A drag turns round the stage, and picks nothing.
    let yaw = view.orbit.yaw;
    frame(
        vec![egui::Event::PointerMoved(middle)],
        &mut st,
        &mut view,
        3.0,
    );
    frame(vec![button(middle, true)], &mut st, &mut view, 3.1);
    for i in 1..6 {
        let p = middle + vec2(20.0 * i as f32, 0.0);
        frame(
            vec![egui::Event::PointerMoved(p)],
            &mut st,
            &mut view,
            3.1 + 0.05 * f64::from(i),
        );
    }
    frame(
        vec![button(middle + vec2(100.0, 0.0), false)],
        &mut st,
        &mut view,
        3.5,
    );
    assert_ne!(view.orbit.yaw, yaw);
    assert_eq!(st.stage.picked, None);
}

/// A view driven by the mouse, frame by frame: each frame's words and where
/// they are.
struct Driven {
    ctx: egui::Context,
    t: f64,
    texts: Vec<(String, Pos2)>,
}

impl Driven {
    fn new() -> Driven {
        Driven {
            ctx: egui::Context::default(),
            t: 1.0,
            texts: Vec::new(),
        }
    }

    fn frame(
        &mut self,
        show: &Show,
        st: &mut UiState,
        view: &mut StageViewState,
        events: Vec<egui::Event>,
    ) {
        self.t += 0.1;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 560.0))),
            time: Some(self.t),
            events,
            ..Default::default()
        };
        let mut out = self.ctx.run_ui(input, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                draw(ui, show, st, view, PreviewId::Stage(0));
            });
        });
        out.textures_delta.clear();
        self.texts = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some((
                    t.galley.text().to_string(),
                    t.pos + t.galley.rect.center().to_vec2(),
                )),
                _ => None,
            })
            .collect();
    }

    fn has(&self, words: &str) -> bool {
        self.texts.iter().any(|(t, _)| t == words)
    }

    /// Click the words `words` where the last frame drew them.
    fn click(&mut self, show: &Show, st: &mut UiState, view: &mut StageViewState, words: &str) {
        let at = self
            .texts
            .iter()
            .find(|(t, _)| t == words)
            .map(|(_, p)| *p)
            .unwrap_or_else(|| panic!("no {words:?} in {:?}", self.texts));
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        self.frame(show, st, view, vec![egui::Event::PointerMoved(at)]);
        self.frame(show, st, view, vec![button(true)]);
        self.frame(show, st, view, vec![button(false)]);
        self.frame(show, st, view, Vec::new());
    }
}

fn with_projector() -> Show {
    let mut show = Show::default();
    let s = crate::ui::stage_edit::add_surface(&mut show, Preset::Flat);
    let surface = show.stage3d.surface(s).unwrap().clone();
    let id = show.add_projector();
    let p = show.projector_mut(id).unwrap();
    p.placement = p.fit_to(&surface, 16.0 / 9.0).unwrap();
    show
}

#[test]
fn the_light_is_switched_on_and_its_views_chosen_with_the_mouse() {
    // No projectors: no Light to switch on.
    let mut plain = Show::default();
    crate::ui::stage_edit::add_surface(&mut plain, Preset::Flat);
    let mut st = UiState::default();
    let mut view = StageViewState::default();
    let mut d = Driven::new();
    d.frame(&plain, &mut st, &mut view, Vec::new());
    assert!(d.has("Grid"));
    assert!(!d.has("Light"));

    let show = with_projector();
    st.stage.scene = Arc::new(show.stage3d.scene(&show, None));
    st.stage.bounds = render3d::scene_bounds(&st.stage.scene);
    d.frame(&show, &mut st, &mut view, Vec::new());
    assert!(d.has("Light"));
    assert!(!view.light, "off at first");
    st.stage.requests.clear();
    d.click(&show, &mut st, &mut view, "Light");
    assert!(view.light);
    let asked = st.stage.requests.last().expect("a picture asked for");
    assert_eq!(asked.options.light, LightView::Picture);
    // The white point by itself: the projector's light on its screen.
    assert!(view.auto_white && view.white_nits > 1.0 && view.white_nits != 100.0);
    assert!(
        (asked.options.white_nits - view.white_nits).abs() < 1e-3,
        "{} {}",
        asked.options.white_nits,
        view.white_nits
    );
    assert!(d.has("Auto exposure"));
    let shown = format!("white at {:.0} nits", view.white_nits);
    assert!(d.has(&shown), "{:?}", d.texts);
    // By hand: the slider, from where it was.
    let auto = view.white_nits;
    d.click(&show, &mut st, &mut view, "Auto exposure");
    assert!(!view.auto_white);
    assert!(!d.has(&shown));
    assert_eq!(view.white_nits, auto);
    d.click(&show, &mut st, &mut view, "Auto exposure");
    assert!(view.auto_white);
    assert!(
        !d.has("Projectors lighting it"),
        "no legend for the picture"
    );
    // Overlap, from the menu: asked for, with its legend.
    d.click(&show, &mut st, &mut view, "Picture");
    d.click(&show, &mut st, &mut view, "Overlap");
    assert_eq!(view.light_view, LightView::Overlap);
    assert_eq!(
        st.stage.requests.last().unwrap().options.light,
        LightView::Overlap
    );
    assert!(d.has("Projectors lighting it"), "{:?}", d.texts);
    assert!(d.has("three or more"));
    assert!(!d.has("Auto exposure"));
    // Pixel size and Brightness, the same way.
    d.click(&show, &mut st, &mut view, "Overlap");
    d.click(&show, &mut st, &mut view, "Pixel size");
    assert_eq!(view.light_view, LightView::PixelSize);
    assert!(d.has("One pixel is") && d.has("2 to 5 mm"));
    d.click(&show, &mut st, &mut view, "Pixel size");
    d.click(&show, &mut st, &mut view, "Brightness");
    assert!(d.has("Brightness at full white") && d.has("400 nits or more"));
    // Off again: not lit, the light's bar gone.
    d.click(&show, &mut st, &mut view, "Light");
    assert!(!view.light);
    assert_eq!(
        st.stage.requests.last().unwrap().options.light,
        LightView::Off
    );
    assert!(!d.has("Show"));
}

#[test]
fn a_view_says_when_its_light_is_left_out_or_some_projectors_are_unlit() {
    let mut show = with_projector();
    assert!(light_notices(&show, None).is_empty());
    let refused = FrameStats {
        light_left_out: true,
        ..FrameStats::default()
    };
    let words = light_notices(&show, Some(&refused));
    assert_eq!(words.len(), 1);
    assert!(words[0].contains("can't draw it"), "{words:?}");
    for _ in 0..17 {
        show.add_projector();
    }
    let words = light_notices(&show, None);
    assert_eq!(
        words,
        ["Light: only the first 16 of the 18 projectors are lit in this view"]
    );
    // Shown in the view itself.
    let mut st = UiState::default();
    st.stage.scene = Arc::new(show.stage3d.scene(&show, None));
    st.stage.stats.insert(PreviewId::Stage(0), refused);
    let mut view = StageViewState {
        light: true,
        ..StageViewState::default()
    };
    let mut d = Driven::new();
    d.frame(&show, &mut st, &mut view, Vec::new());
    assert!(
        d.texts.iter().any(|(t, _)| t.contains("can't draw it")),
        "{:?}",
        d.texts
    );
}

#[test]
fn each_coloured_view_has_a_legend_with_every_colour() {
    use crate::stage3d::light::{OVERLAP, SCALE};
    assert!(legend(LightView::Picture).is_none());
    assert!(legend(LightView::Off).is_none());
    let (title, rows) = legend(LightView::Brightness).unwrap();
    assert_eq!(title, "Brightness at full white");
    assert_eq!(
        rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(),
        [
            "under 50 nits",
            "50 to 100 nits",
            "100 to 200 nits",
            "200 to 400 nits",
            "400 nits or more"
        ]
    );
    assert_eq!(rows.iter().map(|r| r.1).collect::<Vec<_>>(), SCALE);
    let (_, rows) = legend(LightView::PixelSize).unwrap();
    assert_eq!(rows[0].0, "under 1 mm");
    assert_eq!(rows[4].0, "10 mm or more");
    let (_, rows) = legend(LightView::Overlap).unwrap();
    assert_eq!(rows.iter().map(|r| r.1).collect::<Vec<_>>(), OVERLAP);
}
