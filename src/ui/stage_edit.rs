//! Settings ▸ 3D stage: the venue model, the screens in it and what each
//! shows, the projectors, the saved viewpoints, and the displays that show
//! the stage, each on a tab (`StageTab`). A 3D view pinned at the top shows
//! every change as it is made; a screen or projector picked there opens on
//! its tab. Only the chosen tab's content scrolls.
//!
//! The page edits the show directly, as the other settings pages do (undo
//! sees it; show mode greys the page; a follower's edits are put back). What
//! a 3D view asks for (it has no `&mut Show`) comes as `StageEdit`s.

use std::path::PathBuf;

use egui::{RichText, Ui};

use super::settings::{Section, hint, untitled, warning};
use super::stage_view::{self, VenueStatus};
use super::{Action, UiState};
use crate::show::{OutputId, Show};
use crate::stage3d::samples::SampleVenue;
use crate::stage3d::{
    Facet, Feed, LengthUnit, Placement, StageSource, Surface, SurfaceId, SurfaceShape, UpAxis,
    Venue, Viewpoint, WHOLE, geometry,
};

/// What the page says of a screen whose shape a later NewQ wrote.
pub const UNKNOWN_SHAPE_WORDS: &str =
    "Made by a newer NewQ: this one can't show or change its shape. It is kept as it is.";

/// A screen to start from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// A 16:9 screen, 4 m wide.
    Flat,
    /// Part of a cylinder: a curved LED wall 12 m round, 3.4 m high.
    Curved,
    /// A box of four screens hung over an arena, with one underneath.
    Jumbotron,
    /// An LED wall of 6 x 3 panels, each column turned 6°.
    AngledWall,
}

impl Preset {
    pub const ALL: [Preset; 4] = [
        Preset::Flat,
        Preset::Curved,
        Preset::Jumbotron,
        Preset::AngledWall,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Preset::Flat => "Flat screen",
            Preset::Curved => "Curved wall",
            Preset::Jumbotron => "Jumbotron",
            Preset::AngledWall => "Angled LED wall",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Preset::Flat => "A 16:9 screen, 4 m wide",
            Preset::Curved => "Part of a cylinder: an LED wall 12 m round, 3.4 m high",
            Preset::Jumbotron => "Four screens round a box hung over the floor, and one underneath",
            Preset::AngledWall => "LED panels, 6 across and 3 high, each column turned a little",
        }
    }

    pub fn shape(self) -> SurfaceShape {
        match self {
            Preset::Flat => SurfaceShape::default(),
            Preset::Curved => SurfaceShape::Curved {
                width: 12.0,
                radius: 8.0,
                height: 3.4,
                convex: false,
            },
            Preset::Jumbotron => SurfaceShape::Faceted {
                facets: geometry::jumbotron(4, 6.0, 3.4, true, true),
            },
            Preset::AngledWall => SurfaceShape::Faceted {
                facets: geometry::angled_wall(6, 3, 1.0, 1.0, 6.0),
            },
        }
    }

    /// Its name, for "Screen N" in its place.
    fn name(self) -> &'static str {
        match self {
            Preset::Flat => "Screen",
            Preset::Curved => "Curved wall",
            Preset::Jumbotron => "Jumbotron",
            Preset::AngledWall => "LED wall",
        }
    }

    /// Where it stands at first: on the floor (a jumbotron hangs above it).
    fn height_above_floor(self, shape: &SurfaceShape) -> f32 {
        let h = shape_height(shape);
        match self {
            Preset::Jumbotron => 8.0,
            _ => h / 2.0,
        }
    }
}

/// How tall a shape is (its facets' extent, for a faceted one).
fn shape_height(shape: &SurfaceShape) -> f32 {
    match shape {
        SurfaceShape::Flat { height, .. } | SurfaceShape::Curved { height, .. } => *height,
        SurfaceShape::Faceted { facets } => {
            let lo = facets
                .iter()
                .map(|f| f.placement.position[1] - f.height / 2.0)
                .fold(f32::INFINITY, f32::min);
            let hi = facets
                .iter()
                .map(|f| f.placement.position[1] + f.height / 2.0)
                .fold(f32::NEG_INFINITY, f32::max);
            if lo.is_finite() && hi.is_finite() {
                hi - lo
            } else {
                1.0
            }
        }
        SurfaceShape::Unknown(_) => 1.0,
    }
}

/// An edit asked for from a 3D view (`StageShared::actions`).
#[derive(Clone, Debug, PartialEq)]
pub enum StageEdit {
    /// Use this model file as the venue (keeping the old one's settings).
    SetVenue(PathBuf),
    AddSurface(Preset),
    AddViewpoint(Viewpoint),
    /// Use one of NewQ's sample venues, and with `usual` its usual screens
    /// (showing the first display) and viewpoints.
    UseSample {
        kind: SampleVenue,
        usual: bool,
    },
    /// Write a sample venue as a .glb (to change in a 3D program). Not an
    /// edit of the show.
    ExportGlb(SampleVenue, PathBuf),
}

impl StageEdit {
    pub fn edits_the_show(&self) -> bool {
        !matches!(self, StageEdit::ExportGlb(..))
    }
}

/// Use sample `kind` as the venue (keeping where the old venue was put), and
/// with `usual` add its usual screens, each showing the first display, and
/// its viewpoints. The screens added.
pub fn use_sample(show: &mut Show, kind: SampleVenue, usual: bool) -> Vec<SurfaceId> {
    let venue = match show.stage3d.venue.take() {
        Some(old) => Venue {
            placement: old.placement,
            visible: old.visible,
            brightness: old.brightness,
            ..Venue::sample(kind)
        },
        None => Venue::sample(kind),
    };
    show.stage3d.venue = Some(venue);
    if !usual {
        return Vec::new();
    }
    let feed = first_feed(show);
    let ids = show.add_sample_screens(kind);
    for id in &ids {
        if let Some(s) = show.stage3d.surface_mut(*id) {
            s.feed = feed.clone();
        }
    }
    show.stage3d.add_sample_viewpoints(kind);
    ids
}

