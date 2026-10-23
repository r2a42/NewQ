//! The control window UI: transport, cue list, inspector, active cues and outputs.

use crate::engine::{CueStatus, Engine};
use crate::gpu::{Geometry, Homography, Slot};
use crate::media::ProbeState;
use crate::remote::{Command, RemoteStatus};
use crate::render::{OutputGroup, OutputLayer, OutputPreview};
use crate::show::*;
use egui::{Align, Color32, CornerRadius, Key, Layout, RichText, Sense, Stroke, Ui, Vec2};
use std::cell::Cell;
use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::path::PathBuf;

pub struct MonitorInfo {
    pub name: String,
    pub size: (u32, u32),
    pub refresh_hz: Option<f32>,
    pub primary: bool,
}

/// Where an enabled output is currently being shown.
pub enum OutputRoute {
    MainWindow,
    /// Fullscreen on the named display.
    Display(String),
    /// On a sub display, part of the named display.
    SubDisplay {
        name: String,
        display: String,
    },
}

/// An output's current layer stack, previewed live in the main window.
pub struct PreviewOutput {
    pub id: OutputId,
    pub name: String,
    pub layers: Vec<OutputLayer>,
    /// The output's corner pin.
    pub warp: Homography,
    /// Pixel size of the output's canvas (its display or sub display).
    pub canvas: (u32, u32),
}

#[derive(Default)]
pub struct UiState {
    pub selected: Option<CueId>,
    pub show_path: Option<PathBuf>,
    pub monitors: Vec<MonitorInfo>,
    /// Every enabled output, previewed in the main window (set each frame by the app).
    pub previews: Vec<PreviewOutput>,
    pub routes: HashMap<OutputId, OutputRoute>,
    /// Output whose corner pin is being edited on its preview.
    pub geometry_edit: Option<OutputId>,
    /// Sub display whose area is being edited.
    pub area_edit: Option<SubDisplayId>,
    /// What each fullscreen window shows, by monitor index: its size and outputs.
    pub screens: HashMap<usize, ((u32, u32), Vec<OutputGroup>)>,
    /// The monitor each connected sub display is on.
    pub sub_monitors: HashMap<SubDisplayId, usize>,
    /// The corner (or whole area) last clicked in an open editor, which the
    /// arrow keys move. Set while drawing, hence the `Cell`.
    handle_sel: Cell<Option<HandleSel>>,
    pub quit_requested: bool,
    /// MIDI inputs that are connected now (set by the app every few seconds).
    pub midi_ports: Vec<String>,
    /// What the OSC and MIDI listeners are doing (set by the app).
    pub remote_status: RemoteStatus,
    pub dirty: bool,
    /// Serialized show at last save/load, for unsaved-change detection.
    saved_json: Option<String>,
    scroll_to_selected: bool,
}

impl UiState {
    fn editing(&self, q: Quad) -> bool {
        match q {
            Quad::Pin(id) => self.geometry_edit == Some(id),
            Quad::Area(id) => self.area_edit == Some(id),
        }
    }

    /// The selected handle, if its editor is still open.
    fn selected_handle(&self) -> Option<HandleSel> {
        self.handle_sel.get().filter(|s| self.editing(s.quad))
    }
}

/// A four-cornered shape with handles: an output's corner pin, or a sub
/// display's area.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Quad {
    Pin(OutputId),
    Area(SubDisplayId),
}

impl Quad {
    /// How far its corners may go, as fractions of the screen.
    fn range(self) -> RangeInclusive<f32> {
        match self {
            Quad::Pin(_) => -0.5..=1.5,
            Quad::Area(_) => 0.0..=1.0,
        }
    }
}

/// A selected corner of a shape, or with `corner: None` all of it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HandleSel {
    quad: Quad,
    corner: Option<usize>,
}

const GO_GREEN: Color32 = Color32::from_rgb(38, 160, 70);
const STOP_RED: Color32 = Color32::from_rgb(190, 50, 45);
const PLAYING: Color32 = Color32::from_rgb(40, 110, 60);
const PREWAIT: Color32 = Color32::from_rgb(150, 110, 30);
const PLAYHEAD: Color32 = Color32::from_rgb(230, 190, 60);
const ROW_H: f32 = 26.0;
/// How far a group's children are indented in the cue list.
const CHILD_INDENT: f32 = 14.0;
/// Width of the fold arrow in front of a group's name.
const FOLD_W: f32 = 18.0;

enum Action {
    Go,
    Panic,
    TogglePause,
    Select(CueId),
    SelectRelative(i32),
    AddCue(CueType),
    AddFiles(Vec<PathBuf>),
    Delete,
    Duplicate,
    Move(i32),
    New,
    Open,
    Save,
    SaveAs,
    FireSelected,
    StopSelected,
    FadeOutSelected,
    StopInstance(u64, f64),
    FadeOutInstance(u64),
    EditGeometry(Option<OutputId>),
    EditArea(Option<SubDisplayId>),
    /// Move the selected handle by this many screen pixels.
    Nudge(Vec2),
    AddSubDisplay(DisplayRef),
    RemoveSubDisplay(SubDisplayId),
    SetTransition(CueId, Transition),
    /// Collapse or expand a group in the cue list.
    ToggleGroup(CueId),
    /// Wrap a cue in a new group.
    GroupCue(CueId),
    /// Dissolve a group; its children stay.
    Ungroup(CueId),
    /// Move a cue into the end of a group (cue, group).
    MoveIntoGroup(CueId, CueId),
    MoveOutOfGroup(CueId),
    AddOutput,
    RemoveOutput(OutputId),
    Quit,
}

pub fn setup_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.button_padding = Vec2::new(10.0, 4.0);
    });
}

pub fn fmt_time(secs: f64) -> String {
    if !secs.is_finite() {
        return "∞".into();
    }
    let s = secs.max(0.0);
    let m = (s / 60.0).floor();
    format!("{}:{:04.1}", m as i64, s - m * 60.0)
}

fn cue_label(c: &Cue) -> String {
    format!("{} · {}", c.number, c.name)
}

pub fn draw(ui: &mut Ui, show: &mut Show, engine: &mut Engine, st: &mut UiState) {
    if st.saved_json.is_none() {
        st.saved_json = serde_json::to_string(show).ok();
    }
    let mut actions: Vec<Action> = Vec::new();
    keyboard(ui.ctx(), st, &mut actions);

    let dropped: Vec<PathBuf> = ui.ctx().input(|i| {
        i.raw
            .dropped_files
            .iter()
            .map(|f| f.path().to_path_buf())
            .filter(|p| !p.as_os_str().is_empty())
            .collect()
    });
    if !dropped.is_empty() {
        actions.push(Action::AddFiles(dropped));
    }

    egui::Panel::top("menu").show(ui, |ui| menu_bar(ui, show, st, &mut actions));
    egui::Panel::top("transport")
        .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(10))
        .show(ui, |ui| transport(ui, show, engine, &mut actions));
    egui::Panel::bottom("status").show(ui, |ui| status_bar(ui, engine, st));
    egui::Panel::right("side")
        .resizable(true)
        .default_size(420.0)
        .min_size(280.0)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    output_previews(ui, show, st);
                    active_panel(ui, show, engine, &mut actions);
                    ui.add_space(12.0);
                    outputs_panel(ui, show, st, &mut actions);
                    ui.add_space(12.0);
                    sub_displays_panel(ui, show, st, &mut actions);
                    ui.add_space(12.0);
                    remote_panel(ui, show, st);
                });
        });
    egui::Panel::bottom("inspector")
        .resizable(true)
        .default_size(300.0)
        .min_size(120.0)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| inspector(ui, show, engine, st, &mut actions));
        });
    egui::CentralPanel::default().show(ui, |ui| cue_list(ui, show, engine, st, &mut actions));

    // Actions change state after this frame was laid out; show the result now
    // rather than on the next mouse move.
    if !actions.is_empty() {
        ui.ctx().request_repaint();
    }
    for a in actions {
        apply(a, show, engine, st);
    }

    let json = serde_json::to_string(show).ok();
    st.dirty = json != st.saved_json;
}

fn keyboard(ctx: &egui::Context, st: &UiState, actions: &mut Vec<Action>) {
    let typing = ctx.egui_wants_keyboard_input();
    let nudging = st.selected_handle().is_some();
    ctx.input_mut(|i| {
        if i.consume_key(egui::Modifiers::NONE, Key::Escape) {
            actions.push(Action::Panic);
        }
        // Before Ctrl+S, which also matches with Shift held.
        if i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::S) {
            actions.push(Action::SaveAs);
        }
        if i.consume_key(egui::Modifiers::COMMAND, Key::S) {
            actions.push(Action::Save);
        }
        if i.consume_key(egui::Modifiers::COMMAND, Key::O) {
            actions.push(Action::Open);
        }
        if i.consume_key(egui::Modifiers::COMMAND, Key::N) {
            actions.push(Action::New);
        }
        if typing {
            return;
        }
        if nudging {
            // The arrow keys move the selected corner: 1 px, or 5 px with Shift.
            let mut d = Vec2::ZERO;
            i.events.retain(|e| {
                let egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } = e
                else {
                    return true;
                };
                if modifiers.command || modifiers.alt {
                    return true;
                }
                let n = if modifiers.shift { 5.0 } else { 1.0 };
                d += match key {
                    Key::ArrowLeft => Vec2::new(-n, 0.0),
                    Key::ArrowRight => Vec2::new(n, 0.0),
                    Key::ArrowUp => Vec2::new(0.0, -n),
                    Key::ArrowDown => Vec2::new(0.0, n),
                    _ => return true,
                };
                false
            });
            if d != Vec2::ZERO {
                actions.push(Action::Nudge(d));
            }
        }
        if i.consume_key(egui::Modifiers::NONE, Key::Space) {
            actions.push(Action::Go);
        }
        if i.consume_key(egui::Modifiers::NONE, Key::P) {
            actions.push(Action::TogglePause);
        }
        if i.consume_key(egui::Modifiers::COMMAND, Key::D) {
            actions.push(Action::Duplicate);
        }
        if i.consume_key(egui::Modifiers::COMMAND, Key::ArrowUp) {
            actions.push(Action::Move(-1));
        }
        if i.consume_key(egui::Modifiers::COMMAND, Key::ArrowDown) {
            actions.push(Action::Move(1));
        }
        if i.consume_key(egui::Modifiers::NONE, Key::ArrowUp) {
            actions.push(Action::SelectRelative(-1));
        }
        if i.consume_key(egui::Modifiers::NONE, Key::ArrowDown) {
            actions.push(Action::SelectRelative(1));
        }
        if i.consume_key(egui::Modifiers::NONE, Key::Delete)
            || i.consume_key(egui::Modifiers::COMMAND, Key::Backspace)
        {
            actions.push(Action::Delete);
        }
    });
}

fn menu_bar(ui: &mut Ui, show: &Show, st: &UiState, actions: &mut Vec<Action>) {
    egui::containers::menu::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            if ui
                .add(egui::Button::new("New show").shortcut_text("Ctrl+N"))
                .clicked()
            {
                actions.push(Action::New);
            }
            if ui
                .add(egui::Button::new("Open…").shortcut_text("Ctrl+O"))
                .clicked()
            {
                actions.push(Action::Open);
            }
            if ui
                .add(egui::Button::new("Save").shortcut_text("Ctrl+S"))
                .clicked()
            {
                actions.push(Action::Save);
            }
            if ui
                .add(egui::Button::new("Save as…").shortcut_text("Ctrl+Shift+S"))
                .clicked()
            {
                actions.push(Action::SaveAs);
            }
            ui.separator();
            if ui.button("Quit").clicked() {
                actions.push(Action::Quit);
            }
        });
        ui.menu_button("Cue", |ui| {
            for ty in [CueType::Video, CueType::Image, CueType::Audio] {
                if ui
                    .button(format!("{} Add {} cue…", ty.icon(), ty.label()))
                    .clicked()
                {
                    actions.push(Action::AddCue(ty));
                }
            }
            ui.separator();
            for ty in [
                CueType::Text,
                CueType::Fade,
                CueType::Stop,
                CueType::Wait,
                CueType::Group,
            ] {
                if ui
                    .button(format!("{} Add {} cue", ty.icon(), ty.label()))
                    .clicked()
                {
                    actions.push(Action::AddCue(ty));
                }
            }
            if let Some(id) = st.selected
                && let Some(c) = show.cue(id)
            {
                match (&c.kind, c.parent) {
                    (CueKind::Group(_), _) => {
                        if ui.button("Ungroup").clicked() {
                            actions.push(Action::Ungroup(id));
                        }
                    }
                    (_, None) => {
                        if ui.button("Wrap in group").clicked() {
                            actions.push(Action::GroupCue(id));
                        }
                    }
                    (_, Some(_)) => {
                        if ui.button("Move out of group").clicked() {
                            actions.push(Action::MoveOutOfGroup(id));
                        }
                    }
                }
            }
            ui.separator();
            if ui
                .add(egui::Button::new("Duplicate").shortcut_text("Ctrl+D"))
                .clicked()
            {
                actions.push(Action::Duplicate);
            }
            if ui
                .add(egui::Button::new("Move up").shortcut_text("Ctrl+↑"))
                .clicked()
            {
                actions.push(Action::Move(-1));
            }
            if ui
                .add(egui::Button::new("Move down").shortcut_text("Ctrl+↓"))
                .clicked()
            {
                actions.push(Action::Move(1));
            }
            if ui
                .add(egui::Button::new("Delete").shortcut_text("Del"))
                .clicked()
            {
                actions.push(Action::Delete);
            }
        });
        ui.menu_button("Outputs", |ui| {
            if ui.button("Add output").clicked() {
                actions.push(Action::AddOutput);
            }
            ui.menu_button("Add sub display on", |ui| {
                add_sub_display_menu(ui, st, actions);
            });
        });
        ui.menu_button("Help", |ui| {
            ui.label(RichText::new("Keyboard").strong());
            for (k, d) in [
                ("Space", "GO"),
                ("Esc", "Fade out all (press twice: stop now)"),
                ("P", "Pause / resume"),
                ("↑ / ↓", "Move selection and playhead"),
                ("Ctrl+↑ / Ctrl+↓", "Reorder cue"),
                ("Ctrl+D", "Duplicate cue"),
                ("Del", "Delete cue"),
            ] {
                ui.horizontal(|ui| {
                    ui.monospace(format!("{k:<16}"));
                    ui.label(d);
                });
            }
            ui.separator();
            ui.label("Drag media files onto the window to add cues.");
            ui.label(format!("{} cues", show.cues.len()));
        });
    });
}

