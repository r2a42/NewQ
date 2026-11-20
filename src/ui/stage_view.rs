//! The 3D stage in a preview: the venue and its screens, each showing its
//! display's picture, seen by a camera the mouse moves.
//!
//! - Drag to turn round the stage; right- or middle-drag, or Shift-drag, to
//!   slide; the wheel to move in and out; double-click (or F) to see it all;
//!   a click on a screen picks it (its settings are on Settings ▸ 3D stage).
//! - Over the picture: the saved viewpoints (and saving this one), a floor
//!   grid, an x-ray of the venue and "See all".
//!
//! Hosted by a Preview pane in 3D mode (`PreviewId::Stage(host)`, its state in
//! `UiState::preview_stage`) and by Settings ▸ 3D stage (`SETTINGS_VIEW`).
//!
//! Drawing obeys the preview governor (`preview_pace`): each frame the view
//! asks `UiState::preview_budget` and, when it may draw, asks the app for a
//! picture (`StageShared::requests`), which the stage's thread draws
//! (`stage_render`); meanwhile the last picture stays. The pane never touches
//! the graphics card itself: what it shows is the target the app made,
//! through the compositor like any other preview.
//!
//! Edits asked for here (a venue file, a screen added, a viewpoint saved) go
//! through `StageShared::actions` into the app's usual path (undo, show mode,
//! a follower's lock).

use std::collections::HashMap;
use std::sync::Arc;

use egui::{Align2, Color32, Pos2, Rect, RichText, Sense, Ui, Vec2, vec2};

use super::{Action, UiState};
use crate::gpu::{LayerTexture, Slot};
use crate::preview_pace::PreviewId;
use crate::render::{OutputGroup, OutputLayer, OutputPreview};
use crate::render3d::{self, Bounds, Camera, FrameStats, Options, Orbit, Quality, Scene};
use crate::show::Show;
use crate::stage3d::{SurfaceId, Viewpoint};
use crate::ui::stage_edit::{Preset, StageEdit};

/// What the app and the 3D views share (`UiState::stage`).
#[derive(Default)]
pub struct StageShared {
    /// What the views draw (the app's latest).
    pub scene: Arc<Scene>,
    pub scene_key: u64,
    /// Around everything in it, for "See all".
    pub bounds: Option<Bounds>,
    pub venue: VenueStatus,
    /// Pictures asked for this frame (the app takes them each pass).
    pub requests: Vec<ViewRequest>,
    /// Each view's picture, as the app last made it.
    pub targets: HashMap<PreviewId, Arc<LayerTexture>>,
    pub stats: HashMap<PreviewId, FrameStats>,
    /// Actions asked for by a view (which has no `actions` of its own), taken
    /// by `ui::draw` next.
    pub(super) actions: Vec<Action>,
    /// The screen picked (in a view or on the settings page).
    pub picked: Option<SurfaceId>,
    /// Sample venues being written as .glb (`StageEdit::ExportGlb`).
    pub exports: Vec<std::sync::mpsc::Receiver<std::io::Result<std::path::PathBuf>>>,
    /// The screen last opened on the settings page for being picked.
    pub opened_for: Option<SurfaceId>,
    /// The 3D view at the top of Settings ▸ 3D stage.
    pub settings_view: StageViewState,
    /// The projectors: the one picked, their outlines, the light under the
    /// pointer (`stage_projectors`).
    pub projectors: super::stage_projectors::ProjectorsUi,
}

/// How the venue's model stands.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum VenueStatus {
    #[default]
    None,
    Loading(String),
    Loaded {
        triangles: usize,
        warnings: Vec<String>,
    },
    Failed(String),
    /// On a show network's computer: the venue's files are still coming
    /// from the primary. The screens are drawn without the room meanwhile.
    Arriving(String),
}

/// What a view and the settings page say while the venue is on its way.
pub fn arriving_words(name: &str) -> String {
    format!(
        "Venue still arriving from the primary: {name}. The screens are shown without the room until it is here."
    )
}