/// A menu of the sample venues; the one chosen, and whether with its usual
/// screens and viewpoints.
pub(super) fn sample_menu(ui: &mut Ui, label: &str) -> Option<(SampleVenue, bool)> {
    let key = egui::Id::new("stage_sample_usual");
    let mut chosen = None;
    ui.menu_button(label, |ui| {
        let mut usual = ui.data(|d| d.get_temp::<bool>(key)).unwrap_or(true);
        ui.checkbox(&mut usual, "With its usual screens and viewpoints");
        ui.data_mut(|d| d.insert_temp(key, usual));
        ui.separator();
        for k in SampleVenue::ALL {
            if ui.button(k.label()).clicked() {
                chosen = Some((k, usual));
                ui.close();
            }
        }
    })
    .response
    .on_hover_text(
        "A venue made by NewQ: an arena, theatres, a black box, a club, a festival stage, a \
         ballroom. Export it as .glb to change it in a 3D program.",
    );
    chosen
}

/// The first display a new screen shows: the whole picture of the first
/// display (not a sub display) that doesn't itself show the stage.
pub fn first_feed(show: &Show) -> Feed {
    show.outputs
        .iter()
        .find(|o| o.enabled && !o.is_sub_display() && !show.shows_stage(o.id))
        .map_or(Feed::None, |o| Feed::whole_picture(show, o.id))
}

/// Add a screen made from `preset`, showing the first display, a little
/// apart from the screens already there. Its id.
pub fn add_surface(show: &mut Show, preset: Preset) -> SurfaceId {
    let shape = preset.shape();
    let y = preset.height_above_floor(&shape);
    let n = show
        .stage3d
        .surfaces
        .iter()
        .filter(|s| s.name.starts_with(preset.name()))
        .count();
    let feed = first_feed(show);
    let others = show.stage3d.surfaces.len();
    let id = show.add_surface(Some(shape));
    if let Some(s) = show.stage3d.surface_mut(id) {
        s.name = format!("{} {}", preset.name(), n + 1);
        s.feed = feed;
        // Side by side, so a new one isn't hidden in the last.
        let x = if others == 0 {
            0.0
        } else {
            (others as f32) * 5.0
        };
        s.placement.position = [x, y, 0.0];
    }
    id
}

/// Make edit `e` (through `ui::apply`, after the show-mode and lock checks).
pub fn apply(e: StageEdit, show: &mut Show, engine: &mut crate::engine::Engine, st: &mut UiState) {
    match e {
        StageEdit::SetVenue(path) => {
            // A file in place of a sample: the sample goes.
            let venue = match show.stage3d.venue.take() {
                Some(old) => Venue {
                    path,
                    sample: None,
                    ..old
                },
                None => Venue::new(path),
            };
            show.stage3d.venue = Some(venue);
            // See it from the start.
            st.stage.settings_view = Default::default();
            for v in st.preview_stage.values_mut() {
                *v = Default::default();
            }
        }
        StageEdit::AddSurface(preset) => {
            let id = add_surface(show, preset);
            st.stage.picked = Some(id);
        }
        StageEdit::UseSample { kind, usual } => {
            use_sample(show, kind, usual);
            // From its first seat if it has one (an arena from outside is a
            // roof), else all of it.
            let fresh = || match show.stage3d.viewpoints.first() {
                Some(v) if usual => stage_view::StageViewState::from_viewpoint(v),
                _ => Default::default(),
            };
            st.stage.settings_view = fresh();
            for v in st.preview_stage.values_mut() {
                *v = fresh();
            }
        }
        StageEdit::ExportGlb(kind, path) => {
            // Made and written on a thread: an arena is a few megabytes.
            let name = kind.label();
            let (tx, rx) = std::sync::mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("sample-glb".into())
                .spawn(move || {
                    let bytes = crate::stage3d::samples::sample_glb(kind);
                    let _ = tx.send(std::fs::write(&path, bytes).map(|_| path));
                });
            match spawned {
                Ok(_) => st.stage.exports.push(rx),
                Err(e) => engine.problem(format!("{name} wasn't saved: {e}")),
            }
        }
        StageEdit::AddViewpoint(v) => {
            let mut v = v;
            v.name = unique_name(&show.stage3d.viewpoints, &v.name);
            show.stage3d.viewpoints.push(v);
        }
    }
}

/// `name`, or `name 2` and so on if a viewpoint already has it.
pub fn unique_name(viewpoints: &[Viewpoint], name: &str) -> String {
    let taken = |n: &str| viewpoints.iter().any(|v| v.name == n);
    if !taken(name) {
        return name.to_string();
    }
    (2..)
        .map(|i| format!("{name} {i}"))
        .find(|n| !taken(n))
        .unwrap_or_else(|| name.to_string())
}

/// A copy of surface `s` with a new id, next to it.
fn duplicate(show: &mut Show, s: &Surface) -> SurfaceId {
    let id = SurfaceId(show.alloc_id());
    let mut copy = s.clone();
    copy.id = id;
    copy.name = format!("{} copy", s.name);
    copy.placement.position[0] += 1.0;
    show.stage3d.surfaces.push(copy);
    id
}

/// The displays a screen can show: every display that doesn't show the
/// stage itself, with words for a projector (its whole picture is shown).
pub fn feed_choices(show: &Show) -> Vec<(OutputId, String)> {
    show.outputs
        .iter()
        .filter(|o| !show.shows_stage(o.id))
        .map(|o| {
            let label = if show.whole_picture(o.id) != o.id {
                let whole = show
                    .output(show.whole_picture(o.id))
                    .map_or("", |w| w.name.as_str());
                format!("{} (shows {whole}'s whole picture)", o.name)
            } else if o.is_sub_display() {
                format!("{} (sub display)", o.name)
            } else {
                o.name.clone()
            };
            (o.id, label)
        })
        .collect()
}

