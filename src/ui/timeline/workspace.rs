//! Layout B: a workspace of the timeline's own. While it is open it takes the
//! panes' place (the classic interface) or is the new interface's Timeline
//! view: a bar with the way back, the bin at the left (`bin`), the picture of
//! a display (the program monitor) in the middle, the inspector at the right,
//! and the tracks editor across the whole width below (`Host::Workspace`).
//! Which layout opens is `Prefs::timeline_layout`, per computer.

use super::*;

/// Whether the editor opens as the workspace on this computer.
pub fn chosen(st: &UiState) -> bool {
    st.prefs.timeline_layout == TimelineLayout::Workspace
}

/// Whether the classic interface shows the workspace in the panes' place now.
/// The settings, docked, take it back while they are open.
pub fn replaces_panes(st: &UiState) -> bool {
    !st.prefs.new_layout && chosen(st) && st.timeline.open && !st.layout.is_docked(Pane::Settings)
}

/// The timeline the workspace is on screen with now, if it is (in the panes'
/// place, or as the new interface's Timeline view).
pub fn showing(show: &Show, engine: &Engine, st: &UiState) -> Option<TimelineKey> {
    let on_screen = if st.prefs.new_layout {
        chosen(st) && st.modern_view == super::super::modern::View::Timeline
    } else {
        replaces_panes(st)
    };
    on_screen.then(|| target(show, engine, st)).flatten()
}

/// How much of the height the tracks take when the workspace first opens.
const TRACKS_SHARE: f32 = 0.46;

/// The whole workspace, in what is left of `ui`.
pub fn draw(
    ui: &mut Ui,
    show: &mut Show,
    engine: &mut Engine,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) {
    let key = target(show, engine, st);
    egui::Panel::top("ws_bar")
        .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(6))
        .show(ui, |ui| bar(ui, show, st, key, actions));
    let Some(key) = key else {
        egui::CentralPanel::default().show(ui, |ui| empty_state(ui, st, actions));
        return;
    };
    let tall = ui.available_height();
    egui::Panel::bottom("ws_tracks")
        .resizable(true)
        .default_size((tall * TRACKS_SHARE).max(200.0))
        .min_size(170.0)
        .show(ui, |ui| {
            editor(ui, show, engine, st, key, Host::Workspace, actions)
        });
    // The picture and the bin down the left; the inspector, whose two columns
    // want room, has the rest.
    egui::Panel::left("ws_side")
        .resizable(true)
        .default_size(SIDE_W)
        .min_size(220.0)
        .show(ui, |ui| {
            // The picture over the bin, the line between them dragged to
            // share the column out differently.
            let tall = ui.available_height();
            egui::Panel::top("ws_monitor_panel")
                .resizable(true)
                .frame(egui::Frame::NONE.inner_margin(egui::Margin {
                    bottom: 4,
                    ..Default::default()
                }))
                .default_size(monitor_height(st, ui.available_width(), tall))
                .min_size(90.0)
                .max_size((tall - BIN_MIN).max(90.0))
                .show(ui, |ui| monitor(ui, st));
            bin::draw(ui, show, engine, st, key, actions);
        });
    egui::CentralPanel::default().show(ui, |ui| inspector(ui, show, engine, st, actions));
}

/// The least height the bin keeps under the monitor when the line between
/// them is dragged.
const BIN_MIN: f32 = 110.0;

/// The monitor's height when the workspace first opens: the chosen display's
/// picture across the column with its header, but at most 60 % of the column,
/// so the bin keeps the rest.
fn monitor_height(st: &UiState, w: f32, tall: f32) -> f32 {
    let aspect = super::super::preview_index(&st.previews, st.preview_output, None)
        .map(|i| st.previews[i].canvas)
        .map_or(16.0 / 9.0, |(cw, ch)| {
            (cw as f32 / ch.max(1) as f32).max(0.1)
        });
    (MONITOR_HEAD + w / aspect).min(tall * 0.6).max(90.0)
}

/// The monitor's header row (PROGRAM and the display chooser).
const MONITOR_HEAD: f32 = 28.0;

/// How wide the picture and bin column is when the workspace first opens:
/// what the inspector's two columns leave of a 1280-point window.
const SIDE_W: f32 = 330.0;

/// The inspector, with a clip's or a control cue's timeline rows first (in the
/// panes they come last, under everything else).
fn inspector(
    ui: &mut Ui,
    show: &mut Show,
    engine: &mut Engine,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) {
    let title = st
        .selected
        .and_then(|id| show.cue(id))
        .map_or("INSPECTOR".to_string(), |c| {
            format!("INSPECTOR · {}", cue_label(c))
        });
    ui.add(egui::Label::new(RichText::new(title).small().strong().weak()).truncate());
    let locked = st.show_mode;
    ui.add_enabled_ui(!locked, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("ws_inspector_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let Some(idx) = st.selected.and_then(|id| show.cue_index(id)) {
                    // As the inspector does: a change made here is made in
                    // the other cues picked too.
                    let picked = st.selection(show);
                    let before = (picked.len() > 1).then(|| show.cues[idx].clone());
                    let _rows = ui.scope(|ui| inspector_rows(ui, show, idx)).response.rect;
                    #[cfg(test)]
                    ui.data_mut(|d| d.insert_temp(egui::Id::new("ws_rows"), _rows));
                    if let Some(before) = before
                        && super::super::user_touched(ui)
                    {
                        super::super::spread_edit(show, &before, &picked);
                    }
                }
                ui.data_mut(|d| d.insert_temp(egui::Id::new(ROWS_ABOVE), true));
                super::super::inspector(ui, show, engine, st, actions);
                ui.data_mut(|d| d.remove::<bool>(egui::Id::new(ROWS_ABOVE)));
            });
    });
}

