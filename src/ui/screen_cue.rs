//! The screen cue's page in the inspector ("Move a screen", mapping plan 4.2):
//! which screen, where to (each number on its own; one not ticked is left
//! where it is), how long and how, and two buttons so nobody types six
//! numbers: *Use where it is now* and *Put the screen there now*.

use super::{Action, secs};
use crate::engine::Engine;
use crate::show::screen::{AXES, AXIS_WORDS, axes_of};
use crate::show::{CueId, Curve, ScreenCue, ScreenTarget, Show};
use crate::stage3d::{Placement, SurfaceId};
use egui::{RichText, Ui};

/// What the page needs from the show and the engine (taken before the cue
/// is borrowed to be edited).
#[derive(Clone, Debug, Default)]
pub(crate) struct Choices {
    /// Every screen, by name.
    pub screens: Vec<(SurfaceId, String)>,
    /// The chosen screen's home (its placement in the show), where it is now,
    /// and whether a cue (or *Put at*) moved it.
    pub home: Option<Placement>,
    pub now: Option<Placement>,
    pub moved: bool,
    /// Where the screen is before this cue, played in the show's order
    /// (the cue before it for that screen, `Show::screen_places`, or home).
    pub before: Option<Placement>,
}

/// The words the page and its tests share.
pub(crate) const SCREEN_ROW: &str = "Screen";
pub(crate) const USE_NOW: &str = "Use where it is now";
pub(crate) const PUT_THERE: &str = "Put the screen there now";
pub(crate) const NO_SCREEN: &str = "Choose a screen…";
/// Said where the cue is described (and in the README): NewQ isn't told
/// where the scenery is.
pub(crate) const HONEST_WORDS: &str = "NewQ moves the picture as this cue says. It doesn't know \
     where the scenery really is: if the scenery is late, the picture is ahead of it.";

impl Choices {
    /// For the cue `selected` (when it is a screen cue).
    pub(crate) fn of(show: &Show, engine: &Engine, selected: Option<CueId>) -> Choices {
        let screens = show
            .stage3d
            .surfaces
            .iter()
            .map(|s| (s.id, s.name.clone()))
            .collect();
        let mut c = Choices {
            screens,
            ..Choices::default()
        };
        let Some((id, cue)) = selected.and_then(|id| match show.cue(id).map(|c| &c.kind) {
            Some(crate::show::CueKind::Screen(s)) => Some((id, s)),
            _ => None,
        }) else {
            return c;
        };
        let Some(screen) = cue.screen else {
            return c;
        };
        c.home = show.stage3d.surface(screen).map(|s| s.placement);
        c.moved = engine.screen_at(screen).is_some();
        c.now = engine.screen_at(screen).or(c.home);
        let places = show.screen_places(screen);
        c.before = match places.iter().position(|(cue, _)| *cue == id) {
            Some(0) | None => c.home,
            Some(i) => Some(places[i - 1].1),
        };
        c
    }
}

/// A placement in words: "X 1.50 m, Y 0.00 m, Z -2.00 m, turn 90°".
pub(crate) fn place_words(p: &Placement) -> String {
    let v = axes_of(p);
    let mut out = format!("X {:.2} m, Y {:.2} m, Z {:.2} m", v[0], v[1], v[2]);
    for (word, deg) in [("turn", v[3]), ("tilt", v[4]), ("roll", v[5])] {
        if deg != 0.0 {
            out.push_str(&format!(", {word} {deg:.1}°"));
        }
    }
    out
}

/// The cue list's line for it: "» Cyc wall: X 2.00 m, turn 90° over 8 s".
pub(crate) fn target_text(show: &Show, s: &ScreenCue) -> String {
    let Some(name) = s
        .screen
        .and_then(|id| show.stage3d.surface(id))
        .map(|x| x.name.clone())
    else {
        return "(no screen)".into();
    };
    let mut parts = Vec::new();
    for (i, v) in s.to.axes().into_iter().enumerate() {
        if let Some(v) = v {
            let word = ["X", "Y", "Z", "turn", "tilt", "roll"][i];
            parts.push(if i < 3 {
                format!("{word} {v:.2} m")
            } else {
                format!("{word} {v:.1}°")
            });
        }
    }
    let to = if parts.is_empty() {
        "stays where it is".to_string()
    } else {
        parts.join(", ")
    };
    let len = crate::engine::screen_cue_length(s);
    if len > 0.0 {
        format!("» {name}: {to} over {}", super::secs_short(len))
    } else {
        format!("» {name}: {to} at once")
    }
}