fn transport(ui: &mut Ui, show: &Show, engine: &Engine, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        let go = egui::Button::new(
            RichText::new("GO")
                .size(34.0)
                .strong()
                .color(Color32::WHITE),
        )
        .fill(GO_GREEN)
        .corner_radius(CornerRadius::same(8))
        .min_size(Vec2::new(150.0, 76.0));
        if ui
            .add(go)
            .on_hover_text("Fire the cue at the playhead (Space)")
            .clicked()
        {
            actions.push(Action::Go);
        }
        ui.add_space(8.0);
        ui.vertical(|ui| {
            let next = engine.playhead.and_then(|id| show.cue(id));
            ui.label(RichText::new("STANDING BY").small().weak());
            match next {
                Some(c) => {
                    ui.label(
                        RichText::new(format!("{}  {}", c.number, c.name))
                            .size(24.0)
                            .strong()
                            .color(PLAYHEAD),
                    );
                    if !c.notes.trim().is_empty() {
                        ui.label(RichText::new(c.notes.trim()).italics());
                    }
                }
                None => {
                    ui.label(RichText::new("End of cue list").size(24.0).weak());
                }
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let stop = egui::Button::new(
                RichText::new("STOP ALL")
                    .size(18.0)
                    .strong()
                    .color(Color32::WHITE),
            )
            .fill(STOP_RED)
            .min_size(Vec2::new(120.0, 50.0));
            if ui
                .add(stop)
                .on_hover_text("Fade out everything (Esc). Press twice to stop instantly.")
                .clicked()
            {
                actions.push(Action::Panic);
            }
            let label = if engine.paused {
                "▶ Resume"
            } else {
                "⏸ Pause"
            };
            if ui
                .add(
                    egui::Button::new(RichText::new(label).size(16.0))
                        .min_size(Vec2::new(110.0, 50.0)),
                )
                .on_hover_text("Pause / resume everything (P)")
                .clicked()
            {
                actions.push(Action::TogglePause);
            }
        });
    });
}

fn status_bar(ui: &mut Ui, engine: &Engine, st: &UiState) {
    ui.horizontal(|ui| {
        match &engine.ff {
            Some(_) => ui.label(RichText::new("✔ ffmpeg").color(GO_GREEN)),
            None => ui
                .label(RichText::new("⚠ ffmpeg not found").color(STOP_RED))
                .on_hover_text(
                    "Put ffmpeg and ffprobe next to NewQ, in a folder named ffmpeg/bin beside it, \
                     on your PATH, or set NEWQ_FFMPEG_DIR.",
                ),
        };
        match &engine.audio.error {
            None => ui.label(format!(
                "🔊 {} @ {} Hz",
                engine.audio.device_name, engine.audio.sample_rate
            )),
            Some(e) => ui.label(RichText::new(format!("🔇 {e}")).color(STOP_RED)),
        };
        ui.separator();
        let path = st
            .show_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unsaved show".into());
        ui.label(format!("{}{}", path, if st.dirty { " •" } else { "" }));
        ui.separator();
        if let Some((t, msg)) = engine.messages.back()
            && t.elapsed().as_secs() < 30
        {
            ui.label(RichText::new(msg).color(PLAYHEAD));
        }
    });
}

fn cue_target_text(show: &Show, c: &Cue) -> String {
    match &c.kind {
        CueKind::Video(m) | CueKind::Image(m) => {
            let file = m
                .file
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| "(no file)".into());
            let out = m
                .output
                .and_then(|o| show.output(o))
                .map(|o| o.name.as_str())
                .unwrap_or("no output");
            format!("{file}  »  {out}")
        }
        CueKind::Audio(m) => m
            .file
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| "(no file)".into()),
        CueKind::Fade(f) => f
            .target
            .and_then(|t| show.cue(t))
            .map(|t| format!("» {}", cue_label(t)))
            .unwrap_or_else(|| "(no target)".into()),
        CueKind::Stop(s) => s
            .target
            .and_then(|t| show.cue(t))
            .map(|t| format!("» {}", cue_label(t)))
            .unwrap_or_else(|| "» everything".into()),
        CueKind::Wait(_) => String::new(),
        CueKind::Text(t) => {
            let first = t.text.lines().next().unwrap_or("").trim();
            let more = if t.text.lines().nth(1).is_some() {
                " …"
            } else {
                ""
            };
            let out = t
                .media
                .output
                .and_then(|o| show.output(o))
                .map(|o| o.name.as_str())
                .unwrap_or("no output");
            format!("“{first}{more}”  »  {out}")
        }
        CueKind::Group(g) => {
            let n = show.children(c.id).len();
            format!(
                "{n} cue{} · {}",
                if n == 1 { "" } else { "s" },
                g.mode.label().to_lowercase()
            )
        }
    }
}

fn cue_list(
    ui: &mut Ui,
    show: &Show,
    engine: &mut Engine,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) {
    if show.cues.is_empty() {
        ui.centered_and_justified(|ui| {
            ui.label(
                RichText::new(
                    "No cues yet.\n\nDrag video, image or audio files here,\nor use the Cue menu.",
                )
                .size(18.0)
                .weak(),
            );
        });
        return;
    }
    // Column layout: (title, width); a width of 0 takes the remaining space.
    let cols: [(&str, f32); 9] = [
        ("", 26.0),
        ("#", 52.0),
        ("", 30.0),
        ("Name", 0.0),
        ("Target", 200.0),
        ("Fade in / out", 175.0),
        ("Pre-wait", 70.0),
        ("Duration", 80.0),
        ("Cont.", 50.0),
    ];
    let total = ui.available_width();
    let fixed: f32 = cols.iter().map(|c| c.1).sum();
    let flex = (total - fixed).max(120.0);
    let col_x = |i: usize, left: f32| {
        let mut x = left;
        for c in &cols[..i] {
            x += if c.1 == 0.0 { flex } else { c.1 };
        }
        x
    };

    let (header, _) = ui.allocate_exact_size(Vec2::new(total, 20.0), Sense::hover());
    let painter = ui.painter();
    for (i, (title, _)) in cols.iter().enumerate() {
        painter.text(
            egui::pos2(col_x(i, header.left()) + 4.0, header.center().y),
            egui::Align2::LEFT_CENTER,
            *title,
            egui::FontId::proportional(12.0),
            ui.visuals().weak_text_color(),
        );
    }
    ui.separator();

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            let groups: Vec<(CueId, String)> = show
                .cues
                .iter()
                .filter(|c| matches!(c.kind, CueKind::Group(_)))
                .map(|c| (c.id, cue_label(c)))
                .collect();
            for (idx, c) in show.cues.iter().enumerate() {
                if !show.is_visible(idx) {
                    continue;
                }
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(total, ROW_H), Sense::click());
                // The fold arrow of a group sits at the start of its name.
                let indent = if c.parent.is_some() {
                    CHILD_INDENT
                } else {
                    0.0
                };
                let fold = match &c.kind {
                    CueKind::Group(g) => {
                        let x = col_x(3, rect.left()) + 2.0;
                        let arrow = egui::Rect::from_min_size(
                            egui::pos2(x, rect.top()),
                            Vec2::new(FOLD_W, ROW_H),
                        );
                        let r = ui.interact(arrow, resp.id.with("fold"), Sense::click());
                        if r.clicked() {
                            actions.push(Action::ToggleGroup(c.id));
                        }
                        Some(g.collapsed)
                    }
                    _ => None,
                };
                if resp.clicked() {
                    actions.push(Action::Select(c.id));
                }
                if resp.double_clicked() {
                    actions.push(Action::FireSelected);
                }
                resp.context_menu(|ui| cue_context_menu(ui, c, &groups, actions));
                if st.scroll_to_selected && st.selected == Some(c.id) {
                    ui.scroll_to_rect(rect, None);
                    st.scroll_to_selected = false;
                }
                let status = engine.status(c.id);
                let selected = st.selected == Some(c.id);
                let at_playhead = engine.playhead == Some(c.id);
                let painter = ui.painter_at(rect);
                let v = ui.visuals();
                let bg = if selected {
                    v.selection.bg_fill
                } else if resp.hovered() {
                    v.widgets.hovered.weak_bg_fill
                } else {
                    v.faint_bg_color
                };
                painter.rect_filled(rect, 2.0, bg);
                // Progress fill for running cues.
                let (frac, color) = match status {
                    CueStatus::Playing { elapsed, duration } => (
                        duration
                            .map(|d| (elapsed / d.max(1e-6)).clamp(0.0, 1.0))
                            .unwrap_or(1.0),
                        PLAYING,
                    ),
                    CueStatus::PreWait { remaining, total } => {
                        (1.0 - remaining / total.max(1e-6), PREWAIT)
                    }
                    CueStatus::Idle => (0.0, Color32::TRANSPARENT),
                };
                if frac > 0.0 {
                    let mut r = rect;
                    r.set_width(rect.width() * frac as f32);
                    painter.rect_filled(r, 2.0, color.gamma_multiply(0.55));
                }
                if at_playhead {
                    painter.rect_stroke(
                        rect.shrink(1.0),
                        2.0,
                        Stroke::new(2.0, PLAYHEAD),
                        egui::StrokeKind::Inside,
                    );
                }
                let text_color = v.strong_text_color();
                let font = egui::FontId::proportional(14.0);
                let y = rect.center().y;
                let marker = match status {
                    CueStatus::Playing { .. } => "▶",
                    CueStatus::PreWait { .. } => "…",
                    CueStatus::Idle if at_playhead => "➡",
                    CueStatus::Idle => "",
                };
                let cell_dx = |i: usize, dx: f32, text: String, color: Color32| {
                    let x0 = col_x(i, rect.left());
                    let w = if cols[i].1 == 0.0 { flex } else { cols[i].1 };
                    let clip = egui::Rect::from_min_max(
                        egui::pos2(x0, rect.top()),
                        egui::pos2(x0 + w - 4.0, rect.bottom()),
                    );
                    painter.with_clip_rect(clip).text(
                        egui::pos2(x0 + 4.0 + dx, y),
                        egui::Align2::LEFT_CENTER,
                        text,
                        font.clone(),
                        color,
                    );
                };
                let cell = |i: usize, text: String, color: Color32| cell_dx(i, 0.0, text, color);
                cell(0, marker.into(), PLAYHEAD);
                if marker.is_empty() && engine.is_prepared(c.id) {
                    painter.circle_filled(egui::pos2(rect.left() + 12.0, y), 3.5, GO_GREEN);
                }
                cell_dx(1, indent, c.number.clone(), text_color);
                cell_dx(2, indent, c.kind.cue_type().icon().into(), text_color);
                if let Some(collapsed) = fold {
                    // Drawn, not typed: the default font has no triangle glyphs.
                    let mid = egui::pos2(col_x(3, rect.left()) + 2.0 + FOLD_W / 2.0, y);
                    let tri = if collapsed {
                        [(-3.0, -5.0), (-3.0, 5.0), (4.0, 0.0)]
                    } else {
                        [(-5.0, -3.0), (5.0, -3.0), (0.0, 4.0)]
                    };
                    painter.add(egui::Shape::convex_polygon(
                        tri.iter()
                            .map(|(dx, dy)| mid + Vec2::new(*dx, *dy))
                            .collect(),
                        text_color,
                        Stroke::NONE,
                    ));
                    cell_dx(3, FOLD_W, c.name.clone(), text_color);
                } else {
                    cell(3, c.name.clone(), text_color);
                }
                let weak = v.weak_text_color();
                let missing = c
                    .kind
                    .media()
                    .is_some_and(|m| !m.file.as_os_str().is_empty() && !m.file.is_file());
                cell(
                    4,
                    if missing {
                        format!("⚠ missing: {}", cue_target_text(show, c))
                    } else {
                        cue_target_text(show, c)
                    },
                    if missing { STOP_RED } else { weak },
                );
                cell(5, fades_text(c), weak);
                cell(
                    6,
                    if c.pre_wait > 0.0 {
                        fmt_time(c.pre_wait)
                    } else {
                        String::new()
                    },
                    weak,
                );
                let dur = match status {
                    CueStatus::Playing { elapsed, duration } => match duration {
                        Some(d) => format!("-{}", fmt_time(d - elapsed)),
                        None => fmt_time(elapsed),
                    },
                    _ => match engine.cue_duration(show, c) {
                        Some(Some(d)) if d > 0.0 => fmt_time(d),
                        Some(None) => "∞".into(),
                        _ => String::new(),
                    },
                };
                cell(7, dur, weak);
                cell(
                    8,
                    match c.continue_mode {
                        _ if c.parent.is_some() => "",
                        ContinueMode::DoNotContinue => "",
                        ContinueMode::AutoContinue => "⏩ auto",
                        ContinueMode::AutoFollow => "⏭ follow",
                    }
                    .into(),
                    PLAYHEAD,
                );
            }
        });
}