/// The tabs of Settings ▸ 3D stage, under its view. The one chosen is kept
/// for the run (`StageShared::tab`), not in the show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum StageTab {
    #[default]
    Venue,
    Screens,
    Projectors,
    Viewpoints,
    Displays,
}

impl StageTab {
    pub const ALL: [StageTab; 5] = [
        StageTab::Venue,
        StageTab::Screens,
        StageTab::Projectors,
        StageTab::Viewpoints,
        StageTab::Displays,
    ];

    /// The section of the page it is: its label is the section's name, so
    /// a pointer ("Settings ▸ 3D stage ▸ Projectors") names a tab.
    pub fn section(self) -> Section {
        use super::settings::*;
        match self {
            StageTab::Venue => STAGE_VENUE,
            StageTab::Screens => STAGE_SCREENS,
            StageTab::Projectors => STAGE_PROJECTORS,
            StageTab::Viewpoints => STAGE_VIEWPOINTS,
            StageTab::Displays => STAGE_DISPLAYS,
        }
    }

    /// How many of what it holds there are, shown beside its label.
    fn count(self, show: &Show) -> usize {
        let s = &show.stage3d;
        match self {
            StageTab::Venue => 0,
            StageTab::Screens => s.surfaces.len(),
            StageTab::Projectors => s.projectors.len(),
            StageTab::Viewpoints => s.viewpoints.len(),
            StageTab::Displays => show
                .outputs
                .iter()
                .filter(|o| !o.is_sub_display() && show.stage_source(o.id).is_some())
                .count(),
        }
    }

    fn hint(self) -> &'static str {
        match self {
            StageTab::Venue => "The room's model, or a sample venue",
            StageTab::Screens => "The screens in the stage and the display each shows",
            StageTab::Projectors => "Projectors: where they stand, their lenses and their light",
            StageTab::Viewpoints => "Saved places to look from",
            StageTab::Displays => "The displays that show the 3D stage instead of their cues",
        }
    }
}

/// The least height of the view on top, and of the tab's content under it.
const VIEW_LEAST: f32 = 90.0;
const CONTENT_LEAST: f32 = 80.0;
/// What the page has under the view besides the content: its handle, its
/// hint, the tabs (one row) and the line under them.
const CHROME: f32 = 80.0;
/// A page this high has the view on top, pinned, and the tabs under it.
const STACKED_FROM: f32 = 420.0;
/// A page less high but this wide has the view pinned on the left and the
/// tabs beside it (the classic window, whose settings have little height
/// under the transport). The least widths of the two.
const SIDE_BY_SIDE_FROM: f32 = 560.0;
const SIDE_VIEW_LEAST: f32 = 200.0;
const SIDE_CONTENT_LEAST: f32 = 360.0;
/// Below this the page, short and narrow, has no room to pin anything: the
/// view, the tabs and the content scroll as one.
const SHORT_PAGE: f32 = VIEW_LEAST + CHROME + CONTENT_LEAST;

/// How the page is laid out, by the room it has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arrangement {
    /// The view on top, the tabs under it.
    Stacked,
    /// The view on the left, the tabs on the right.
    SideBySide,
    /// Everything in one scroll area.
    Cramped,
}

fn arrangement(size: egui::Vec2) -> Arrangement {
    if size.y >= STACKED_FROM {
        Arrangement::Stacked
    } else if size.x >= SIDE_BY_SIDE_FROM && size.y >= VIEW_LEAST {
        Arrangement::SideBySide
    } else if size.y >= SHORT_PAGE {
        Arrangement::Stacked
    } else {
        Arrangement::Cramped
    }
}

/// The view's height on a page `page` high: a share of it until the handle
/// is dragged, never so tall that the content has less than its least.
fn view_height(page: f32, dragged: Option<f32>) -> f32 {
    let most = (page - CHROME - CONTENT_LEAST).max(VIEW_LEAST);
    dragged
        .unwrap_or((page * 0.45).min(460.0))
        .max(VIEW_LEAST)
        .min(most)
}

/// The view's width beside the tabs on a page `page` wide.
fn view_width(page: f32, dragged: Option<f32>) -> f32 {
    let most = (page - SIDE_CONTENT_LEAST).max(SIDE_VIEW_LEAST);
    dragged
        .unwrap_or((page * 0.35).min(640.0))
        .max(SIDE_VIEW_LEAST)
        .min(most)
}

const VIEW_HINT: &str = "Drag to turn, right-drag or Shift-drag to slide, the wheel to come \
                         closer; double-click to see it all. Click a screen or a projector to \
                         pick it.";

/// The page: the view pinned (its size can be dragged), the tabs beside or
/// under it, and the chosen tab's content in a scroll area of its own.
pub(super) fn page(ui: &mut Ui, show: &mut Show, st: &mut UiState, actions: &mut Vec<Action>) {
    let room = ui.available_size();
    match arrangement(room) {
        Arrangement::Stacked => {
            // The view first: every change below shows in it.
            let h = view_height(room.y, st.stage.view_height);
            the_view(ui, show, st, h);
            if let Some(d) = grip(ui, egui::vec2(ui.available_width(), 8.0), false) {
                st.stage.view_height = Some(view_height(room.y, Some(h + d)));
            }
            if room.y >= STACKED_FROM {
                hint(ui, VIEW_HINT);
            }
            tabs(ui, show, st);
            tab_scroll(ui, show, st, actions);
        }
        Arrangement::SideBySide => {
            let rect = ui.available_rect_before_wrap();
            let w = view_width(rect.width(), st.stage.view_width);
            let left = egui::Rect::from_min_size(rect.min, egui::vec2(w, rect.height()));
            let mut l = ui.new_child(egui::UiBuilder::new().max_rect(left));
            the_view(&mut l, show, st, rect.height());
            let mut g = ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(
                egui::pos2(left.max.x, rect.min.y),
                rect.max,
            )));
            if let Some(d) = grip(&mut g, egui::vec2(8.0, rect.height()), true) {
                st.stage.view_width = Some(view_width(rect.width(), Some(w + d)));
            }
            let right =
                egui::Rect::from_min_max(egui::pos2(left.max.x + 12.0, rect.min.y), rect.max);
            let mut r = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(right)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            tabs(&mut r, show, st);
            tab_scroll(&mut r, show, st, actions);
            ui.advance_cursor_after_rect(rect);
        }
        Arrangement::Cramped => {
            let out = egui::ScrollArea::both()
                .id_salt("stage_cramped")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.set_max_width((ui.available_width() - 16.0).max(120.0));
                    the_view(ui, show, st, VIEW_LEAST);
                    tabs(ui, show, st);
                    tab_content(ui, show, st, actions);
                });
            note_rect(ui, "3D stage ▸ tab content", out.inner_rect);
        }
    }
}

