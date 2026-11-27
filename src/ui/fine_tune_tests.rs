//! A mapped projector's fine-tune, by mouse on Settings ▸ Displays: the
//! display's mesh editor pointed at `Projector::fine` (mapping plan 4.1).

use super::*;
use egui::{Event, PointerButton, Pos2, RawInput, Rect, vec2};

const SIZE: egui::Vec2 = vec2(1280.0, 760.0);

struct Win {
    ctx: egui::Context,
    e: Engine,
    t: f64,
}

impl Win {
    fn frame(&mut self, show: &mut Show, st: &mut UiState, events: Vec<Event>) {
        self.t += 1.0 / 60.0;
        self.ctx.data_mut(|d| {
            d.remove::<Vec<(String, Rect, bool)>>(egui::Id::new("stage_sources"));
        });
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
            time: Some(self.t),
            events,
            ..Default::default()
        };
        let e = &mut self.e;
        let mut out = self.ctx.run_ui(input, |ui| draw(ui, show, e, st));
        out.textures_delta.clear();
    }

    fn settle(&mut self, show: &mut Show, st: &mut UiState) {
        for _ in 0..3 {
            self.frame(show, st, vec![]);
        }
    }

    fn find(&self, name: &str) -> Option<(Rect, bool)> {
        self.ctx
            .data(|d| d.get_temp::<Vec<(String, Rect, bool)>>(egui::Id::new("stage_sources")))
            .unwrap_or_default()
            .into_iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, r, seen)| (r, seen))
    }

    /// Clicks the control `name`, scrolling the page to it first.
    fn press(&mut self, show: &mut Show, st: &mut UiState, name: &str) {
        for _ in 0..40 {
            match self.find(name) {
                Some((_, true)) => break,
                Some((r, false)) => {
                    let at = Pos2::new(SIZE.x * 0.6, SIZE.y * 0.5);
                    let down = if r.center().y > SIZE.y * 0.5 {
                        -80.0
                    } else {
                        80.0
                    };
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
                None => panic!("{name} isn't drawn"),
            }
        }
        let (r, seen) = self.find(name).expect("drawn");
        assert!(seen, "{name} is in sight");
        let p = r.center();
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
}

/// A projector display with a monitor, mapped onto a screen that shows a
/// canvas.
fn mapped_show() -> (Show, OutputId, crate::stage3d::ProjectorId) {
    let mut show = Show::default();
    let display = show.outputs[0].id;
    show.outputs[0].name = "Projector A".into();
    show.outputs[0].monitor = Some(crate::show::DisplayRef {
        name: "Left".into(),
        index: 0,
        hardware: None,
    });
    let canvas = show.add_canvas();
    let s = show.add_surface(None);
    show.stage3d.surface_mut(s).unwrap().feed = crate::stage3d::Feed::Display {
        output: canvas,
        region: [0.0, 0.0, 1.0, 1.0],
    };
    let p = show.add_projector();
    let pr = show.projector_mut(p).unwrap();
    pr.display = Some(display);
    pr.mapped = true;
    (show, display, p)
}

#[test]
fn a_mapped_projectors_fine_tune_is_edited_with_the_displays_mesh_editor() {
    for modern in [false, true] {
        let (mut show, display, p) = mapped_show();
        let mut st = UiState::default();
        st.prefs.new_layout = modern;
        st.layout.open(Pane::Settings);
        st.settings_page = SettingsPage::Displays;
        let mut w = Win {
            ctx: egui::Context::default(),
            e: Engine::for_tests(),
            t: 1.0,
        };
        w.settle(&mut show, &mut st);
        let fine_tune = "Displays ▸ Projector A ▸ Fine-tune";
        // Started and left as it was: nothing is kept.
        w.press(&mut show, &mut st, fine_tune);
        assert_eq!(st.geometry_edit, Some(display));
        let fine = show
            .projector(p)
            .unwrap()
            .fine
            .clone()
            .expect("a grid to edit");
        assert!(fine.is_flat() && (fine.cols(), fine.rows()) == (3, 3));
        w.press(&mut show, &mut st, fine_tune);
        assert_eq!(st.geometry_edit, None);
        assert_eq!(
            show.projector(p).unwrap().fine,
            None,
            "flat is no fine-tune"
        );
        // Started again, its middle point moved 5 px right with the arrow
        // keys' action: the projector's fine-tune moves, not the display's
        // own mesh.
        w.press(&mut show, &mut st, fine_tune);
        st.handle_sel.set(Some(HandleSel {
            quad: Quad::Mesh(display),
            corner: Some(4),
        }));
        apply(Action::Nudge(vec2(5.0, 0.0)), &mut show, &mut w.e, &mut st);
        let moved = show.projector(p).unwrap().fine.clone().expect("kept");
        assert!(!moved.is_flat());
        assert!(moved.points[4][0] > 0.5, "{:?}", moved.points[4]);
        assert!(show.output(display).unwrap().mesh.is_none());
        w.settle(&mut show, &mut st);
        w.press(&mut show, &mut st, fine_tune);
        assert_eq!(st.geometry_edit, None);
        assert_eq!(
            show.projector(p).unwrap().fine,
            Some(moved),
            "kept when done"
        );
        // Reset: back to none.
        w.press(
            &mut show,
            &mut st,
            "Displays ▸ Projector A ▸ Fine-tune ▸ Reset",
        );
        assert_eq!(show.projector(p).unwrap().fine, None, "modern {modern}");
    }
}

#[test]
fn a_display_that_isnt_mapped_keeps_its_own_mesh() {
    let (mut show, display, p) = mapped_show();
    show.projector_mut(p).unwrap().mapped = false;
    show.outputs[0].mesh = Some(Mesh::from_pin(3, 3, |u, v| [u, v]));
    assert!(fine_tune_of(&mut show, display).is_none());
    let mut st = UiState::default();
    st.geometry_edit = Some(display);
    st.handle_sel.set(Some(HandleSel {
        quad: Quad::Mesh(display),
        corner: Some(4),
    }));
    let mut e = Engine::for_tests();
    apply(Action::Nudge(vec2(5.0, 0.0)), &mut show, &mut e, &mut st);
    assert!(
        !show
            .output(display)
            .unwrap()
            .mesh
            .as_ref()
            .unwrap()
            .is_flat()
    );
    assert_eq!(show.projector(p).unwrap().fine, None);
}