fn secs_short(t: f64) -> String {
    format!("{t:.1}s")
}

/// The cue list's "Fade in / out" column: a cue's built-in transition and fade-out.
fn fades_text(c: &Cue) -> String {
    let Some(m) = c.kind.media() else {
        return String::new();
    };
    let visual = c.kind.cue_type() != CueType::Audio;
    let entry = match (visual, m.transition) {
        (true, Transition::Cut) => "Cut".to_string(),
        (true, Transition::FadeIn) | (false, _) if m.fade_in <= 0.0 => String::new(),
        (true, t) => format!("{} {}", t.short_label(), secs_short(m.fade_in)),
        (false, _) => format!("Fade in {}", secs_short(m.fade_in)),
    };
    match (entry.is_empty(), m.fade_out > 0.0) {
        (_, false) => entry,
        (true, true) => format!("out {}", secs_short(m.fade_out)),
        (false, true) => format!("{entry} · out {}", secs_short(m.fade_out)),
    }
}

fn cue_context_menu(ui: &mut Ui, c: &Cue, groups: &[(CueId, String)], actions: &mut Vec<Action>) {
    actions.push(Action::Select(c.id));
    if ui.button("▶ Fire").clicked() {
        actions.push(Action::FireSelected);
        ui.close();
    }
    if ui.button("Fade out").clicked() {
        actions.push(Action::FadeOutSelected);
        ui.close();
    }
    if ui.button("⏹ Stop").clicked() {
        actions.push(Action::StopSelected);
        ui.close();
    }
    if let Some(m) = c
        .kind
        .media()
        .filter(|_| c.kind.cue_type() != CueType::Audio)
    {
        ui.separator();
        ui.label(RichText::new("Transition").weak());
        for t in Transition::ALL {
            if ui.radio(m.transition == t, t.label()).clicked() {
                actions.push(Action::SetTransition(c.id, t));
                ui.close();
            }
        }
    }
    ui.separator();
    match (&c.kind, c.parent) {
        (CueKind::Group(_), _) => {
            if ui.button("Ungroup").clicked() {
                actions.push(Action::Ungroup(c.id));
                ui.close();
            }
        }
        (_, Some(_)) => {
            if ui.button("Move out of group").clicked() {
                actions.push(Action::MoveOutOfGroup(c.id));
                ui.close();
            }
        }
        (_, None) => {
            if ui.button("Wrap in group").clicked() {
                actions.push(Action::GroupCue(c.id));
                ui.close();
            }
            if !groups.is_empty() {
                ui.menu_button("Move into group", |ui| {
                    for (gid, label) in groups {
                        if ui.button(label).clicked() {
                            actions.push(Action::MoveIntoGroup(c.id, *gid));
                            ui.close();
                        }
                    }
                });
            }
        }
    }
    ui.separator();
    if ui.button("Duplicate").clicked() {
        actions.push(Action::Duplicate);
        ui.close();
    }
    if ui.button("Delete").clicked() {
        actions.push(Action::Delete);
        ui.close();
    }
}

fn active_panel(ui: &mut Ui, show: &Show, engine: &Engine, actions: &mut Vec<Action>) {
    ui.heading("Active");
    if !engine.is_active() {
        ui.label(RichText::new("Nothing playing").weak());
    }
    for p in &engine.pending {
        let name = show.cue(p.cue_id).map(cue_label).unwrap_or_default();
        ui.label(format!("{name}: pre-wait {}", fmt_time(p.remaining)));
    }
    for a in engine.active.iter().rev() {
        let mut name = show
            .cue(a.cue_id)
            .map(cue_label)
            .unwrap_or_else(|| "(deleted cue)".into());
        if a.is_matte() {
            let t = show
                .cue(a.cue_id)
                .and_then(|c| c.kind.media())
                .map_or("dip", |m| m.transition.label());
            name = format!("{name}  ({})", t.to_lowercase());
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .small_button("Stop")
                    .on_hover_text("Stop immediately")
                    .clicked()
                {
                    actions.push(Action::StopInstance(a.instance, 0.0));
                }
                if ui
                    .small_button("Fade out")
                    .on_hover_text("Fade out with the cue's own fade-out time, then stop")
                    .clicked()
                {
                    actions.push(Action::FadeOutInstance(a.instance));
                }
            });
        });
        let (frac, text) = match a.duration {
            Some(d) => (
                (a.elapsed / d.max(1e-6)).clamp(0.0, 1.0) as f32,
                format!(
                    "{} / {}  (-{})",
                    fmt_time(a.elapsed),
                    fmt_time(d),
                    fmt_time(d - a.elapsed)
                ),
            ),
            None => (1.0, format!("{} / ∞", fmt_time(a.elapsed))),
        };
        let mut text = text;
        if !a.started {
            text = "loading…".into();
        } else if a.held {
            text.push_str("  holding");
        } else if a.stopping {
            text.push_str("  fading out");
        }
        ui.add(egui::ProgressBar::new(frac).text(text).fill(if a.stopping {
            PREWAIT
        } else {
            PLAYING
        }));
        ui.add_space(4.0);
    }
}

/// A live preview of every output. For an output with no display of its own,
/// this is where it is shown.
fn output_previews(ui: &mut Ui, show: &mut Show, st: &UiState) {
    for e in &st.previews {
        let where_ = match st.routes.get(&e.id) {
            Some(OutputRoute::Display(name)) => format!("also fullscreen on {name}"),
            Some(OutputRoute::SubDisplay { name, display }) => {
                format!("also on {name} ({display})")
            }
            _ => "main window only".to_string(),
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(&e.name).strong());
            ui.label(RichText::new(where_).weak().small());
        });
        let editing = st.geometry_edit == Some(e.id);
        // Leave room around the screen to drag corners past its edges.
        let margin = if editing { 14.0 } else { 0.0 };
        let full_w = ui.available_width() - 2.0 * margin;
        // Tall sub displays are previewed narrower, not taller.
        let aspect = (e.canvas.0 as f32 / e.canvas.1.max(1) as f32).max(0.1);
        let h = (full_w / aspect).min(full_w * if editing { 1.2 } else { 0.75 });
        let w = h * aspect;
        let (outer, _) = ui.allocate_exact_size(
            Vec2::new(w + 2.0 * margin, h + 2.0 * margin),
            Sense::hover(),
        );
        let rect = outer.shrink(margin);
        ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            OutputPreview {
                groups: vec![OutputGroup {
                    layers: e.layers.clone(),
                    geometry: Geometry {
                        canvas: e.canvas,
                        warp: e.warp,
                        pin: e.warp,
                    },
                }],
                slot: Slot::Preview,
            },
        ));
        if editing && let Some(cfg) = show.outputs.iter_mut().find(|o| o.id == e.id) {
            corner_handles(ui, rect, e.id, &mut cfg.corners, &st.handle_sel);
        }
        ui.add_space(8.0);
    }
}

/// Draggable corner-pin handles over an output preview. Hold Shift for fine moves.
fn corner_handles(
    ui: &mut Ui,
    screen: egui::Rect,
    id: OutputId,
    corners: &mut Corners,
    sel: &Cell<Option<HandleSel>>,
) {
    quad_handles(ui, screen, Quad::Pin(id), corners, sel);
}

/// Draggable handles for a sub display's area over a picture of its
/// display: corners reshape it, dragging inside moves it.
fn area_handles(
    ui: &mut Ui,
    screen: egui::Rect,
    id: SubDisplayId,
    area: &mut Corners,
    sel: &Cell<Option<HandleSel>>,
) {
    quad_handles(ui, screen, Quad::Area(id), area, sel);
}

/// Draggable handles on the four corners of a shape drawn over `screen`,
/// kept within its range. An area can also be dragged from inside. Hold Shift
/// for fine moves. Clicking a handle selects it for the arrow keys; clicking
/// anywhere else deselects it.
fn quad_handles(
    ui: &mut Ui,
    screen: egui::Rect,
    quad: Quad,
    corners: &mut Corners,
    sel: &Cell<Option<HandleSel>>,
) {
    let to_screen =
        |c: [f32; 2]| screen.min + Vec2::new(c[0] * screen.width(), c[1] * screen.height());
    let painter = ui.painter().clone();
    painter.rect_stroke(
        screen,
        0.0,
        Stroke::new(1.0, Color32::from_gray(90)),
        egui::StrokeKind::Outside,
    );
    let fine = ui.input(|i| i.modifiers.shift);
    let scale = if fine { 0.1 } else { 1.0 };
    let range = quad.range();
    let (lo, hi) = (*range.start(), *range.end());
    let mut pressed_on = None;
    // Registered before the corners, so the corner handles stay on top.
    if let Quad::Area(_) = quad {
        let [x, y, w, h] = corners.bounds();
        let body = egui::Rect::from_min_size(
            to_screen([x, y]),
            Vec2::new(w * screen.width(), h * screen.height()),
        );
        let resp = ui
            .interact(body, ui.id().with((quad, "body")), Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::Move)
            .on_hover_text(
                "Drag to move the area (Shift = fine). Click, then the arrow keys move it                  1 px (Shift = 5 px).",
            );
        if resp.is_pointer_button_down_on() {
            pressed_on = Some(None);
        }
        if resp.dragged() || resp.drag_stopped() {
            let d = drag_moved(ui, &resp) * scale;
            let dx = (d.x / screen.width()).clamp(lo - x, hi - (x + w));
            let dy = (d.y / screen.height()).clamp(lo - y, hi - (y + h));
            for c in &mut corners.0 {
                c[0] += dx;
                c[1] += dy;
            }
        }
    }
    let mut hits = [None; 4];
    for (i, hit) in hits.iter_mut().enumerate() {
        let pos = to_screen(corners.0[i]);
        let rect = egui::Rect::from_center_size(pos, Vec2::splat(22.0));
        let resp = ui
            .interact(rect, ui.id().with((quad, i)), Sense::click_and_drag())
            .on_hover_text(format!(
                "{}: drag to move (Shift = fine). Click, then the arrow keys move it 1 px                  (Shift = 5 px).",
                Corners::LABELS[i]
            ));
        if resp.is_pointer_button_down_on() {
            pressed_on = Some(Some(i));
        }
        if resp.dragged() || resp.drag_stopped() {
            let d = drag_moved(ui, &resp) * scale;
            let c = &mut corners.0[i];
            c[0] = (c[0] + d.x / screen.width()).clamp(lo, hi);
            c[1] = (c[1] + d.y / screen.height()).clamp(lo, hi);
        }
        *hit = Some(resp.hovered() || resp.dragged());
    }
    // Select what was pressed on; a press anywhere else deselects.
    if let Some(corner) = pressed_on {
        sel.set(Some(HandleSel { quad, corner }));
    } else if ui.input(|i| i.pointer.primary_pressed()) && sel.get().is_some_and(|s| s.quad == quad)
    {
        sel.set(None);
    }
    let selected = sel.get().filter(|s| s.quad == quad).map(|s| s.corner);

    let quad_color = if corners.is_convex() {
        PLAYHEAD
    } else {
        STOP_RED
    };
    let width = if selected == Some(None) { 3.0 } else { 1.5 };
    for i in 0..4 {
        painter.line_segment(
            [to_screen(corners.0[i]), to_screen(corners.0[(i + 1) % 4])],
            Stroke::new(width, quad_color),
        );
    }
    for (i, hit) in hits.iter().enumerate() {
        let pos = to_screen(corners.0[i]);
        let active = hit.unwrap_or(false);
        if selected == Some(Some(i)) {
            painter.circle(pos, 9.0, PLAYHEAD, Stroke::new(2.0, Color32::WHITE));
        } else {
            painter.circle(
                pos,
                if active { 8.0 } else { 6.0 },
                if active {
                    PLAYHEAD
                } else {
                    Color32::from_black_alpha(160)
                },
                Stroke::new(2.0, PLAYHEAD),
            );
        }
    }
}

/// How far a handle was dragged this frame. That includes the movement
/// before egui counted it as a drag, and on the frame it is let go, so the
/// handle ends up where the pointer did.
fn drag_moved(ui: &Ui, resp: &egui::Response) -> Vec2 {
    if resp.drag_stopped() {
        return ui.input(|i| i.pointer.delta());
    }
    if !resp.drag_started() {
        return resp.drag_delta();
    }
    ui.input(
        |i| match (i.pointer.press_origin(), i.pointer.interact_pos()) {
            (Some(from), Some(to)) => to - from,
            _ => resp.drag_delta(),
        },
    )
}

/// Move one corner (or with `None`, the whole shape) by `d` pixels of a
/// `size`-pixel screen, stepping from the nearest whole pixel, within `range`.
fn nudge(
    corners: &mut Corners,
    corner: Option<usize>,
    d: Vec2,
    size: (u32, u32),
    range: RangeInclusive<f32>,
) {
    let size = [size.0.max(1) as f32, size.1.max(1) as f32];
    let (lo, hi) = (*range.start(), *range.end());
    let d = [d.x, d.y];
    let step = |v: f32, axis: usize| ((v * size[axis]).round() + d[axis]) / size[axis];
    for axis in (0..2).filter(|&a| d[a] != 0.0) {
        match corner {
            Some(i) => {
                let c = &mut corners.0[i][axis];
                *c = step(*c, axis).clamp(lo, hi);
            }
            None => {
                let b = corners.bounds();
                let (min, len) = (b[axis], b[axis + 2]);
                let delta = (step(min, axis) - min).clamp(lo - min, hi - (min + len));
                for c in &mut corners.0 {
                    c[axis] += delta;
                }
            }
        }
    }
}