/// The page.
pub(crate) fn inspector(
    ui: &mut Ui,
    cue_id: CueId,
    s: &mut ScreenCue,
    c: &Choices,
    actions: &mut Vec<Action>,
) {
    egui::Grid::new(("screen_cue", cue_id.0))
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label(SCREEN_ROW);
            let chosen = s
                .screen
                .and_then(|id| c.screens.iter().find(|(i, _)| *i == id))
                .map(|(_, n)| n.clone())
                .unwrap_or_else(|| NO_SCREEN.to_string());
            egui::ComboBox::from_id_salt(("screen_cue_screen", cue_id.0))
                .selected_text(chosen)
                .width(220.0)
                .show_ui(ui, |ui| {
                    for (id, name) in &c.screens {
                        ui.selectable_value(&mut s.screen, Some(*id), name);
                    }
                });
            ui.end_row();
            ui.label("Takes");
            secs(ui, &mut s.duration).on_hover_text("0 moves it at once");
            ui.end_row();
            ui.label("Curve");
            egui::ComboBox::from_id_salt(("screen_cue_curve", cue_id.0))
                .selected_text(s.curve.label())
                .show_ui(ui, |ui| {
                    for k in Curve::ALL {
                        ui.selectable_value(&mut s.curve, k, k.label());
                    }
                });
            ui.end_row();
        });
    if c.screens.is_empty() {
        ui.label(
            RichText::new("The 3D stage has no screens yet: add one in Settings ▸ 3D stage.")
                .weak(),
        );
        return;
    }
    ui.add_space(4.0);
    ui.label(RichText::new("To").strong());
    let base = c.now.or(c.home).unwrap_or_default();
    let base = axes_of(&base);
    egui::Grid::new(("screen_cue_to", cue_id.0))
        .num_columns(3)
        .spacing([10.0, 4.0])
        .show(ui, |ui| {
            for i in 0..AXES {
                let Some(slot) = s.to.axis_mut(i) else {
                    continue;
                };
                let mut on = slot.is_some();
                if ui.checkbox(&mut on, AXIS_WORDS[i]).changed() {
                    *slot = on.then_some(base[i]);
                }
                match slot {
                    Some(v) => {
                        let drag =
                            egui::DragValue::new(v).fixed_decimals(if i < 3 { 2 } else { 1 });
                        let drag = if i < 3 {
                            drag.speed(0.01).suffix(" m")
                        } else {
                            drag.speed(0.5).suffix("°")
                        };
                        ui.add(drag);
                    }
                    None => {
                        ui.label(RichText::new("left where it is").weak());
                    }
                }
                ui.end_row();
            }
        });
    ui.horizontal(|ui| {
        let now = c.now;
        if ui
            .add_enabled(now.is_some(), egui::Button::new(USE_NOW))
            .on_hover_text("Every number from where the screen is now")
            .clicked()
            && let Some(p) = now
        {
            s.to = ScreenTarget::whole(&p);
        }
        // Where this cue leaves it, played in order, with the cue as it is
        // now (an edit made this frame included).
        let there = s.to.apply(&c.before.unwrap_or_default());
        if ui
            .add_enabled(s.screen.is_some(), egui::Button::new(PUT_THERE))
            .on_hover_text("A jump, no move: for rehearsals, and after a restart mid-show")
            .clicked()
            && let Some(screen) = s.screen
        {
            actions.push(Action::PutScreen(screen, there));
        }
    });
    if let Some(now) = c.now {
        let state = if c.moved { "" } else { " (at home)" };
        ui.label(RichText::new(format!("Now at: {}{state}", place_words(&now))).weak());
    }
    ui.label(RichText::new(HONEST_WORDS).weak());
}