/// A handle to drag the view's size by, `size` big (a line across it, or
/// down it when `across`). How far it was dragged this frame.
fn grip(ui: &mut Ui, size: egui::Vec2, across: bool) -> Option<f32> {
    let (rect, drag) = ui.allocate_exact_size(size, egui::Sense::drag());
    note(ui, "3D stage ▸ view size", &drag);
    let drag = if across {
        drag.on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
            .on_hover_text("Drag to make the view wider or narrower")
    } else {
        drag.on_hover_cursor(egui::CursorIcon::ResizeVertical)
            .on_hover_text("Drag to make the view taller or shorter")
    };
    let colour = if drag.hovered() || drag.dragged() {
        ui.visuals().widgets.hovered.fg_stroke.color
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke.color
    };
    let half = if across {
        egui::vec2(0.0, 24.0)
    } else {
        egui::vec2(24.0, 0.0)
    };
    ui.painter().line_segment(
        [rect.center() - half, rect.center() + half],
        egui::Stroke::new(2.0, colour),
    );
    let d = drag.drag_delta();
    drag.dragged().then_some(if across { d.x } else { d.y })
}

/// The chosen tab's content in its own scroll area (each tab keeps its
/// place; rows wider than a cramped page scroll sideways).
fn tab_scroll(ui: &mut Ui, show: &mut Show, st: &mut UiState, actions: &mut Vec<Action>) {
    let tab = st.stage.tab;
    let out = egui::ScrollArea::both()
        .id_salt(("stage_tab", tab))
        .auto_shrink([false, false])
        .show(ui, |ui| {
            // Room for the frame's edges and the scroll bar.
            ui.set_max_width((ui.available_width() - 16.0).max(120.0));
            tab_content(ui, show, st, actions);
        });
    note_rect(ui, "3D stage ▸ tab content", out.inner_rect);
}

/// The 3D view, `h` high and as wide as the page.
fn the_view(ui: &mut Ui, show: &Show, st: &mut UiState, h: f32) {
    let mut view = std::mem::take(&mut st.stage.settings_view);
    let r = ui.allocate_ui(egui::vec2(ui.available_width(), h), |ui| {
        ui.set_min_size(egui::vec2(ui.available_width(), h));
        stage_view::draw(ui, show, st, &mut view, stage_view::SETTINGS_VIEW);
    });
    st.stage.settings_view = view;
    note(ui, "3D stage ▸ view", &r.response);
}

/// The row of tabs, each with how many it holds.
fn tabs(ui: &mut Ui, show: &Show, st: &mut UiState) {
    // What is newly picked (in a view, or by adding it) opens on its tab,
    // once: then any tab can be chosen.
    let s = &mut st.stage;
    if s.picked != s.tab_saw.0 {
        s.tab_saw.0 = s.picked;
        if s.picked.is_some() {
            s.tab = StageTab::Screens;
        }
    }
    if s.projectors.picked != s.tab_saw.1 {
        s.tab_saw.1 = s.projectors.picked;
        if s.projectors.picked.is_some() {
            s.tab = StageTab::Projectors;
        }
    }
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        for tab in StageTab::ALL {
            let label = tab.section().name;
            super::settings::note_heading(ui, label);
            let n = tab.count(show);
            let text = if n > 0 {
                format!("{label} ({n})")
            } else {
                label.to_string()
            };
            let r = ui
                .selectable_label(st.stage.tab == tab, RichText::new(text).size(14.0))
                .on_hover_text(tab.hint());
            note(ui, &format!("3D stage ▸ tab ▸ {label}"), &r);
            if r.clicked() {
                st.stage.tab = tab;
            }
        }
    });
    ui.separator();
}

/// What the chosen tab holds.
fn tab_content(ui: &mut Ui, show: &mut Show, st: &mut UiState, actions: &mut Vec<Action>) {
    match st.stage.tab {
        StageTab::Venue => venue_section(ui, show, st, actions),
        StageTab::Screens => surfaces_section(ui, show, st),
        StageTab::Projectors => super::stage_projectors::section_ui(ui, show, st),
        StageTab::Viewpoints => viewpoints_section(ui, show),
        StageTab::Displays => sources_section(ui, show, st, actions),
    }
    ui.add_space(8.0);
}