/// Pixel size of a sub display's display, assuming 1920×1080 while it isn't
/// connected.
fn sub_display_size(st: &UiState, id: SubDisplayId) -> (u32, u32) {
    st.sub_monitors
        .get(&id)
        .and_then(|m| st.monitors.get(*m))
        .map_or((1920, 1080), |m| m.size)
}

fn monitor_label(st: &UiState, i: usize) -> String {
    let m = &st.monitors[i];
    format!(
        "{}: {} ({}×{}{}){}",
        i + 1,
        m.name,
        m.size.0,
        m.size.1,
        m.refresh_hz
            .map(|r| format!(" @ {r:.0} Hz"))
            .unwrap_or_default(),
        if m.primary { " primary" } else { "" }
    )
}

/// Settings for controlling NewQ from OSC and MIDI.
fn remote_panel(ui: &mut Ui, show: &mut Show, st: &UiState) {
    ui.heading("Remote control");
    ui.label(
        RichText::new("Run NewQ from a lighting desk, Companion, a phone or a MIDI controller.")
            .weak()
            .small(),
    );
    let status = &st.remote_status;
    let r = &mut show.remote;
    ui.checkbox(&mut r.osc, "OSC over UDP");
    if r.osc {
        ui.horizontal(|ui| {
            ui.label("Port");
            ui.add(
                egui::DragValue::new(&mut r.osc_port)
                    .range(1..=65535)
                    .speed(1.0),
            );
        });
        match (&status.osc_port, &status.osc_error) {
            (_, Some(e)) => {
                ui.label(RichText::new(format!("⚠ {e}")).color(STOP_RED));
            }
            (Some(p), None) => {
                ui.label(RichText::new(format!("Listening on UDP port {p}")).color(GO_GREEN));
            }
            _ => {
                ui.label(RichText::new("Starting…").weak());
            }
        }
        ui.label(
            RichText::new(
                "/go  /panic  /stop  /pause  /resume\n/playhead/next  /playhead/previous\n\
                 /cue/<number>/start  go  select  stop  fade",
            )
            .weak()
            .small(),
        );
        ui.label(
            RichText::new("Anyone who can reach this port on your network can control the show.")
                .small()
                .color(PREWAIT),
        );
    }
    ui.add_space(4.0);
    ui.checkbox(&mut r.midi, "MIDI input");
    if r.midi {
        ui.horizontal(|ui| {
            ui.label("Input");
            egui::ComboBox::from_id_salt("midi_port")
                .selected_text(r.midi_port.clone().unwrap_or_else(|| "Choose…".into()))
                .show_ui(ui, |ui| {
                    for p in &st.midi_ports {
                        ui.selectable_value(&mut r.midi_port, Some(p.clone()), p);
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("Channel");
            let text = r
                .midi_channel
                .map_or("Any".to_string(), |c| (c + 1).to_string());
            egui::ComboBox::from_id_salt("midi_channel")
                .selected_text(text)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut r.midi_channel, None, "Any");
                    for c in 0..16u8 {
                        ui.selectable_value(&mut r.midi_channel, Some(c), (c + 1).to_string());
                    }
                });
        });
        match (&status.midi_port, &status.midi_error) {
            (_, Some(e)) => {
                ui.label(RichText::new(format!("⚠ {e}")).color(STOP_RED));
            }
            (Some(p), None) => {
                ui.label(RichText::new(format!("Listening to {p}")).color(GO_GREEN));
            }
            _ => {}
        }
        ui.label(
            RichText::new(
                "Note on N: fire cue N · Program change N: select cue N\n\
                 MIDI Show Control: GO, STOP, RESUME, LOAD, ALL OFF (cue numbers work)",
            )
            .weak()
            .small(),
        );
    }
}

fn outputs_panel(ui: &mut Ui, show: &mut Show, st: &UiState, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.heading("Outputs");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("+ Add").clicked() {
                actions.push(Action::AddOutput);
            }
        });
    });
    const AUTO_HELP: &str = "Fullscreen on a display other than the one NewQ is on; inside the main window \
         when there isn't one. Switches automatically when displays are plugged in or out.";
    let n_outputs = show.outputs.len();
    let show_subs = &show.sub_displays;
    for (oi, out) in show.outputs.iter_mut().enumerate() {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("{}", oi + 1)).strong().size(18.0));
                ui.add(egui::TextEdit::singleline(&mut out.name).desired_width(140.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if n_outputs > 1
                        && ui
                            .small_button("🗑")
                            .on_hover_text("Remove output")
                            .clicked()
                    {
                        actions.push(Action::RemoveOutput(out.id));
                    }
                });
            });
            let current = match &out.placement {
                Placement::Auto => "Automatic".to_string(),
                Placement::MainWindow => "Main window".to_string(),
                Placement::Display(d) => format!("Fullscreen on {}", d.name),
                Placement::SubDisplay(v) => match show_subs.iter().find(|x| x.id == *v) {
                    Some(x) => format!("On sub display {}", x.name),
                    None => "Removed sub display".to_string(),
                },
            };
            egui::ComboBox::from_id_salt(("placement", out.id.0))
                .width(ui.available_width() - 8.0)
                .selected_text(current)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut out.placement, Placement::Auto, "Automatic")
                        .on_hover_text(AUTO_HELP);
                    ui.selectable_value(&mut out.placement, Placement::MainWindow, "Main window")
                        .on_hover_text("Always show this output inside NewQ's main window");
                    for i in 0..st.monitors.len() {
                        let d = DisplayRef {
                            name: st.monitors[i].name.clone(),
                            index: i,
                        };
                        ui.selectable_value(
                            &mut out.placement,
                            Placement::Display(d),
                            format!("Fullscreen on {}", monitor_label(st, i)),
                        );
                    }
                    for v in show_subs.iter() {
                        ui.selectable_value(
                            &mut out.placement,
                            Placement::SubDisplay(v.id),
                            format!("On sub display {} (on {})", v.name, v.parent.name),
                        );
                    }
                });
            let status = match (st.routes.get(&out.id), &out.placement) {
                (None, _) => "Off".to_string(),
                (Some(OutputRoute::Display(name)), _) => format!("Fullscreen on {name}"),
                (Some(OutputRoute::SubDisplay { name, display }), _) => {
                    format!("On {name}, part of {display}")
                }
                (Some(OutputRoute::MainWindow), Placement::SubDisplay(v)) => {
                    match show_subs.iter().find(|x| x.id == *v) {
                        Some(x) => format!(
                            "{} isn't connected: showing in the main window",
                            x.parent.name
                        ),
                        None => {
                            "Its sub display was removed: showing in the main window".to_string()
                        }
                    }
                }
                (Some(OutputRoute::MainWindow), Placement::Display(d)) => {
                    format!("{} isn't connected: showing in the main window", d.name)
                }
                (Some(OutputRoute::MainWindow), Placement::Auto) => {
                    "No second display: showing in the main window".to_string()
                }
                (Some(OutputRoute::MainWindow), Placement::MainWindow) => {
                    "Showing in the main window".to_string()
                }
            };
            ui.label(RichText::new(status).weak().small());
            ui.horizontal(|ui| {
                ui.checkbox(&mut out.enabled, "On");
                ui.checkbox(&mut out.test_pattern, "Test pattern")
                    .on_hover_text("Show colour bars with the output number");
            });
            corner_pin_editor(ui, out, st, actions);
        });
    }
}

fn add_sub_display_menu(ui: &mut Ui, st: &UiState, actions: &mut Vec<Action>) {
    if st.monitors.is_empty() {
        ui.label(RichText::new("No displays found").weak());
    }
    for i in 0..st.monitors.len() {
        if ui.button(monitor_label(st, i)).clicked() {
            actions.push(Action::AddSubDisplay(DisplayRef {
                name: st.monitors[i].name.clone(),
                index: i,
            }));
            ui.close();
        }
    }
}

fn sub_displays_panel(ui: &mut Ui, show: &mut Show, st: &UiState, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.heading("Sub displays");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.menu_button("+ Add", |ui| add_sub_display_menu(ui, st, actions))
                .response
                .on_hover_text("Add an area of a display that outputs can be placed on");
        });
    });
    if show.sub_displays.is_empty() {
        ui.label(
            RichText::new(
                "Split a display into areas that each work like a display of their own: \
                 place an output on each, with its own corner pin.",
            )
            .weak()
            .small(),
        );
        return;
    }
    // Everyone's areas, to draw the neighbours of the one being edited.
    let all: Vec<SubDisplay> = show.sub_displays.clone();
    for v in show.sub_displays.iter_mut() {
        let monitor = st.sub_monitors.get(&v.id).copied();
        let screen_size = sub_display_size(st, v.id);
        let users: Vec<&str> = show
            .outputs
            .iter()
            .filter(|o| o.placement == Placement::SubDisplay(v.id))
            .map(|o| o.name.as_str())
            .collect();
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut v.name).desired_width(140.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .small_button("🗑")
                        .on_hover_text("Remove sub display")
                        .clicked()
                    {
                        actions.push(Action::RemoveSubDisplay(v.id));
                    }
                });
            });
            egui::ComboBox::from_id_salt(("vd_parent", v.id.0))
                .width(ui.available_width() - 8.0)
                .selected_text(format!("Part of {}", v.parent.name))
                .show_ui(ui, |ui| {
                    for i in 0..st.monitors.len() {
                        let d = DisplayRef {
                            name: st.monitors[i].name.clone(),
                            index: i,
                        };
                        ui.selectable_value(&mut v.parent, d, monitor_label(st, i));
                    }
                });
            let (w, h) = v.area.size_on(screen_size);
            let status = match monitor {
                Some(_) => format!("{w}×{h} px"),
                None => format!("{} isn't connected", v.parent.name),
            };
            let used = if users.is_empty() {
                "no outputs on it yet".to_string()
            } else {
                format!("shows {}", users.join(", "))
            };
            ui.label(RichText::new(format!("{status} · {used}")).weak().small());

            let editing = st.area_edit == Some(v.id);
            ui.horizontal(|ui| {
                let label = if editing { "Done" } else { "Edit area…" };
                if ui
                    .selectable_label(editing, label)
                    .on_hover_text(
                        "Drag inside the area to move it and its corners to reshape it, on a \
                         picture of the whole display",
                    )
                    .clicked()
                {
                    actions.push(Action::EditArea((!editing).then_some(v.id)));
                }
                if v.area.as_rect().is_none()
                    && ui
                        .small_button("Make rectangle")
                        .on_hover_text("Square the area up to the rectangle around it")
                        .clicked()
                {
                    let [x, y, w, h] = v.area.bounds();
                    v.area = Corners::rect(x, y, w, h);
                }
            });
            if !editing {
                return;
            }
            area_editor(ui, v, &all, monitor, screen_size, st);
        });
    }
}

/// A picture of a sub display's whole display, showing what is on it now,
/// with every sub display on it outlined and handles on the one being edited,
/// plus exact pixel fields.
fn area_editor(
    ui: &mut Ui,
    v: &mut SubDisplay,
    all: &[SubDisplay],
    monitor: Option<usize>,
    screen_size: (u32, u32),
    st: &UiState,
) {
    let (sw, sh) = (screen_size.0 as f32, screen_size.1 as f32);
    let margin = 10.0;
    let w = ui.available_width() - 2.0 * margin;
    let h = (w * sh / sw).min(w * 1.5);
    let (outer, _) = ui.allocate_exact_size(
        Vec2::new(w + 2.0 * margin, h + 2.0 * margin),
        Sense::hover(),
    );
    let rect = outer.shrink(margin);
    ui.painter().rect_filled(rect, 0.0, Color32::BLACK);
    if let Some((_, groups)) = monitor.and_then(|m| st.screens.get(&m)) {
        ui.painter().add(egui_wgpu::Callback::new_paint_callback(
            rect,
            OutputPreview {
                groups: groups.clone(),
                slot: Slot::Screen,
            },
        ));
    }
    let to_screen = |c: [f32; 2]| rect.min + Vec2::new(c[0] * rect.width(), c[1] * rect.height());
    for other in all.iter().filter(|o| o.id != v.id && o.parent == v.parent) {
        let p = other.area.0.map(to_screen);
        for i in 0..4 {
            ui.painter().line_segment(
                [p[i], p[(i + 1) % 4]],
                Stroke::new(1.0, Color32::from_gray(140)),
            );
        }
        let centre = p.iter().fold(Vec2::ZERO, |a, q| a + q.to_vec2()) / 4.0;
        let label = ui.painter().layout_no_wrap(
            other.name.clone(),
            egui::FontId::proportional(12.0),
            Color32::from_gray(200),
        );
        let at = egui::Rect::from_center_size(centre.to_pos2(), label.size());
        ui.painter()
            .rect_filled(at.expand(3.0), 3.0, Color32::from_black_alpha(180));
        ui.painter().galley(at.min, label, Color32::from_gray(200));
    }
    area_handles(ui, rect, v.id, &mut v.area, &st.handle_sel);
    if !v.area.is_convex() {
        ui.label(
            RichText::new("⚠ Corners cross or fold in; the picture will look wrong.")
                .color(STOP_RED)
                .small(),
        );
    }
    if monitor.is_none() {
        ui.label(
            RichText::new(format!(
                "Pixels assume {}×{} until {} is connected.",
                screen_size.0, screen_size.1, v.parent.name
            ))
            .weak()
            .small(),
        );
    }
    let px = |v: f32, axis: usize| v * if axis == 0 { sw } else { sh };
    let frac = |p: f32, axis: usize| p / if axis == 0 { sw } else { sh };
    egui::Grid::new(("area", v.id.0))
        .num_columns(3)
        .spacing([8.0, 4.0])
        .show(ui, |ui| match v.area.as_rect() {
            Some([x, y, w, h]) => {
                let mut r = [px(x, 0), px(y, 1), px(w, 0), px(h, 1)];
                let mut changed = false;
                for (row, label) in [(0, "Position"), (2, "Size")] {
                    ui.label(label);
                    for axis in 0..2 {
                        let i = row + axis;
                        let max = if row == 0 {
                            [sw, sh][axis] - 1.0
                        } else {
                            [sw, sh][axis] - r[axis]
                        };
                        let prefix = ["X ", "Y ", "W ", "H "][i];
                        changed |= ui
                            .add(
                                egui::DragValue::new(&mut r[i])
                                    .speed(1.0)
                                    .range(if row == 0 { 0.0 } else { 1.0 }..=max)
                                    .fixed_decimals(0)
                                    .prefix(prefix)
                                    .suffix(" px"),
                            )
                            .changed();
                    }
                    ui.end_row();
                }
                if changed {
                    // Keep it on the display when the position grows.
                    r[2] = r[2].min(sw - r[0]).max(1.0);
                    r[3] = r[3].min(sh - r[1]).max(1.0);
                    v.area =
                        Corners::rect(frac(r[0], 0), frac(r[1], 1), frac(r[2], 0), frac(r[3], 1));
                }
            }
            None => {
                for (i, label) in Corners::LABELS.iter().enumerate() {
                    ui.label(*label);
                    for axis in 0..2 {
                        let mut p = px(v.area.0[i][axis], axis);
                        if ui
                            .add(
                                egui::DragValue::new(&mut p)
                                    .speed(1.0)
                                    .range(0.0..=[sw, sh][axis])
                                    .fixed_decimals(0)
                                    .prefix(if axis == 0 { "X " } else { "Y " })
                                    .suffix(" px"),
                            )
                            .changed()
                        {
                            v.area.0[i][axis] = frac(p, axis);
                        }
                    }
                    ui.end_row();
                }
            }
        });
}