/// A picture of the stage one view wants.
#[derive(Clone, Debug)]
pub struct ViewRequest {
    pub id: PreviewId,
    /// Its size in pixels (the view's, in steps; the governor's scale is in
    /// `options.quality`).
    pub size: (u32, u32),
    pub camera: Camera,
    pub options: Options,
    /// Drawn over the picture: the projectors' cones and outlines.
    pub lines: Arc<Vec<render3d::Line>>,
}

/// One 3D view's own state: where its camera is, what it shows.
#[derive(Clone, Debug)]
pub struct StageViewState {
    pub orbit: Orbit,
    /// The bounds it last looked at all of by itself: until the user moves
    /// the camera, it looks at all of the stage again when that changes (a
    /// venue that loads after its screens were seen).
    framed: Option<Bounds>,
    /// The user has moved the camera (or chosen a viewpoint).
    moved: bool,
    pub grid: bool,
    /// The venue's opacity: below 1 the screens show through its walls.
    pub xray: f32,
    /// A name being typed for "Save this view…".
    saving: Option<String>,
    /// Looking through this projector's lens (a drag or the wheel leaves it,
    /// orbiting from where it was).
    pub through: Option<crate::stage3d::ProjectorId>,
}

impl Default for StageViewState {
    fn default() -> Self {
        StageViewState {
            orbit: Orbit::default(),
            framed: None,
            moved: false,
            grid: true,
            xray: 1.0,
            saving: None,
            through: None,
        }
    }
}

impl StageViewState {
    /// A view looking from viewpoint `v` (as if chosen from its menu).
    pub fn from_viewpoint(v: &Viewpoint) -> StageViewState {
        StageViewState {
            orbit: Orbit::from_camera(&v.camera()),
            moved: true,
            ..StageViewState::default()
        }
    }
}

/// The 3D view at the top of Settings ▸ 3D stage.
pub const SETTINGS_VIEW: PreviewId = PreviewId::Other("stage-settings", 0);

/// Pictures are asked for in steps of this many pixels, so a pane being
/// resized doesn't make a new target every frame.
const STEP: u32 = 16;

/// The size asked for a view `px` pixels big.
pub fn request_size(px: Vec2) -> (u32, u32) {
    let f = |v: f32| (v.max(1.0).ceil() as u32).div_ceil(STEP).max(1) * STEP;
    (f(px.x), f(px.y))
}

/// What a view's picture is drawn with.
pub fn view_options(view: &StageViewState, scale: f32, bounds: Option<&Bounds>) -> Options {
    let radius = bounds.map_or(10.0, |b| b.radius());
    Options {
        quality: Quality::default().scaled_by(scale),
        grid: view.grid,
        grid_extent: (radius * 2.0).clamp(10.0, 500.0),
        axes: view.grid,
        venue_opacity: view.xray.clamp(0.05, 1.0),
        ..Options::default()
    }
}

/// Whether a view may ask for a picture this frame: when the governor says
/// so, or when it has none yet (a paused preview still gets its first).
pub fn wants_picture(budget: crate::preview_pace::PreviewBudget, has_picture: bool) -> bool {
    budget.draw || !has_picture
}

/// Whether the stage can be changed from here (as the multiview's layout:
/// not in show mode, on a follower, or in a test's copy of the show).
pub(crate) fn editable(st: &UiState) -> bool {
    !st.show_mode && !super::network::locked(st) && !st.test_locked
}