fn venue_section(ui: &mut Ui, show: &mut Show, st: &mut UiState, actions: &mut Vec<Action>) {
    untitled(ui, |ui| {
        let unit = show.stage3d.units;
        match &mut show.stage3d.venue {
            None => {
                ui.horizontal(|ui| {
                    ui.label("No venue model.");
                    if ui.button("Load a venue model…").clicked() {
                        actions.push(Action::AskFile(super::DialogFor::Venue));
                    }
                    if let Some((kind, usual)) = sample_menu(ui, "Use a sample venue ⏷") {
                        actions.push(Action::Stage(StageEdit::UseSample { kind, usual }));
                    }
                });
                hint(
                    ui,
                    "A model of the room, stage or arena: glTF (.glb, .gltf) or OBJ (.obj). \
                     FBX can't be read yet: export the model as glTF from your 3D program. \
                     The screens can be placed without one.",
                );
            }
            Some(v) => {
                let mut remove = false;
                let mut usual = None;
                ui.horizontal_wrapped(|ui| {
                    match v.sample {
                        Some(kind) => {
                            ui.label(RichText::new(format!("Sample: {}", kind.label())).strong());
                        }
                        None => {
                            ui.label(RichText::new(file_label(&v.path)).strong())
                                .on_hover_text(v.path.display().to_string());
                        }
                    }
                    if ui
                        .button("Load a model…")
                        .on_hover_text("Use a model file instead (glTF or OBJ)")
                        .clicked()
                    {
                        actions.push(Action::AskFile(super::DialogFor::Venue));
                    }
                    if let Some((kind, usual)) = sample_menu(ui, "Sample ⏷") {
                        actions.push(Action::Stage(StageEdit::UseSample { kind, usual }));
                    }
                    if let Some(kind) = v.sample {
                        if ui
                            .button("Export as .glb…")
                            .on_hover_text(
                                "Save this sample as a glTF model, to change in Blender or \
                                 another 3D program, then load it back",
                            )
                            .clicked()
                        {
                            actions.push(Action::AskFile(super::DialogFor::SampleGlb(kind)));
                        }
                        if ui
                            .button("Add the usual screens")
                            .on_hover_text(
                                "The screens this venue usually has, and its viewpoints, each \
                                 screen showing the first display",
                            )
                            .clicked()
                        {
                            usual = Some(kind);
                        }
                    }
                    if ui.button("Remove").clicked() {
                        remove = true;
                    }
                });
                match &st.stage.venue {
                    VenueStatus::Loading(_) => hint(ui, "Loading…"),
                    VenueStatus::Loaded {
                        triangles,
                        warnings,
                    } => {
                        hint(ui, &format!("{triangles} triangles"));
                        for w in warnings {
                            warning(ui, w);
                        }
                    }
                    VenueStatus::Failed(why) => warning(ui, why),
                    VenueStatus::Arriving(name) => hint(ui, &stage_view::arriving_words(name)),
                    VenueStatus::None => {}
                }
                egui::Grid::new("stage_venue")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Shown");
                        ui.checkbox(&mut v.visible, "");
                        ui.end_row();
                        if v.sample.is_some() {
                            // A sample is in metres, Y up.
                        } else {
                            ui.label("The file is in");
                            egui::ComboBox::from_id_salt("venue_units")
                                .selected_text(v.units.label())
                                .show_ui(ui, |ui| {
                                    for u in LengthUnit::ALL {
                                        ui.selectable_value(&mut v.units, u, u.label());
                                    }
                                })
                                .response
                                .on_hover_text(
                                    "What the model's numbers are: glTF is in metres; an OBJ is \
                                 in whatever its program used",
                                );
                            ui.end_row();
                            ui.label("Up is");
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut v.up, UpAxis::Y, "Y");
                                ui.selectable_value(&mut v.up, UpAxis::Z, "Z")
                                    .on_hover_text(
                                        "For a model from a Z-up program (3ds Max, SketchUp, CAD)",
                                    );
                            });
                            ui.end_row();
                        }
                        ui.label("Brightness");
                        ui.add(
                            egui::Slider::new(&mut v.brightness, 0.05..=4.0)
                                .logarithmic(true)
                                .clamping(egui::SliderClamping::Always),
                        )
                        .on_hover_text("Dim a bright model so the screens stand out");
                        ui.end_row();
                        placement_rows(ui, "venue", &mut v.placement, unit);
                    });
                if remove {
                    show.stage3d.venue = None;
                }
                if let Some(kind) = usual {
                    let feed = first_feed(show);
                    for id in show.add_sample_screens(kind) {
                        if let Some(s) = show.stage3d.surface_mut(id) {
                            s.feed = feed.clone();
                        }
                    }
                    show.stage3d.add_sample_viewpoints(kind);
                }
            }
        }
    });
}