/// Set while the workspace's inspector draws: the timeline rows are drawn
/// above it already (`inspector_rows` then draws nothing).
pub(super) const ROWS_ABOVE: &str = "timeline_rows_above";

/// The bar across the top: back to the cue list, which timeline, the other layout.
fn bar(
    ui: &mut Ui,
    show: &Show,
    st: &UiState,
    key: Option<TimelineKey>,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        let r = ui
            .button(RichText::new("◀ Back to cue list").strong())
            .on_hover_text(if st.prefs.new_layout {
                "Close the timeline workspace and go back to the Editor view"
            } else {
                "Close the timeline workspace: the cue list and the panes come back"
            });
        note(ui, "back", &r);
        if r.clicked() {
            actions.push(Action::Timeline(TimelineAction::Close));
        }
        ui.separator();
        ui.label(RichText::new("TIMELINE WORKSPACE").small().weak());
        if let Some(key) = key {
            ui.label(RichText::new(timeline_name(show, key)).strong());
        }
        if st.show_mode {
            ui.label(RichText::new("· show mode: nothing can be edited").weak());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let r = ui
                .button("Show as a pane")
                .on_hover_text(if st.prefs.new_layout {
                    "Open the timeline editor as a panel of the Editor view instead, from now on \
                 (Settings ▸ General ▸ Timeline editor)"
                } else {
                    "Open the timeline editor as a pane under the cue list instead, from now on \
                 (Settings ▸ General ▸ Timeline editor)"
                });
            note(ui, "as_pane", &r);
            if r.clicked() {
                actions.push(Action::Timeline(TimelineAction::SetLayout(
                    TimelineLayout::Pane,
                )));
            }
        });
    });
}

/// The program monitor: a display's picture across the column, and which
/// display (the same choice as the Preview pane's).
fn monitor(ui: &mut Ui, st: &mut UiState) {
    let Some(i) = super::super::preview_index(&st.previews, st.preview_output, None) else {
        ui.label(RichText::new("PROGRAM").small().strong().weak());
        ui.label(RichText::new("No display to show yet").weak());
        return;
    };
    let mut chosen = st.previews[i].id;
    ui.horizontal(|ui| {
        ui.label(RichText::new("PROGRAM").small().strong().weak());
        if st.previews.len() > 1 {
            egui::ComboBox::from_id_salt("ws_program")
                .selected_text(&st.previews[i].name)
                .show_ui(ui, |ui| {
                    for p in &st.previews {
                        ui.selectable_value(&mut chosen, p.id, &p.name);
                    }
                })
                .response
                .on_hover_text("Which display to show here");
        } else {
            ui.label(&st.previews[i].name);
        }
    });
    if chosen != st.previews[i].id {
        st.preview_output = Some(chosen);
    }
    let e = &st.previews[i];
    let aspect = (e.canvas.0 as f32 / e.canvas.1.max(1) as f32).max(0.1);
    // As much of the panel as the picture needs (the panel's height is the
    // bin's line).
    let w = ui.available_width();
    let h = (w / aspect).min(ui.available_height()).max(40.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
    #[cfg(test)]
    ui.data_mut(|d| d.insert_temp(egui::Id::new("ws_monitor"), rect));
    let picture =
        egui::Rect::from_center_size(rect.center(), egui::vec2(h * aspect, h).min(rect.size()));
    ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);
    ui.painter().add(egui_wgpu::Callback::new_paint_callback(
        picture,
        super::super::OutputPreview {
            groups: vec![super::super::OutputGroup {
                layers: e.layers.clone(),
                geometry: super::super::Geometry {
                    canvas: e.canvas,
                    warp: e.warp,
                    pin: e.warp,
                    mesh: e.mesh.clone(),
                    slice: e.slice,
                },
            }],
            slot: super::super::Slot::Preview,
        },
    ));
}

/// What a Timeline pane (docked or in a window of its own) says while the
/// editor opens as the workspace.
pub(super) fn pointer(
    ui: &mut Ui,
    show: &Show,
    engine: &Engine,
    st: &UiState,
    actions: &mut Vec<Action>,
) {
    #[cfg(test)]
    ui.data_mut(|d| d.insert_temp(egui::Id::new("timeline_pointer"), true));
    ui.add_space(8.0);
    ui.label("The timeline editor opens in a workspace of its own on this computer.");
    let key = target(show, engine, st);
    ui.horizontal(|ui| {
        if ui
            .add_enabled(key.is_some(), egui::Button::new("Open the workspace"))
            .on_disabled_hover_text("Select a timeline group, or go to a timeline list's tab")
            .clicked()
        {
            actions.push(Action::Timeline(TimelineAction::Open(key)));
        }
        if ui
            .button("Show it here instead")
            .on_hover_text("Open the timeline editor as a pane from now on")
            .clicked()
        {
            actions.push(Action::Timeline(TimelineAction::SetLayout(
                TimelineLayout::Pane,
            )));
        }
    });
}

/// Note, for the tests, where one of the workspace's buttons is.
fn note(_ui: &Ui, _name: &'static str, _r: &egui::Response) {
    #[cfg(test)]
    _ui.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(&'static str, egui::Rect)>>(egui::Id::new(
            "workspace_buttons",
        ))
        .push((_name, _r.rect))
    });
}