/// The 3D stage in `ui`'s space, as preview `id`.
pub fn draw(ui: &mut Ui, show: &Show, st: &mut UiState, view: &mut StageViewState, id: PreviewId) {
    if show.stage3d.is_empty() {
        empty(ui, show, st);
        return;
    }
    if super::previews_off_line(ui, st) {
        return;
    }
    let size = ui.available_size().max(vec2(80.0, 60.0));
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let ppp = ui.ctx().pixels_per_point();
    let px = request_size(rect.size() * ppp);
    let aspect = rect.width().max(1.0) / rect.height().max(1.0);
    let bounds = st.stage.bounds;
    if !view.moved
        && let Some(b) = bounds
        && view.framed != Some(b)
    {
        view.orbit.frame(&b, aspect);
        view.framed = Some(b);
    }

    // The mouse.
    let (shift, scroll, f_key) = ui.input(|i| {
        (
            i.modifiers.shift,
            i.smooth_scroll_delta.y,
            i.key_pressed(egui::Key::F),
        )
    });
    let delta = response.drag_delta() * ppp;
    // Looking through a projector: the lens's camera, until the mouse moves
    // the view (which then turns round from where the lens was).
    let lens = view
        .through
        .and_then(|p| super::stage_projectors::look_through(show, st, p));
    if lens.is_none() {
        view.through = None;
    }
    if let Some(cam) = lens
        && (response.dragged()
            || (response.hovered() && scroll != 0.0)
            || response.double_clicked()
            || ui.input(|i| i.key_pressed(egui::Key::Escape)))
    {
        view.orbit = Orbit::from_camera(&cam);
        view.through = None;
    }
    view.moved |= response.dragged() || (response.hovered() && scroll != 0.0);
    if response.dragged_by(egui::PointerButton::Primary) && !shift {
        view.orbit.orbit(delta.x, delta.y);
    } else if response.dragged_by(egui::PointerButton::Secondary)
        || response.dragged_by(egui::PointerButton::Middle)
        || (response.dragged_by(egui::PointerButton::Primary) && shift)
    {
        view.orbit.pan(delta.x, delta.y, rect.height() * ppp);
    }
    if response.hovered() && scroll != 0.0 {
        view.orbit.dolly(scroll / 50.0);
    }
    if (response.double_clicked() || (response.hovered() && f_key))
        && let Some(b) = &bounds
    {
        view.orbit.frame(b, aspect);
    }
    let camera = match view.through {
        Some(p) => super::stage_projectors::look_through(show, st, p)
            .unwrap_or_else(|| view.orbit.camera()),
        None => view.orbit.camera(),
    };
    // Where the picture is (the target's shape, fitted in the rect), for
    // the pointer's rays.
    let shown_at = st.stage.targets.get(&id).map_or(rect, |t| {
        super::preview::fit_picture((t.width, t.height), rect)
    });
    let ray_at = |at: Pos2| {
        let pixel = [(at.x - shown_at.min.x) * ppp, (at.y - shown_at.min.y) * ppp];
        let size = (
            (shown_at.width() * ppp).round().max(1.0) as u32,
            (shown_at.height() * ppp).round().max(1.0) as u32,
        );
        camera.ray(pixel, size)
    };
    if response.clicked()
        && let Some(at) = response.interact_pointer_pos()
    {
        let ray = ray_at(at);
        let scene = st.stage.scene.clone();
        // The screens and the projectors' bodies (not the venue).
        let from = show.stage3d.projectors_from(&scene);
        let hit = render3d::pick(&scene, &ray, &|i, p| !p.lit || i >= from);
        let projector = hit.and_then(|h| show.stage3d.part_projector(&scene, h.part));
        st.stage.projectors.picked = projector;
        st.stage.picked = match projector {
            Some(_) => None,
            None => hit.and_then(|h| show.stage3d.part_surface(&scene, h.part)),
        };
    }
    // The light under the pointer, worked out off this thread.
    if !show.stage3d.projectors.is_empty()
        && !response.dragged()
        && let Some(at) = response.hover_pos()
    {
        super::stage_projectors::ask_here(show, st, ray_at(at));
    } else if !response.hovered() {
        st.stage.projectors.here = None;
    }

    // Ask for a picture when the governor lets it (the last one stays).
    let budget = st.preview_budget(id);
    let has = st.stage.targets.contains_key(&id);
    if wants_picture(budget, has) {
        let options = view_options(view, budget.scale, bounds.as_ref());
        let lines = super::stage_projectors::view_lines(show, st, view.through);
        st.stage.requests.push(ViewRequest {
            id,
            size: px,
            camera,
            options,
            lines,
        });
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::from_rgb(15, 16, 19));
    if let Some(t) = st.stage.targets.get(&id) {
        let shown = super::preview::fit_picture((t.width, t.height), rect);
        painter.add(super::preview::callback(
            shown,
            OutputPreview {
                groups: vec![picture_group(t)],
                slot: Slot::Preview,
                pace: None,
            },
        ));
        // Through a lens: what lies outside its picture is shaded.
        if view.through.is_some() {
            letterbox(&painter, &camera, (t.width, t.height), shown);
        }
    }
    // The picked projector's name, where it stands.
    if let Some(pid) = st.stage.projectors.picked
        && view.through != Some(pid)
        && let Some(p) = show.projector(pid)
    {
        let size = (rect.width().max(1.0) as u32, rect.height().max(1.0) as u32);
        if let Some([x, y]) = camera.project(p.position(), size) {
            let at = rect.min + vec2(x, y);
            if rect.contains(at) {
                painter.text(
                    at + vec2(9.0, -9.0),
                    Align2::LEFT_BOTTOM,
                    &p.name,
                    egui::FontId::proportional(13.0),
                    Color32::from_rgb(255, 210, 90),
                );
            }
        }
    }
    // The picked screen's name, where it is.
    if let Some(sid) = st.stage.picked
        && let Some(s) = show.stage3d.surface(sid)
    {
        let size = (
            (rect.width()).max(1.0) as u32,
            (rect.height()).max(1.0) as u32,
        );
        if let Some([x, y]) = camera.project(s.placement.position, size) {
            let at = rect.min + vec2(x, y);
            if rect.contains(at) {
                painter.circle_stroke(
                    at,
                    6.0,
                    egui::Stroke::new(2.0, Color32::from_rgb(255, 200, 60)),
                );
                painter.text(
                    at + vec2(9.0, -9.0),
                    Align2::LEFT_BOTTOM,
                    &s.name,
                    egui::FontId::proportional(13.0),
                    Color32::from_rgb(255, 210, 90),
                );
            }
        }
    }
    overlay(ui, rect, show, st, view, id, aspect);
    if response.hovered() {
        ui.ctx().set_cursor_icon(if response.dragged() {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        });
    }
}