fn file_label(p: &std::path::Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// A length in metres, shown and typed in `unit`.
fn length(ui: &mut Ui, metres: &mut f32, unit: LengthUnit) -> egui::Response {
    let per = unit.metres();
    let mut shown = *metres / per;
    let suffix = match unit {
        LengthUnit::Metres => " m",
        LengthUnit::Centimetres => " cm",
        LengthUnit::Millimetres => " mm",
        LengthUnit::Inches => " in",
        LengthUnit::Feet => " ft",
    };
    let r = ui.add(
        egui::DragValue::new(&mut shown)
            .speed(0.02 / per)
            .max_decimals(3)
            .suffix(suffix),
    );
    if r.changed() && shown.is_finite() {
        *metres = shown * per;
    }
    r
}

fn degrees(ui: &mut Ui, v: &mut f32) -> egui::Response {
    ui.add(
        egui::DragValue::new(v)
            .speed(0.5)
            .max_decimals(1)
            .suffix("°"),
    )
}

pub(super) fn placement_rows(ui: &mut Ui, salt: &str, p: &mut Placement, unit: LengthUnit) {
    ui.label("Position");
    ui.push_id((salt, "pos"), |ui| {
        ui.horizontal(|ui| {
            for (axis, v) in ["x", "y", "z"].iter().zip(p.position.iter_mut()) {
                ui.label(RichText::new(*axis).weak());
                length(ui, v, unit);
            }
        });
    });
    ui.end_row();
    ui.label("Turn");
    ui.push_id((salt, "turn"), |ui| {
        ui.horizontal(|ui| {
            ui.label(RichText::new("yaw").weak());
            degrees(ui, &mut p.yaw).on_hover_text("Round the upright: which way it faces");
            ui.label(RichText::new("pitch").weak());
            degrees(ui, &mut p.pitch).on_hover_text("Tipped forwards (positive) or back");
            ui.label(RichText::new("roll").weak());
            degrees(ui, &mut p.roll).on_hover_text("Turned in its own plane");
        });
    });
    ui.end_row();
}

fn surfaces_section(ui: &mut Ui, show: &mut Show, st: &mut UiState) {
    untitled(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("Units shown");
            egui::ComboBox::from_id_salt("stage_units")
                .selected_text(show.stage3d.units.label())
                .show_ui(ui, |ui| {
                    for u in LengthUnit::ALL {
                        ui.selectable_value(&mut show.stage3d.units, u, u.label());
                    }
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.menu_button("+ Add a screen", |ui| {
                    for p in Preset::ALL {
                        if ui.button(p.label()).on_hover_text(p.hint()).clicked() {
                            st.stage.picked = Some(add_surface(show, p));
                            ui.close();
                        }
                    }
                });
            });
        });
        if show.stage3d.surfaces.is_empty() {
            hint(
                ui,
                "No screens yet. Add one: it shows the first display's picture, and can be \
                 moved, turned and given another display below.",
            );
            return;
        }
        let unit = show.stage3d.units;
        let choices = feed_choices(show);
        let mut remove = None;
        let mut copy = None;
        let ids: Vec<SurfaceId> = show.stage3d.surfaces.iter().map(|s| s.id).collect();
        let newly_picked = st.stage.picked != st.stage.opened_for;
        st.stage.opened_for = st.stage.picked;
        for id in ids {
            let picked = st.stage.picked == Some(id);
            let Some(s) = show.stage3d.surface_mut(id) else {
                continue;
            };
            let header =
                egui::CollapsingHeader::new(RichText::new(&s.name).strong().color(if picked {
                    egui::Color32::from_rgb(255, 210, 90)
                } else {
                    ui.visuals().text_color()
                }))
                .id_salt(("surface", id.0))
                // A screen picked in a view opens here, once (then it can be closed).
                .open((picked && newly_picked).then_some(true));
            let r = header.show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut s.name);
                    ui.checkbox(&mut s.visible, "Shown");
                    if ui.button("Duplicate").clicked() {
                        copy = Some(s.clone());
                    }
                    if ui.button("Delete").clicked() {
                        remove = Some(id);
                    }
                });
                egui::Grid::new(("surface_grid", id.0))
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        feed_rows(ui, id, &mut s.feed, &choices);
                        placement_rows(ui, &format!("s{}", id.0), &mut s.placement, unit);
                        shape_rows(ui, id, &mut s.shape, unit);
                    });
            });
            if picked && newly_picked {
                r.header_response.scroll_to_me(Some(egui::Align::TOP));
            }
            if r.header_response.clicked() {
                st.stage.picked = Some(id);
                st.stage.opened_for = Some(id);
            }
        }
        if let Some(s) = copy {
            st.stage.picked = Some(duplicate(show, &s));
        }
        if let Some(id) = remove {
            show.remove_surface(id);
            if st.stage.picked == Some(id) {
                st.stage.picked = None;
            }
        }
    });
}

fn feed_rows(ui: &mut Ui, id: SurfaceId, feed: &mut Feed, choices: &[(OutputId, String)]) {
    ui.label("Shows");
    let current = feed.output();
    let text = current
        .and_then(|o| choices.iter().find(|(c, _)| *c == o))
        .map_or("Nothing".to_string(), |(_, l)| l.clone());
    egui::ComboBox::from_id_salt(("feed", id.0))
        .selected_text(text)
        .width(260.0)
        .show_ui(ui, |ui| {
            if ui.selectable_label(current.is_none(), "Nothing").clicked() {
                *feed = Feed::None;
            }
            for (o, label) in choices {
                if ui.selectable_label(current == Some(*o), label).clicked() {
                    let region = match feed {
                        Feed::Display { region, .. } => *region,
                        Feed::None | Feed::Unknown(_) => WHOLE,
                    };
                    *feed = Feed::Display { output: *o, region };
                }
            }
        })
        .response
        .on_hover_text(
            "Which display's picture is on it. A display that itself shows the stage \
             can't be: it would show itself.",
        );
    ui.end_row();
    if let Feed::Display { region, .. } = feed {
        ui.label("");
        ui.horizontal(|ui| {
            let mut part = *region != WHOLE;
            if ui
                .checkbox(&mut part, "Only part of the picture")
                .on_hover_text("For a screen that shows a share of a wider picture")
                .changed()
            {
                *region = if part { [0.0, 0.0, 0.5, 1.0] } else { WHOLE };
            }
            if part {
                for (name, v) in ["left", "top", "width", "height"]
                    .iter()
                    .zip(region.iter_mut())
                {
                    ui.label(RichText::new(*name).weak());
                    ui.add(
                        egui::DragValue::new(v)
                            .speed(0.005)
                            .range(0.0..=1.0)
                            .max_decimals(3),
                    );
                }
            }
        });
        ui.end_row();
    }
}

fn shape_rows(ui: &mut Ui, id: SurfaceId, shape: &mut SurfaceShape, unit: LengthUnit) {
    match shape {
        SurfaceShape::Flat { width, height } => {
            ui.label("Size");
            ui.horizontal(|ui| {
                ui.label(RichText::new("width").weak());
                length(ui, width, unit);
                ui.label(RichText::new("height").weak());
                length(ui, height, unit);
                if ui
                    .small_button("16:9")
                    .on_hover_text("Height to suit a 16:9 picture")
                    .clicked()
                {
                    *height = *width * 9.0 / 16.0;
                }
            });
            ui.end_row();
        }
        SurfaceShape::Curved {
            width,
            radius,
            height,
            convex,
        } => {
            ui.label("Curve");
            ui.horizontal(|ui| {
                ui.label(RichText::new("width").weak());
                length(ui, width, unit).on_hover_text("Measured along the curve");
                ui.label(RichText::new("radius").weak());
                length(ui, radius, unit);
                ui.label(RichText::new("height").weak());
                length(ui, height, unit);
                ui.checkbox(convex, "Bulging out")
                    .on_hover_text("Round a pillar rather than round the audience");
            });
            ui.end_row();
        }
        SurfaceShape::Faceted { facets } => {
            ui.label("Faces");
            ui.vertical(|ui| {
                ui.label(format!("{} flat faces", facets.len()));
                rebuild(ui, id, facets, unit);
            });
            ui.end_row();
        }
        SurfaceShape::Unknown(_) => {
            ui.label("Shape");
            ui.label(RichText::new(UNKNOWN_SHAPE_WORDS).weak());
            ui.end_row();
        }
    }
}

