//! The screen cue's page, in the whole window at 1280×760 in both
//! interfaces, driven with the mouse: Cue ▸ Add ▸ Move a screen, its words,
//! *Use where it is now*, *Put the screen there now*, and a number unticked.

use super::{HONEST_WORDS, PUT_THERE, SCREEN_ROW, USE_NOW};
use crate::engine::Engine;
use crate::show::{CueKind, CueType, Show};
use crate::stage3d::Placement;
use crate::ui::{UiState, draw};
use egui::{Event, PointerButton, Pos2, RawInput, Rect, vec2};

const SIZE: egui::Vec2 = vec2(1280.0, 760.0);

struct Win {
    ctx: egui::Context,
    e: Engine,
    t: f64,
    spots: Vec<(String, Pos2)>,
}

impl Win {
    fn new(show: &mut Show, st: &mut UiState) -> Win {
        let mut w = Win {
            ctx: egui::Context::default(),
            e: Engine::for_tests(),
            t: 1.0,
            spots: Vec::new(),
        };
        w.settle(show, st);
        w
    }

    fn frame(&mut self, show: &mut Show, st: &mut UiState, events: Vec<Event>) {
        self.t += 1.0 / 60.0;
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
            time: Some(self.t),
            events,
            ..Default::default()
        };
        let e = &mut self.e;
        let mut out = self.ctx.run_ui(input, |ui| draw(ui, show, e, st));
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
    }

    fn settle(&mut self, show: &mut Show, st: &mut UiState) {
        for _ in 0..3 {
            self.frame(show, st, vec![]);
        }
    }

    fn has(&self, words: &str) -> bool {
        self.spots.iter().any(|(t, _)| t == words)
    }

    fn at(&self, words: &str) -> Pos2 {
        self.spots
            .iter()
            .rev()
            .find(|(t, _)| t == words)
            .map(|(_, p)| *p)
            .unwrap_or_else(|| {
                panic!(
                    "no {words:?} in {:?}",
                    self.spots.iter().map(|s| &s.0).collect::<Vec<_>>()
                )
            })
    }

    fn hover(&mut self, show: &mut Show, st: &mut UiState, words: &str) {
        let p = self.at(words);
        self.frame(show, st, vec![Event::PointerMoved(p)]);
        self.settle(show, st);
    }

    /// Turns the wheel over the text `words`.
    fn wheel(&mut self, show: &mut Show, st: &mut UiState, words: &str, down: f32) {
        let p = self.at(words);
        self.frame(show, st, vec![Event::PointerMoved(p)]);
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
        for _ in 0..30 {
            self.frame(show, st, vec![]);
        }
    }

    fn click(&mut self, show: &mut Show, st: &mut UiState, words: &str) {
        let p = self.at(words);
        assert!(
            Rect::from_min_size(Pos2::ZERO, SIZE).contains(p),
            "{words} is in the window"
        );
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

fn the_cue(show: &Show) -> &crate::show::ScreenCue {
    show.cues
        .iter()
        .find_map(|c| match &c.kind {
            CueKind::Screen(s) => Some(s),
            _ => None,
        })
        .expect("a screen cue")
}

#[test]
fn a_screen_cue_is_added_from_the_cue_menu_and_set_from_where_the_screen_is() {
    for modern in [false, true] {
        let mut show = Show::default();
        let id = show.add_surface(None);
        let home = Placement {
            position: [1.0, 0.0, -2.0],
            ..Default::default()
        };
        {
            let s = show.stage3d.surface_mut(id).unwrap();
            s.name = "Cyc wall".into();
            s.placement = home;
        }
        let mut st = UiState::default();
        st.prefs.new_layout = modern;
        let mut w = Win::new(&mut show, &mut st);
        // Cue ▸ Add ▸ Move a screen.
        w.click(&mut show, &mut st, "Cue");
        w.hover(&mut show, &mut st, "Add");
        w.click(
            &mut show,
            &mut st,
            &format!("{} {}", CueType::Screen.icon(), CueType::Screen.label()),
        );
        let s = the_cue(&show);
        assert_eq!(
            s.screen,
            Some(id),
            "modern {modern}: the screen there is chosen"
        );
        assert_eq!(s.duration, 5.0);
        let cue = show.cues.last().unwrap().id;
        assert_eq!(
            st.selected,
            Some(cue),
            "modern {modern}: the new cue is picked"
        );
        w.settle(&mut show, &mut st);
        for words in [SCREEN_ROW, "Cyc wall", "Takes", "To", USE_NOW, PUT_THERE] {
            assert!(
                w.has(words),
                "modern {modern}: {words:?} on the page: {:?}",
                w.spots
            );
        }
        assert!(
            w.has("Now at: X 1.00 m, Y 0.00 m, Z -2.00 m (at home)"),
            "modern {modern}"
        );
        // The screen somewhere else (*Put at*, as after a rehearsal), then
        // the cue takes every number from there.
        let there = Placement {
            position: [3.0, 0.5, -1.0],
            yaw: 45.0,
            ..Default::default()
        };
        w.e.put_screen(&show, id, there);
        w.settle(&mut show, &mut st);
        assert!(w.has("Now at: X 3.00 m, Y 0.50 m, Z -1.00 m, turn 45.0°"));
        w.click(&mut show, &mut st, USE_NOW);
        assert_eq!(
            the_cue(&show).to,
            crate::show::ScreenTarget::whole(&there),
            "modern {modern}"
        );
        // Further down the page: the turn unticked is left where it is, and
        // the plain warning.
        w.wheel(&mut show, &mut st, "Takes", -600.0);
        assert!(w.has(HONEST_WORDS), "modern {modern}");
        w.click(&mut show, &mut st, "Turn");
        assert_eq!(the_cue(&show).to.yaw, None, "modern {modern}");
        assert!(w.has("left where it is"));
        // Put the screen there now: from home (the cue before it), the
        // numbers it gives.
        w.e.put_screen(&show, id, home);
        w.settle(&mut show, &mut st);
        w.wheel(&mut show, &mut st, "Turn", 600.0);
        w.click(&mut show, &mut st, PUT_THERE);
        let p = w.e.screen_at(id).expect("put there");
        assert_eq!(p.position, there.position, "modern {modern}");
        assert_eq!(p.yaw, 0.0, "modern {modern}: the turn as at home");
        // The cue list says what it does.
        let line = crate::ui::screen_cue::target_text(&show, the_cue(&show));
        assert!(
            line.starts_with("» Cyc wall: X 3.00 m, Y 0.50 m, Z -1.00 m, tilt")
                && line.ends_with("over 5.0s"),
            "{line}"
        );
    }
}