/// Shade what lies outside a lens's own picture (`Camera::picture_in`) in a
/// target of `size` shown at `shown`, and frame the picture.
fn letterbox(painter: &egui::Painter, camera: &Camera, size: (u32, u32), shown: Rect) {
    let [l, t, r, b] = camera.picture_in(size);
    let sx = shown.width() / size.0.max(1) as f32;
    let sy = shown.height() / size.1.max(1) as f32;
    let frame = Rect::from_min_max(
        shown.min + vec2(l * sx, t * sy),
        shown.min + vec2(r * sx, b * sy),
    );
    let shade = Color32::from_black_alpha(190);
    for band in [
        Rect::from_min_max(shown.min, egui::pos2(shown.max.x, frame.min.y)),
        Rect::from_min_max(egui::pos2(shown.min.x, frame.max.y), shown.max),
        Rect::from_min_max(
            egui::pos2(shown.min.x, frame.min.y),
            egui::pos2(frame.min.x, frame.max.y),
        ),
        Rect::from_min_max(
            egui::pos2(frame.max.x, frame.min.y),
            egui::pos2(shown.max.x, frame.max.y),
        ),
    ] {
        if band.width() > 0.5 && band.height() > 0.5 {
            painter.rect_filled(band, 0.0, shade);
        }
    }
    painter.rect_stroke(
        frame,
        0.0,
        egui::Stroke::new(1.0, Color32::from_rgb(255, 210, 90)),
        egui::StrokeKind::Inside,
    );
}