/// What a faceted screen can be made again as.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rebuild {
    sides: u32,
    width: f32,
    height: f32,
    bottom: bool,
    columns: u32,
    rows: u32,
    panel_w: f32,
    panel_h: f32,
    angle: f32,
}

impl Default for Rebuild {
    fn default() -> Self {
        Rebuild {
            sides: 4,
            width: 6.0,
            height: 3.4,
            bottom: true,
            columns: 6,
            rows: 3,
            panel_w: 1.0,
            panel_h: 1.0,
            angle: 6.0,
        }
    }
}

fn rebuild(ui: &mut Ui, id: SurfaceId, facets: &mut Vec<Facet>, unit: LengthUnit) {
    let key = egui::Id::new(("stage_rebuild", id.0));
    let mut r: Rebuild = ui.data_mut(|d| *d.get_temp_mut_or_default::<Rebuild>(key));
    egui::CollapsingHeader::new("Make it again as…")
        .id_salt(("rebuild", id.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Jumbotron:");
                ui.add(
                    egui::DragValue::new(&mut r.sides)
                        .range(3..=12)
                        .suffix(" sides"),
                );
                length(ui, &mut r.width, unit);
                ui.label("×");
                length(ui, &mut r.height, unit);
                ui.checkbox(&mut r.bottom, "and underneath");
                if ui.button("Make").clicked() {
                    *facets = geometry::jumbotron(r.sides, r.width, r.height, r.bottom, true);
                }
            });
            ui.horizontal(|ui| {
                ui.label("Angled wall:");
                ui.add(
                    egui::DragValue::new(&mut r.columns)
                        .range(1..=64)
                        .suffix(" across"),
                );
                ui.add(
                    egui::DragValue::new(&mut r.rows)
                        .range(1..=64)
                        .suffix(" high"),
                );
                ui.label("panels");
                length(ui, &mut r.panel_w, unit);
                ui.label("×");
                length(ui, &mut r.panel_h, unit);
                degrees(ui, &mut r.angle)
                    .on_hover_text("Each column turned this much from the last");
                if ui.button("Make").clicked() {
                    *facets =
                        geometry::angled_wall(r.columns, r.rows, r.panel_w, r.panel_h, r.angle);
                }
            });
        });
    ui.data_mut(|d| d.insert_temp(key, r));
}

fn viewpoints_section(ui: &mut Ui, show: &mut Show) {
    untitled(ui, |ui| {
        if show.stage3d.viewpoints.is_empty() {
            hint(
                ui,
                "None saved. In a 3D view, look from where you want and choose Viewpoint ▸ \
                 Save this view…",
            );
            return;
        }
        let mut remove = None;
        let mut rename = None;
        let names: Vec<String> = show
            .stage3d
            .viewpoints
            .iter()
            .map(|v| v.name.clone())
            .collect();
        for (i, v) in show.stage3d.viewpoints.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.push_id(("viewpoint", i), |ui| {
                    // Typed into a draft: the name changes (and the displays
                    // that show the stage from it with it) only while it is
                    // a name no other viewpoint has.
                    let key = ui.id().with("draft");
                    let mut draft = ui
                        .data(|d| d.get_temp::<String>(key))
                        .unwrap_or_else(|| v.name.clone());
                    let r = ui.text_edit_singleline(&mut draft);
                    if r.changed() {
                        rename = Some((i, draft.clone()));
                    }
                    if r.has_focus() {
                        ui.data_mut(|d| d.insert_temp(key, draft.clone()));
                    } else {
                        ui.data_mut(|d| d.remove::<String>(key));
                    }
                    let typed = draft.trim();
                    if typed.is_empty() {
                        ui.label(RichText::new("It needs a name").weak());
                    } else if typed != v.name
                        && names.iter().enumerate().any(|(j, n)| j != i && n == typed)
                    {
                        ui.label(RichText::new("Another viewpoint has this name").weak());
                    }
                    ui.label(RichText::new("field of view").weak());
                    ui.add(
                        egui::DragValue::new(&mut v.fov_deg)
                            .range(5.0..=150.0)
                            .suffix("°"),
                    );
                    if ui.button("Delete").clicked() {
                        remove = Some(i);
                    }
                });
            });
        }
        if let Some((i, name)) = rename {
            show.rename_viewpoint(i, &name);
        }
        if let Some(i) = remove {
            show.stage3d.viewpoints.remove(i);
        }
    });
}

/// The words for a display that shows the 3D stage from `s`.
pub(super) fn stage_source_words(s: &StageSource) -> String {
    match &s.viewpoint {
        None => "The 3D stage, all of it".to_string(),
        Some(n) => format!("The 3D stage, from {n}"),
    }
}

/// What a display may show of the stage, in its *Shows* choice on
/// Settings ▸ Displays: all of it (from in front), then each saved viewpoint.
pub(super) fn stage_choices(show: &Show) -> Vec<StageSource> {
    std::iter::once(StageSource::default())
        .chain(show.stage3d.viewpoints.iter().map(|v| StageSource {
            viewpoint: Some(v.name.clone()),
        }))
        .collect()
}

/// How wide the reason a display can't show the stage is let be.
const ITEMS_WIDTH: f32 = 280.0;