/// Corner-pin (keystone) controls for one output.
fn corner_pin_editor(ui: &mut Ui, out: &mut OutputConfig, st: &UiState, actions: &mut Vec<Action>) {
    let editing = st.geometry_edit == Some(out.id);
    ui.horizontal(|ui| {
        let label = if editing { "Done" } else { "Corner pin…" };
        if ui
            .selectable_label(editing, label)
            .on_hover_text(
                "Keystone the picture to fit the screen: drag the four corners on this \
                 output's preview. Turn on Test pattern to line it up on the real screen.",
            )
            .clicked()
        {
            actions.push(Action::EditGeometry((!editing).then_some(out.id)));
        }
        if !out.corners.is_identity() {
            ui.label(RichText::new("pinned").color(PLAYHEAD).small());
            if ui
                .small_button("Reset")
                .on_hover_text("Back to the full screen")
                .clicked()
            {
                out.corners = Corners::default();
            }
        }
    });
    if !editing {
        return;
    }
    if !out.corners.is_convex() {
        ui.label(
            RichText::new("⚠ Corners cross or fold in; the picture will look wrong.")
                .color(STOP_RED)
                .small(),
        );
    }
    // In pixels of the output's own display or sub display.
    let size = st
        .previews
        .iter()
        .find(|p| p.id == out.id)
        .map_or((1920, 1080), |p| p.canvas);
    let size = [size.0.max(1) as f32, size.1.max(1) as f32];
    egui::Grid::new(("corners", out.id.0))
        .num_columns(3)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            for (i, label) in Corners::LABELS.iter().enumerate() {
                ui.label(*label);
                #[allow(clippy::needless_range_loop)]
                for axis in 0..2 {
                    let mut px = (out.corners.0[i][axis] * size[axis]).round();
                    if ui
                        .add(
                            egui::DragValue::new(&mut px)
                                .speed(1.0)
                                .range(-0.5 * size[axis]..=1.5 * size[axis])
                                .fixed_decimals(0)
                                .prefix(if axis == 0 { "X " } else { "Y " })
                                .suffix(" px"),
                        )
                        .changed()
                    {
                        out.corners.0[i][axis] = px / size[axis];
                    }
                }
                ui.end_row();
            }
        });
}

fn secs(ui: &mut Ui, v: &mut f64) -> egui::Response {
    ui.add(
        egui::DragValue::new(v)
            .speed(0.05)
            .range(0.0..=36_000.0)
            .fixed_decimals(2)
            .suffix(" s"),
    )
}

fn db_slider(ui: &mut Ui, v: &mut f32) -> egui::Response {
    ui.add(
        egui::Slider::new(v, MIN_DB..=12.0)
            .suffix(" dB")
            .fixed_decimals(1)
            .custom_formatter(|v, _| {
                if v <= MIN_DB as f64 {
                    "-∞".into()
                } else {
                    format!("{v:.1}")
                }
            }),
    )
}

fn cue_picker(
    ui: &mut Ui,
    id_salt: &str,
    candidates: &[(CueId, String)],
    target: &mut Option<CueId>,
    none_label: &str,
) {
    let selected = target
        .and_then(|t| candidates.iter().find(|c| c.0 == t))
        .map(|c| c.1.clone())
        .unwrap_or_else(|| none_label.to_string());
    egui::ComboBox::from_id_salt(id_salt)
        .width(260.0)
        .selected_text(selected)
        .show_ui(ui, |ui| {
            ui.selectable_value(target, None, none_label);
            for (id, label) in candidates {
                ui.selectable_value(target, Some(*id), label);
            }
        });
}

fn inspector(
    ui: &mut Ui,
    show: &mut Show,
    engine: &mut Engine,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) {
    let Some(idx) = st.selected.and_then(|id| show.cue_index(id)) else {
        ui.label(RichText::new("Select a cue to edit it.").weak());
        return;
    };
    let media_cues: Vec<(CueId, String)> = show
        .cues
        .iter()
        .filter(|c| c.kind.media().is_some() || matches!(c.kind, CueKind::Group(_)))
        .map(|c| (c.id, cue_label(c)))
        .collect();
    let outputs: Vec<(OutputId, String)> = show
        .outputs
        .iter()
        .map(|o| (o.id, o.name.clone()))
        .collect();
    let cue = &mut show.cues[idx];

    ui.horizontal(|ui| {
        ui.heading(format!(
            "{} {} cue",
            cue.kind.cue_type().icon(),
            cue.kind.cue_type().label()
        ));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("⏹ Stop").clicked() {
                actions.push(Action::StopSelected);
            }
            if ui
                .button("Fade out")
                .on_hover_text("Fade this cue out with its own fade-out time")
                .clicked()
            {
                actions.push(Action::FadeOutSelected);
            }
            if ui
                .button("▶ Fire")
                .on_hover_text("Play this cue now without moving the playhead")
                .clicked()
            {
                actions.push(Action::FireSelected);
            }
        });
    });
    ui.separator();

    ui.columns(2, |cols| {
        let ui = &mut cols[0];
        egui::Grid::new("basics")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Number");
                ui.add(egui::TextEdit::singleline(&mut cue.number).desired_width(80.0));
                ui.end_row();
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut cue.name).desired_width(260.0));
                ui.end_row();
                ui.label("Pre-wait");
                secs(ui, &mut cue.pre_wait);
                ui.end_row();
                ui.label("Continue");
                if cue.parent.is_some() {
                    ui.label(RichText::new("Decided by the group").weak());
                } else {
                    egui::ComboBox::from_id_salt("continue")
                        .selected_text(cue.continue_mode.label())
                        .show_ui(ui, |ui| {
                            for m in ContinueMode::ALL {
                                ui.selectable_value(&mut cue.continue_mode, m, m.label());
                            }
                        });
                }
                ui.end_row();
                ui.label("Notes");
                ui.add(
                    egui::TextEdit::multiline(&mut cue.notes)
                        .desired_rows(3)
                        .desired_width(260.0),
                );
                ui.end_row();
            });

        let ui = &mut cols[1];
        let cue_type = cue.kind.cue_type();
        let cue_id = cue.id;
        match &mut cue.kind {
            CueKind::Video(m) | CueKind::Image(m) | CueKind::Audio(m) => {
                media_inspector(ui, cue_type, m, &outputs, engine)
            }
            CueKind::Fade(f) => {
                let candidates: Vec<_> = media_cues
                    .iter()
                    .filter(|c| c.0 != cue_id)
                    .cloned()
                    .collect();
                egui::Grid::new("fade")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Target");
                        cue_picker(ui, "fade_target", &candidates, &mut f.target, "(none)");
                        ui.end_row();
                        ui.label("Duration");
                        secs(ui, &mut f.duration);
                        ui.end_row();
                        ui.label("Curve");
                        curve_combo(ui, "fade_curve", &mut f.curve);
                        ui.end_row();
                        let mut set_op = f.opacity.is_some();
                        ui.checkbox(&mut set_op, "Opacity");
                        let mut op = f.opacity.unwrap_or(0.0);
                        ui.add_enabled(set_op, egui::Slider::new(&mut op, 0.0..=1.0));
                        f.opacity = set_op.then_some(op);
                        ui.end_row();
                        let mut set_vol = f.volume_db.is_some();
                        ui.checkbox(&mut set_vol, "Volume");
                        let mut vol = f.volume_db.unwrap_or(MIN_DB);
                        ui.add_enabled_ui(set_vol, |ui| db_slider(ui, &mut vol));
                        f.volume_db = set_vol.then_some(vol);
                        ui.end_row();
                        ui.label("");
                        ui.checkbox(&mut f.stop_target, "Stop target when done");
                        ui.end_row();
                        ui.label("Presets");
                        ui.horizontal(|ui| {
                            if ui.button("Fade out & stop").clicked() {
                                f.opacity = Some(0.0);
                                f.volume_db = Some(MIN_DB);
                                f.stop_target = true;
                            }
                            if ui.button("Fade to full").clicked() {
                                f.opacity = Some(1.0);
                                f.volume_db = Some(0.0);
                                f.stop_target = false;
                            }
                            if ui.button("Dim to 50%").clicked() {
                                f.opacity = Some(0.5);
                                f.volume_db = Some(-6.0);
                                f.stop_target = false;
                            }
                        });
                        ui.end_row();
                    });
            }
            CueKind::Stop(s) => {
                egui::Grid::new("stop")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Target");
                        cue_picker(ui, "stop_target", &media_cues, &mut s.target, "Everything");
                        ui.end_row();
                        ui.label("Fade out");
                        ui.checkbox(&mut s.use_own_fade, "Use each cue's own fade-out");
                        ui.end_row();
                        ui.label("");
                        ui.add_enabled_ui(!s.use_own_fade, |ui| secs(ui, &mut s.fade));
                        ui.end_row();
                    });
            }
            CueKind::Text(t) => {
                text_inspector(ui, t);
                ui.separator();
                media_inspector(ui, cue_type, &mut t.media, &outputs, engine);
            }
            CueKind::Group(g) => {
                egui::Grid::new("group")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Mode");
                        egui::ComboBox::from_id_salt("group_mode")
                            .selected_text(g.mode.label())
                            .show_ui(ui, |ui| {
                                for m in GroupMode::ALL {
                                    ui.selectable_value(&mut g.mode, m, m.label())
                                        .on_hover_text(m.description());
                                }
                            });
                        ui.end_row();
                    });
                ui.label(RichText::new(g.mode.description()).weak());
                ui.label(
                    RichText::new(
                        "Adding a cue while the group is selected puts it inside. Children's \
                         pre-waits are measured within the group.",
                    )
                    .weak(),
                );
            }
            CueKind::Wait(w) => {
                egui::Grid::new("wait")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Duration");
                        secs(ui, &mut w.duration);
                        ui.end_row();
                    });
                ui.label(
                    RichText::new(
                        "Set Continue to Auto-follow to fire the next cue when the wait ends.",
                    )
                    .weak(),
                );
            }
        }
    });
}

fn curve_combo(ui: &mut Ui, id: &str, curve: &mut Curve) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(curve.label())
        .show_ui(ui, |ui| {
            for c in Curve::ALL {
                ui.selectable_value(curve, c, c.label());
            }
        });
}