/// The view's picture as a layer filling its own canvas.
fn picture_group(t: &Arc<LayerTexture>) -> OutputGroup {
    OutputGroup {
        layers: vec![OutputLayer {
            texture: t.clone(),
            opacity: 1.0,
            fit: crate::show::Fit::Stretch,
            transform: Default::default(),
            color: Default::default(),
            fx: Default::default(),
        }],
        geometry: crate::gpu::Geometry {
            canvas: (t.width, t.height),
            warp: crate::gpu::IDENTITY,
            pin: crate::gpu::IDENTITY,
            mesh: None,
            slice: crate::gpu::Geometry::WHOLE,
        },
    }
}

/// The controls over the picture.
fn overlay(
    ui: &mut Ui,
    rect: Rect,
    show: &Show,
    st: &mut UiState,
    view: &mut StageViewState,
    id: PreviewId,
    aspect: f32,
) {
    let edit = editable(st);
    let bar = Rect::from_min_size(rect.min + vec2(6.0, 6.0), vec2(rect.width() - 12.0, 26.0));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(bar)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let ui = &mut child;
    egui::Frame::new()
        .fill(Color32::from_black_alpha(170))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.menu_button("Viewpoint ⏷", |ui| {
                    if show.stage3d.viewpoints.is_empty() {
                        ui.label(RichText::new("No saved viewpoints yet").weak());
                    }
                    for v in &show.stage3d.viewpoints {
                        if ui.button(&v.name).clicked() {
                            view.orbit = Orbit::from_camera(&v.camera());
                            view.moved = true;
                            view.through = None;
                            ui.close();
                        }
                    }
                    if !show.stage3d.projectors.is_empty() {
                        ui.separator();
                        for p in &show.stage3d.projectors {
                            if ui
                                .button(format!("Through {}", p.name))
                                .on_hover_text("See the room as this projector's lens sees it")
                                .clicked()
                            {
                                view.through = Some(p.id);
                                view.moved = true;
                                st.stage.projectors.picked = Some(p.id);
                                ui.close();
                            }
                        }
                    }
                    ui.separator();
                    if ui
                        .add_enabled(edit, egui::Button::new("Save this view…"))
                        .clicked()
                    {
                        view.saving = Some(format!("View {}", show.stage3d.viewpoints.len() + 1));
                        ui.close();
                    }
                });
                if ui
                    .button("See all")
                    .on_hover_text("Look at the whole stage (double-click, or F)")
                    .clicked()
                    && let Some(b) = &st.stage.bounds
                {
                    view.through = None;
                    view.orbit.frame(b, aspect);
                }
                ui.checkbox(&mut view.grid, "Grid")
                    .on_hover_text("A floor grid in metres, and the axes at the centre");
                if show.stage3d.venue.is_some() {
                    ui.label("X-ray");
                    ui.add(
                        egui::Slider::new(&mut view.xray, 0.1..=1.0)
                            .show_value(false)
                            .clamping(egui::SliderClamping::Always),
                    )
                    .on_hover_text("See the screens through the venue's walls");
                }
                // (Not on the settings page itself.)
                if id != SETTINGS_VIEW
                    && ui
                        .button("Edit…")
                        .on_hover_text(
                            "The venue, the screens and the viewpoints: Settings ▸ 3D stage",
                        )
                        .clicked()
                {
                    st.stage
                        .actions
                        .push(Action::OpenSettings(super::settings::SettingsPage::Stage3d));
                }
            });
        });
    if let Some(name) = &mut view.saving {
        let mut done = None;
        egui::Frame::new()
            .fill(Color32::from_black_alpha(190))
            .corner_radius(4.0)
            .inner_margin(egui::Margin::symmetric(6, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name");
                    let r = ui.add(egui::TextEdit::singleline(name).desired_width(140.0));
                    r.request_focus();
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("Save").clicked() || enter {
                        done = Some(true);
                    }
                    if ui.button("Cancel").clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                    {
                        done = Some(false);
                    }
                });
            });
        match done {
            Some(true) => {
                let cam = view.orbit.camera();
                let name = name.trim().to_string();
                if !name.is_empty() {
                    st.stage
                        .actions
                        .push(Action::Stage(StageEdit::AddViewpoint(Viewpoint {
                            name,
                            eye: cam.eye,
                            target: cam.target,
                            fov_deg: cam.fov_y_deg,
                        })));
                }
                view.saving = None;
            }
            Some(false) => view.saving = None,
            None => {}
        }
    }
    // Through a lens, and the light under the pointer.
    let mut corner = Vec::new();
    if let Some(p) = view.through.and_then(|p| show.projector(p)) {
        corner.push(format!("Through {} · drag or Esc to leave", p.name));
    }
    if let Some(h) = &st.stage.projectors.here {
        corner.push(super::stage_projectors::here_words(h, show.stage3d.units));
    }
    let n = corner.len();
    for (i, words) in corner.iter().enumerate() {
        ui.painter().text(
            Pos2::new(
                rect.max.x - 8.0,
                rect.max.y - 22.0 - 16.0 * (n - 1 - i) as f32,
            ),
            Align2::RIGHT_BOTTOM,
            words,
            egui::FontId::proportional(12.0),
            Color32::from_rgb(255, 225, 150),
        );
    }
    // What is happening, at the bottom.
    let status = status_words(st, id);
    if !status.is_empty() {
        ui.painter().text(
            Pos2::new(rect.min.x + 8.0, rect.max.y - 6.0),
            Align2::LEFT_BOTTOM,
            status,
            egui::FontId::proportional(11.0),
            Color32::from_gray(170),
        );
    }
}