/// What the stage's items in a display's *Shows* choice need to know of the
/// show, worked out before the displays are borrowed to be edited.
pub(super) struct StageItems {
    choices: Vec<StageSource>,
    /// Why a display can't show the stage (`Show::stage_source_refused`).
    refused: Vec<(OutputId, &'static str)>,
    /// Displays whose picture is on one of the stage's screens.
    on_a_screen: Vec<OutputId>,
    /// Displays a projector of the stage throws (itself, or a part of its
    /// picture), with the projector's name.
    thrown: Vec<(OutputId, String)>,
}

impl StageItems {
    pub(super) fn of(show: &Show) -> StageItems {
        StageItems {
            choices: stage_choices(show),
            refused: show
                .outputs
                .iter()
                .filter_map(|o| Some((o.id, show.stage_source_refused(o.id)?)))
                .collect(),
            on_a_screen: show
                .stage3d
                .surfaces
                .iter()
                .filter_map(|s| s.feed.output())
                .map(|o| show.whole_picture(o))
                .collect(),
            thrown: show
                .stage3d
                .projectors
                .iter()
                .filter_map(|p| Some((show.whole_picture(p.display?), p.name.clone())))
                .collect(),
        }
    }

    pub(super) fn refused(&self, output: OutputId) -> Option<&'static str> {
        self.refused
            .iter()
            .find(|(o, _)| *o == output)
            .map(|(_, why)| *why)
    }

    /// Whether display `output`'s picture is on a screen of the stage.
    pub(super) fn on_a_screen(&self, output: OutputId) -> bool {
        self.on_a_screen.contains(&output)
    }

    /// The stage's items of display `output`'s *Shows* menu (`menu` names it
    /// for the tests): picking one writes its `source`. A display that can't
    /// show the stage has them greyed, and the reason under them. True when
    /// one was picked.
    pub(super) fn items(
        &self,
        ui: &mut Ui,
        menu: &str,
        output: OutputId,
        source: &mut Option<StageSource>,
    ) -> bool {
        let refused = self.refused(output);
        // Why not, first (the menu may be cut short), and kept narrow so
        // the menu doesn't spread across the page.
        if let Some(why) = refused {
            ui.set_max_width(ITEMS_WIDTH);
            ui.add(egui::Label::new(RichText::new(why).weak().small()).wrap());
        }
        let mut picked = false;
        for c in &self.choices {
            let words = stage_source_words(c);
            let r = ui.add_enabled(
                refused.is_none(),
                egui::Button::selectable(source.as_ref() == Some(c), &words),
            );
            note(ui, &format!("{menu} ▸ {words}"), &r);
            let r = match self.thrown.iter().find(|(o, _)| *o == output) {
                Some((_, p)) => r.on_hover_text(format!(
                    "{p}, a projector of the 3D stage, throws this display's picture: a \
                     display that shows the stage can't be a projector in it, so {p} is \
                     left without one when the show is next opened."
                )),
                None if self.on_a_screen(output) => r.on_hover_text(
                    "This display is on a screen of the 3D stage: showing the stage here \
                     leaves that screen blank (it can't show itself).",
                ),
                None => r.on_hover_text("Show the 3D stage instead of this display's cues"),
            };
            if r.clicked() && source.as_ref() != Some(c) {
                *source = Some(c.clone());
                picked = true;
            }
        }
        picked
    }
}

/// For the tests: where a control of a display's *Shows* or of the page's
/// list of displays is, by name, and whether it is in sight (not scrolled
/// out of it).
pub(super) fn note(ui: &Ui, name: &str, r: &egui::Response) {
    note_rect(ui, name, r.rect);
}

/// The same for a rectangle.
fn note_rect(ui: &Ui, name: &str, rect: egui::Rect) {
    #[cfg(test)]
    {
        let seen = ui.clip_rect().contains_rect(rect);
        ui.data_mut(|d| {
            d.get_temp_mut_or_default::<Vec<(String, egui::Rect, bool)>>(egui::Id::new(
                "stage_sources",
            ))
            .push((name.to_string(), rect, seen))
        });
    }
    #[cfg(not(test))]
    let _ = (ui, name, rect);
}

/// The displays that show the stage, each with the way to it on the
/// Displays page, where a display's *Shows* is chosen.
fn sources_section(ui: &mut Ui, show: &Show, st: &mut UiState, actions: &mut Vec<Action>) {
    let displays = super::settings::path(super::SettingsPage::Displays, None);
    untitled(ui, |ui| {
        hint(
            ui,
            &format!(
                "A display can show the 3D stage instead of its cues, on its monitor and in any \
                 stream it sends: for a previz screen in the room, or to send the view out. \
                 Choose it with the display's {} on {displays}.",
                super::SHOWS_ROW
            ),
        );
        let items = StageItems::of(show);
        let mut any = false;
        egui::Grid::new("stage_sources")
            .num_columns(3)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for o in show.outputs.iter().filter(|o| !o.is_sub_display()) {
                    let Some(s) = show.stage_source(o.id) else {
                        continue;
                    };
                    any = true;
                    ui.label(&o.name);
                    let whole = show.whole_picture(o.id);
                    if whole == o.id {
                        let words = stage_source_words(s);
                        if items.on_a_screen(o.id) {
                            ui.label(&words).on_hover_text(
                                "It is on a screen of the stage: that screen is left blank \
                                 (it can't show itself).",
                            );
                        } else {
                            ui.label(&words);
                        }
                    } else {
                        let of = show.output(whole).map_or("", |w| w.name.as_str());
                        ui.label(RichText::new(format!("Its part of {of}'s")).weak());
                    }
                    let r = ui
                        .button("Open on the Displays page")
                        .on_hover_text(format!("{displays}: {}", o.name));
                    note(ui, &format!("3D stage ▸ {} ▸ Open", o.name), &r);
                    if r.clicked() {
                        st.reveal_display.set(Some(o.id));
                        actions.push(Action::OpenSettings(super::SettingsPage::Displays));
                    }
                    ui.end_row();
                }
            });
        if !any {
            ui.horizontal(|ui| {
                ui.label(RichText::new("No display shows the stage.").weak());
                let r = ui.button(format!("Go to {displays}"));
                note(ui, "3D stage ▸ Go to Displays", &r);
                if r.clicked() {
                    actions.push(Action::OpenSettings(super::SettingsPage::Displays));
                }
            });
        }
    });
}

#[cfg(test)]
#[path = "stage_edit_tests.rs"]
mod tests;