fn media_inspector(
    ui: &mut Ui,
    ty: CueType,
    m: &mut MediaCue,
    outputs: &[(OutputId, String)],
    engine: &mut Engine,
) {
    let visual = ty != CueType::Audio;
    // Text and images are held for a time rather than played from a file.
    let still = matches!(ty, CueType::Image | CueType::Text);
    let is_text = ty == CueType::Text;
    egui::Grid::new("media")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            if !is_text {
                ui.label("File");
            ui.horizontal(|ui| {
                let name = m
                    .file
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "(none)".into());
                ui.label(RichText::new(name).strong())
                    .on_hover_text(m.file.display().to_string());
                if ui.button("Browse…").clicked()
                    && let Some(f) = media_dialog(ty).pick_file()
                {
                    engine.probe.invalidate(&f);
                    m.file = f;
                }
            });
            ui.end_row();
            ui.label("");
            let info = if m.file.as_os_str().is_empty() {
                RichText::new("Choose a file").weak()
            } else {
                match engine.probe_state(&m.file) {
                    ProbeState::Pending => RichText::new("Reading file…").weak(),
                    ProbeState::Failed(e) => RichText::new(format!("⚠ {e}")).color(STOP_RED),
                    ProbeState::Ready(i) => {
                        let mut parts = Vec::new();
                        if let Some(v) = &i.video {
                            parts.push(format!("{}×{}", v.width, v.height));
                            if !i.is_still {
                                parts.push(format!("{:.2} fps", v.fps));
                            }
                        }
                        if let Some(d) = i.duration {
                            parts.push(fmt_time(d));
                        }
                        parts.push(if i.has_audio {
                            "audio".into()
                        } else {
                            "no audio".into()
                        });
                        RichText::new(parts.join(" · ")).weak()
                    }
                }
            };
            ui.label(info);
            ui.end_row();
            }

            if visual {
                ui.label("Output");
                let sel = m
                    .output
                    .and_then(|o| outputs.iter().find(|x| x.0 == o))
                    .map(|x| x.1.clone())
                    .unwrap_or_else(|| "(none)".into());
                egui::ComboBox::from_id_salt("output")
                    .selected_text(sel)
                    .show_ui(ui, |ui| {
                        for (id, name) in outputs {
                            ui.selectable_value(&mut m.output, Some(*id), name);
                        }
                    });
                ui.end_row();
                ui.label("Layer / Fit");
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut m.layer).range(-100..=100))
                        .on_hover_text("Higher layers draw on top");
                    if !is_text {
                        egui::ComboBox::from_id_salt("fit")
                            .selected_text(m.fit.label())
                            .show_ui(ui, |ui| {
                                for f in Fit::ALL {
                                    ui.selectable_value(&mut m.fit, f, f.label());
                                }
                            });
                    }
                });
                ui.end_row();
                ui.label("Opacity");
                ui.add(egui::Slider::new(&mut m.opacity, 0.0..=1.0));
                ui.end_row();
            }
            if !still {
                ui.label("Volume");
                db_slider(ui, &mut m.volume_db);
                ui.end_row();
            }
            if visual {
                ui.label("Transition in");
                ui.horizontal_wrapped(|ui| {
                    for t in Transition::ALL {
                        ui.selectable_value(&mut m.transition, t, t.short_label())
                            .on_hover_text(t.description());
                    }
                });
                ui.end_row();
                ui.label("");
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(m.transition.uses_time(), |ui| {
                        secs(ui, &mut m.fade_in).on_hover_text(
                            "Transition time. Dips spend half going to the colour and half coming up.",
                        );
                    });
                    curve_combo(ui, "media_curve", &mut m.curve);
                });
                ui.end_row();
                ui.label("");
                ui.label(RichText::new(m.transition.description()).weak().small());
                ui.end_row();
            } else {
                ui.label("Fade in");
                ui.horizontal(|ui| {
                    secs(ui, &mut m.fade_in);
                    curve_combo(ui, "media_curve", &mut m.curve);
                });
                ui.end_row();
            }
            ui.label("Fade out");
            secs(ui, &mut m.fade_out).on_hover_text(
                "This cue's own fade-out: applied before the media ends, and when the cue is \
                 faded out by hand or by a Stop cue using own fades.",
            );
            ui.end_row();
            if still {
                ui.label("Hold for");
                let mut d = m.end.unwrap_or(0.0);
                secs(ui, &mut d).on_hover_text("0 = until stopped");
                m.end = (d > 0.0).then_some(d);
                ui.end_row();
            } else {
                ui.label("In / Out");
                ui.horizontal(|ui| {
                    secs(ui, &mut m.start);
                    let mut has_end = m.end.is_some();
                    ui.checkbox(&mut has_end, "");
                    let mut end = m.end.unwrap_or(m.start + 10.0);
                    ui.add_enabled_ui(has_end, |ui| secs(ui, &mut end));
                    m.end = has_end.then_some(end.max(m.start));
                });
                ui.end_row();
                ui.label("");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut m.loop_media, "Loop");
                    if m.loop_media {
                        let mut forever = m.loop_count == 0;
                        ui.checkbox(&mut forever, "Forever");
                        if forever {
                            m.loop_count = 0;
                        } else {
                            m.loop_count = m.loop_count.max(2);
                            ui.add(egui::DragValue::new(&mut m.loop_count).range(1..=9999).suffix(" plays"))
                                .on_hover_text("How many times in all the slice plays");
                        }
                    }
                    if ty == CueType::Video {
                        ui.checkbox(&mut m.hold_last_frame, "Hold last frame");
                    }
                });
                ui.end_row();
            }
        });
    if visual {
        let title = if m.transform.is_identity() {
            "Geometry"
        } else {
            "Geometry •"
        };
        egui::CollapsingHeader::new(title)
            .id_salt("geometry")
            .default_open(!m.transform.is_identity())
            .show(ui, |ui| geometry_inspector(ui, &mut m.transform));
    }
    if m.loop_media && !still {
        let what = if m.end.is_some() {
            "Loops from the in point to the out point."
        } else {
            "Loops from the in point to the end of the file."
        };
        ui.label(RichText::new(what).weak());
    }
}

/// What a text cue says and how it looks; where, when and how it plays is the
/// shared media section.
fn text_inspector(ui: &mut Ui, t: &mut TextCue) {
    ui.add(
        egui::TextEdit::multiline(&mut t.text)
            .desired_rows(3)
            .desired_width(f32::INFINITY)
            .hint_text("Text to show"),
    );
    egui::Grid::new("text_style")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Size");
            let mut pct = t.size * 100.0;
            if ui
                .add(
                    egui::DragValue::new(&mut pct)
                        .speed(0.1)
                        .range(0.5..=100.0)
                        .suffix("% of height"),
                )
                .changed()
            {
                t.size = pct / 100.0;
            }
            ui.end_row();
            ui.label("Colour");
            ui.horizontal(|ui| {
                let [r, g, b, a] = t.color;
                let mut c = Color32::from_rgba_unmultiplied(r, g, b, a);
                if ui.color_edit_button_srgba(&mut c).changed() {
                    t.color = c.to_srgba_unmultiplied();
                }
                ui.checkbox(&mut t.shadow, "Shadow")
                    .on_hover_text("A dark shadow behind the letters, so they read over video");
                ui.checkbox(&mut t.bold, "Bold");
            });
            ui.end_row();
            ui.label("Align");
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("text_align")
                    .selected_text(t.align.label())
                    .show_ui(ui, |ui| {
                        for a in TextAlign::ALL {
                            ui.selectable_value(&mut t.align, a, a.label());
                        }
                    });
                egui::ComboBox::from_id_salt("text_valign")
                    .selected_text(t.valign.label())
                    .show_ui(ui, |ui| {
                        for a in TextVAlign::ALL {
                            ui.selectable_value(&mut t.valign, a, a.label());
                        }
                    });
            });
            ui.end_row();
            ui.label("Wrap at");
            let mut w = t.width * 100.0;
            if ui
                .add(
                    egui::DragValue::new(&mut w)
                        .speed(0.5)
                        .range(5.0..=100.0)
                        .suffix("% of width"),
                )
                .changed()
            {
                t.width = w / 100.0;
            }
            ui.end_row();
        });
}

/// A percentage field for a fraction (0.25 shows as 25%).
fn percent(ui: &mut Ui, v: &mut f32, speed: f64) -> egui::Response {
    let mut p = *v * 100.0;
    let r = ui.add(egui::DragValue::new(&mut p).speed(speed).suffix("%"));
    if r.changed() {
        *v = p / 100.0;
    }
    r
}

/// Position, scale, rotation and crop of a visual cue on its output.
fn geometry_inspector(ui: &mut Ui, t: &mut LayerTransform) {
    let uniform_id = ui.id().with("uniform_scale");
    let mut uniform = ui.data_mut(|d| *d.get_temp_mut_or(uniform_id, true));
    egui::Grid::new("geometry_grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Position");
            ui.horizontal(|ui| {
                percent(ui, &mut t.x, 0.2)
                    .on_hover_text("Across, as a share of the output's width");
                percent(ui, &mut t.y, 0.2).on_hover_text("Down, as a share of the output's height");
            });
            ui.end_row();
            ui.label("Scale");
            ui.horizontal(|ui| {
                if percent(ui, &mut t.scale_x, 0.5)
                    .on_hover_text("Negative values mirror the picture")
                    .changed()
                    && uniform
                {
                    t.scale_y = t.scale_x;
                }
                ui.add_enabled_ui(!uniform, |ui| percent(ui, &mut t.scale_y, 0.5));
                ui.checkbox(&mut uniform, "Together");
            });
            ui.end_row();
            ui.label("Rotation");
            ui.add(
                egui::DragValue::new(&mut t.rotation)
                    .speed(0.5)
                    .suffix("°")
                    .range(-360.0..=360.0),
            )
            .on_hover_text("Clockwise, about the middle of the picture");
            ui.end_row();
            ui.label("Crop");
            ui.horizontal(|ui| {
                for (side, name) in ["left", "top", "right", "bottom"].iter().enumerate() {
                    ui.label(RichText::new(name[..1].to_uppercase()).weak());
                    percent(ui, &mut t.crop[side], 0.2)
                        .on_hover_text(format!("Cut from the {name}"));
                    t.crop[side] = t.crop[side].clamp(0.0, 0.95);
                }
            });
            ui.end_row();
        });
    ui.data_mut(|d| d.insert_temp(uniform_id, uniform));
    if !t.is_identity() && ui.small_button("Reset geometry").clicked() {
        *t = LayerTransform::default();
    }
}

fn media_dialog(ty: CueType) -> rfd::FileDialog {
    let d = rfd::FileDialog::new();
    match ty {
        CueType::Video => d.add_filter(
            "Video",
            &[
                "mp4", "mov", "mkv", "avi", "webm", "m4v", "mpg", "mpeg", "mxf", "wmv", "ts", "gif",
            ],
        ),
        CueType::Image => d.add_filter(
            "Image",
            &["png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp", "tga"],
        ),
        CueType::Audio => d.add_filter(
            "Audio",
            &[
                "wav", "mp3", "aif", "aiff", "flac", "ogg", "m4a", "aac", "opus",
            ],
        ),
        _ => d,
    }
    .add_filter("All files", &["*"])
}

fn show_dialog() -> rfd::FileDialog {
    rfd::FileDialog::new().add_filter("NewQ show", &[SHOW_EXTENSION])
}

/// Ask what to do with unsaved changes. Returns true if it is OK to continue.
pub fn confirm_discard(show: &Show, st: &mut UiState) -> bool {
    if !st.dirty {
        return true;
    }
    let answer = rfd::MessageDialog::new()
        .set_title("Unsaved changes")
        .set_description("Save changes to this show first?")
        .set_buttons(rfd::MessageButtons::YesNoCancel)
        .show();
    match answer {
        rfd::MessageDialogResult::Yes => save(show, st, false),
        rfd::MessageDialogResult::No => true,
        _ => false,
    }
}

fn save(show: &Show, st: &mut UiState, save_as: bool) -> bool {
    let path = match (&st.show_path, save_as) {
        (Some(p), false) => p.clone(),
        _ => match show_dialog()
            .set_file_name(format!("{}.{SHOW_EXTENSION}", show.name))
            .save_file()
        {
            Some(p) => p.with_extension(SHOW_EXTENSION),
            None => return false,
        },
    };
    match show.save(&path) {
        Ok(()) => {
            st.show_path = Some(path);
            st.saved_json = serde_json::to_string(show).ok();
            true
        }
        Err(e) => {
            rfd::MessageDialog::new()
                .set_title("Save failed")
                .set_description(format!("{e:#}"))
                .show();
            false
        }
    }
}

fn load_into(
    new: Show,
    path: Option<PathBuf>,
    show: &mut Show,
    engine: &mut Engine,
    st: &mut UiState,
) {
    engine.stop_all(0.0);
    *show = new;
    engine.playhead = show.cues.first().map(|c| c.id);
    st.selected = engine.playhead;
    st.show_path = path;
    st.saved_json = serde_json::to_string(show).ok();
}

/// Do what a remote message asks for, as if the operator had done it.
pub fn run_remote(cmd: Command, show: &mut Show, engine: &mut Engine, st: &mut UiState) {
    let find = |show: &Show, n: &str| {
        show.cues
            .iter()
            .find(|c| c.number == n.trim())
            .map(|c| c.id)
    };
    match cmd {
        Command::Go => apply(Action::Go, show, engine, st),
        Command::Panic => apply(Action::Panic, show, engine, st),
        Command::StopAll => engine.stop_all(0.0),
        Command::Pause => engine.set_paused(true),
        Command::Resume => engine.set_paused(false),
        Command::PlayheadNext | Command::PlayheadPrevious => {
            let here = engine
                .playhead
                .and_then(|id| show.cue_index(id))
                .map(|i| show.top_index(i));
            let next = match (here, cmd == Command::PlayheadNext) {
                (Some(i), true) => show.next_top(i),
                (Some(i), false) if i > 0 => Some(show.top_index(i - 1)),
                (Some(_), false) => None,
                (None, false) if !show.cues.is_empty() => Some(show.top_index(show.cues.len() - 1)),
                (None, _) => None,
            };
            if let Some(i) = next {
                let id = show.cues[i].id;
                select(show, st, engine, Some(id));
                st.scroll_to_selected = true;
            }
        }
        Command::GoCue(ref n)
        | Command::Select(ref n)
        | Command::Fire(ref n)
        | Command::Stop(ref n)
        | Command::FadeOut(ref n) => {
            let Some(id) = find(show, n) else {
                engine.log(format!("Remote: there is no cue numbered {n}"));
                return;
            };
            match cmd {
                Command::GoCue(_) => {
                    select(show, st, engine, Some(id));
                    apply(Action::Go, show, engine, st);
                }
                Command::Select(_) => {
                    select(show, st, engine, Some(id));
                    st.scroll_to_selected = true;
                }
                Command::Fire(_) => engine.fire(show, id),
                Command::Stop(_) => engine.stop_cue(id, 0.0),
                _ => engine.fade_out_cue(show, id),
            }
        }
    }
}

