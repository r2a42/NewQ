//! A Preview pane in 3D mode: the show's stage, with each display's picture on
//! its surfaces. (The renderer is wired in separately; until then this is a
//! placeholder that says so.) It is drawn by `ui::output_previews` for a pane
//! whose mode is `dock::PreviewMode::Stage`, with that pane's own
//! `StageViewState` and `PreviewId::Stage(pane)`. Like every preview, a view
//! that draws a picture asks the governor (`UiState::preview_budget`) with that
//! id each frame it would draw (the caller doesn't ask for it: asking stamps
//! the time of its last picture). The placeholder draws none, so it asks not.

use egui::{Color32, RichText};

/// What a Preview pane in 3D mode keeps between frames (camera, picked surface…).
#[derive(Default)]
pub struct StageViewState {}

/// Draws a Preview pane in 3D mode into `ui`'s available rect.
pub fn draw(
    ui: &mut egui::Ui,
    _show: &crate::show::Show,
    _st: &mut crate::ui::UiState,
    _view: &mut StageViewState,
    _id: crate::preview_pace::PreviewId,
) {
    // A framed note "3D view" until the renderer is wired in.
    let width = ui
        .available_width()
        .min(ui.clip_rect().right() - ui.cursor().min.x)
        .max(1.0);
    let height = (width * 9.0 / 16.0).max(80.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    ui.painter().rect_filled(rect, 4.0, Color32::from_gray(18));
    ui.painter().rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, Color32::from_gray(70)),
        egui::StrokeKind::Inside,
    );
    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink(12.0))
            .layout(egui::Layout::top_down(egui::Align::Center)),
    );
    inner.add_space((rect.height() / 2.0 - 30.0).max(0.0));
    inner.label(
        RichText::new("3D view")
            .strong()
            .color(Color32::from_gray(200)),
    );
    inner.label(RichText::new("No 3D stage yet").weak());
}