/// The line at the bottom of a view: the venue's state, the previews giving
/// way, what the picture cost.
fn status_words(st: &UiState, id: PreviewId) -> String {
    let mut parts = Vec::new();
    match &st.stage.venue {
        VenueStatus::Loading(name) => parts.push(format!("Loading venue {name}…")),
        VenueStatus::Failed(why) => parts.push(format!("Venue not loaded: {why}")),
        VenueStatus::Arriving(name) => parts.push(format!("Venue still arriving: {name}")),
        VenueStatus::Loaded { warnings, .. } if !warnings.is_empty() => {
            parts.push(format!("Venue: {}", warnings[0]))
        }
        _ => {}
    }
    if let Some(n) = st.preview_governor.notice(st.prefs.preview_limits()) {
        parts.push(n);
    }
    if let Some(s) = st.stage.stats.get(&id) {
        parts.push(format!(
            "{} triangles, {}×{}",
            s.triangles, s.drawn.0, s.drawn.1
        ));
    }
    parts.join("  ·  ")
}

/// No stage yet: say what it is and offer a start.
fn empty(ui: &mut Ui, show: &Show, st: &mut UiState) {
    let edit = editable(st);
    ui.add_space(12.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new("No 3D stage yet").heading());
        ui.label(
            RichText::new(
                "Put your screens in a model of the venue and see the show on them, from any seat.",
            )
            .weak(),
        );
        ui.add_space(8.0);
        ui.add_enabled_ui(edit, |ui| {
            if ui
                .button("Load a venue model…")
                .on_hover_text("glTF (.glb, .gltf) or OBJ (.obj). FBX: export it as glTF first.")
                .clicked()
            {
                st.stage
                    .actions
                    .push(Action::AskFile(super::DialogFor::Venue));
            }
            if ui
                .button("Add a screen")
                .on_hover_text("A flat 16:9 screen, 4 m wide, showing the first display")
                .clicked()
            {
                st.stage
                    .actions
                    .push(Action::Stage(StageEdit::AddSurface(Preset::Flat)));
            }
            if let Some((kind, usual)) =
                crate::ui::stage_edit::sample_menu(ui, "Use a sample venue ⏷")
            {
                st.stage
                    .actions
                    .push(Action::Stage(StageEdit::UseSample { kind, usual }));
            }
        });
        if !edit {
            ui.label(
                RichText::new("Switch to edit mode to set up the stage.")
                    .weak()
                    .small(),
            );
        }
        let _ = show;
    });
}

#[cfg(test)]
#[path = "stage_view_tests.rs"]
mod tests;