/// Make a new cue and put it where the selection says (see `Show::insert_slot`).
fn add_cue(
    show: &mut Show,
    st: &mut UiState,
    engine: &mut Engine,
    ty: CueType,
    file: Option<PathBuf>,
) {
    let slot = show.insert_slot(st.selected, ty == CueType::Group);
    let cue = show.make_cue(ty, file, slot.at);
    select_new(show, st, engine, slot.parent, cue.id);
    show.insert_cue(slot, cue);
}

/// Select a cue that was just added, opening its group so it can be seen.
fn select_new(
    show: &mut Show,
    st: &mut UiState,
    engine: &mut Engine,
    parent: Option<CueId>,
    id: CueId,
) {
    if let Some(CueKind::Group(g)) = parent
        .and_then(|p| show.cue_index(p))
        .map(|i| &mut show.cues[i].kind)
    {
        g.collapsed = false;
    }
    st.selected = Some(id);
    st.scroll_to_selected = true;
    if engine.playhead.is_none() {
        engine.playhead = Some(parent.unwrap_or(id));
    }
}

/// Selecting a cue puts the playhead on it, or on its group for a child.
fn select(show: &Show, st: &mut UiState, engine: &mut Engine, id: Option<CueId>) {
    st.selected = id;
    engine.playhead = id.map(|id| show.top_id(id));
}

