//! Help ▸ Keyboard shortcuts: every key NewQ answers to, from one table that
//! the README's Keyboard section must equal (a test reads it).

use super::*;

/// (key, what it does), in the README's order.
pub(super) const KEYS: [(&str, &str); 29] = [
    (
        "Space",
        "GO on a cue list; on a timeline you are working on (a timeline list's tab, or the tracks editor you last clicked in or are pointing at): play / pause it — running, it pauses; paused, parked, at a pause cue or in a loop it does what GO does there; stopped, it runs",
    ),
    (
        "0 (top row or keypad)",
        "GO, everywhere (on a timeline, as GO does there: nothing while it runs)",
    ),
    ("Esc", "Fade out everything (press twice: stop now)"),
    ("P", "Pause / resume everything"),
    ("A", "Arm / disarm the selected cue"),
    (
        "any cue's hotkey",
        "Fire that cue (letters, digits and F1–F12; a cue given 0 takes it from GO)",
    ),
    ("↑ / ↓", "Move selection and playhead"),
    (
        "→ / Page Down",
        "Show mode: GO (the next slide of a slide show that advances on GO)",
    ),
    (
        "← / Page Up",
        "Show mode: the slide before in such a slide show; otherwise pause everything, and again to fade it out",
    ),
    ("Ctrl+↑ / Ctrl+↓", "Reorder the selected cue"),
    (
        "← ↑ → ↓ / Shift+arrow",
        "Move the selected corner 1 px / 5 px (while editing)",
    ),
    ("Ctrl+D / Del", "Duplicate / delete cue (all selected)"),
    ("Ctrl+C / Ctrl+X / Ctrl+V", "Copy / cut / paste cues"),
    (
        "Ctrl-click / Shift-click",
        "Add to the selection / select a run of cues",
    ),
    ("Ctrl+Z / Ctrl+Y", "Undo / redo (Ctrl+Shift+Z also redoes)"),
    ("Ctrl+N / Ctrl+O / Ctrl+S", "New / open / save show"),
    (
        "Double-click a timeline group",
        "Open it in the tracks editor",
    ),
    ("Ctrl+wheel (tracks editor)", "Zoom around the pointer"),
    (
        "Alt while dragging (tracks editor)",
        "Place freely, without snapping",
    ),
    (
        "Ctrl+K (tracks editor)",
        "Split the picked clips (or those under the playhead) at the playhead",
    ),
    (
        "Del / Shift+Del (tracks editor)",
        "Delete the picked clips / ripple delete (close the gap)",
    ),
    (
        "Ctrl+C / Ctrl+X / Ctrl+V (tracks editor)",
        "Copy / cut the picked clips / paste at the playhead on their tracks",
    ),
    (
        "Ctrl+D (tracks editor)",
        "Duplicate the picked clips just after them",
    ),
    ("Ctrl+Shift+S", "Save as"),
    ("Ctrl+F", "Find cues"),
    ("Ctrl+,", "Open / close the settings"),
    (
        "Ctrl+Shift+L",
        "Show mode / edit mode (leaving show mode asks first)",
    ),
    (
        "F (pointing at a 3D view)",
        "See all of the 3D stage (double-click does too)",
    ),
    (
        "Esc (in a 3D view looking through a projector)",
        "Leave the look through its lens",
    ),
];

/// The width of the keys' column.
const KEY_WIDTH: f32 = 200.0;

/// Said under the table, as the README says it under its own.
const AFTER: &str = "Space, 0, Esc and the cues' hotkeys also work while an output window has \
                     focus; in show mode, so do the arrows and Page Up/Down. On a Mac, Ctrl \
                     is Cmd. Drag media files onto the window to add cues.";

/// The keyboard shortcuts window, while `UiState::show_keys`.
pub(super) fn window(ctx: &egui::Context, st: &mut UiState) {
    if !st.show_keys {
        return;
    }
    let mut open = true;
    egui::Window::new(settings::KEYS_ITEM.label.trim_end_matches('…'))
        .open(&mut open)
        .collapsible(false)
        .default_width(640.0)
        .show(ctx, |ui| {
            let high = (ui.ctx().content_rect().height() - 160.0).max(160.0);
            egui::ScrollArea::vertical()
                .max_height(high)
                .show(ui, |ui| {
                    egui::Grid::new("keyboard_shortcuts")
                        .num_columns(2)
                        .striped(true)
                        .spacing([16.0, 4.0])
                        .show(ui, |ui| {
                            for (key, does) in KEYS {
                                // The keys in a column of their own width,
                                // wrapped, so the words have the room.
                                ui.allocate_ui(egui::vec2(KEY_WIDTH, 0.0), |ui| {
                                    ui.set_width(KEY_WIDTH);
                                    ui.add(
                                        egui::Label::new(RichText::new(key).monospace().strong())
                                            .wrap(),
                                    );
                                });
                                ui.add(egui::Label::new(does).wrap());
                                ui.end_row();
                            }
                        });
                });
            ui.separator();
            ui.add(egui::Label::new(RichText::new(AFTER).weak().small()).wrap());
        });
    st.show_keys = open;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The README's Keyboard table, row by row.
    fn readme_rows() -> Vec<(String, String)> {
        let readme = include_str!("../../README.md");
        let at = readme.find("\n## Keyboard\n").expect("a Keyboard section");
        readme[at..]
            .lines()
            .skip_while(|l| !l.starts_with('|'))
            .take_while(|l| l.starts_with('|'))
            .skip(2)
            .map(|l| {
                let cells: Vec<&str> = l.trim().trim_matches('|').split('|').collect();
                assert_eq!(cells.len(), 2, "{l}");
                (cells[0].trim().to_string(), cells[1].trim().to_string())
            })
            .collect()
    }

    #[test]
    fn the_readme_keyboard_table_is_this_table() {
        let here: Vec<(String, String)> = KEYS
            .iter()
            .map(|(k, d)| (k.to_string(), d.to_string()))
            .collect();
        assert_eq!(readme_rows(), here);
    }

    #[test]
    fn every_shortcut_a_menu_prints_is_in_the_table() {
        let keys: Vec<&str> = KEYS
            .iter()
            .flat_map(|(k, _)| k.split(" / "))
            .map(|k| k.split(" (").next().unwrap_or(k).trim())
            .collect();
        for s in super::super::menus::SHORTCUTS {
            assert!(
                keys.contains(&s),
                "{s} is printed in a menu but not in the table"
            );
        }
    }

    fn texts(out: &egui::FullOutput) -> Vec<String> {
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_window_lists_the_keys_and_closes() {
        let ctx = egui::Context::default();
        let mut st = UiState {
            show_keys: true,
            ..Default::default()
        };
        let mut seen = Vec::new();
        for n in 0..3 {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 760.0),
                )),
                time: Some(1.0 + f64::from(n) * 0.05),
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| window(ui.ctx(), &mut st));
            out.textures_delta.clear();
            seen = texts(&out);
        }
        assert!(seen.iter().any(|t| t == "Keyboard shortcuts"), "{seen:?}");
        assert!(seen.iter().any(|t| t == "Space"), "{seen:?}");
        assert!(seen.iter().any(|t| t == "Esc"), "{seen:?}");
        assert!(st.show_keys);
        st.show_keys = false;
        let mut out = ctx.run_ui(egui::RawInput::default(), |ui| window(ui.ctx(), &mut st));
        out.textures_delta.clear();
        assert!(texts(&out).is_empty());
    }
}