fn apply(action: Action, show: &mut Show, engine: &mut Engine, st: &mut UiState) {
    let sel_idx = st.selected.and_then(|id| show.cue_index(id));
    match action {
        Action::Go => {
            engine.go(show);
            st.selected = engine.playhead;
            st.scroll_to_selected = true;
        }
        Action::Panic => engine.panic(show),
        Action::TogglePause => {
            let p = !engine.paused;
            engine.set_paused(p);
        }
        Action::Select(id) => select(show, st, engine, Some(id)),
        Action::SelectRelative(d) => {
            if show.cues.is_empty() {
                return;
            }
            let i = match sel_idx {
                Some(i) => show.step_visible(i, d).unwrap_or(i),
                None => 0,
            };
            let id = show.cues[i].id;
            select(show, st, engine, Some(id));
            st.scroll_to_selected = true;
        }
        Action::AddCue(ty) => {
            let file = match ty {
                CueType::Video | CueType::Image | CueType::Audio => {
                    match media_dialog(ty).pick_file() {
                        Some(f) => Some(f),
                        None => return,
                    }
                }
                _ => None,
            };
            add_cue(show, st, engine, ty, file);
        }
        Action::AddFiles(files) => {
            for f in files {
                if f.extension().is_some_and(|e| e == SHOW_EXTENSION) {
                    if confirm_discard(show, st) {
                        match Show::load(&f) {
                            Ok(new) => load_into(new, Some(f), show, engine, st),
                            Err(e) => engine.log(format!("Could not open show: {e:#}")),
                        }
                    }
                    return;
                }
                match guess_cue_type(&f) {
                    Some(ty) => add_cue(show, st, engine, ty, Some(f)),
                    None => engine.log(format!("Unsupported file type: {}", f.display())),
                }
            }
        }
        Action::Delete => {
            if let Some(i) = sel_idx {
                // A group goes with its children.
                let removed = show.delete(i);
                for id in &removed {
                    engine.stop_cue(*id, 0.0);
                }
                let next = show
                    .cues
                    .get(i.min(show.cues.len().saturating_sub(1)))
                    .map(|c| c.id);
                if engine.playhead.is_some_and(|p| removed.contains(&p)) {
                    engine.playhead = next.map(|id| show.top_id(id));
                }
                st.selected = next;
            }
        }
        Action::Duplicate => {
            if let Some(i) = sel_idx {
                let parent = show.cues[i].parent;
                let copy = show.duplicate(i);
                select_new(show, st, engine, parent, copy);
            }
        }
        Action::Move(d) => {
            if let Some(i) = sel_idx
                && show.move_cue(i, d).is_some()
            {
                // A child that left its group moves the playhead's idea of its home.
                select(show, st, engine, st.selected);
                st.scroll_to_selected = true;
            }
        }
        Action::New => {
            if confirm_discard(show, st) {
                load_into(Show::default(), None, show, engine, st);
            }
        }
        Action::Open => {
            if confirm_discard(show, st)
                && let Some(path) = show_dialog().pick_file()
            {
                match Show::load(&path) {
                    Ok(new) => load_into(new, Some(path), show, engine, st),
                    Err(e) => engine.log(format!("Could not open show: {e:#}")),
                }
            }
        }
        Action::Save => {
            save(show, st, false);
        }
        Action::SaveAs => {
            save(show, st, true);
        }
        Action::FireSelected => {
            if let Some(id) = st.selected {
                engine.fire(show, id);
            }
        }
        Action::StopSelected => {
            if let Some(id) = st.selected {
                engine.stop_cue(id, 0.0);
            }
        }
        Action::StopInstance(i, fade) => engine.stop_instance(i, fade),
        Action::FadeOutInstance(i) => engine.fade_out_instance(show, i),
        Action::EditGeometry(id) => {
            st.geometry_edit = id;
            st.handle_sel.set(None);
        }
        Action::EditArea(id) => {
            st.area_edit = id;
            st.handle_sel.set(None);
        }
        Action::Nudge(d) => {
            if let Some(sel) = st.selected_handle() {
                let target = match sel.quad {
                    Quad::Pin(id) => {
                        let size = st
                            .previews
                            .iter()
                            .find(|p| p.id == id)
                            .map_or((1920, 1080), |p| p.canvas);
                        show.outputs
                            .iter_mut()
                            .find(|o| o.id == id)
                            .map(|o| (&mut o.corners, size))
                    }
                    Quad::Area(id) => {
                        let size = sub_display_size(st, id);
                        show.sub_displays
                            .iter_mut()
                            .find(|v| v.id == id)
                            .map(|v| (&mut v.area, size))
                    }
                };
                if let Some((corners, size)) = target {
                    nudge(corners, sel.corner, d, size, sel.quad.range());
                }
            }
        }
        Action::AddSubDisplay(parent) => {
            st.area_edit = Some(show.add_sub_display(parent));
            st.handle_sel.set(None);
        }
        Action::RemoveSubDisplay(id) => {
            show.remove_sub_display(id);
            if st.area_edit == Some(id) {
                st.area_edit = None;
            }
        }
        Action::FadeOutSelected => {
            if let Some(id) = st.selected {
                engine.fade_out_cue(show, id);
            }
        }
        Action::SetTransition(id, t) => {
            if let Some(i) = show.cue_index(id)
                && let Some(m) = show.cues[i].kind.media_mut()
            {
                m.transition = t;
                if t.uses_time() && m.fade_in <= 0.0 {
                    m.fade_in = 1.0;
                }
            }
        }
        Action::ToggleGroup(id) => {
            if let Some(i) = show.cue_index(id)
                && let CueKind::Group(g) = &mut show.cues[i].kind
            {
                g.collapsed = !g.collapsed;
                // Don't leave the selection on a cue that just disappeared.
                if g.collapsed
                    && st
                        .selected
                        .and_then(|s| show.cue(s))
                        .is_some_and(|c| c.parent == Some(id))
                {
                    select(show, st, engine, Some(id));
                }
            }
        }
        Action::GroupCue(id) => {
            if let Some(i) = show.cue_index(id)
                && let Some(g) = show.group_cue(i)
            {
                if engine.playhead == Some(id) {
                    engine.playhead = Some(g);
                }
                st.selected = Some(g);
                st.scroll_to_selected = true;
            }
        }
        Action::Ungroup(id) => {
            if let Some(i) = show.cue_index(id) {
                let first_child = show.children(id).first().map(|c| c.id);
                show.ungroup(i);
                let next = first_child.or_else(|| show.cues.get(i).map(|c| c.id));
                if st.selected == Some(id) {
                    select(show, st, engine, next);
                } else if engine.playhead == Some(id) {
                    engine.playhead = next;
                }
            }
        }
        Action::MoveIntoGroup(id, group) => {
            if let Some(i) = show.cue_index(id)
                && show.move_into(i, group)
            {
                if let Some(g) = show.cue_index(group)
                    && let CueKind::Group(g) = &mut show.cues[g].kind
                {
                    g.collapsed = false;
                }
                select(show, st, engine, Some(id));
                st.scroll_to_selected = true;
            }
        }
        Action::MoveOutOfGroup(id) => {
            if let Some(i) = show.cue_index(id)
                && show.move_out(i)
            {
                select(show, st, engine, Some(id));
                st.scroll_to_selected = true;
            }
        }
        Action::AddOutput => {
            show.add_output();
        }
        Action::RemoveOutput(id) => {
            show.outputs.retain(|o| o.id != id);
            let fallback = show.outputs.first().map(|o| o.id);
            for c in &mut show.cues {
                if c.kind.cue_type() != CueType::Audio
                    && let Some(m) = c.kind.media_mut()
                    && m.output == Some(id)
                {
                    m.output = fallback;
                }
            }
        }
        Action::Quit => st.quit_requested = true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, Pos2, RawInput, Rect, pos2, vec2};

    /// Run one egui frame of the corner-pin handles with the given input events.
    fn frame(ctx: &egui::Context, t: f64, events: Vec<Event>, screen: Rect, corners: &mut Corners) {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            time: Some(t),
            events,
            ..Default::default()
        };
        let sel = Cell::new(None);
        let mut out = ctx.run_ui(input, |ui| {
            corner_handles(ui, screen, OutputId(1), corners, &sel)
        });
        out.textures_delta.clear();
    }

    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// Press on `from`, drag to `to` over several frames, release in the same
    /// frame as the last move. `t` is egui's clock, which must keep moving
    /// forward between drags.
    fn drag(
        ctx: &egui::Context,
        t: &mut f64,
        screen: Rect,
        corners: &mut Corners,
        from: Pos2,
        to: Pos2,
    ) {
        let mut step = |events: Vec<Event>, corners: &mut Corners| {
            *t += 1.0 / 60.0;
            frame(ctx, *t, events, screen, corners);
        };
        step(vec![Event::PointerMoved(from)], corners);
        step(vec![button(from, true)], corners);
        for i in 1..8 {
            let p = from + (to - from) * (i as f32 / 8.0);
            step(vec![Event::PointerMoved(p)], corners);
        }
        step(vec![Event::PointerMoved(to), button(to, false)], corners);
        step(vec![], corners);
    }

    #[test]
    fn dragging_a_corner_handle_moves_that_corner() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(pos2(100.0, 100.0), vec2(320.0, 180.0));
        let mut corners = Corners::default();
        let mut t = 0.0;
        // Drag the top-left handle 20% right and 20% down.
        drag(
            &ctx,
            &mut t,
            screen,
            &mut corners,
            screen.min,
            pos2(164.0, 136.0),
        );
        let [x, y] = corners.0[0];
        assert!(
            (x - 0.2).abs() < 0.002 && (y - 0.2).abs() < 0.002,
            "{:?}",
            corners.0
        );
        assert_eq!(
            corners.0[1..],
            Corners::default().0[1..],
            "other corners unchanged"
        );
        // Drag the bottom-right handle out past the screen edge.
        drag(
            &ctx,
            &mut t,
            screen,
            &mut corners,
            screen.max,
            screen.max + vec2(32.0, 18.0),
        );
        let [x, y] = corners.0[2];
        assert!(
            (x - 1.1).abs() < 0.002 && (y - 1.1).abs() < 0.002,
            "{:?}",
            corners.0
        );
    }

    #[test]
    fn dragging_inside_an_area_moves_it_and_keeps_it_on_the_display() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(pos2(100.0, 100.0), vec2(320.0, 180.0));
        let mut area = Corners::rect(0.0, 0.0, 0.5, 1.0);
        let mut t = 0.0;
        let mut run = |area: &mut Corners, from: Pos2, to: Pos2| {
            let mut step = |events: Vec<Event>, area: &mut Corners| {
                t += 1.0 / 60.0;
                let input = RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
                    time: Some(t),
                    events,
                    ..Default::default()
                };
                let sel = Cell::new(None);
                let mut out = ctx.run_ui(input, |ui| {
                    area_handles(ui, screen, SubDisplayId(7), area, &sel)
                });
                out.textures_delta.clear();
            };
            step(vec![Event::PointerMoved(from)], area);
            step(vec![button(from, true)], area);
            for i in 1..8 {
                step(
                    vec![Event::PointerMoved(from + (to - from) * (i as f32 / 8.0))],
                    area,
                );
            }
            step(vec![Event::PointerMoved(to), button(to, false)], area);
            step(vec![], area);
        };
        // Drag the middle of the left half 25% of the width to the right.
        let mid = pos2(180.0, 190.0);
        run(&mut area, mid, mid + vec2(80.0, 0.0));
        let r = area.as_rect().expect("still a rectangle");
        assert!((r[0] - 0.25).abs() < 0.002 && r[1] == 0.0, "{r:?}");
        assert!(
            (r[2] - 0.5).abs() < 1e-5 && (r[3] - 1.0).abs() < 1e-5,
            "{r:?}"
        );
        // Dragging far past the right edge stops at the edge.
        let mid = pos2(260.0, 190.0);
        run(&mut area, mid, mid + vec2(400.0, 50.0));
        let r = area.as_rect().expect("still a rectangle");
        assert!((r[0] - 0.5).abs() < 1e-5 && r[1].abs() < 1e-5, "{r:?}");
        // A corner can't leave the display either.
        run(&mut area, pos2(420.0, 280.0), pos2(500.0, 330.0));
        assert_eq!(area.0[2], [1.0, 1.0]);
    }

    #[test]
    fn dragging_empty_space_moves_nothing() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(pos2(100.0, 100.0), vec2(320.0, 180.0));
        let mut corners = Corners::default();
        let mut t = 0.0;
        drag(
            &ctx,
            &mut t,
            screen,
            &mut corners,
            screen.center(),
            screen.center() + vec2(50.0, 50.0),
        );
        assert!(corners.is_identity());
    }

    #[test]
    fn nudging_moves_by_whole_pixels_and_stays_in_range() {
        let size = (1000, 500);
        // A corner: 1 px right from the nearest whole pixel, then 5 px up.
        let mut c = Corners::default();
        c.0[0] = [0.1003, 0.2];
        nudge(&mut c, Some(0), vec2(1.0, 0.0), size, -0.5..=1.5);
        assert_eq!(c.0[0], [0.101, 0.2]);
        nudge(&mut c, Some(0), vec2(0.0, -5.0), size, -0.5..=1.5);
        assert_eq!(c.0[0], [0.101, 0.19]);
        assert_eq!(c.0[1..], Corners::default().0[1..]);
        // A corner stops at the edge of its range.
        nudge(&mut c, Some(2), vec2(5.0, 5.0), size, 0.0..=1.0);
        assert_eq!(c.0[2], [1.0, 1.0]);
        // A whole area moves together and stops at the display's edge.
        let mut a = Corners::rect(0.5, 0.0, 0.5, 1.0);
        nudge(&mut a, None, vec2(-5.0, 0.0), size, 0.0..=1.0);
        let r = a.as_rect().expect("still a rectangle");
        assert!(
            (r[0] - 0.495).abs() < 1e-6 && (r[2] - 0.5).abs() < 1e-6,
            "{r:?}"
        );
        nudge(&mut a, None, vec2(0.0, 1.0), size, 0.0..=1.0);
        assert_eq!(a.bounds()[1], 0.0, "already at the bottom");
    }

    /// Run `keyboard` for one frame of key presses.
    fn keys(st: &UiState, keys: &[(Key, egui::Modifiers)]) -> Vec<Action> {
        let ctx = egui::Context::default();
        let events = keys
            .iter()
            .map(|&(key, modifiers)| Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            })
            .collect();
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut actions = Vec::new();
        let mut out = ctx.run_ui(input, |ui| keyboard(ui.ctx(), st, &mut actions));
        out.textures_delta.clear();
        actions
    }

    #[test]
    fn arrow_keys_move_a_selected_corner_instead_of_the_cue_selection() {
        use egui::Modifiers as M;
        let mut st = UiState {
            geometry_edit: Some(OutputId(1)),
            ..Default::default()
        };
        let presses = [
            (Key::ArrowRight, M::NONE),
            (Key::ArrowDown, M::SHIFT),
            (Key::ArrowUp, M::COMMAND),
        ];
        // Nothing selected: Up/Down pick cues, Ctrl+Up moves the cue.
        let a = keys(&st, &presses);
        assert!(matches!(
            a[..],
            [Action::Move(-1), Action::SelectRelative(1)]
        ));

        st.handle_sel.set(Some(HandleSel {
            quad: Quad::Pin(OutputId(1)),
            corner: Some(0),
        }));
        let a = keys(&st, &presses);
        assert!(
            matches!(a[..], [Action::Nudge(d), Action::Move(-1)] if d == vec2(1.0, 5.0)),
            "{} actions",
            a.len()
        );

        // Once its editor is closed, the arrows pick cues again.
        st.geometry_edit = None;
        let a = keys(&st, &[(Key::ArrowDown, M::NONE)]);
        assert!(matches!(a[..], [Action::SelectRelative(1)]));
    }

    #[test]
    fn clicking_a_handle_selects_it_and_clicking_elsewhere_deselects() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(pos2(100.0, 100.0), vec2(320.0, 180.0));
        let mut area = Corners::rect(0.0, 0.0, 0.5, 1.0);
        let sel = Cell::new(None);
        let mut t = 0.0;
        let mut click = |at: Pos2| {
            for events in [
                vec![Event::PointerMoved(at)],
                vec![button(at, true)],
                vec![button(at, false)],
                vec![],
            ] {
                t += 1.0 / 60.0;
                let input = RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
                    time: Some(t),
                    events,
                    ..Default::default()
                };
                let mut out = ctx.run_ui(input, |ui| {
                    area_handles(ui, screen, SubDisplayId(7), &mut area, &sel)
                });
                out.textures_delta.clear();
            }
        };
        let q = Quad::Area(SubDisplayId(7));
        // The top-right corner of the left half.
        click(pos2(260.0, 100.0));
        assert_eq!(
            sel.get(),
            Some(HandleSel {
                quad: q,
                corner: Some(1)
            })
        );
        // Inside the area: all of it.
        click(pos2(180.0, 190.0));
        assert_eq!(
            sel.get(),
            Some(HandleSel {
                quad: q,
                corner: None
            })
        );
        // Outside it.
        click(pos2(380.0, 190.0));
        assert_eq!(sel.get(), None);
        assert_eq!(
            area.as_rect(),
            Some([0.0, 0.0, 0.5, 1.0]),
            "clicks move nothing"
        );
    }

    /// A show of `a`, a group `G` holding `b` and `c`, and `d`.
    fn grouped_show() -> Show {
        let mut show = Show::default();
        for n in ["a", "b", "c", "d"] {
            let mut c = show.make_cue(CueType::Wait, None, show.cues.len());
            c.name = n.into();
            show.cues.push(c);
        }
        let g = show.group_cue(1).unwrap();
        show.move_into(3, g);
        show
    }

    /// Click `at` on a fresh cue list and return what it asked for.
    fn click_list(show: &Show, at: Pos2) -> Vec<Action> {
        let ctx = egui::Context::default();
        let mut engine = Engine::for_tests();
        let mut st = UiState::default();
        let mut actions = Vec::new();
        let mut t = 0.0;
        for events in [
            vec![],
            vec![Event::PointerMoved(at)],
            vec![button(at, true)],
            vec![button(at, false)],
            vec![],
        ] {
            t += 1.0 / 60.0;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 400.0))),
                time: Some(t),
                events,
                ..Default::default()
            };
            actions.clear();
            let mut out = ctx.run_ui(input, |ui| {
                cue_list(ui, show, &mut engine, &mut st, &mut actions)
            });
            out.textures_delta.clear();
            if !actions.is_empty() {
                return actions;
            }
        }
        actions
    }

    #[test]
    fn the_fold_arrow_toggles_a_group_and_the_rest_of_the_row_selects_it() {
        let show = grouped_show();
        let group = show.cues[1].id;
        // Scan the name column for the arrow, and a plain part of the row for selecting.
        let mut toggles = Vec::new();
        let mut selects = Vec::new();
        for y in (0..170).step_by(4) {
            for x in (100..150).step_by(2) {
                let at = pos2(x as f32, y as f32);
                let actions = click_list(&show, at);
                let toggled = actions
                    .iter()
                    .any(|a| matches!(a, Action::ToggleGroup(id) if *id == group));
                if toggled {
                    assert!(
                        !actions.iter().any(|a| matches!(a, Action::Select(_))),
                        "the arrow also selected the row at {at:?}"
                    );
                    toggles.push(at);
                }
            }
            selects.push((
                pos2(300.0, y as f32),
                click_list(&show, pos2(300.0, y as f32)),
            ));
        }
        assert!(!toggles.is_empty(), "no click toggled the group");
        let (x0, x1) = toggles.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
            (lo.min(p.x), hi.max(p.x))
        });
        let (y0, y1) = toggles.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
            (lo.min(p.y), hi.max(p.y))
        });
        assert!(
            x1 - x0 < FOLD_W + 3.0 && y1 - y0 < ROW_H,
            "{x0}..{x1} {y0}..{y1}"
        );
        // Clicking the group's name, in the same row, selects the group instead.
        let name = pos2(x1 + FOLD_W + 20.0, (y0 + y1) / 2.0);
        let actions = click_list(&show, name);
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::Select(id) if *id == group)),
            "no select at {name:?}"
        );
        // All four visible kinds of row can be selected: a, the group, b, c and d.
        let mut ids: Vec<CueId> = selects
            .iter()
            .flat_map(|(_, a)| a.iter())
            .filter_map(|a| match a {
                Action::Select(id) => Some(*id),
                _ => None,
            })
            .collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 5, "every row should be clickable: {ids:?}");
    }

    #[test]
    fn collapsed_children_are_not_listed() {
        let mut show = grouped_show();
        let kids: Vec<CueId> = show
            .children(show.cues[1].id)
            .iter()
            .map(|c| c.id)
            .collect();
        if let CueKind::Group(g) = &mut show.cues[1].kind {
            g.collapsed = true;
        }
        let mut seen = std::collections::HashSet::new();
        for y in (0..160).step_by(4) {
            for a in click_list(&show, pos2(200.0, y as f32)) {
                if let Action::Select(id) = a {
                    seen.insert(id);
                }
            }
        }
        assert!(kids.iter().all(|k| !seen.contains(k)), "{seen:?}");
        assert_eq!(seen.len(), 3, "a, the group and d remain");
    }

    fn running() -> (Show, Engine, UiState) {
        let show = grouped_show();
        let mut engine = Engine::for_tests();
        let mut st = UiState::default();
        engine.playhead = Some(show.cues[0].id);
        st.selected = engine.playhead;
        (show, engine, st)
    }

    fn number(show: &Show, n: &str) -> CueId {
        show.cues.iter().find(|c| c.number == n).unwrap().id
    }

    fn is_playing(e: &Engine, id: CueId) -> bool {
        matches!(e.status(id), CueStatus::Playing { .. })
    }

    #[test]
    fn remote_fire_and_stop_work_by_cue_number() {
        let (mut show, mut e, mut st) = running();
        let d = number(&show, "4");
        run_remote(Command::Fire("4".into()), &mut show, &mut e, &mut st);
        assert!(is_playing(&e, d));
        assert_eq!(
            e.playhead,
            Some(show.cues[0].id),
            "firing leaves the playhead"
        );
        run_remote(Command::Stop("4".into()), &mut show, &mut e, &mut st);
        assert!(!is_playing(&e, d));
        run_remote(Command::Fire(" 4 ".into()), &mut show, &mut e, &mut st);
        assert!(is_playing(&e, d), "stray spaces in the number are fine");
        run_remote(Command::StopAll, &mut show, &mut e, &mut st);
        assert!(!e.is_active());
    }

    #[test]
    fn remote_go_and_go_cue() {
        let (mut show, mut e, mut st) = running();
        let (a, d) = (number(&show, "1"), number(&show, "4"));
        run_remote(Command::Go, &mut show, &mut e, &mut st);
        assert!(is_playing(&e, a));
        assert_eq!(st.selected, e.playhead, "the list follows the playhead");
        // GO on a cue by number selects it, fires it, and moves on.
        run_remote(Command::GoCue("4".into()), &mut show, &mut e, &mut st);
        assert!(is_playing(&e, d));
        assert_eq!(e.playhead, None, "cue 4 is last");
        // Cue 2 is inside a group: GO fires the whole group.
        let (mut show, mut e, mut st) = running();
        let group = show.cues[1].id;
        run_remote(Command::GoCue("2".into()), &mut show, &mut e, &mut st);
        assert!(is_playing(&e, group));
    }

    #[test]
    fn remote_playhead_moves_over_whole_groups() {
        let (mut show, mut e, mut st) = running();
        let (group, d) = (show.cues[1].id, number(&show, "4"));
        run_remote(Command::PlayheadNext, &mut show, &mut e, &mut st);
        assert_eq!(e.playhead, Some(group));
        run_remote(Command::PlayheadNext, &mut show, &mut e, &mut st);
        assert_eq!(e.playhead, Some(d), "children are skipped");
        run_remote(Command::PlayheadNext, &mut show, &mut e, &mut st);
        assert_eq!(e.playhead, Some(d), "the end of the list stops it");
        run_remote(Command::PlayheadPrevious, &mut show, &mut e, &mut st);
        assert_eq!(e.playhead, Some(group));
        assert_eq!(st.selected, Some(group));
        run_remote(Command::Select("1".into()), &mut show, &mut e, &mut st);
        assert_eq!(e.playhead, Some(number(&show, "1")));
        run_remote(Command::PlayheadPrevious, &mut show, &mut e, &mut st);
        assert_eq!(e.playhead, Some(number(&show, "1")), "the start stops it");
    }

    #[test]
    fn remote_pause_resume_and_unknown_cues() {
        let (mut show, mut e, mut st) = running();
        run_remote(Command::Fire("1".into()), &mut show, &mut e, &mut st);
        run_remote(Command::Pause, &mut show, &mut e, &mut st);
        assert!(e.paused);
        run_remote(Command::Resume, &mut show, &mut e, &mut st);
        assert!(!e.paused);
        let before = e.active.len();
        run_remote(Command::Fire("99".into()), &mut show, &mut e, &mut st);
        run_remote(Command::Select("nope".into()), &mut show, &mut e, &mut st);
        assert_eq!(e.active.len(), before);
        assert!(e.messages.back().unwrap().1.contains("nope"));
    }
}
