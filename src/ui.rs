//! The control window UI: transport, cue list, inspector, active cues and outputs.

use crate::clock::{self, TimeOfDay};
use crate::engine::{CueStatus, Engine};
use crate::gpu::{Geometry, Homography, Slot};
use crate::history::History;
use crate::media::waveform::WaveState;
use crate::media::{ProbeState, camera};
use crate::recovery;
use crate::relink;
use crate::remote::{Command, RemoteStatus};
use crate::render::{OutputGroup, OutputLayer, OutputPreview};
use crate::show::*;
use egui::{Align, Color32, CornerRadius, Key, Layout, RichText, Sense, Stroke, Ui, Vec2};
use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};

pub struct MonitorInfo {
    /// What shows keep to find the display again (the system's name for it).
    pub name: String,
    /// What the display is, if the system says ("Built-in Retina Display").
    pub model: Option<String>,
    pub size: (u32, u32),
    pub refresh_hz: Option<f32>,
    pub primary: bool,
}

impl MonitorInfo {
    /// What to call the display on screen: what it is, else the system's name.
    pub fn title(&self) -> &str {
        self.model.as_deref().unwrap_or(&self.name)
    }
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

/// The scrub bar under the preview: where the bar was dragged to, kept on
/// screen until the cue has caught up with it.
#[derive(Clone, Copy)]
struct Scrub {
    instance: u64,
    pos: f64,
    dragging: bool,
    /// When a seek was last asked for, and when the bar was let go.
    sent: f64,
    released: f64,
}

/// How often a seek is asked for while the bar is dragged (seconds).
const SCRUB_EVERY: f64 = 0.2;
/// How long the bar stays where it was let go, waiting for the picture to arrive.
const SCRUB_HOLD: f64 = 1.0;

/// What a file dialog that is open is for, and so what its answer does.
#[derive(Clone, Debug, PartialEq)]
pub enum DialogFor {
    OpenShow,
    SaveShow,
    /// A file for a new cue of this type.
    AddCue(CueType),
    /// A different file for this cue.
    BrowseFile(CueId, CueType),
    /// Where this cue's missing file is now.
    LocateFile(CueId, CueType),
    ExportCueSheet,
    /// A folder to search for missing files.
    RelinkFolder,
    /// A folder to copy the show's media into.
    CollectMedia,
}

impl DialogFor {
    /// What the answer `path` asks for. `changed` is whether the show was
    /// edited while the dialog was open.
    fn answer(self, path: PathBuf, changed: bool) -> Action {
        match self {
            DialogFor::OpenShow => Action::OpenShowFile(path, changed),
            DialogFor::SaveShow => Action::SaveShowTo(path),
            DialogFor::AddCue(ty) => Action::AddCueFile(ty, path),
            DialogFor::BrowseFile(id, _) => Action::SetCueFile(id, path),
            DialogFor::LocateFile(id, _) => Action::RelinkCue(id, path),
            DialogFor::ExportCueSheet => Action::ExportCueSheet(path),
            DialogFor::RelinkFolder => Action::RelinkFolder(path),
            DialogFor::CollectMedia => Action::CollectMedia(path),
        }
    }
}

/// A save being written on a thread of its own: serialising the show and
/// flushing it to the disk can take tens of milliseconds, a dropped frame on
/// every output if it happens on the main thread.
struct SaveJob {
    path: PathBuf,
    done: std::sync::mpsc::Receiver<Result<(), String>>,
    /// The show as it was saved, which is what it is "unchanged" against.
    snapshot: Option<String>,
}

/// A file or folder dialog that is open on a thread of its own, so the show
/// keeps running behind it: a dialog on the event loop's thread would freeze
/// every fade, wait and picture until it was closed.
struct OpenDialog {
    purpose: DialogFor,
    answer: std::sync::mpsc::Receiver<Option<PathBuf>>,
    /// The show as it was when the dialog opened.
    shown: String,
}

/// Copying a show's media into a folder, on its own thread.
struct CollectJob {
    done: std::sync::mpsc::Receiver<relink::Collected>,
    progress: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    total: usize,
    dest: PathBuf,
}

#[derive(Default)]
pub struct UiState {
    pub selected: Option<CueId>,
    pub show_path: Option<PathBuf>,
    pub monitors: Vec<MonitorInfo>,
    /// Every enabled output, previewed in the main window (set each frame by the app).
    pub previews: Vec<PreviewOutput>,
    pub routes: HashMap<OutputId, OutputRoute>,
    /// The output shown in the main window's preview (the first when unset
    /// or gone).
    pub preview_output: Option<OutputId>,
    /// A scrub bar being dragged, or just released.
    scrub: Option<Scrub>,
    /// Output whose corner pin is being edited on its preview.
    pub geometry_edit: Option<OutputId>,
    /// Sub display whose area is being edited.
    pub area_edit: Option<SubDisplayId>,
    /// What each fullscreen window shows, by monitor index: its size and outputs.
    pub screens: HashMap<usize, ((u32, u32), Vec<OutputGroup>)>,
    /// The monitor each connected sub display is on.
    pub sub_monitors: HashMap<SubDisplayId, usize>,
    /// What each of the show's displays is attached to right now.
    pub display_states: HashMap<DisplayId, SlotState>,
    /// The corner (or whole area) last clicked in an open editor, which the
    /// arrow keys move. Set while drawing, hence the `Cell`.
    handle_sel: Cell<Option<HandleSel>>,
    pub quit_requested: bool,
    /// MIDI inputs that are connected now (set by the app every few seconds).
    pub midi_ports: Vec<String>,
    /// MIDI outputs that are connected now (set with the inputs).
    pub midi_out_ports: Vec<String>,
    /// Audio output devices that are connected now (set by the app when idle).
    pub audio_devices: Vec<String>,
    /// What the OSC and MIDI listeners are doing (set by the app).
    pub remote_status: RemoteStatus,
    pub dirty: bool,
    /// Whether the cart (buttons for the cues put on it) is showing.
    pub show_cart: bool,
    /// Whether the media window is open, and what it last did.
    pub show_media: bool,
    /// Whether the window for lining sound up with picture is open, and the
    /// clip being made for it.
    pub show_sync: bool,
    sync_clip: Option<std::sync::mpsc::Receiver<Result<PathBuf, String>>>,
    media_note: String,
    sync_note: String,
    /// Media being copied into one folder, in the background.
    collect_job: Option<CollectJob>,
    /// A search of this computer for the missing files.
    search: Option<relink::Search>,
    /// A search of a folder the operator chose, for the missing files.
    folder_search: Option<relink::FolderSearch>,
    /// The file dialog that is open, if any.
    dialog: Option<OpenDialog>,
    /// A save being written, if any.
    saving: Option<SaveJob>,
    /// The show as last serialised, for the unsaved-changes mark and the undo
    /// history, and when (egui time). It is redone when something may have
    /// changed it, not every frame: serialising a big show is over a
    /// millisecond.
    json: Option<String>,
    json_at: f64,
    /// Something outside `draw` (a remote command) changed the show.
    json_stale: bool,
    /// The media files that aren't there, checked once a second on a thread of
    /// its own (a stat of a file on a network share that has gone can take
    /// seconds, and the cue list must not wait for it), and when that was
    /// asked, and the check being made.
    missing: HashSet<PathBuf>,
    missing_asked: Option<f64>,
    missing_check: Option<std::sync::mpsc::Receiver<HashSet<PathBuf>>>,
    /// The cue waiting for the next key press to become its hotkey.
    pub capture_hotkey: Option<CueId>,
    /// Serialized show at last save/load, for unsaved-change detection.
    saved_json: Option<String>,
    /// What can be undone and redone.
    pub history: History,
    scroll_to_selected: bool,
}

impl UiState {
    /// Open the media window if the show has files that can't be found.
    pub fn check_media(&mut self, show: &Show) {
        if !relink::missing(show).is_empty() {
            self.show_media = true;
        }
    }

    /// Treat `show` as the one on disk, for the unsaved-changes mark (used
    /// when a recovered copy is opened over the saved show).
    pub fn set_saved(&mut self, show: &Show) {
        self.saved_json = serde_json::to_string(show).ok();
    }

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
    /// Fire a cue (from its hotkey or the cart) without moving the playhead.
    FireCue(CueId),
    StopCue(CueId),
    FadeOutCue(CueId),
    ToggleCart,
    ToggleMedia,
    ToggleSync,
    /// Make the clip for checking the sync (about a second of work).
    MakeSyncClip,
    /// A cue for the sync clip, looping.
    AddSyncCue(PathBuf),
    /// Write the cue sheet (CSV or a web page, by the file's extension).
    ExportCueSheet(PathBuf),
    /// Look for the missing files in this folder.
    RelinkFolder(PathBuf),
    /// Point the cues that used this cue's file at another file.
    RelinkCue(CueId, PathBuf),
    /// Copy all the media into this folder.
    CollectMedia(PathBuf),
    /// Search this computer for the missing files by name.
    FindOnComputer,
    /// Take suggested replacements, as (file it was, file to use).
    RelinkMany(Vec<(PathBuf, PathBuf)>),
    /// Wait for a key press to use as this cue's hotkey (None: stop waiting).
    CaptureHotkey(Option<CueId>),
    SetHotkey(CueId, Option<String>),
    SetMidiNote(CueId, Option<u8>),
    StopSelected,
    FadeOutSelected,
    StopInstance(u64, f64),
    FadeOutInstance(u64),
    /// Jump a playing cue (by instance) to this many seconds from its start.
    Seek(u64, f64),
    /// Open a file dialog (if none is open); its answer comes back as one of
    /// the actions below.
    AskFile(DialogFor),
    /// The show chosen to open, and whether the show was edited meanwhile.
    OpenShowFile(PathBuf, bool),
    SaveShowTo(PathBuf),
    /// A new cue of this type, for this file.
    AddCueFile(CueType, PathBuf),
    /// This cue now plays this file.
    SetCueFile(CueId, PathBuf),
    EditGeometry(Option<OutputId>),
    EditArea(Option<SubDisplayId>),
    /// Move the selected handle by this many screen pixels.
    Nudge(Vec2),
    AddSubDisplay(Option<DisplayId>),
    AddDisplay,
    RemoveDisplay(DisplayId),
    RemoveSubDisplay(SubDisplayId),
    SetTransition(CueId, Transition),
    /// Arm or disarm a cue.
    SetArmed(CueId, bool),
    /// Flip the selected cue between armed and disarmed.
    ToggleArmed,
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
    AddAudioOutput,
    RemoveAudioOutput(AudioOutputId),
    Undo,
    Redo,
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
    if !st.history.started()
        && let Ok(json) = serde_json::to_string(show)
    {
        st.history.reset(json);
    }
    let mut actions: Vec<Action> = Vec::new();
    poll_collect(ui.ctx(), show, st);
    poll_folder_search(ui.ctx(), show, st);
    poll_dialog(ui.ctx(), show, st, &mut actions);
    poll_save(ui.ctx(), engine, st);
    refresh_missing(ui.ctx(), show, st);
    keyboard(ui.ctx(), show, st, &mut actions);

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
    egui::Panel::bottom("status").show(ui, |ui| status_bar(ui, show, engine, st));
    egui::Panel::right("side")
        .resizable(true)
        .default_size(420.0)
        .min_size(280.0)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    output_previews(ui, show, engine, st, &mut actions);
                    active_panel(ui, show, engine, &mut actions);
                    ui.add_space(12.0);
                    displays_panel(ui, show, st, &mut actions);
                    ui.add_space(12.0);
                    outputs_panel(ui, show, st, &mut actions);
                    ui.add_space(12.0);
                    audio_panel(ui, show, engine, st, &mut actions);
                    ui.add_space(12.0);
                    sub_displays_panel(ui, show, st, &mut actions);
                    ui.add_space(12.0);
                    remote_panel(ui, show, st, engine);
                });
        });
    if st.show_cart {
        egui::Panel::bottom("cart")
            .resizable(true)
            .default_size(150.0)
            .min_size(80.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| cart(ui, show, engine, &mut actions));
            });
    }
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
    media_window(ui.ctx(), show, st, &mut actions);
    sync_window(ui.ctx(), show, st, &mut actions);

    // Actions change state after this frame was laid out; show the result now
    // rather than on the next mouse move.
    let acted = !actions.is_empty();
    if acted {
        ui.ctx().request_repaint();
        // What an action changes is a step of its own, not part of the edit before.
        st.history.end_burst();
    }
    for a in actions {
        apply(a, show, engine, st);
    }

    let (now, input) = ui.ctx().input(|i| (i.time, editing_input(i)));
    if json_due(
        st.json.is_some(),
        st.json_stale || acted,
        input,
        now - st.json_at,
    ) {
        st.json = serde_json::to_string(show).ok();
        st.json_at = now;
        st.json_stale = false;
    }
    st.dirty = st.json != st.saved_json;
    if let Some(json) = st.json.take() {
        track_history(st, &json, now, acted);
        st.json = Some(json);
    }
}

/// How long the show may go without being looked at for changes (seconds).
const JSON_EVERY: f64 = 0.25;

/// Whether the show should be serialised again now: if it never has been, if
/// something is known to have changed it, if the operator is doing something
/// that can (anything but moving the mouse about), or, so that nothing is
/// missed, every so often.
fn json_due(have: bool, changed: bool, input: bool, age: f64) -> bool {
    !have || changed || input || age >= JSON_EVERY
}

/// Whether this frame has input that can edit the show: a button held (a
/// slider or a drag), a key, text, a click. Just moving the mouse doesn't.
fn editing_input(i: &egui::InputState) -> bool {
    i.pointer.any_down()
        || i.events.iter().any(|e| {
            !matches!(
                e,
                egui::Event::PointerMoved(_)
                    | egui::Event::MouseMoved(_)
                    | egui::Event::PointerGone
            )
        })
}

/// Tell the undo history what the show looks like after this frame.
fn track_history(st: &mut UiState, json: &str, now: f64, acted: bool) {
    st.history.observe(json, now);
    if acted {
        st.history.end_burst();
    }
}

/// Switch to a show from the undo history, keeping the selection and the
/// playhead on cues that still exist.
fn restore(json: &str, show: &mut Show, engine: &mut Engine, st: &mut UiState) {
    match serde_json::from_str::<Show>(json) {
        Ok(back) => {
            *show = back;
            if engine.playhead.is_some_and(|id| show.cue(id).is_none()) {
                engine.playhead = show.cues.first().map(|c| c.id);
            }
            if st.selected.is_some_and(|id| show.cue(id).is_none()) {
                st.selected = engine.playhead;
            }
            st.capture_hotkey = None;
        }
        Err(e) => engine.log(format!("Could not restore that step: {e}")),
    }
}

/// Keys a cue can take as its hotkey: letters (except the transport's P and
/// A), digits and function keys. Everything else already means something.
pub fn hotkey_allowed(key: Key) -> bool {
    let name = key.name();
    let letter = name.len() == 1 && name.chars().all(|c| c.is_ascii_uppercase());
    let digit = name.len() == 1 && name.chars().all(|c| c.is_ascii_digit());
    let function = key
        .name()
        .strip_prefix('F')
        .is_some_and(|n| n.parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)));
    (letter && !matches!(key, Key::P | Key::A)) || digit || function
}

/// Take a key press, acting only on a fresh one. The repeats of a key held
/// down are taken too, but ignored, so holding Space doesn't GO again and
/// again and a held Esc can't turn the panic fade into an instant stop.
fn fresh_key(i: &mut egui::InputState, modifiers: egui::Modifiers, key: Key) -> bool {
    let fresh = i.events.iter().any(|e| {
        matches!(e, egui::Event::Key { key: k, pressed: true, repeat: false, modifiers: m, .. }
            if *k == key && m.matches_logically(modifiers))
    });
    i.consume_key(modifiers, key) && fresh
}

fn keyboard(ctx: &egui::Context, show: &Show, st: &UiState, actions: &mut Vec<Action>) {
    if let Some(id) = st.capture_hotkey {
        // Waiting for the key to bind: nothing else reacts meanwhile.
        ctx.input_mut(|i| {
            let pressed = i.events.iter().find_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if !modifiers.command && !modifiers.alt => Some(*key),
                _ => None,
            });
            match pressed {
                Some(Key::Escape) => actions.push(Action::CaptureHotkey(None)),
                Some(key) if hotkey_allowed(key) => {
                    actions.push(Action::SetHotkey(id, Some(key.name().to_string())));
                }
                _ => {}
            }
            if pressed.is_some() {
                i.events.clear();
            }
        });
        return;
    }
    let typing = ctx.egui_wants_keyboard_input();
    let nudging = st.selected_handle().is_some();
    ctx.input_mut(|i| {
        if fresh_key(i, egui::Modifiers::NONE, Key::Escape) {
            actions.push(Action::Panic);
        }
        // Before Ctrl+S, which also matches with Shift held.
        if fresh_key(i, egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::S) {
            actions.push(Action::SaveAs);
        }
        if fresh_key(i, egui::Modifiers::COMMAND, Key::S) {
            actions.push(Action::Save);
        }
        if fresh_key(i, egui::Modifiers::COMMAND, Key::O) {
            actions.push(Action::Open);
        }
        if fresh_key(i, egui::Modifiers::COMMAND, Key::N) {
            actions.push(Action::New);
        }
        if typing {
            // A text box has its own undo.
            return;
        }
        // Before Ctrl+Z, which also matches with Shift held.
        if i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::Z)
            || i.consume_key(egui::Modifiers::COMMAND, Key::Y)
        {
            actions.push(Action::Redo);
        }
        if i.consume_key(egui::Modifiers::COMMAND, Key::Z) {
            actions.push(Action::Undo);
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
        // Hotkeys fire their cue wherever the playhead is.
        if show.cues.iter().any(|c| c.hotkey.is_some()) {
            for key in i
                .events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key {
                        key,
                        pressed: true,
                        repeat: false,
                        modifiers,
                        ..
                    } if !modifiers.command && !modifiers.alt => Some(*key),
                    _ => None,
                })
                .collect::<Vec<_>>()
            {
                if let Some(id) = show.cue_for_hotkey(key.name()) {
                    i.consume_key(egui::Modifiers::NONE, key);
                    i.consume_key(egui::Modifiers::SHIFT, key);
                    actions.push(Action::FireCue(id));
                }
            }
        }
        if fresh_key(i, egui::Modifiers::NONE, Key::Space) {
            actions.push(Action::Go);
        }
        if fresh_key(i, egui::Modifiers::NONE, Key::P) {
            actions.push(Action::TogglePause);
        }
        if fresh_key(i, egui::Modifiers::NONE, Key::A) {
            actions.push(Action::ToggleArmed);
        }
        if fresh_key(i, egui::Modifiers::COMMAND, Key::D) {
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
        if fresh_key(i, egui::Modifiers::NONE, Key::Delete)
            || fresh_key(i, egui::Modifiers::COMMAND, Key::Backspace)
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
            if ui.button("Media…").clicked() {
                actions.push(Action::ToggleMedia);
            }
            if ui
                .button("Sync…")
                .on_hover_text("Line the sound up with the picture.")
                .clicked()
            {
                actions.push(Action::ToggleSync);
            }
            if ui
                .button("Export cue sheet…")
                .on_hover_text(
                    "The cue list as a web page to print, or as a CSV for a spreadsheet.",
                )
                .clicked()
            {
                actions.push(Action::AskFile(DialogFor::ExportCueSheet));
            }
            ui.separator();
            if ui.button("Quit").clicked() {
                actions.push(Action::Quit);
            }
        });
        ui.menu_button("Edit", |ui| {
            if ui
                .add_enabled(
                    st.history.can_undo(),
                    egui::Button::new("Undo").shortcut_text("Ctrl+Z"),
                )
                .clicked()
            {
                actions.push(Action::Undo);
            }
            if ui
                .add_enabled(
                    st.history.can_redo(),
                    egui::Button::new("Redo").shortcut_text("Ctrl+Y"),
                )
                .clicked()
            {
                actions.push(Action::Redo);
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
                CueType::Camera,
                CueType::Send,
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
            ui.separator();
            if ui
                .button(if st.show_cart {
                    "Hide cart"
                } else {
                    "Show cart"
                })
                .clicked()
            {
                actions.push(Action::ToggleCart);
            }
            if let Some(id) = st.selected
                && let Some(c) = show.cue(id)
            {
                if ui
                    .add(
                        egui::Button::new(if c.armed { "Disarm" } else { "Arm" })
                            .shortcut_text("A"),
                    )
                    .clicked()
                {
                    actions.push(Action::SetArmed(id, !c.armed));
                }
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
            if ui.button("Add display").clicked() {
                actions.push(Action::AddDisplay);
            }
            ui.menu_button("Add sub display on", |ui| {
                add_sub_display_menu(ui, show, actions);
            });
        });
        ui.menu_button("Help", |ui| {
            ui.label(RichText::new("Keyboard").strong());
            for (k, d) in [
                ("Space", "GO"),
                ("Esc", "Fade out all (press twice: stop now)"),
                ("P", "Pause / resume"),
                ("A", "Arm / disarm the selected cue"),
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
            // The cue GO will fire: disarmed cues at the playhead are passed over.
            let at_playhead = engine.playhead.and_then(|id| show.cue_index(id));
            let next = at_playhead
                .and_then(|i| show.armed_top_from(i))
                .map(|i| &show.cues[i]);
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
                None if at_playhead.is_some() => {
                    ui.label(RichText::new("Nothing armed").size(24.0).weak());
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

fn status_bar(ui: &mut Ui, show: &Show, engine: &Engine, st: &UiState) {
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
        match engine.audio.summary() {
            Ok(text) => ui.label(format!("🔊 {text}")),
            Err(e) => ui.label(RichText::new(format!("🔇 {e}")).color(STOP_RED)),
        };
        // The next cue set for a time of day, if there is one.
        let timed: Vec<_> = show
            .cues
            .iter()
            .filter(|c| c.armed)
            .filter_map(|c| c.clock.map(|t| (c.id, t)))
            .collect();
        if let Some((id, at, secs)) = clock::upcoming(&timed, clock::now())
            && let Some(c) = show.cue(id)
        {
            ui.label(
                RichText::new(format!("⏰ {at}  {}  in {}", c.name, fmt_wait(secs))).color(PREWAIT),
            )
            .on_hover_text("The next cue set to fire at a time of day");
        }
        if engine.timecode.running()
            && let Some((tc, rate)) = engine.timecode.position()
        {
            ui.label(RichText::new(format!("⏱ {tc} {}", rate.label())).color(GO_GREEN));
        }
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

pub(crate) fn cue_target_text(show: &Show, c: &Cue) -> String {
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
        CueKind::Send(s) => crate::output::describe(s),
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
        CueKind::Camera(cam) => {
            let device = if cam.device.is_empty() {
                "(no camera)".to_string()
            } else {
                device_label(&cam.device)
            };
            let out = cam
                .media
                .output
                .and_then(|o| show.output(o))
                .map(|o| o.name.as_str())
                .unwrap_or("no output");
            format!("{device}  »  {out}")
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

    // Only the rows in view are laid out and drawn: a show of thousands of cues
    // costs no more per frame than one of a few dozen.
    let visible = show.visible_indices();
    ui.spacing_mut().item_spacing.y = 1.0;
    let row_pitch = ROW_H + ui.spacing().item_spacing.y;
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, ROW_H, visible.len(), |ui, rows| {
            if st.scroll_to_selected {
                st.scroll_to_selected = false;
                // The row may not be drawn, so find where it would be.
                if let Some(r) = visible
                    .iter()
                    .position(|&i| Some(show.cues[i].id) == st.selected)
                {
                    let top = ui.max_rect().top() - rows.start as f32 * row_pitch;
                    let at = egui::pos2(ui.max_rect().left(), top + r as f32 * row_pitch);
                    ui.scroll_to_rect(egui::Rect::from_min_size(at, Vec2::new(total, ROW_H)), None);
                }
            }
            let groups: Vec<(CueId, String)> = show
                .cues
                .iter()
                .filter(|c| matches!(c.kind, CueKind::Group(_)))
                .map(|c| (c.id, cue_label(c)))
                .collect();
            for &idx in &visible[rows] {
                let c = &show.cues[idx];
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
                // A disarmed cue, or one in a disarmed group, is greyed out.
                let live = show.is_armed(idx);
                let text_color = if live {
                    v.strong_text_color()
                } else {
                    v.weak_text_color()
                };
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
                } else if marker.is_empty() && !live {
                    painter.circle_stroke(
                        egui::pos2(rect.left() + 12.0, y),
                        3.5,
                        Stroke::new(1.0, v.weak_text_color()),
                    );
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
                    cell_dx(3, FOLD_W, name_with_key(c), text_color);
                } else {
                    cell(3, name_with_key(c), text_color);
                }
                let weak = v.weak_text_color();
                let missing = c.kind.media().is_some_and(|m| {
                    !m.file.as_os_str().is_empty() && st.missing.contains(&m.file)
                });
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

/// A wait for the status bar: "42 s", "12 min", "3 h 05 min".
fn fmt_wait(secs: u32) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min {:02} s", secs / 60, secs % 60),
        _ => format!("{} h {:02} min", secs / 3600, secs % 3600 / 60),
    }
}

/// A cue's name, with its hotkey after it if it has one.
fn name_with_key(c: &Cue) -> String {
    let mut name = c.name.clone();
    if let Some(k) = &c.hotkey {
        name.push_str(&format!("   [{k}]"));
    }
    if let Some(t) = c.timecode {
        name.push_str(&format!("   @{t}"));
    }
    if let Some(t) = c.clock {
        name.push_str(&format!("   at {t}"));
    }
    name
}

/// The cart: one big button per cue put on it. A click fires the cue, a
/// right click stops it.
fn cart(ui: &mut Ui, show: &Show, engine: &Engine, actions: &mut Vec<Action>) {
    let on_cart: Vec<&Cue> = show.cues.iter().filter(|c| c.cart).collect();
    if on_cart.is_empty() {
        ui.label(
            RichText::new(
                "Nothing on the cart yet. Tick “On cart” in a cue's inspector to add a button for it.",
            )
            .weak(),
        );
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = Vec2::splat(8.0);
        for c in on_cart {
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(132.0, 66.0), Sense::click());
            let idx = show.cue_index(c.id).unwrap_or(0);
            let live = show.is_armed(idx);
            let status = engine.status(c.id);
            let v = ui.visuals();
            let painter = ui.painter_at(rect);
            let bg = if resp.hovered() {
                v.widgets.hovered.weak_bg_fill
            } else {
                v.widgets.inactive.weak_bg_fill
            };
            painter.rect_filled(rect, 6.0, bg);
            match status {
                CueStatus::Playing { elapsed, duration } => {
                    let frac = duration
                        .map(|d| (elapsed / d.max(1e-6)).clamp(0.0, 1.0))
                        .unwrap_or(1.0);
                    let mut r = rect;
                    r.set_width(rect.width() * frac as f32);
                    painter.rect_filled(r, 6.0, PLAYING);
                }
                CueStatus::PreWait { remaining, total } => {
                    let mut r = rect;
                    r.set_width(rect.width() * (1.0 - remaining / total.max(1e-6)) as f32);
                    painter.rect_filled(r, 6.0, PREWAIT);
                }
                CueStatus::Idle => {}
            }
            if engine.is_prepared(c.id) {
                painter.circle_filled(rect.right_top() + Vec2::new(-10.0, 10.0), 3.5, GO_GREEN);
            }
            let text = if live {
                v.strong_text_color()
            } else {
                v.weak_text_color()
            };
            painter.rect_stroke(
                rect,
                6.0,
                Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
                egui::StrokeKind::Inside,
            );
            let key = c.hotkey.clone().unwrap_or_default();
            painter.text(
                rect.left_top() + Vec2::new(8.0, 6.0),
                egui::Align2::LEFT_TOP,
                key,
                egui::FontId::proportional(13.0),
                PLAYHEAD,
            );
            painter.with_clip_rect(rect.shrink(4.0)).text(
                rect.center() + Vec2::new(0.0, 6.0),
                egui::Align2::CENTER_CENTER,
                &c.name,
                egui::FontId::proportional(15.0),
                text,
            );
            let hint = format!(
                "{} {}\nClick to fire · right-click for more",
                c.number, c.name
            );
            let resp = resp.on_hover_text(hint);
            if resp.clicked() {
                actions.push(Action::FireCue(c.id));
            }
            resp.context_menu(|ui| {
                if ui.button("Stop").clicked() {
                    actions.push(Action::StopCue(c.id));
                    ui.close();
                }
                if ui.button("Fade out").clicked() {
                    actions.push(Action::FadeOutCue(c.id));
                    ui.close();
                }
                if ui.button("Edit").clicked() {
                    actions.push(Action::Select(c.id));
                    ui.close();
                }
            });
        }
    });
}

/// Apply a finished copy of the media to the show's cues.
/// Open a file dialog on a thread of its own, answering with `ask` (which
/// blocks until the dialog is closed). One at a time: a request while one is
/// open is ignored.
fn start_dialog_with(
    st: &mut UiState,
    show: &Show,
    purpose: DialogFor,
    ask: impl FnOnce() -> Option<PathBuf> + Send + 'static,
) {
    if st.dialog.is_some() {
        return;
    }
    let (tx, answer) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("file-dialog".into())
        .spawn(move || {
            let _ = tx.send(ask());
        });
    if spawned.is_ok() {
        st.dialog = Some(OpenDialog {
            purpose,
            answer,
            shown: serde_json::to_string(show).unwrap_or_default(),
        });
    }
}

/// Open the dialog `purpose` asks for.
fn start_dialog(st: &mut UiState, show: &Show, purpose: DialogFor) {
    let name = show.name.clone();
    let show_dir = st
        .show_path
        .as_deref()
        .and_then(|p| p.parent())
        .map(PathBuf::from);
    let kind = purpose.clone();
    start_dialog_with(st, show, purpose, move || match kind {
        DialogFor::OpenShow => show_dialog().pick_file(),
        DialogFor::SaveShow => show_dialog()
            .set_file_name(format!("{name}.{SHOW_EXTENSION}"))
            .save_file()
            .map(|p| p.with_extension(SHOW_EXTENSION)),
        DialogFor::AddCue(ty) | DialogFor::BrowseFile(_, ty) | DialogFor::LocateFile(_, ty) => {
            media_dialog(ty).pick_file()
        }
        DialogFor::ExportCueSheet => rfd::FileDialog::new()
            .set_title("Export the cue sheet")
            .set_file_name(format!("{name} cue sheet.html"))
            .add_filter("Web page", &["html"])
            .add_filter("Spreadsheet (CSV)", &["csv"])
            .save_file(),
        DialogFor::RelinkFolder => rfd::FileDialog::new()
            .set_title("Where are the media files now?")
            .pick_folder(),
        DialogFor::CollectMedia => {
            let mut dialog = rfd::FileDialog::new().set_title("Copy the media into…");
            if let Some(dir) = show_dir {
                dialog = dialog.set_directory(dir);
            }
            dialog.pick_folder()
        }
    });
}

/// How often the media files are checked to be there (seconds).
const MISSING_EVERY: f64 = 1.0;

/// Keep `st.missing` up to date: take the answer of the check being made, and
/// start another once it is time. The check runs on a thread of its own.
fn refresh_missing(ctx: &egui::Context, show: &Show, st: &mut UiState) {
    let now = ctx.input(|i| i.time);
    if let Some(check) = &st.missing_check {
        match check.try_recv() {
            Ok(missing) => {
                st.missing = missing;
                st.missing_check = None;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
                return;
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => st.missing_check = None,
        }
    }
    if st.missing_check.is_some() || st.missing_asked.is_some_and(|t| now - t < MISSING_EVERY) {
        return;
    }
    st.missing_asked = Some(now);
    let files: HashSet<PathBuf> = show
        .cues
        .iter()
        .filter_map(|c| c.kind.media())
        .map(|m| m.file.clone())
        .filter(|f| !f.as_os_str().is_empty())
        .collect();
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("missing-check".into())
        .spawn(move || {
            let _ = tx.send(files.into_iter().filter(|f| !f.is_file()).collect());
        });
    if spawned.is_ok() {
        st.missing_check = Some(rx);
    }
}

/// Take the answer of the open file dialog when it has come.
fn poll_dialog(ctx: &egui::Context, show: &Show, st: &mut UiState, actions: &mut Vec<Action>) {
    let Some(dialog) = &st.dialog else { return };
    match dialog.answer.try_recv() {
        Ok(answer) => {
            let Some(dialog) = st.dialog.take() else {
                return;
            };
            if let Some(path) = answer {
                let changed = serde_json::to_string(show).is_ok_and(|now| now != dialog.shown);
                actions.push(dialog.purpose.answer(path, changed));
            }
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => st.dialog = None,
    }
}

/// Take the answer of a folder search when it has come, and relink with it.
fn poll_folder_search(ctx: &egui::Context, show: &mut Show, st: &mut UiState) {
    let Some(search) = &st.folder_search else {
        return;
    };
    let Some(result) = search.poll() else {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        return;
    };
    let (root, wanted) = (search.root.clone(), search.wanted);
    st.folder_search = None;
    // Its own step in the undo history, apart from any edit in progress.
    st.history.end_burst();
    relink::apply(show, &result.found);
    st.history.observe(
        &serde_json::to_string(show).unwrap_or_default(),
        ctx.input(|i| i.time),
    );
    st.history.end_burst();
    let mut note = format!(
        "Found {} of {wanted} missing files in {}.",
        result.found.len(),
        root.display()
    );
    if result.stopped {
        note.push_str(" The search was stopped.");
    } else if result.timed_out {
        note.push_str(" It stopped at the time limit before looking everywhere.");
    }
    st.media_note = note;
}

fn poll_collect(ctx: &egui::Context, show: &mut Show, st: &mut UiState) {
    let Some(job) = &st.collect_job else { return };
    match job.done.try_recv() {
        Ok(result) => {
            let dest = job.dest.clone();
            st.collect_job = None;
            // Its own step in the undo history, apart from any edit in progress.
            st.history.end_burst();
            let n = relink::apply(show, &result.moved);
            st.history.observe(
                &serde_json::to_string(show).unwrap_or_default(),
                ctx.input(|i| i.time),
            );
            st.history.end_burst();
            let mut note = format!(
                "Copied {} file{} into {}",
                result.copied,
                if result.copied == 1 { "" } else { "s" },
                dest.display()
            );
            if result.already_there > 0 {
                note.push_str(&format!(" ({} were already there)", result.already_there));
            }
            note.push_str(&format!(
                "; {n} cue{} now use the copies.",
                if n == 1 { "" } else { "s" }
            ));
            if let Some((file, why)) = result.failed.first() {
                note.push_str(&format!(
                    " {} could not be copied (first: {}: {why}).",
                    result.failed.len(),
                    file.display()
                ));
            }
            st.media_note = note;
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            st.collect_job = None;
            st.media_note = "Copying stopped unexpectedly.".into();
        }
    }
}

/// The media window: every file the show uses, which are missing, finding
/// them again, and copying them all into one folder.
/// The window for lining sound up with picture: how long to hold each back.
fn sync_window(ctx: &egui::Context, show: &mut Show, st: &mut UiState, actions: &mut Vec<Action>) {
    if !st.show_sync {
        return;
    }
    if let Some(rx) = &st.sync_clip {
        match rx.try_recv() {
            Ok(Ok(path)) => {
                st.sync_clip = None;
                actions.push(Action::AddSyncCue(path));
            }
            Ok(Err(e)) => {
                st.sync_clip = None;
                st.sync_note = format!("Couldn't make the test clip: {e}");
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => st.sync_clip = None,
        }
    }
    let delay = |ui: &mut Ui, ms: &mut u32| {
        ui.add(
            egui::DragValue::new(ms)
                .range(0..=MAX_DELAY_MS)
                .speed(1.0)
                .suffix(" ms"),
        )
        .changed()
    };
    let mut open = true;
    egui::Window::new("Sync")
        .open(&mut open)
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.label(
                "Hold back whichever comes first. If the picture is ahead of the sound, \
                 delay the picture; if the sound is ahead, delay the sound.",
            );
            egui::Grid::new("sync_all").num_columns(2).show(ui, |ui| {
                ui.label("Sound delay");
                delay(ui, &mut show.sync.audio_ms);
                ui.end_row();
                ui.label("Picture delay");
                delay(ui, &mut show.sync.video_ms);
                ui.end_row();
            });
            ui.label(
                RichText::new("These apply to every output. A delay changes cues as they start.")
                    .weak()
                    .small(),
            );
            if !show.audio_outputs.is_empty() || !show.outputs.is_empty() {
                ui.separator();
                ui.label("On top of that, for one output:");
                egui::Grid::new("sync_outputs")
                    .num_columns(2)
                    .show(ui, |ui| {
                        for o in &mut show.audio_outputs {
                            ui.label(format!("Sound: {}", o.name));
                            delay(ui, &mut o.delay_ms);
                            ui.end_row();
                        }
                        for o in &mut show.outputs {
                            ui.label(format!("Picture: {}", o.name));
                            delay(ui, &mut o.delay_ms);
                            ui.end_row();
                        }
                    });
            }
            ui.separator();
            ui.horizontal(|ui| {
                let making = st.sync_clip.is_some();
                if ui
                    .add_enabled(!making, egui::Button::new("Add a sync test cue"))
                    .on_hover_text(
                        "A looping clip that flashes white and beeps together once a second. \
                         Play it and adjust until the flash and the beep land together.",
                    )
                    .clicked()
                {
                    actions.push(Action::MakeSyncClip);
                }
                if making {
                    ui.spinner();
                }
                if ui.button("Reset").clicked() {
                    show.sync = SyncSettings::default();
                    for o in &mut show.audio_outputs {
                        o.delay_ms = 0;
                    }
                    for o in &mut show.outputs {
                        o.delay_ms = 0;
                    }
                }
            });
            if !st.sync_note.is_empty() {
                ui.label(RichText::new(&st.sync_note).color(STOP_RED).small());
            }
        });
    st.show_sync = open;
}

fn media_window(ctx: &egui::Context, show: &Show, st: &mut UiState, actions: &mut Vec<Action>) {
    if !st.show_media {
        return;
    }
    let mut open = true;
    egui::Window::new("Media")
        .open(&mut open)
        .default_width(620.0)
        .default_height(360.0)
        .show(ctx, |ui| {
            let files: Vec<(&Cue, &MediaCue)> = show
                .cues
                .iter()
                .filter_map(|c| c.kind.file_media().map(|m| (c, m)))
                .filter(|(_, m)| !m.file.as_os_str().is_empty())
                .collect();
            let missing = relink::missing_among(show, &st.missing);
            // What the search of the computer has found so far, best match first.
            let found = st.search.as_ref().map(|s| s.state());
            let wanted: Vec<PathBuf> = {
                let mut v: Vec<PathBuf> = missing.iter().map(|m| m.1.clone()).collect();
                v.sort();
                v.dedup();
                v
            };
            let suggested = found
                .as_ref()
                .map(|f| relink::suggestions(&f.hits, &wanted))
                .unwrap_or_default();
            ui.horizontal_wrapped(|ui| {
                if missing.is_empty() {
                    ui.label(
                        RichText::new(format!("All {} files are in place.", files.len()))
                            .color(GO_GREEN),
                    );
                } else {
                    ui.label(
                        RichText::new(format!(
                            "{} of {} files can't be found.",
                            missing.len(),
                            files.len()
                        ))
                        .color(STOP_RED),
                    );
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        !missing.is_empty() && st.folder_search.is_none(),
                        egui::Button::new("Search a folder for missing files…"),
                    )
                    .on_hover_text(
                        "Looks in the folder you choose, and everything inside it, for files \
                         with the same names.",
                    )
                    .clicked()
                {
                    actions.push(Action::AskFile(DialogFor::RelinkFolder));
                }
                let searching = found.as_ref().is_some_and(|f| !f.done);
                if ui
                    .add_enabled(
                        !missing.is_empty() && !searching,
                        egui::Button::new("Find on this computer"),
                    )
                    .on_hover_text(
                        "Searches this computer's folders and disks for files with the same \
                         names and suggests them. The show's own folder and the usual media \
                         folders go first, so likely matches turn up quickly.",
                    )
                    .clicked()
                {
                    actions.push(Action::FindOnComputer);
                }
                if ui
                    .add_enabled(
                        !files.is_empty() && st.collect_job.is_none(),
                        egui::Button::new("Copy all media into a folder…"),
                    )
                    .on_hover_text(
                        "Copies every file the show uses into one folder and points the cues \
                         at the copies, so the show and the folder can move together.",
                    )
                    .clicked()
                {
                    actions.push(Action::AskFile(DialogFor::CollectMedia));
                }
            });
            if let Some(f) = &found {
                ui.horizontal(|ui| {
                    if !f.done {
                        ui.spinner();
                        ui.label(format!(
                            "Searching… {} folders looked through, {} suggested so far",
                            f.folders,
                            suggested.len()
                        ));
                        if ui.small_button("Stop").clicked()
                            && let Some(s) = &st.search
                        {
                            s.stop();
                        }
                    } else {
                        let how = if f.timed_out {
                            "Stopped after the time limit"
                        } else {
                            "Searched everywhere"
                        };
                        ui.label(
                            RichText::new(format!(
                                "{how} ({} folders): {} of {} missing files have a suggestion.",
                                f.folders,
                                suggested.len(),
                                wanted.len()
                            ))
                            .weak(),
                        );
                    }
                    if !suggested.is_empty()
                        && ui
                            .button(format!("Use all suggestions ({})", suggested.len()))
                            .clicked()
                    {
                        actions.push(Action::RelinkMany(
                            suggested
                                .iter()
                                .filter_map(|(old, c)| Some((old.clone(), c.first()?.clone())))
                                .collect(),
                        ));
                    }
                });
            }
            if let Some(search) = &st.folder_search {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(format!("Searching {}…", search.root.display()));
                    if ui.small_button("Stop").clicked() {
                        search.stop();
                    }
                });
            }
            if let Some(job) = &st.collect_job {
                let done = job.progress.load(std::sync::atomic::Ordering::Relaxed);
                ui.add(
                    egui::ProgressBar::new(done as f32 / job.total.max(1) as f32).text(format!(
                        "Copying {done} of {} into {}",
                        job.total,
                        job.dest.display()
                    )),
                );
            }
            if !st.media_note.is_empty() {
                ui.label(RichText::new(&st.media_note).color(PLAYHEAD));
            }
            ui.separator();
            ui.set_min_width(560.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("media_files")
                    .num_columns(3)
                    .spacing([12.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for (cue, m) in &files {
                            let gone = !m.file.is_file();
                            ui.label(if gone {
                                RichText::new("⚠").color(STOP_RED)
                            } else {
                                RichText::new("✔").color(GO_GREEN)
                            });
                            ui.vertical(|ui| {
                                ui.set_min_width(420.0);
                                ui.add(
                                    egui::Label::new(format!("{}  {}", cue.number, cue.name))
                                        .truncate(),
                                );
                                let path_color = if gone {
                                    STOP_RED
                                } else {
                                    ui.visuals().weak_text_color()
                                };
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(m.file.display().to_string())
                                            .small()
                                            .color(path_color),
                                    )
                                    .truncate(),
                                );
                                if gone
                                    && let Some(best) =
                                        suggested.get(&m.file).and_then(|o| o.first())
                                {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(format!("Suggested: {}", best.display()))
                                                .small()
                                                .color(GO_GREEN),
                                        )
                                        .truncate(),
                                    );
                                }
                            });
                            ui.horizontal(|ui| {
                                if gone && let Some(options) = suggested.get(&m.file) {
                                    if let Some(best) = options.first()
                                        && ui
                                            .button("Use suggestion")
                                            .on_hover_text(best.display().to_string())
                                            .clicked()
                                    {
                                        actions.push(Action::RelinkCue(cue.id, best.clone()));
                                    }
                                    if options.len() > 1 {
                                        egui::ComboBox::from_id_salt(("others", cue.id))
                                            .selected_text(format!("{} more", options.len() - 1))
                                            .show_ui(ui, |ui| {
                                                for o in options.iter().skip(1).take(12) {
                                                    if ui
                                                        .selectable_label(
                                                            false,
                                                            o.display().to_string(),
                                                        )
                                                        .clicked()
                                                    {
                                                        actions.push(Action::RelinkCue(
                                                            cue.id,
                                                            o.clone(),
                                                        ));
                                                    }
                                                }
                                            });
                                    }
                                }
                                if gone && ui.button("Locate…").clicked() {
                                    actions.push(Action::AskFile(DialogFor::LocateFile(
                                        cue.id,
                                        cue.kind.cue_type(),
                                    )));
                                }
                            });
                            ui.end_row();
                        }
                    });
            });
        });
    if !open {
        // Closing the window ends the search.
        st.search = None;
    }
    st.show_media = open;
}

fn secs_short(t: f64) -> String {
    format!("{t:.1}s")
}

/// The cue list's "Fade in / out" column: a cue's built-in transition and fade-out.
pub(crate) fn fades_text(c: &Cue) -> String {
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
    if ui.button(if c.armed { "Disarm" } else { "Arm" }).clicked() {
        actions.push(Action::SetArmed(c.id, !c.armed));
        ui.close();
    }
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
        if p.reading.is_some() {
            ui.label(format!("{name}: reading its file…"));
        } else {
            ui.label(format!("{name}: pre-wait {}", fmt_time(p.remaining)));
        }
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

/// Which of `previews` the main window shows: the output whose corners are
/// being edited, else the one chosen, else the first.
fn preview_index(
    previews: &[PreviewOutput],
    chosen: Option<OutputId>,
    editing: Option<OutputId>,
) -> Option<usize> {
    let find = |id: Option<OutputId>| id.and_then(|id| previews.iter().position(|p| p.id == id));
    find(editing)
        .or(find(chosen))
        .or(if previews.is_empty() { None } else { Some(0) })
}

/// A live preview of one output, with a scrub bar under it for the cue
/// playing there. With several outputs, a menu picks which is shown. For an
/// output with no display of its own, this is where it is shown.
fn output_previews(
    ui: &mut Ui,
    show: &mut Show,
    engine: &Engine,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) {
    let Some(i) = preview_index(&st.previews, st.preview_output, st.geometry_edit) else {
        return;
    };
    let id = st.previews[i].id;
    let editing = st.geometry_edit == Some(id);
    let where_ = match st.routes.get(&id) {
        Some(OutputRoute::Display(name)) => format!("also fullscreen on {name}"),
        Some(OutputRoute::SubDisplay { name, display }) => {
            format!("also on {name} ({display})")
        }
        _ => "main window only".to_string(),
    };
    let mut chosen = id;
    ui.horizontal(|ui| {
        if st.previews.len() > 1 {
            ui.add_enabled_ui(!editing, |ui| {
                egui::ComboBox::from_id_salt("preview_output")
                    .selected_text(RichText::new(&st.previews[i].name).strong())
                    .show_ui(ui, |ui| {
                        for p in &st.previews {
                            ui.selectable_value(&mut chosen, p.id, &p.name);
                        }
                    })
                    .response
                    .on_hover_text("Which output to preview here")
                    .on_disabled_hover_text("Showing the output whose corners are being edited");
            });
        } else {
            ui.label(RichText::new(&st.previews[i].name).strong());
        }
        ui.label(RichText::new(where_).weak().small());
    });
    if chosen != id {
        st.preview_output = Some(chosen);
    }

    let e = &st.previews[i];
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
    if editing && let Some(cfg) = show.outputs.iter_mut().find(|o| o.id == id) {
        corner_handles(ui, rect, id, &mut cfg.corners, &st.handle_sel);
    }
    scrub_bar(ui, show, engine, st, id, actions);
    ui.add_space(8.0);
}

/// A bar under the preview that moves the cue playing on `output`: drag it
/// to jump around in the video, sound or timed still.
fn scrub_bar(
    ui: &mut Ui,
    show: &Show,
    engine: &Engine,
    st: &mut UiState,
    output: OutputId,
    actions: &mut Vec<Action>,
) {
    let Some(a) = engine.scrub_target(show, output) else {
        st.scrub = None;
        ui.label(
            RichText::new("Nothing on this output to scrub")
                .weak()
                .small(),
        );
        return;
    };
    let length = a.scrub_length(show).unwrap_or(0.0);
    let now = ui.input(|i| i.time);
    // Stay where the bar was dragged until the cue has caught up with it.
    let held = st.scrub.filter(|s| {
        s.instance == a.instance && (s.dragging || a.seeking() || now - s.released < SCRUB_HOLD)
    });
    let mut pos = held.map_or(a.elapsed, |s| s.pos).clamp(0.0, length);
    let name = show.cue(a.cue_id).map(cue_label).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.label(RichText::new(name).small());
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // Clear of the panel's scroll bar.
            ui.add_space(8.0);
            ui.label(RichText::new(format!("{} / {}", fmt_time(pos), fmt_time(length))).small());
        });
    });
    ui.spacing_mut().slider_width = (ui.available_width() - 8.0).max(40.0);
    let bar = ui
        .add(egui::Slider::new(&mut pos, 0.0..=length).show_value(false))
        .on_hover_text("Drag to jump to another point in the cue");
    let mut scrub = held.unwrap_or(Scrub {
        instance: a.instance,
        pos,
        dragging: false,
        sent: f64::NEG_INFINITY,
        released: now,
    });
    if bar.changed() {
        scrub.pos = pos;
        scrub.dragging = bar.dragged();
        scrub.released = now;
        // While dragging, ask now and then so the picture follows; the
        // final position is always asked for when the bar is let go.
        if now - scrub.sent >= SCRUB_EVERY || !bar.dragged() {
            actions.push(Action::Seek(a.instance, pos));
            scrub.sent = now;
        }
    }
    if bar.drag_stopped() {
        scrub.dragging = false;
        scrub.released = now;
        // (Letting go without having moved it asks for nothing.)
        if scrub.sent.is_finite() && scrub.sent < now {
            actions.push(Action::Seek(a.instance, scrub.pos));
            scrub.sent = now;
        }
    }
    st.scrub = (bar.changed() || bar.dragged() || held.is_some()).then_some(scrub);
    if st.scrub.is_some() {
        ui.ctx().request_repaint();
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
        m.title(),
        m.size.0,
        m.size.1,
        m.refresh_hz
            .map(|r| format!(" @ {r:.0} Hz"))
            .unwrap_or_default(),
        if m.primary { " primary" } else { "" }
    )
}

/// Where the show's sound goes: named outputs on audio devices.
fn audio_panel(
    ui: &mut Ui,
    show: &mut Show,
    engine: &Engine,
    st: &UiState,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        ui.heading("Audio outputs");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("+ Add").clicked() {
                actions.push(Action::AddAudioOutput);
            }
        });
    });
    if show.audio_outputs.is_empty() {
        ui.label(
            RichText::new(
                "All sound plays on the system's default device. Add an output to choose a \
                 device, or to send cues to different devices.",
            )
            .weak()
            .small(),
        );
        return;
    }
    for o in &mut show.audio_outputs {
        ui.group(|ui| {
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut o.name).desired_width(140.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("Remove").clicked() {
                        actions.push(Action::RemoveAudioOutput(o.id));
                    }
                });
            });
            let shown = o.device.clone().unwrap_or_else(|| "System default".into());
            egui::ComboBox::from_id_salt(("audio_device", o.id.0))
                .width(ui.available_width() - 8.0)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut o.device, None, "System default");
                    for d in &st.audio_devices {
                        ui.selectable_value(&mut o.device, Some(d.clone()), d);
                    }
                });
            match engine.audio.device_status(Some(o.id)) {
                Some(d) if d.error.is_none() => {
                    ui.label(
                        RichText::new(format!("{} channels · {} Hz", d.channels, d.sample_rate))
                            .color(GO_GREEN)
                            .small(),
                    );
                }
                Some(d) => {
                    ui.label(
                        RichText::new(format!("⚠ {}", d.error.unwrap_or_default()))
                            .color(STOP_RED)
                            .small(),
                    );
                }
                None => {
                    ui.label(RichText::new("Starting…").weak().small());
                }
            }
        });
    }
    ui.label(
        RichText::new("The first output is the default for cues that don't choose one.")
            .weak()
            .small(),
    );
}

/// Settings for controlling NewQ from OSC and MIDI.
fn remote_panel(ui: &mut Ui, show: &mut Show, st: &UiState, engine: &Engine) {
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
                "Note on N: fire the cue given that note, else cue N · Program change N: \
                 select cue N\nMIDI Show Control: GO, STOP, RESUME, LOAD, ALL OFF (cue numbers work)",
            )
            .weak()
            .small(),
        );
        ui.checkbox(&mut r.mtc, "Follow MIDI Time Code")
            .on_hover_text("Fire cues that have a timecode as the incoming time reaches it.");
        if r.mtc {
            match engine.timecode.position() {
                Some((tc, rate)) if engine.timecode.running() => {
                    ui.label(RichText::new(format!("{tc}  ·  {}", rate.label())).color(GO_GREEN))
                }
                Some((tc, _)) => ui.label(RichText::new(format!("{tc}  ·  stopped")).weak()),
                None => ui.label(RichText::new("Waiting for timecode…").weak()),
            };
        }
    }
    ui.add_space(4.0);
    ui.checkbox(&mut r.web, "Web remote (phone or tablet)")
        .on_hover_text("Serves a page that runs the show from a browser on this network.");
    if r.web {
        ui.horizontal(|ui| {
            ui.label("Port");
            ui.add(
                egui::DragValue::new(&mut r.web_port)
                    .range(1..=65535)
                    .speed(1.0),
            );
            ui.label("PIN");
            ui.add(
                egui::TextEdit::singleline(&mut r.web_pin)
                    .password(true)
                    .desired_width(70.0)
                    .hint_text("none"),
            );
            // What the page can send: no spaces or accented letters.
            let clean = crate::web::clean_pin(&r.web_pin);
            if clean != r.web_pin {
                r.web_pin = clean;
            }
        });
        match (&status.web_port, &status.web_error) {
            (_, Some(e)) => {
                ui.label(RichText::new(format!("⚠ {e}")).color(STOP_RED));
            }
            (Some(p), None) => {
                let host = crate::web::lan_address()
                    .map_or("<this computer>".to_string(), |ip| ip.to_string());
                ui.label(
                    RichText::new(format!("Open http://{host}:{p} on a phone")).color(GO_GREEN),
                );
            }
            _ => {
                ui.label(RichText::new("Starting…").weak());
            }
        }
        ui.label(
            RichText::new(
                "GO, Stop All, Pause, the playhead and a button for every cue. Anyone on \
                 your network who can reach the port (and has the PIN, if you set one) \
                 can run the show. The PIN is saved in the show file.",
            )
            .small()
            .color(PREWAIT),
        );
    }
}

/// What to tell the user when the display an output is on isn't showing it.
fn display_problem(displays: &[DisplaySlot], st: &UiState, id: DisplayId) -> String {
    let Some(d) = displays.iter().find(|x| x.id == id) else {
        return "Its display was removed: showing in the main window".to_string();
    };
    match st.display_states.get(&id) {
        Some(SlotState::Missing) => format!(
            "{} ({}) isn't connected: not shown on any display",
            d.name,
            d.monitor.as_ref().map_or("", |r| r.name.as_str())
        ),
        Some(SlotState::OnMainWindowDisplay(_)) => format!(
            "{} is the monitor NewQ's main window is on: showing in the main window",
            d.name
        ),
        _ => format!("{} has no monitor yet: showing in the main window", d.name),
    }
}

/// The show's displays: names that outputs are placed on, each assigned by
/// hand to a monitor. They stay as assigned whatever is plugged in or out.
fn displays_panel(ui: &mut Ui, show: &mut Show, st: &UiState, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.heading("Displays");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.button("+ Add").clicked() {
                actions.push(Action::AddDisplay);
            }
        });
    });
    if show.displays.is_empty() {
        ui.label(
            RichText::new(
                "Add a display for each screen or projector, assign it to a monitor, and place \
                 outputs on it. A display stays on the monitor you chose: if that monitor is \
                 unplugged, nothing moves to another one.",
            )
            .weak()
            .small(),
        );
        return;
    }
    for d in &mut show.displays {
        let users: Vec<&str> = show
            .outputs
            .iter()
            .filter(|o| match o.placement {
                Placement::Slot(id) => id == d.id,
                Placement::SubDisplay(v) => show
                    .sub_displays
                    .iter()
                    .any(|s| s.id == v && s.display == Some(d.id)),
                _ => false,
            })
            .map(|o| o.name.as_str())
            .collect();
        let state = st
            .display_states
            .get(&d.id)
            .copied()
            .unwrap_or(SlotState::Unassigned);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(140.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .small_button("🗑")
                        .on_hover_text("Remove display")
                        .clicked()
                    {
                        actions.push(Action::RemoveDisplay(d.id));
                    }
                });
            });
            let shown = match (&d.monitor, state) {
                (None, _) => "Not assigned".to_string(),
                (Some(_), SlotState::Connected(i) | SlotState::OnMainWindowDisplay(i)) => {
                    monitor_label(st, i)
                }
                (Some(r), _) => format!("{} (not connected)", r.name),
            };
            egui::ComboBox::from_id_salt(("display_monitor", d.id.0))
                .width(ui.available_width() - 8.0)
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(d.monitor.is_none(), "Not assigned")
                        .clicked()
                    {
                        d.monitor = None;
                        d.twins = 1;
                    }
                    for i in 0..st.monitors.len() {
                        let chosen = matches!(
                            state,
                            SlotState::Connected(c) | SlotState::OnMainWindowDisplay(c) if c == i
                        );
                        if ui.selectable_label(chosen, monitor_label(st, i)).clicked() {
                            let m = &st.monitors[i];
                            d.monitor = Some(DisplayRef {
                                name: m.title().to_string(),
                                index: i,
                            });
                            d.twins = st
                                .monitors
                                .iter()
                                .filter(|x| x.title() == m.title())
                                .count() as u32;
                        }
                    }
                });
            let (text, color) = match state {
                SlotState::Unassigned => ("Choose a monitor for it".to_string(), None),
                SlotState::Missing => (
                    format!(
                        "⚠ {} isn't connected: nothing is shown on this display",
                        d.monitor.as_ref().map_or("", |r| r.name.as_str())
                    ),
                    Some(STOP_RED),
                ),
                SlotState::OnMainWindowDisplay(_) => (
                    "⚠ NewQ's main window is on this monitor, so outputs don't cover it"
                        .to_string(),
                    Some(STOP_RED),
                ),
                SlotState::Connected(_) => (
                    if users.is_empty() {
                        "Connected · no outputs on it yet".to_string()
                    } else {
                        format!("Connected · shows {}", users.join(", "))
                    },
                    Some(GO_GREEN),
                ),
            };
            let label = RichText::new(text).small();
            ui.label(match color {
                Some(c) => label.color(c),
                None => label.weak(),
            });
        });
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
    let n_outputs = show.outputs.len();
    let show_subs = &show.sub_displays;
    let displays = &show.displays;
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
                Placement::MainWindow => "Main window only".to_string(),
                Placement::Slot(d) => match displays.iter().find(|x| x.id == *d) {
                    Some(x) => format!("Fullscreen on {}", x.name),
                    None => "Removed display".to_string(),
                },
                Placement::SubDisplay(v) => match show_subs.iter().find(|x| x.id == *v) {
                    Some(x) => format!("On sub display {}", x.name),
                    None => "Removed sub display".to_string(),
                },
                // Older shows: given a display as they open.
                Placement::Auto | Placement::Display(_) => "…".to_string(),
            };
            egui::ComboBox::from_id_salt(("placement", out.id.0))
                .width(ui.available_width() - 8.0)
                .selected_text(current)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut out.placement,
                        Placement::MainWindow,
                        "Main window only",
                    )
                    .on_hover_text("Show this output inside NewQ's main window only");
                    for d in displays.iter() {
                        ui.selectable_value(
                            &mut out.placement,
                            Placement::Slot(d.id),
                            format!("Fullscreen on {}", d.name),
                        );
                    }
                    for v in show_subs.iter() {
                        let on = v
                            .display
                            .and_then(|d| displays.iter().find(|x| x.id == d))
                            .map_or("no display", |x| x.name.as_str());
                        ui.selectable_value(
                            &mut out.placement,
                            Placement::SubDisplay(v.id),
                            format!("On sub display {} (on {on})", v.name),
                        );
                    }
                });
            let status = match (st.routes.get(&out.id), &out.placement) {
                (None, _) => "Off".to_string(),
                (Some(OutputRoute::Display(name)), _) => format!("Fullscreen on {name}"),
                (Some(OutputRoute::SubDisplay { name, display }), _) => {
                    format!("On {name}, part of {display}")
                }
                (Some(OutputRoute::MainWindow), Placement::Slot(d)) => {
                    display_problem(displays, st, *d)
                }
                (Some(OutputRoute::MainWindow), Placement::SubDisplay(v)) => {
                    match show_subs.iter().find(|x| x.id == *v) {
                        Some(x) => match x.display {
                            Some(d) => display_problem(displays, st, d),
                            None => {
                                format!("{} has no display yet: showing in the main window", x.name)
                            }
                        },
                        None => {
                            "Its sub display was removed: showing in the main window".to_string()
                        }
                    }
                }
                (Some(OutputRoute::MainWindow), _) => "Showing in the main window".to_string(),
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

fn add_sub_display_menu(ui: &mut Ui, show: &Show, actions: &mut Vec<Action>) {
    if show.displays.is_empty() {
        ui.label(RichText::new("Add a display first").weak());
    }
    for d in &show.displays {
        if ui.button(&d.name).clicked() {
            actions.push(Action::AddSubDisplay(Some(d.id)));
            ui.close();
        }
    }
}

fn sub_displays_panel(ui: &mut Ui, show: &mut Show, st: &UiState, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.heading("Sub displays");
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.menu_button("+ Add", |ui| add_sub_display_menu(ui, show, actions))
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
    let displays = show.displays.clone();
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
            let on = v
                .display
                .and_then(|d| displays.iter().find(|x| x.id == d))
                .map_or("no display", |x| x.name.as_str());
            egui::ComboBox::from_id_salt(("vd_parent", v.id.0))
                .width(ui.available_width() - 8.0)
                .selected_text(format!("Part of {on}"))
                .show_ui(ui, |ui| {
                    for d in displays.iter() {
                        ui.selectable_value(&mut v.display, Some(d.id), &d.name);
                    }
                });
            let (w, h) = v.area.size_on(screen_size);
            let status = match monitor {
                Some(_) => format!("{w}×{h} px"),
                None => format!("{on} isn't available"),
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
    let audio_outputs: Vec<(AudioOutputId, String)> = show
        .audio_outputs
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
                ui.label("Armed");
                ui.checkbox(&mut cue.armed, "").on_hover_text(
                    "A disarmed cue is skipped as if it weren't in the list: GO passes over \
                     it, continues don't fire it, and its group doesn't start it.",
                );
                ui.end_row();
                ui.label("Hotkey");
                ui.horizontal(|ui| {
                    if st.capture_hotkey == Some(cue.id) {
                        ui.label(RichText::new("Press a key…").color(PLAYHEAD));
                        if ui.button("Cancel").clicked() {
                            actions.push(Action::CaptureHotkey(None));
                        }
                    } else {
                        let text = cue.hotkey.clone().unwrap_or_else(|| "none".into());
                        if ui
                            .button(text)
                            .on_hover_text(
                                "Click, then press a letter, digit or F-key. Pressing it \
                                 later fires this cue without moving the playhead.",
                            )
                            .clicked()
                        {
                            actions.push(Action::CaptureHotkey(Some(cue.id)));
                        }
                        if cue.hotkey.is_some() && ui.small_button("Clear").clicked() {
                            actions.push(Action::SetHotkey(cue.id, None));
                        }
                    }
                });
                ui.end_row();
                ui.label("MIDI note");
                ui.horizontal(|ui| {
                    let mut on = cue.midi_note.is_some();
                    let mut note = cue.midi_note.unwrap_or(60);
                    let on_changed = ui
                        .checkbox(&mut on, "")
                        .on_hover_text(
                            "A MIDI note-on with this number fires this cue (needs MIDI \
                             control on). Otherwise the note number is looked up as a cue number.",
                        )
                        .changed();
                    let note_changed = ui
                        .add_enabled(on, egui::DragValue::new(&mut note).range(0..=127))
                        .changed();
                    if on_changed || note_changed {
                        actions.push(Action::SetMidiNote(cue.id, on.then_some(note)));
                    }
                });
                ui.end_row();
                ui.label("On cart");
                ui.checkbox(&mut cue.cart, "")
                    .on_hover_text("Show this cue as a button on the cart (Cue ▸ Show cart).");
                ui.end_row();
                ui.label("Timecode");
                ui.horizontal(|ui| {
                    let mut on = cue.timecode.is_some();
                    let mut t = cue.timecode.unwrap_or_default();
                    ui.checkbox(&mut on, "").on_hover_text(
                        "Fire this cue when incoming MIDI Time Code reaches this time \
                         (turn on “Follow MIDI Time Code” under Remote control).",
                    );
                    ui.add_enabled_ui(on, |ui| {
                        let part = |ui: &mut Ui, v: &mut u8, max: u8, sep: bool| {
                            ui.add(
                                egui::DragValue::new(v)
                                    .range(0..=max)
                                    .custom_formatter(|n, _| format!("{:02}", n as u8)),
                            );
                            if sep {
                                ui.label(":");
                            }
                        };
                        ui.spacing_mut().item_spacing.x = 2.0;
                        part(ui, &mut t.hours, 23, true);
                        part(ui, &mut t.minutes, 59, true);
                        part(ui, &mut t.seconds, 59, true);
                        part(ui, &mut t.frames, 29, false);
                    });
                    cue.timecode = on.then_some(t);
                });
                ui.end_row();
                ui.label("Clock");
                ui.horizontal(|ui| {
                    let mut on = cue.clock.is_some();
                    let (mut h, mut m, mut s) = cue.clock.map_or((18, 0, 0), TimeOfDay::hms);
                    ui.checkbox(&mut on, "").on_hover_text(
                        "Fire this cue when the time of day reaches this time, to the second. \
                         It only fires for time that passes while NewQ is running.",
                    );
                    ui.add_enabled_ui(on, |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        for (v, max, sep) in
                            [(&mut h, 23, true), (&mut m, 59, true), (&mut s, 59, false)]
                        {
                            ui.add(
                                egui::DragValue::new(v)
                                    .range(0..=max)
                                    .custom_formatter(|n, _| format!("{:02}", n as u32)),
                            );
                            if sep {
                                ui.label(":");
                            }
                        }
                    });
                    cue.clock = on.then(|| TimeOfDay::new(h, m, s).unwrap_or_default());
                });
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
            CueKind::Video(m) | CueKind::Image(m) | CueKind::Audio(m) => media_inspector(
                ui,
                cue_id,
                cue_type,
                m,
                OutputChoices {
                    video: &outputs,
                    audio: &audio_outputs,
                },
                engine,
                actions,
            ),
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
                media_inspector(
                    ui,
                    cue_id,
                    cue_type,
                    &mut t.media,
                    OutputChoices {
                        video: &outputs,
                        audio: &audio_outputs,
                    },
                    engine,
                    actions,
                );
            }
            CueKind::Send(s) => send_inspector(ui, s, &st.midi_out_ports),
            CueKind::Camera(c) => {
                camera_inspector(ui, c, engine);
                ui.separator();
                media_inspector(
                    ui,
                    cue_id,
                    cue_type,
                    &mut c.media,
                    OutputChoices {
                        video: &outputs,
                        audio: &audio_outputs,
                    },
                    engine,
                    actions,
                );
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

/// The outputs a cue can be sent to, by id and name.
struct OutputChoices<'a> {
    video: &'a [(OutputId, String)],
    audio: &'a [(AudioOutputId, String)],
}

fn media_inspector(
    ui: &mut Ui,
    cue_id: CueId,
    ty: CueType,
    m: &mut MediaCue,
    choices: OutputChoices,
    engine: &mut Engine,
    actions: &mut Vec<Action>,
) {
    let OutputChoices {
        video: outputs,
        audio: audio_outputs,
    } = choices;
    let visual = ty != CueType::Audio;
    // Text and images are held for a time rather than played from a file.
    let still = matches!(ty, CueType::Image | CueType::Text | CueType::Camera);
    // Text and cameras have no file to choose.
    let is_text = matches!(ty, CueType::Text | CueType::Camera);
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
                if ui.button("Browse…").clicked() {
                    actions.push(Action::AskFile(DialogFor::BrowseFile(cue_id, ty)));
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
                ui.label("Audio output");
                let chosen = m
                    .audio_output
                    .and_then(|id| audio_outputs.iter().find(|o| o.0 == id))
                    .map_or_else(|| "Default".to_string(), |o| o.1.clone());
                egui::ComboBox::from_id_salt("audio_output")
                    .selected_text(chosen)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut m.audio_output, None, "Default");
                        for (id, name) in audio_outputs {
                            ui.selectable_value(&mut m.audio_output, Some(*id), name);
                        }
                    });
                ui.end_row();
                ui.label("Channels");
                ui.horizontal(|ui| {
                    let mut first = m.audio_channel + 1;
                    ui.add(egui::DragValue::new(&mut first).range(1..=64))
                        .on_hover_text(
                            "The first device channel; left plays here and right on the next",
                        );
                    m.audio_channel = first.saturating_sub(1);
                    let device = engine.audio.device_status(m.audio_output);
                    let hint = match device {
                        Some(d) if d.error.is_none() => {
                            let end = usize::from(m.audio_channel) + 2;
                            if end > usize::from(d.channels) {
                                RichText::new(format!("⚠ device has {} channels", d.channels))
                                    .color(PREWAIT)
                            } else {
                                RichText::new(format!("to {} (of {})", end, d.channels)).weak()
                            }
                        }
                        Some(_) => RichText::new("⚠ device unavailable").color(STOP_RED),
                        None => RichText::new("").weak(),
                    };
                    ui.label(hint);
                });
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
                ui.label(if ty == CueType::Camera {
                    "Run for"
                } else {
                    "Hold for"
                });
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
                if ty != CueType::Image {
                    ui.label("Waveform");
                    waveform_trim(ui, m, engine);
                    ui.end_row();
                }
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
        let title = if m.color.is_default() {
            "Colour and key"
        } else {
            "Colour and key •"
        };
        egui::CollapsingHeader::new(title)
            .id_salt("colour")
            .default_open(!m.color.is_default())
            .show(ui, |ui| color_inspector(ui, &mut m.color));
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

/// What a send cue sends, and where.
fn send_inspector(ui: &mut Ui, s: &mut SendCue, midi_outputs: &[String]) {
    let is_osc = matches!(s.target, SendTarget::Osc(_));
    ui.horizontal(|ui| {
        if ui.selectable_label(is_osc, "OSC").clicked() && !is_osc {
            s.target = SendTarget::Osc(OscOut::default());
        }
        if ui.selectable_label(!is_osc, "MIDI").clicked() && is_osc {
            s.target = SendTarget::Midi(MidiOut::default());
        }
    });
    match &mut s.target {
        SendTarget::Osc(o) => {
            egui::Grid::new("send_osc")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("To");
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut o.host).desired_width(150.0));
                        ui.label(":");
                        ui.add(
                            egui::DragValue::new(&mut o.port)
                                .range(1..=65535)
                                .speed(1.0),
                        );
                    });
                    ui.end_row();
                    ui.label("Address");
                    ui.add(egui::TextEdit::singleline(&mut o.address).desired_width(220.0));
                    ui.end_row();
                    ui.label("Arguments");
                    ui.add(
                        egui::TextEdit::singleline(&mut o.args)
                            .desired_width(220.0)
                            .hint_text("1 0.5 \"some text\""),
                    );
                    ui.end_row();
                });
            ui.label(
                RichText::new(
                    "Sent as one UDP packet when the cue fires. An IP address is safest: \
                     a name has to be looked up. Whole numbers go as integers, other \
                     numbers as floats, the rest as text.",
                )
                .weak(),
            );
            if !o.address.trim().starts_with('/') {
                ui.label(RichText::new("⚠ An OSC address starts with /").color(PREWAIT));
            }
        }
        SendTarget::Midi(m) => {
            egui::Grid::new("send_midi")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Output");
                    egui::ComboBox::from_id_salt("midi_out")
                        .selected_text(m.port.clone().unwrap_or_else(|| "Choose…".into()))
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for p in midi_outputs {
                                ui.selectable_value(&mut m.port, Some(p.clone()), p);
                            }
                        });
                    ui.end_row();
                    ui.label("Message");
                    egui::ComboBox::from_id_salt("midi_kind")
                        .selected_text(m.kind.label())
                        .show_ui(ui, |ui| {
                            for k in MidiKind::ALL {
                                ui.selectable_value(&mut m.kind, k, k.label());
                            }
                        });
                    ui.end_row();
                    let show_control = matches!(m.kind, MidiKind::ShowControl(_));
                    if show_control {
                        if let MidiKind::ShowControl(c) = m.kind
                            && c.takes_cue()
                        {
                            ui.label("Cue number");
                            ui.add(
                                egui::TextEdit::singleline(&mut m.cue)
                                    .desired_width(100.0)
                                    .hint_text("next cue"),
                            );
                            ui.end_row();
                        }
                    } else {
                        ui.label("Channel");
                        let mut ch = m.channel + 1;
                        ui.add(egui::DragValue::new(&mut ch).range(1..=16));
                        m.channel = ch - 1;
                        ui.end_row();
                        let (first, second) = match m.kind {
                            MidiKind::ControlChange => ("Controller", Some("Value")),
                            MidiKind::ProgramChange => ("Program", None),
                            _ => ("Note", Some("Velocity")),
                        };
                        ui.label(first);
                        ui.add(egui::DragValue::new(&mut m.data1).range(0..=127));
                        ui.end_row();
                        if let Some(second) = second {
                            ui.label(second);
                            ui.add(egui::DragValue::new(&mut m.data2).range(0..=127));
                            ui.end_row();
                        }
                    }
                });
            if matches!(m.kind, MidiKind::ShowControl(_)) {
                ui.label(
                    RichText::new(
                        "MIDI Show Control, to all devices: lighting and sound desks that \
                         understand it will GO, STOP, RESUME or LOAD their cue.",
                    )
                    .weak(),
                );
            }
        }
    }
    ui.label(RichText::new("Use ▶ Fire to try it.").weak());
}

/// A camera's name as shown to the operator.
fn device_label(device: &str) -> String {
    if device.starts_with("/dev/")
        && let Some(name) = camera::v4l2_label(device)
    {
        return format!("{name} ({device})");
    }
    device.to_string()
}

/// Which camera a camera cue opens and in what mode; where, when and how it
/// shows is the shared media section.
fn camera_inspector(ui: &mut Ui, c: &mut CameraCue, engine: &mut Engine) {
    egui::Grid::new("camera")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Camera");
            ui.horizontal(|ui| {
                let selected = if c.device.is_empty() {
                    "(choose a camera)".to_string()
                } else {
                    device_label(&c.device)
                };
                egui::ComboBox::from_id_salt("camera_device")
                    .selected_text(selected)
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        for d in engine.cameras() {
                            ui.selectable_value(&mut c.device, d.clone(), device_label(d));
                        }
                    });
                if ui
                    .button("↻")
                    .on_hover_text("Look for cameras again")
                    .clicked()
                {
                    engine.refresh_cameras();
                }
            });
            ui.end_row();
            ui.label("Mode");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut c.width).range(16..=7680));
                ui.label("×");
                ui.add(egui::DragValue::new(&mut c.height).range(16..=4320));
                ui.add(
                    egui::DragValue::new(&mut c.fps)
                        .range(1..=240)
                        .suffix(" fps"),
                );
            });
            ui.end_row();
        });
    ui.label(
        RichText::new(
            "The mode must be one the camera supports. The camera opens when the cue is next \
             in line, so GO is instant; one cue at a time can use a given camera.",
        )
        .weak(),
    );
    if c.device == TEST_PATTERN_DEVICE {
        ui.label(
            RichText::new("A moving test picture, for trying a camera cue without a camera.")
                .weak(),
        );
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
/// Which end of the played part a drag on the waveform moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TrimHandle {
    In,
    Out,
}

/// The shortest played part the handles allow, in seconds.
const MIN_TRIM: f64 = 0.05;

/// The handle nearest to time `t`. With no out point the out handle is at the
/// end of the file.
fn nearest_handle(t: f64, start: f64, end: Option<f64>, duration: f64) -> TrimHandle {
    let out = end.unwrap_or(duration);
    if (t - start).abs() <= (t - out).abs() {
        TrimHandle::In
    } else {
        TrimHandle::Out
    }
}

/// Move a handle to time `t`, keeping the in point before the out point and
/// both inside the file. An out point at the very end of the file is no out
/// point at all. Returns the new in point and out point.
fn move_handle(
    handle: TrimHandle,
    t: f64,
    start: f64,
    end: Option<f64>,
    duration: f64,
) -> (f64, Option<f64>) {
    let out = end.unwrap_or(duration);
    match handle {
        TrimHandle::In => (t.clamp(0.0, (out - MIN_TRIM).max(0.0)), end),
        TrimHandle::Out => {
            let new = t.clamp((start + MIN_TRIM).min(duration), duration);
            (start, (new < duration - 0.02).then_some(new))
        }
    }
}

/// The sound of a media cue, with the played part marked: drag the yellow
/// ends to set the in and out points by eye (click moves the nearer one).
fn waveform_trim(ui: &mut Ui, m: &mut MediaCue, engine: &mut Engine) {
    if m.file.as_os_str().is_empty() {
        ui.label(RichText::new("Choose a file").weak());
        return;
    }
    let hint = match engine.probe_state(&m.file) {
        ProbeState::Ready(i) => i.duration,
        _ => None,
    };
    let ff = engine.ff.clone();
    let wave = match engine.waveforms.get(ff.as_ref(), &m.file, hint) {
        WaveState::Ready(w) => w.clone(),
        WaveState::Pending => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Reading the sound…").weak());
            });
            return;
        }
        WaveState::Failed(why) => {
            ui.label(RichText::new(format!("No waveform: {why}")).weak().small());
            return;
        }
    };
    let dur = wave.duration.max(0.001);
    ui.vertical(|ui| {
        let width = ui.available_width().clamp(260.0, 720.0);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(width, 64.0), Sense::click_and_drag());
        let to_t = |x: f32| (f64::from(x - rect.left()) / f64::from(rect.width())) * dur;
        let to_x = |t: f64| rect.left() + (t / dur) as f32 * rect.width();
        let pos = resp.interact_pointer_pos();
        // The handle picked when a drag begins stays the one dragged.
        let id = resp.id.with("handle");
        if (resp.drag_started() || resp.clicked())
            && let Some(p) = pos
        {
            let h = nearest_handle(to_t(p.x), m.start, m.end, dur);
            ui.memory_mut(|mem| mem.data.insert_temp(id, h));
        }
        if (resp.dragged() || resp.clicked())
            && let Some(p) = pos
            && let Some(h) = ui.memory(|mem| mem.data.get_temp::<TrimHandle>(id))
        {
            (m.start, m.end) = move_handle(h, to_t(p.x), m.start, m.end, dur);
        }

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
        let (from, to) = (
            to_x(m.start.clamp(0.0, dur)),
            to_x(m.end.unwrap_or(dur).clamp(0.0, dur)),
        );
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(from, rect.top()), egui::pos2(to, rect.bottom())),
            0.0,
            ui.visuals().faint_bg_color,
        );
        let mid = rect.center().y;
        let half = rect.height() / 2.0 - 3.0;
        for px in 0..rect.width() as usize {
            let x = rect.left() + px as f32;
            let peak = wave.peak_between(to_t(x), to_t(x + 1.0)).sqrt();
            let h = (peak * half).max(0.5);
            let color = if x >= from && x <= to {
                PLAYING.gamma_multiply(1.4)
            } else {
                ui.visuals().weak_text_color().gamma_multiply(0.5)
            };
            painter.line_segment(
                [egui::pos2(x + 0.5, mid - h), egui::pos2(x + 0.5, mid + h)],
                Stroke::new(1.0, color),
            );
        }
        for x in [from, to] {
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                Stroke::new(2.0, PLAYHEAD),
            );
            let grip = egui::Rect::from_center_size(
                egui::pos2(x, rect.top() + 6.0),
                Vec2::new(10.0, 10.0),
            );
            painter.rect_filled(grip, 2.0, PLAYHEAD);
        }
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let out = m.end.unwrap_or(dur);
        ui.label(
            RichText::new(format!(
                "In {} · Out {}{} · plays {}   (drag the yellow ends)",
                fmt_time(m.start),
                fmt_time(out),
                if m.end.is_none() {
                    " (end of file)"
                } else {
                    ""
                },
                fmt_time((out - m.start).max(0.0)),
            ))
            .weak()
            .small(),
        );
    });
}

/// Colour correction and the chroma key of a visual cue; edits apply live.
fn color_inspector(ui: &mut Ui, c: &mut ColorAdjust) {
    egui::Grid::new("colour_adjust")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Brightness");
            ui.add(egui::Slider::new(&mut c.brightness, -1.0..=1.0));
            ui.end_row();
            ui.label("Contrast");
            ui.add(egui::Slider::new(&mut c.contrast, 0.0..=2.0));
            ui.end_row();
            ui.label("Saturation");
            ui.add(egui::Slider::new(&mut c.saturation, 0.0..=2.0));
            ui.end_row();
            ui.label("Hue");
            ui.add(egui::Slider::new(&mut c.hue, -180.0..=180.0).suffix("°"));
            ui.end_row();
            ui.label("Chroma key");
            let mut on = c.key.is_some();
            ui.checkbox(&mut on, "Make a colour clear").on_hover_text(
                "For a green or blue screen: the chosen colour becomes transparent, \
                     so what is under this cue shows through.",
            );
            if on && c.key.is_none() {
                c.key = Some(ChromaKey::default());
            } else if !on {
                c.key = None;
            }
            ui.end_row();
            if let Some(k) = &mut c.key {
                ui.label("Screen colour");
                ui.horizontal(|ui| {
                    ui.color_edit_button_srgb(&mut k.color);
                    if ui.small_button("Green").clicked() {
                        k.color = [0, 255, 0];
                    }
                    if ui.small_button("Blue").clicked() {
                        k.color = [0, 0, 255];
                    }
                });
                ui.end_row();
                ui.label("Tolerance");
                ui.add(egui::Slider::new(&mut k.tolerance, 0.0..=0.6))
                    .on_hover_text("How far from the screen colour is made fully clear.");
                ui.end_row();
                ui.label("Softness");
                ui.add(egui::Slider::new(&mut k.softness, 0.0..=0.4))
                    .on_hover_text("A soft edge, where the picture fades back in.");
                ui.end_row();
                ui.label("Spill");
                ui.add(egui::Slider::new(&mut k.spill, 0.0..=1.0))
                    .on_hover_text("Removes the screen colour's tint from what is kept.");
                ui.end_row();
            }
        });
    if ui
        .add_enabled(!c.is_default(), egui::Button::new("Reset"))
        .clicked()
    {
        *c = ColorAdjust::default();
    }
}

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
    if let Some(why) = finish_save(show, st) {
        tell_save_failed(&why, true);
    }
    if !st.dirty {
        recovery::clear(st.show_path.as_deref());
        return true;
    }
    let answer = rfd::MessageDialog::new()
        .set_title("Unsaved changes")
        .set_description("Save changes to this show first?")
        .set_buttons(rfd::MessageButtons::YesNoCancel)
        .show();
    let proceed = match answer {
        rfd::MessageDialogResult::Yes => save(show, st, false),
        rfd::MessageDialogResult::No => true,
        _ => false,
    };
    if proceed {
        // Deliberately leaving this show behind (saved or not): nothing to recover.
        recovery::clear(st.show_path.as_deref());
    }
    proceed
}

/// Save, asking where first if the show has no file or `save_as`. This waits
/// for the dialog, which is fine when something else waits on the answer
/// (leaving a show); otherwise `Action::Save` asks on a thread of its own.
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
    write_show(show, st, &path)
}

/// Save the show to `path` and wait for it, telling the operator if that
/// fails. For when something waits on the answer (leaving the show).
fn write_show(show: &Show, st: &mut UiState, path: &Path) -> bool {
    match show.save(path) {
        Ok(()) => {
            saved(st, path.to_path_buf(), serde_json::to_string(show).ok());
            true
        }
        Err(e) => {
            tell_save_failed(&format!("{e:#}"), true);
            false
        }
    }
}

/// The show is on the disk at `path`, as `snapshot`.
fn saved(st: &mut UiState, path: PathBuf, snapshot: Option<String>) {
    // What was kept as a recovery copy, for the old name and the new, is now saved.
    recovery::clear(st.show_path.as_deref());
    recovery::clear(Some(&path));
    st.show_path = Some(path);
    st.saved_json = snapshot;
}

/// Say that a save failed. Waiting for the operator to read it would stop the
/// show, so unless something is waiting on the answer anyway, it is shown on
/// a thread of its own.
fn tell_save_failed(why: &str, wait: bool) {
    // (No dialogs in tests.)
    if cfg!(test) {
        return;
    }
    let why = why.to_string();
    let show = move || {
        rfd::MessageDialog::new()
            .set_title("Save failed")
            .set_description(why)
            .show();
    };
    if wait {
        show();
    } else {
        let _ = std::thread::Builder::new()
            .name("save-failed".into())
            .spawn(show);
    }
}

/// Start writing the show to `path` on a thread of its own. One save at a time.
fn start_save(show: &Show, st: &mut UiState, path: PathBuf) {
    if st.saving.is_some() {
        return;
    }
    let copy = show.clone();
    let snapshot = serde_json::to_string(show).ok();
    let (tx, done) = std::sync::mpsc::channel();
    let to = path.clone();
    let spawned = std::thread::Builder::new()
        .name("save".into())
        .spawn(move || {
            let _ = tx.send(copy.save(&to).map_err(|e| format!("{e:#}")));
        });
    if spawned.is_ok() {
        st.saving = Some(SaveJob {
            path,
            done,
            snapshot,
        });
    }
}

/// Take the result of the save being written, when it is in.
fn poll_save(ctx: &egui::Context, engine: &mut Engine, st: &mut UiState) {
    let Some(job) = &st.saving else { return };
    let result = match job.done.try_recv() {
        Ok(r) => r,
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => Err("the save stopped".into()),
    };
    let Some(job) = st.saving.take() else { return };
    match result {
        Ok(()) => saved(st, job.path, job.snapshot),
        Err(why) => {
            engine.log(format!("Could not save the show: {why}"));
            tell_save_failed(&why, false);
        }
    }
}

/// Wait for the save being written, if there is one, so leaving the show
/// doesn't race it. Returns why it failed, if it did.
fn finish_save(show: &Show, st: &mut UiState) -> Option<String> {
    let job = st.saving.take()?;
    let result = job
        .done
        .recv()
        .unwrap_or_else(|_| Err("the save stopped".into()));
    match result {
        Ok(()) => {
            saved(st, job.path, job.snapshot);
            // Unchanged since, unless edited meanwhile.
            st.dirty = serde_json::to_string(show).ok() != st.saved_json;
            None
        }
        Err(why) => Some(why),
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
    st.check_media(show);
    // A different show has a history of its own.
    if let Some(json) = &st.saved_json {
        st.history.reset(json.clone());
    }
}

/// Do what a remote message asks for, as if the operator had done it.
pub fn run_remote(cmd: Command, show: &mut Show, engine: &mut Engine, st: &mut UiState) {
    // It may change the show; look at it again soon.
    st.json_stale = true;
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
        Command::Timecode(tc, rate) => engine.on_timecode(show, tc, rate),
        Command::TimecodeLocate(tc, rate) => engine.timecode.locate(tc, rate),
        Command::Note(note) => {
            // A note bound to a cue fires it; otherwise it names a cue number.
            let id = show
                .cue_for_note(note)
                .or_else(|| find(show, &note.to_string()));
            match id {
                Some(id) => engine.fire(show, id),
                None => engine.log(format!("Remote: no cue for MIDI note {note}")),
            }
        }
        Command::GoCue(ref n)
        | Command::Select(ref n)
        | Command::Fire(ref n)
        | Command::Stop(ref n)
        | Command::FadeOut(ref n)
        | Command::Arm(ref n)
        | Command::Disarm(ref n) => {
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
                Command::Arm(_) | Command::Disarm(_) => {
                    let armed = matches!(cmd, Command::Arm(_));
                    apply(Action::SetArmed(id, armed), show, engine, st);
                }
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
        Action::AddCue(ty) => match ty {
            // A cue made from a file: ask which, and add it when it is chosen.
            CueType::Video | CueType::Image | CueType::Audio => {
                start_dialog(st, show, DialogFor::AddCue(ty));
            }
            _ => add_cue(show, st, engine, ty, None),
        },
        Action::AddCueFile(ty, file) => add_cue(show, st, engine, ty, Some(file)),
        Action::AskFile(purpose) => start_dialog(st, show, purpose),
        Action::SetCueFile(id, file) => {
            if let Some(i) = show.cue_index(id)
                && let Some(m) = show.cues[i].kind.media_mut()
            {
                engine.probe.invalidate(&file);
                engine.waveforms.invalidate(&file);
                m.file = file;
            }
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
            if confirm_discard(show, st) {
                start_dialog(st, show, DialogFor::OpenShow);
            }
        }
        Action::OpenShowFile(path, changed) => {
            // Edited while the dialog was open: ask again before they are lost.
            if !changed || confirm_discard(show, st) {
                match Show::load(&path) {
                    Ok(new) => load_into(new, Some(path), show, engine, st),
                    Err(e) => engine.log(format!("Could not open show: {e:#}")),
                }
            }
        }
        Action::Save => match st.show_path.clone() {
            Some(path) => start_save(show, st, path),
            None => start_dialog(st, show, DialogFor::SaveShow),
        },
        Action::SaveAs => start_dialog(st, show, DialogFor::SaveShow),
        Action::SaveShowTo(path) => start_save(show, st, path),
        Action::FireSelected => {
            if let Some(id) = st.selected {
                engine.fire(show, id);
            }
        }
        Action::Undo => {
            if let Some(json) = st.history.undo() {
                restore(&json, show, engine, st);
            }
        }
        Action::Redo => {
            if let Some(json) = st.history.redo() {
                restore(&json, show, engine, st);
            }
        }
        Action::FireCue(id) => engine.fire(show, id),
        Action::StopCue(id) => engine.stop_cue(id, 0.0),
        Action::FadeOutCue(id) => engine.fade_out_cue(show, id),
        Action::ToggleCart => st.show_cart = !st.show_cart,
        Action::ToggleMedia => st.show_media = !st.show_media,
        Action::ToggleSync => st.show_sync = !st.show_sync,
        Action::MakeSyncClip => match engine.ff.clone() {
            None => st.sync_note = "ffmpeg wasn't found, so there's no clip to make.".into(),
            Some(ff) => {
                st.sync_note.clear();
                let path = crate::crash::log_dir().join("newq-sync-test.mp4");
                let (tx, rx) = std::sync::mpsc::channel();
                let started =
                    std::thread::Builder::new()
                        .name("sync-clip".into())
                        .spawn(move || {
                            let made = std::fs::create_dir_all(path.parent().unwrap_or(&path))
                                .map_err(|e| e.to_string())
                                .and_then(|_| {
                                    ff.make_sync_clip(&path).map_err(|e| format!("{e:#}"))
                                });
                            let _ = tx.send(made.map(|_| path));
                        });
                match started {
                    Ok(_) => st.sync_clip = Some(rx),
                    Err(e) => st.sync_note = format!("Couldn't start: {e}"),
                }
            }
        },
        Action::AddSyncCue(path) => {
            add_cue(show, st, engine, CueType::Video, Some(path));
            let new = st.selected.and_then(|id| show.cue_index(id));
            if let Some(c) = new.map(|i| &mut show.cues[i]) {
                c.name = "Sync test".into();
                if let Some(m) = c.kind.media_mut() {
                    m.loop_media = true;
                }
            }
        }
        Action::ExportCueSheet(path) => match crate::export::write(&path, show, engine) {
            Ok(n) => engine.log(format!(
                "Exported {n} cue{} to {}",
                if n == 1 { "" } else { "s" },
                path.display()
            )),
            Err(e) => engine.log(format!("Could not export the cue sheet: {e}")),
        },
        Action::RelinkFolder(root) => {
            // On its own thread: a big folder takes a while to read, and the
            // show must not freeze meanwhile. One search at a time.
            if st.folder_search.is_none() {
                let wanted: Vec<PathBuf> = {
                    let mut v: Vec<PathBuf> =
                        relink::missing(show).into_iter().map(|m| m.1).collect();
                    v.sort();
                    v.dedup();
                    v
                };
                st.media_note = String::new();
                st.folder_search = Some(relink::FolderSearch::start(
                    root,
                    wanted,
                    relink::SEARCH_BUDGET,
                ));
            }
        }
        Action::FindOnComputer => {
            let names: std::collections::HashSet<std::ffi::OsString> = relink::missing(show)
                .iter()
                .filter_map(|(_, p)| p.file_name().map(|n| n.to_os_string()))
                .collect();
            if !names.is_empty() {
                let show_dir = st.show_path.as_deref().and_then(|p| p.parent());
                let roots = relink::default_roots(show_dir);
                st.media_note = String::new();
                st.search = Some(relink::Search::start(roots, names, relink::SEARCH_BUDGET));
            }
        }
        Action::RelinkMany(pairs) => {
            let n = relink::apply(show, &pairs.into_iter().collect());
            st.media_note = format!(
                "Linked {n} cue{} to the suggested files.",
                if n == 1 { "" } else { "s" }
            );
        }
        Action::RelinkCue(id, new) => {
            let old = show
                .cue(id)
                .and_then(|c| c.kind.file_media())
                .map(|m| m.file.clone());
            if let Some(old) = old {
                let n = relink::apply(show, &HashMap::from([(old, new.clone())]));
                st.media_note = format!(
                    "Linked {n} cue{} to {}.",
                    if n == 1 { "" } else { "s" },
                    new.display()
                );
                engine.probe.invalidate(&new);
            }
        }
        Action::CollectMedia(dest) => {
            if st.collect_job.is_none() {
                let mut files: Vec<PathBuf> = show
                    .cues
                    .iter()
                    .filter_map(|c| c.kind.file_media())
                    .map(|m| m.file.clone())
                    .filter(|f| !f.as_os_str().is_empty())
                    .collect();
                files.sort();
                files.dedup();
                let progress = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
                let (tx, done) = std::sync::mpsc::channel();
                let (p, d, total) = (progress.clone(), dest.clone(), files.len());
                let _ = std::thread::Builder::new()
                    .name("collect-media".into())
                    .spawn(move || {
                        let _ = tx.send(relink::collect(&d, &files, &p));
                    });
                st.collect_job = Some(CollectJob {
                    done,
                    progress,
                    total,
                    dest,
                });
            }
        }
        Action::CaptureHotkey(id) => st.capture_hotkey = id,
        Action::SetHotkey(id, key) => {
            show.set_hotkey(id, key);
            st.capture_hotkey = None;
        }
        Action::SetMidiNote(id, note) => show.set_midi_note(id, note),
        Action::StopSelected => {
            if let Some(id) = st.selected {
                engine.stop_cue(id, 0.0);
            }
        }
        Action::StopInstance(i, fade) => engine.stop_instance(i, fade),
        Action::FadeOutInstance(i) => engine.fade_out_instance(show, i),
        Action::Seek(i, to) => {
            if let Err(e) = engine.seek(show, i, to) {
                engine.log(format!("Can't scrub: {e}"));
            }
        }
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
        Action::AddSubDisplay(display) => {
            st.area_edit = Some(show.add_sub_display(display));
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
        Action::SetArmed(id, armed) => {
            if let Some(i) = show.cue_index(id) {
                show.cues[i].armed = armed;
            }
        }
        Action::ToggleArmed => {
            if let Some(i) = sel_idx {
                show.cues[i].armed = !show.cues[i].armed;
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
        Action::AddDisplay => {
            show.add_display();
        }
        Action::RemoveDisplay(id) => show.remove_display(id),
        Action::AddAudioOutput => {
            show.add_audio_output();
        }
        Action::RemoveAudioOutput(id) => show.remove_audio_output(id),
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
        keys_with(&Show::default(), st, keys)
    }

    fn keys_with(show: &Show, st: &UiState, keys: &[(Key, egui::Modifiers)]) -> Vec<Action> {
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
        let mut out = ctx.run_ui(input, |ui| keyboard(ui.ctx(), show, st, &mut actions));
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

    #[test]
    fn the_a_key_arms_and_disarms_but_not_while_typing() {
        use egui::Modifiers as M;
        let st = UiState::default();
        let a = keys(&st, &[(Key::A, M::NONE)]);
        assert!(matches!(a[..], [Action::ToggleArmed]));
        // Ctrl+A is not the shortcut.
        assert!(keys(&st, &[(Key::A, M::COMMAND)]).is_empty());
    }

    #[test]
    fn arming_from_the_ui_and_over_osc() {
        let (mut show, mut e, mut st) = running();
        let d = number(&show, "4");
        let idx = |show: &Show| show.cue_index(d).unwrap();
        // The A key's action flips the selected cue.
        st.selected = Some(d);
        apply(Action::ToggleArmed, &mut show, &mut e, &mut st);
        assert!(!show.cues[idx(&show)].armed);
        apply(Action::ToggleArmed, &mut show, &mut e, &mut st);
        assert!(show.cues[idx(&show)].armed);
        // OSC/MIDI-style commands, by cue number.
        run_remote(Command::Disarm("4".into()), &mut show, &mut e, &mut st);
        assert!(!show.cues[idx(&show)].armed);
        run_remote(Command::Fire("4".into()), &mut show, &mut e, &mut st);
        assert!(!is_playing(&e, d), "a disarmed cue won't fire");
        run_remote(Command::Arm("4".into()), &mut show, &mut e, &mut st);
        run_remote(Command::Fire("4".into()), &mut show, &mut e, &mut st);
        assert!(is_playing(&e, d));
        // SetArmed, as from the context menu and inspector.
        apply(Action::SetArmed(d, false), &mut show, &mut e, &mut st);
        assert!(!show.cues[idx(&show)].armed);
    }

    // ----- Hotkeys, MIDI notes and the cart -------------------------------------

    #[test]
    fn only_free_keys_can_be_hotkeys() {
        for k in [Key::Q, Key::Z, Key::Num0, Key::Num7, Key::F1, Key::F12] {
            assert!(hotkey_allowed(k), "{k:?}");
        }
        for k in [
            Key::Space,
            Key::Escape,
            Key::P,
            Key::A,
            Key::ArrowUp,
            Key::Delete,
            Key::Enter,
            Key::Tab,
            Key::F13,
        ] {
            assert!(!hotkey_allowed(k), "{k:?}");
        }
    }

    #[test]
    fn a_hotkey_fires_its_cue_and_other_keys_still_work() {
        use egui::Modifiers as M;
        let (mut show, _, st) = running();
        let d = number(&show, "4");
        show.set_hotkey(d, Some("Q".into()));
        let a = keys_with(&show, &st, &[(Key::Q, M::NONE)]);
        assert!(
            matches!(a[..], [Action::FireCue(id)] if id == d),
            "{}",
            a.len()
        );
        // Shift doesn't matter; Ctrl does.
        let a = keys_with(&show, &st, &[(Key::Q, M::SHIFT)]);
        assert!(matches!(a[..], [Action::FireCue(id)] if id == d));
        assert!(keys_with(&show, &st, &[(Key::Q, M::COMMAND)]).is_empty());
        // Unbound keys do nothing, and the transport keys are untouched.
        assert!(keys_with(&show, &st, &[(Key::W, M::NONE)]).is_empty());
        let a = keys_with(&show, &st, &[(Key::Space, M::NONE)]);
        assert!(matches!(a[..], [Action::Go]));
    }

    #[test]
    fn hotkeys_fire_without_moving_the_playhead() {
        use egui::Modifiers as M;
        let (mut show, mut e, mut st) = running();
        let d = number(&show, "4");
        show.set_hotkey(d, Some("F2".into()));
        for a in keys_with(&show, &st, &[(Key::F2, M::NONE)]) {
            apply(a, &mut show, &mut e, &mut st);
        }
        assert!(is_playing(&e, d));
        assert_eq!(e.playhead, Some(show.cues[0].id));
    }

    #[test]
    fn capturing_a_hotkey_takes_the_next_allowed_key() {
        use egui::Modifiers as M;
        let (mut show, mut e, mut st) = running();
        let d = number(&show, "4");
        let other = number(&show, "1");
        show.set_hotkey(other, Some("Q".into()));
        apply(Action::CaptureHotkey(Some(d)), &mut show, &mut e, &mut st);
        assert_eq!(st.capture_hotkey, Some(d));
        // While waiting, Space and Esc don't GO or panic; a key that
        // can't be a hotkey is ignored and the wait goes on.
        assert!(keys_with(&show, &st, &[(Key::Space, M::NONE)]).is_empty());
        assert!(keys_with(&show, &st, &[(Key::ArrowUp, M::NONE)]).is_empty());
        // A usable key becomes the hotkey, and takes it from the cue that had it.
        let a = keys_with(&show, &st, &[(Key::Q, M::NONE)]);
        assert_eq!(a.len(), 1);
        for a in a {
            apply(a, &mut show, &mut e, &mut st);
        }
        assert_eq!(st.capture_hotkey, None);
        assert_eq!(show.cue(d).unwrap().hotkey.as_deref(), Some("Q"));
        assert_eq!(show.cue(other).unwrap().hotkey, None);
        // Esc cancels the wait without panicking.
        st.capture_hotkey = Some(d);
        let a = keys_with(&show, &st, &[(Key::Escape, M::NONE)]);
        assert!(matches!(a[..], [Action::CaptureHotkey(None)]));
    }

    #[test]
    fn cart_buttons_fire_and_stop_cues() {
        let (mut show, mut e, mut st) = running();
        let d = number(&show, "4");
        apply(Action::ToggleCart, &mut show, &mut e, &mut st);
        assert!(st.show_cart);
        apply(Action::FireCue(d), &mut show, &mut e, &mut st);
        assert!(is_playing(&e, d));
        apply(Action::StopCue(d), &mut show, &mut e, &mut st);
        assert!(!is_playing(&e, d));
        // A disarmed cue is not fired by its button either.
        let i = show.cue_index(d).unwrap();
        show.cues[i].armed = false;
        apply(Action::FireCue(d), &mut show, &mut e, &mut st);
        assert!(!is_playing(&e, d));
    }

    #[test]
    fn midi_notes_fire_the_bound_cue_or_the_cue_with_that_number() {
        let (mut show, mut e, mut st) = running();
        let d = number(&show, "4");
        // Unbound: the note number is a cue number, as before.
        run_remote(Command::Note(4), &mut show, &mut e, &mut st);
        assert!(is_playing(&e, d));
        run_remote(Command::StopAll, &mut show, &mut e, &mut st);
        // Bound: note 60 fires cue 4 whatever its number.
        apply(Action::SetMidiNote(d, Some(60)), &mut show, &mut e, &mut st);
        run_remote(Command::Note(60), &mut show, &mut e, &mut st);
        assert!(is_playing(&e, d));
        // Binding wins over a cue whose number is the note.
        run_remote(Command::StopAll, &mut show, &mut e, &mut st);
        let a = number(&show, "1");
        apply(Action::SetMidiNote(a, Some(4)), &mut show, &mut e, &mut st);
        run_remote(Command::Note(4), &mut show, &mut e, &mut st);
        assert!(is_playing(&e, a) && !is_playing(&e, d));
        // Neither: logged, nothing plays.
        run_remote(Command::StopAll, &mut show, &mut e, &mut st);
        run_remote(Command::Note(99), &mut show, &mut e, &mut st);
        assert!(!e.is_active());
        assert!(e.messages.back().unwrap().1.contains("MIDI note 99"));
    }

    // ----- Undo and redo -----------------------------------------------------------

    /// Run what draw() does about history at the end of a frame.
    fn frame_end(show: &Show, st: &mut UiState, now: f64, acted: bool) {
        let json = serde_json::to_string(show).unwrap();
        if !st.history.started() {
            st.history.reset(json.clone());
        }
        track_history(st, &json, now, acted);
    }

    fn name_of(show: &Show, n: &str) -> String {
        show.cue(number(show, n)).unwrap().name.clone()
    }

    #[test]
    fn undo_and_redo_step_through_edits() {
        let (mut show, mut e, mut st) = running();
        frame_end(&show, &mut st, 0.0, false);
        let d = number(&show, "4");
        let original = name_of(&show, "4");
        // Typing a new name for a while is one step.
        let i = show.cue_index(d).unwrap();
        for (n, t) in [("S", 1.0), ("St", 1.1), ("Strike", 1.2)] {
            show.cues[i].name = n.into();
            frame_end(&show, &mut st, t, false);
        }
        frame_end(&show, &mut st, 3.0, false);
        show.cues[i].notes = "check the fly".into();
        frame_end(&show, &mut st, 4.0, false);
        frame_end(&show, &mut st, 6.0, false);

        apply(Action::Undo, &mut show, &mut e, &mut st);
        assert_eq!(show.cue(d).unwrap().notes, "");
        assert_eq!(name_of(&show, "4"), "Strike");
        apply(Action::Undo, &mut show, &mut e, &mut st);
        assert_eq!(name_of(&show, "4"), original);
        assert!(!st.history.can_undo());
        apply(Action::Redo, &mut show, &mut e, &mut st);
        assert_eq!(name_of(&show, "4"), "Strike");
        apply(Action::Redo, &mut show, &mut e, &mut st);
        assert_eq!(show.cue(d).unwrap().notes, "check the fly");
        assert!(!st.history.can_redo());
        // Being undone and redone doesn't count as new edits.
        frame_end(&show, &mut st, 9.0, false);
        frame_end(&show, &mut st, 12.0, false);
        assert!(st.history.can_undo() && !st.history.can_redo());
    }

    #[test]
    fn structure_changes_undo_as_their_own_steps() {
        let (mut show, mut e, mut st) = running();
        frame_end(&show, &mut st, 0.0, false);
        let before = show.cues.len();
        // Two actions in quick succession are two steps, not one burst.
        apply(Action::AddCue(CueType::Wait), &mut show, &mut e, &mut st);
        frame_end(&show, &mut st, 1.0, true);
        apply(Action::AddCue(CueType::Wait), &mut show, &mut e, &mut st);
        frame_end(&show, &mut st, 1.05, true);
        assert_eq!(show.cues.len(), before + 2);
        apply(Action::Undo, &mut show, &mut e, &mut st);
        assert_eq!(show.cues.len(), before + 1);
        apply(Action::Undo, &mut show, &mut e, &mut st);
        assert_eq!(show.cues.len(), before);
        apply(Action::Redo, &mut show, &mut e, &mut st);
        assert_eq!(show.cues.len(), before + 1);
    }

    #[test]
    fn undo_leaves_playback_alone_and_keeps_the_selection_valid() {
        let (mut show, mut e, mut st) = running();
        frame_end(&show, &mut st, 0.0, false);
        let d = number(&show, "4");
        e.fire(&show, d);
        assert!(is_playing(&e, d));
        // Add a cue and select it, then undo the add.
        apply(Action::AddCue(CueType::Wait), &mut show, &mut e, &mut st);
        frame_end(&show, &mut st, 1.0, true);
        let added = st.selected.unwrap();
        assert!(show.cue(added).is_some());
        apply(Action::Undo, &mut show, &mut e, &mut st);
        assert!(show.cue(added).is_none());
        assert!(
            st.selected.is_none_or(|id| show.cue(id).is_some()),
            "the selection must not point at a cue that's gone"
        );
        assert!(e.playhead.is_none_or(|id| show.cue(id).is_some()));
        assert!(is_playing(&e, d), "what was playing keeps playing");
    }

    #[test]
    fn opening_or_creating_a_show_starts_a_fresh_history() {
        let (mut show, mut e, mut st) = running();
        frame_end(&show, &mut st, 0.0, false);
        show.cues[0].name = "Edited".into();
        frame_end(&show, &mut st, 1.0, false);
        frame_end(&show, &mut st, 3.0, false);
        assert!(st.history.can_undo());
        load_into(Show::default(), None, &mut show, &mut e, &mut st);
        assert!(!st.history.can_undo() && !st.history.can_redo());
        apply(Action::Undo, &mut show, &mut e, &mut st);
        assert!(show.cues.is_empty(), "undo can't reach into the old show");
    }

    #[test]
    fn undo_redo_keys() {
        use egui::Modifiers as M;
        let st = UiState::default();
        assert!(matches!(
            keys(&st, &[(Key::Z, M::COMMAND)])[..],
            [Action::Undo]
        ));
        assert!(matches!(
            keys(&st, &[(Key::Y, M::COMMAND)])[..],
            [Action::Redo]
        ));
        assert!(matches!(
            keys(&st, &[(Key::Z, M::COMMAND | M::SHIFT)])[..],
            [Action::Redo]
        ));
        assert!(keys(&st, &[(Key::Z, M::NONE)]).is_empty());
    }

    #[test]
    fn a_restored_show_still_shows_unsaved_changes() {
        let (mut show, mut e, mut st) = running();
        st.set_saved(&show);
        frame_end(&show, &mut st, 0.0, false);
        show.cues[0].name = "Edited".into();
        frame_end(&show, &mut st, 1.0, false);
        frame_end(&show, &mut st, 3.0, false);
        // Undoing all the way back is the saved show again.
        apply(Action::Undo, &mut show, &mut e, &mut st);
        assert_eq!(
            serde_json::to_string(&show).ok(),
            st.saved_json,
            "undo to the saved state matches what is on disk"
        );
    }

    #[test]
    fn remote_timecode_fires_cues_without_moving_the_playhead() {
        use crate::timecode::{Rate, Timecode};
        let (mut show, mut e, mut st) = running();
        let d = number(&show, "4");
        show.cue_mut(d).unwrap().timecode = Some(Timecode::new(0, 0, 2, 0));
        run_remote(
            Command::TimecodeLocate(Timecode::new(0, 0, 1, 20), Rate::Fps25),
            &mut show,
            &mut e,
            &mut st,
        );
        assert!(!is_playing(&e, d), "locating fires nothing");
        for f in [22, 24] {
            let t = Timecode::new(0, 0, 1, f);
            run_remote(
                Command::Timecode(t, Rate::Fps25),
                &mut show,
                &mut e,
                &mut st,
            );
        }
        assert!(!is_playing(&e, d));
        run_remote(
            Command::Timecode(Timecode::new(0, 0, 2, 1), Rate::Fps25),
            &mut show,
            &mut e,
            &mut st,
        );
        assert!(is_playing(&e, d));
        assert_eq!(e.playhead, Some(show.cues[0].id));
    }

    #[test]
    fn a_send_cue_is_listed_by_what_it_sends() {
        let mut show = Show::default();
        let c = show.make_cue(CueType::Send, None, 0);
        assert_eq!(cue_target_text(&show, &c), "OSC 127.0.0.1:53000 /go");
        assert_eq!(c.kind.cue_type().label(), "Send");
    }

    // ----- The media window ----------------------------------------------------------

    fn media_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("newq_ui_media_{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("new")).unwrap();
        std::fs::write(d.join("new/a.mp4"), "a").unwrap();
        std::fs::write(d.join("new/b.mp4"), "b").unwrap();
        d
    }

    fn video_cue(show: &mut Show, file: PathBuf) -> CueId {
        let c = show.make_cue(CueType::Video, Some(file), show.cues.len());
        let id = c.id;
        show.cues.push(c);
        id
    }

    fn file_of(show: &Show, id: CueId) -> PathBuf {
        show.cue(id)
            .unwrap()
            .kind
            .file_media()
            .unwrap()
            .file
            .clone()
    }

    /// Poll a folder search until it has finished.
    fn wait_for_folder_search(show: &mut Show, st: &mut UiState) {
        let ctx = egui::Context::default();
        let t0 = std::time::Instant::now();
        while st.folder_search.is_some() {
            assert!(t0.elapsed().as_secs() < 10, "the search never finished");
            std::thread::sleep(std::time::Duration::from_millis(10));
            poll_folder_search(&ctx, show, st);
        }
    }

    #[test]
    fn missing_media_opens_the_window_and_a_folder_search_relinks_it() {
        let d = media_dir("search");
        let mut show = Show::default();
        let a = video_cue(&mut show, PathBuf::from("/old/place/a.mp4"));
        let b = video_cue(&mut show, PathBuf::from("/old/place/b.mp4"));
        let lost = video_cue(&mut show, PathBuf::from("/old/place/lost.mp4"));
        let mut e = Engine::for_tests();
        let mut st = UiState::default();
        st.check_media(&show);
        assert!(st.show_media, "files are missing, so the window opens");
        frame_end(&show, &mut st, 0.0, false);

        apply(
            Action::RelinkFolder(d.join("new")),
            &mut show,
            &mut e,
            &mut st,
        );
        wait_for_folder_search(&mut show, &mut st);
        assert_eq!(file_of(&show, a), d.join("new/a.mp4"));
        assert_eq!(file_of(&show, b), d.join("new/b.mp4"));
        assert_eq!(file_of(&show, lost), PathBuf::from("/old/place/lost.mp4"));
        assert!(
            st.media_note.starts_with("Found 2 of 3"),
            "{}",
            st.media_note
        );
        assert_eq!(relink::missing(&show).len(), 1);
        // It is one step in the undo history.
        frame_end(&show, &mut st, 1.0, true);
        apply(Action::Undo, &mut show, &mut e, &mut st);
        assert_eq!(relink::missing(&show).len(), 3);

        // Nothing missing: opening a show doesn't pop the window.
        let mut ok = Show::default();
        video_cue(&mut ok, d.join("new/a.mp4"));
        let mut st2 = UiState::default();
        st2.check_media(&ok);
        assert!(!st2.show_media);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn locating_one_file_relinks_every_cue_that_shared_it() {
        let d = media_dir("locate");
        let mut show = Show::default();
        let old = PathBuf::from("/old/shared.mp4");
        let one = video_cue(&mut show, old.clone());
        let two = video_cue(&mut show, old);
        let other = video_cue(&mut show, PathBuf::from("/old/other.mp4"));
        let mut e = Engine::for_tests();
        let mut st = UiState::default();
        apply(
            Action::RelinkCue(one, d.join("new/a.mp4")),
            &mut show,
            &mut e,
            &mut st,
        );
        assert_eq!(file_of(&show, one), d.join("new/a.mp4"));
        assert_eq!(file_of(&show, two), d.join("new/a.mp4"));
        assert_eq!(file_of(&show, other), PathBuf::from("/old/other.mp4"));
        assert!(
            st.media_note.starts_with("Linked 2 cues"),
            "{}",
            st.media_note
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn searching_a_folder_runs_in_the_background_and_then_relinks_the_cues() {
        let d = media_dir("folder_search");
        let mut show = Show::default();
        let a = video_cue(&mut show, d.join("old/act1/a.mp4"));
        let gone = video_cue(&mut show, d.join("old/gone.mp4"));
        let b = video_cue(&mut show, d.join("old/b.mp4"));
        std::fs::create_dir_all(d.join("new/act1")).unwrap();
        std::fs::write(d.join("new/act1/a.mp4"), "a").unwrap();
        std::fs::write(d.join("new/b.mp4"), "b").unwrap();
        let mut e = Engine::for_tests();
        let mut st = UiState::default();
        frame_end(&show, &mut st, 0.0, false);
        apply(
            Action::RelinkFolder(d.join("new")),
            &mut show,
            &mut e,
            &mut st,
        );
        // The request returns at once, with the search still to do.
        assert!(st.folder_search.is_some());
        assert_eq!(file_of(&show, a), d.join("old/act1/a.mp4"), "not yet");
        // A second request while one runs is ignored.
        apply(
            Action::RelinkFolder(d.join("other")),
            &mut show,
            &mut e,
            &mut st,
        );
        assert_eq!(st.folder_search.as_ref().unwrap().root, d.join("new"));
        wait_for_folder_search(&mut show, &mut st);
        assert_eq!(file_of(&show, a), d.join("new/act1/a.mp4"));
        assert_eq!(file_of(&show, b), d.join("new/b.mp4"));
        assert_eq!(
            file_of(&show, gone),
            d.join("old/gone.mp4"),
            "not found stays"
        );
        assert!(
            st.media_note.starts_with("Found 2 of 3 missing files"),
            "{}",
            st.media_note
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn collecting_media_copies_in_the_background_and_then_updates_the_cues() {
        let d = media_dir("collect");
        let mut show = Show::default();
        let a = video_cue(&mut show, d.join("new/a.mp4"));
        let b = video_cue(&mut show, d.join("new/b.mp4"));
        let gone = video_cue(&mut show, d.join("new/gone.mp4"));
        let mut e = Engine::for_tests();
        let mut st = UiState::default();
        frame_end(&show, &mut st, 0.0, false);
        let dest = d.join("show/media");
        apply(
            Action::CollectMedia(dest.clone()),
            &mut show,
            &mut e,
            &mut st,
        );
        assert!(st.collect_job.is_some());
        // A second request while one runs is ignored.
        apply(
            Action::CollectMedia(d.join("other")),
            &mut show,
            &mut e,
            &mut st,
        );
        let ctx = egui::Context::default();
        let t0 = std::time::Instant::now();
        while st.collect_job.is_some() {
            assert!(t0.elapsed().as_secs() < 10, "the copy never finished");
            std::thread::sleep(std::time::Duration::from_millis(10));
            poll_collect(&ctx, &mut show, &mut st);
        }
        assert_eq!(file_of(&show, a), dest.join("a.mp4"));
        assert_eq!(file_of(&show, b), dest.join("b.mp4"));
        assert_eq!(
            file_of(&show, gone),
            d.join("new/gone.mp4"),
            "a missing file stays as it was"
        );
        assert_eq!(std::fs::read_to_string(dest.join("b.mp4")).unwrap(), "b");
        assert!(
            st.media_note.starts_with("Copied 2 files"),
            "{}",
            st.media_note
        );
        assert!(
            st.media_note.contains("1 could not be copied"),
            "{}",
            st.media_note
        );
        assert!(!d.join("other").exists(), "the second request was ignored");
        // The change can be undone (the copies stay on disk).
        assert!(st.history.can_undo());
        apply(Action::Undo, &mut show, &mut e, &mut st);
        assert_eq!(file_of(&show, a), d.join("new/a.mp4"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn finding_on_the_computer_suggests_files_and_they_can_all_be_taken() {
        let d = media_dir("find");
        // The show is in d/shows; its media turned up in d/shows/media. (The
        // helper's own copies elsewhere would be fair suggestions too.)
        std::fs::remove_dir_all(d.join("new")).unwrap();
        std::fs::create_dir_all(d.join("shows/media")).unwrap();
        std::fs::write(d.join("shows/media/a.mp4"), "a").unwrap();
        std::fs::write(d.join("shows/media/b.mp4"), "b").unwrap();
        let mut show = Show::default();
        let a = video_cue(&mut show, PathBuf::from("/old/a.mp4"));
        let b = video_cue(&mut show, PathBuf::from("/old/b.mp4"));
        let lost = video_cue(&mut show, PathBuf::from("/old/lost.mp4"));
        let mut e = Engine::for_tests();
        let mut st = UiState {
            show_path: Some(d.join("shows/hamlet.newq")),
            ..UiState::default()
        };
        frame_end(&show, &mut st, 0.0, false);
        apply(Action::FindOnComputer, &mut show, &mut e, &mut st);
        let search = st.search.as_ref().expect("a search started");
        // Suggestions arrive while it is still looking; wait for these two.
        let wanted = relink::missing(&show)
            .into_iter()
            .map(|m| m.1)
            .collect::<Vec<_>>();
        let t0 = std::time::Instant::now();
        let suggested = loop {
            let s = relink::suggestions(&search.state().hits, &wanted);
            if s.len() == 2 {
                break s;
            }
            assert!(t0.elapsed().as_secs() < 20, "no suggestions: {s:?}");
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        assert_eq!(
            suggested[&PathBuf::from("/old/a.mp4")][0],
            d.join("shows/media/a.mp4")
        );
        // Take them all, as the button does.
        let pairs: Vec<(PathBuf, PathBuf)> = suggested
            .iter()
            .filter_map(|(old, c)| Some((old.clone(), c.first()?.clone())))
            .collect();
        apply(Action::RelinkMany(pairs), &mut show, &mut e, &mut st);
        assert_eq!(file_of(&show, a), d.join("shows/media/a.mp4"));
        assert_eq!(file_of(&show, b), d.join("shows/media/b.mp4"));
        assert_eq!(file_of(&show, lost), PathBuf::from("/old/lost.mp4"));
        assert!(
            st.media_note.starts_with("Linked 2 cues"),
            "{}",
            st.media_note
        );
        // One step to undo.
        frame_end(&show, &mut st, 1.0, true);
        apply(Action::Undo, &mut show, &mut e, &mut st);
        assert_eq!(relink::missing(&show).len(), 3);
        // Nothing missing means nothing to search for.
        let mut fine = Show::default();
        video_cue(&mut fine, d.join("shows/media/a.mp4"));
        st.search = None;
        apply(Action::FindOnComputer, &mut fine, &mut e, &mut st);
        assert!(st.search.is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    // ----- Trimming on the waveform ---------------------------------------------------

    #[test]
    fn a_drag_picks_the_nearer_end_and_moves_only_that() {
        // A 10 s file played from 2 s to 6 s.
        assert_eq!(nearest_handle(1.0, 2.0, Some(6.0), 10.0), TrimHandle::In);
        assert_eq!(nearest_handle(3.9, 2.0, Some(6.0), 10.0), TrimHandle::In);
        assert_eq!(nearest_handle(4.1, 2.0, Some(6.0), 10.0), TrimHandle::Out);
        assert_eq!(nearest_handle(9.0, 2.0, Some(6.0), 10.0), TrimHandle::Out);
        // With no out point the out end is the end of the file.
        assert_eq!(nearest_handle(9.5, 2.0, None, 10.0), TrimHandle::Out);
        assert_eq!(nearest_handle(5.0, 2.0, None, 10.0), TrimHandle::In);

        let m = |h, t, s, e| move_handle(h, t, s, e, 10.0);
        assert_eq!(m(TrimHandle::In, 3.0, 2.0, Some(6.0)), (3.0, Some(6.0)));
        assert_eq!(m(TrimHandle::Out, 8.0, 2.0, Some(6.0)), (2.0, Some(8.0)));
        assert_eq!(m(TrimHandle::Out, 8.0, 2.0, None), (2.0, Some(8.0)));
    }

    #[test]
    fn the_ends_stay_in_order_and_inside_the_file() {
        let m = |h, t, s, e| move_handle(h, t, s, e, 10.0);
        // The in point can't pass the out point, or go before the start.
        let (s, e) = m(TrimHandle::In, 9.0, 2.0, Some(6.0));
        assert!((s - (6.0 - MIN_TRIM)).abs() < 1e-9 && e == Some(6.0));
        assert_eq!(m(TrimHandle::In, -3.0, 2.0, Some(6.0)), (0.0, Some(6.0)));
        // Nor can the out point come back past the in point, or run off the end.
        let (s, e) = m(TrimHandle::Out, 0.5, 2.0, Some(6.0));
        assert_eq!(s, 2.0);
        assert!((e.unwrap() - (2.0 + MIN_TRIM)).abs() < 1e-9);
        // Dragged to the end of the file it is no out point at all.
        assert_eq!(m(TrimHandle::Out, 10.0, 2.0, Some(6.0)), (2.0, None));
        assert_eq!(m(TrimHandle::Out, 99.0, 2.0, Some(6.0)), (2.0, None));
        assert_eq!(m(TrimHandle::Out, 9.99, 2.0, Some(6.0)), (2.0, None));
        // With no out point, the in point is limited by the end of the file.
        let (s, e) = m(TrimHandle::In, 12.0, 2.0, None);
        assert!((s - (10.0 - MIN_TRIM)).abs() < 1e-9 && e.is_none());
        // A very short file doesn't break the limits.
        let (s, e) = move_handle(TrimHandle::Out, 0.0, 0.0, None, 0.03);
        assert_eq!(s, 0.0);
        assert!(e.is_none());
    }

    #[test]
    fn exporting_the_cue_sheet_writes_the_file_and_says_so() {
        let dir = std::env::temp_dir().join("newq_ui_export");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (mut show, mut e, mut st) = running();
        let page = dir.join("sheet.html");
        apply(
            Action::ExportCueSheet(page.clone()),
            &mut show,
            &mut e,
            &mut st,
        );
        let text = std::fs::read_to_string(&page).unwrap();
        assert!(text.contains("<table>") && text.contains(&show.cues[0].name));
        let (_, msg) = e.messages.back().unwrap();
        assert!(
            msg.starts_with(&format!("Exported {} cues", show.cues.len())),
            "{msg}"
        );
        let csv = dir.join("sheet.csv");
        apply(
            Action::ExportCueSheet(csv.clone()),
            &mut show,
            &mut e,
            &mut st,
        );
        assert!(
            std::fs::read_to_string(&csv)
                .unwrap()
                .contains("Number,Name,Type")
        );
        // A place that can't be written to is reported, and nothing else happens.
        apply(
            Action::ExportCueSheet(dir.join("nope/x.csv")),
            &mut show,
            &mut e,
            &mut st,
        );
        assert!(e.messages.back().unwrap().1.starts_with("Could not export"));
        assert!(!st.dirty, "exporting isn't an edit");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clock_cue_shows_its_time_in_the_list_and_waits_are_readable() {
        let mut show = Show::default();
        let mut c = show.make_cue(CueType::Wait, None, 0);
        c.clock = TimeOfDay::new(18, 30, 0);
        c.hotkey = Some("Q".into());
        assert!(
            name_with_key(&c).ends_with("[Q]   at 18:30:00"),
            "{}",
            name_with_key(&c)
        );
        assert_eq!(fmt_wait(42), "42 s");
        assert_eq!(fmt_wait(754), "12 min 34 s");
        assert_eq!(fmt_wait(3 * 3600 + 5 * 60 + 9), "3 h 05 min");
        assert_eq!(fmt_wait(86_400), "24 h 00 min");
    }

    fn preview(n: u64) -> PreviewOutput {
        PreviewOutput {
            id: OutputId(n),
            name: format!("Output {n}"),
            layers: Vec::new(),
            warp: crate::gpu::IDENTITY,
            canvas: (1920, 1080),
        }
    }

    #[test]
    fn the_preview_follows_the_menu_but_not_while_editing_corners() {
        let previews = [preview(1), preview(2), preview(3)];
        let pick = |chosen: Option<u64>, editing: Option<u64>| {
            preview_index(&previews, chosen.map(OutputId), editing.map(OutputId))
        };
        assert_eq!(pick(None, None), Some(0), "the first by default");
        assert_eq!(pick(Some(3), None), Some(2));
        assert_eq!(pick(Some(9), None), Some(0), "an output that is gone");
        assert_eq!(pick(Some(3), Some(2)), Some(1), "editing wins");
        assert_eq!(
            pick(Some(2), Some(9)),
            Some(1),
            "editing something not shown"
        );
        assert_eq!(preview_index(&[], None, None), None);
    }

    /// Run frames of the scrub bar under `events`; the bar's rectangle and
    /// every action it asked for.
    fn scrub_frames(
        show: &Show,
        engine: &Engine,
        st: &mut UiState,
        mut frames: Vec<Vec<Event>>,
    ) -> (Rect, Vec<Action>) {
        let ctx = egui::Context::default();
        let out = show.outputs[0].id;
        let mut all = Vec::new();
        let mut area = Rect::NOTHING;
        frames.insert(0, vec![]);
        for (n, events) in frames.into_iter().enumerate() {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 200.0))),
                time: Some(1.0 + n as f64 * 0.01),
                events,
                ..Default::default()
            };
            let mut actions = Vec::new();
            let mut o = ctx.run_ui(input, |ui| {
                scrub_bar(ui, show, engine, st, out, &mut actions);
                area = ui.min_rect();
            });
            o.textures_delta.clear();
            all.extend(actions);
        }
        (area, all)
    }

    /// A show with a ten second still playing, or None to skip.
    fn playing_still() -> Option<(Show, Engine)> {
        let (ff, img) = crate::media::test_media("color_red.png")?;
        let mut show = Show::default();
        let mut c = show.make_cue(CueType::Image, Some(img), 0);
        c.kind.media_mut().unwrap().end = Some(10.0);
        let id = c.id;
        show.cues.push(c);
        let mut engine = Engine::for_tests();
        engine.ff = Some(ff);
        engine.playhead = Some(id);
        engine.go(&show);
        let t0 = std::time::Instant::now();
        while engine.active.is_empty() || !engine.active.iter().all(|a| a.started) {
            assert!(t0.elapsed().as_secs() < 10, "the still never loaded");
            std::thread::sleep(std::time::Duration::from_millis(5));
            engine.tick(&show, 0.005);
        }
        Some((show, engine))
    }

    #[test]
    fn dragging_the_scrub_bar_asks_to_seek() {
        let Some((show, engine)) = playing_still() else {
            return;
        };
        let instance = engine.active[0].instance;
        let mut st = UiState::default();
        let (area, none) = scrub_frames(&show, &engine, &mut st, vec![vec![], vec![]]);
        assert!(none.is_empty(), "looking asks for nothing");
        let y = area.bottom() - 9.0;
        let at = |f: f32| pos2(area.left() + 4.0 + f * (area.width() - 16.0), y);
        // Press near the start, drag to the middle, let go there.
        let (_, seeks) = scrub_frames(
            &show,
            &engine,
            &mut st,
            vec![
                vec![Event::PointerMoved(at(0.1))],
                vec![button(at(0.1), true)],
                vec![Event::PointerMoved(at(0.5))],
                vec![Event::PointerMoved(at(0.5)), button(at(0.5), false)],
                vec![],
            ],
        );
        let asked: Vec<(u64, f64)> = seeks
            .iter()
            .filter_map(|a| match a {
                Action::Seek(i, t) => Some((*i, *t)),
                _ => None,
            })
            .collect();
        assert!(!asked.is_empty(), "dragging moves the cue");
        assert!(asked.iter().all(|(i, _)| *i == instance));
        let last = asked.last().unwrap().1;
        assert!((4.0..6.0).contains(&last), "the last ask was {last}");
    }

    #[test]
    fn nothing_playing_means_nothing_to_scrub() {
        let show = Show::default();
        let engine = Engine::for_tests();
        let mut st = UiState::default();
        let (_, seeks) = scrub_frames(&show, &engine, &mut st, vec![vec![]]);
        assert!(seeks.is_empty() && st.scrub.is_none());
    }

    fn monitor(name: &str, model: Option<&str>) -> MonitorInfo {
        MonitorInfo {
            name: name.into(),
            model: model.map(str::to_string),
            size: (1920, 1080),
            refresh_hz: None,
            primary: false,
        }
    }

    #[test]
    fn displays_are_called_what_they_are_when_the_system_says() {
        let st = UiState {
            monitors: vec![
                monitor("Monitor #1234", Some("Built-in Retina Display")),
                monitor("Monitor #5678", Some("SAMSUNG")),
                monitor("\\\\.\\DISPLAY3", None),
            ],
            ..Default::default()
        };
        assert!(monitor_label(&st, 0).starts_with("1: Built-in Retina Display ("));
        assert!(monitor_label(&st, 1).starts_with("2: SAMSUNG ("));
        assert!(
            monitor_label(&st, 2).starts_with("3: \\\\.\\DISPLAY3 ("),
            "no model known: the system's name"
        );
    }

    #[test]
    fn the_displays_panel_draws_and_says_why_an_output_is_not_on_its_display() {
        let mut show = Show::default();
        let a = show.add_display();
        let b = show.add_display();
        let c = show.add_display();
        show.displays[0].name = "Stage TV".into();
        show.displays[0].monitor = Some(DisplayRef {
            name: "SAMSUNG".into(),
            index: 1,
        });
        show.displays[1].monitor = Some(DisplayRef {
            name: "Laptop".into(),
            index: 0,
        });
        show.outputs[0].placement = Placement::Slot(a);
        let mut st = UiState {
            monitors: vec![
                monitor("Laptop", None),
                monitor("Monitor #2", Some("SAMSUNG")),
            ],
            ..Default::default()
        };
        st.display_states.insert(a, SlotState::Connected(1));
        st.display_states
            .insert(b, SlotState::OnMainWindowDisplay(0));
        st.display_states.insert(c, SlotState::Unassigned);
        let ctx = egui::Context::default();
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(500.0, 700.0))),
            time: Some(1.0),
            ..Default::default()
        };
        let mut actions = Vec::new();
        let mut out = ctx.run_ui(input, |ui| displays_panel(ui, &mut show, &st, &mut actions));
        out.textures_delta.clear();
        assert!(out.shapes.len() > 10 && actions.is_empty());

        // Why an output isn't on its display, by what the display is attached to.
        st.display_states.insert(a, SlotState::Missing);
        let why = |st: &UiState, id| display_problem(&show.displays, st, id);
        assert_eq!(
            why(&st, a),
            "Stage TV (SAMSUNG) isn't connected: not shown on any display"
        );
        assert!(why(&st, b).contains("main window is on"));
        assert!(why(&st, c).contains("no monitor yet"));
        assert!(why(&st, DisplayId(999)).contains("removed"));
    }

    /// What the keyboard asks for in the last of these frames of events. (egui
    /// decides for itself whether a key event is a repeat, from whether the
    /// key was already down, so a repeat needs a press before it.)
    fn keyboard_asks(frames: Vec<Vec<Event>>) -> Vec<Action> {
        let ctx = egui::Context::default();
        let (show, st) = (Show::default(), UiState::default());
        let mut actions = Vec::new();
        for (n, events) in frames.into_iter().enumerate() {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 200.0))),
                time: Some(1.0 + n as f64 * 0.03),
                events,
                ..Default::default()
            };
            actions.clear();
            let mut out = ctx.run_ui(input, |ui| keyboard(ui.ctx(), &show, &st, &mut actions));
            out.textures_delta.clear();
        }
        actions
    }

    fn press(key: Key) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    fn release(key: Key) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: false,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    #[test]
    fn holding_a_key_does_not_repeat_its_action() {
        let go = |a: &[Action]| a.iter().filter(|a| matches!(a, Action::Go)).count();
        let panic = |a: &[Action]| a.iter().filter(|a| matches!(a, Action::Panic)).count();
        let down = |a: &[Action]| {
            a.iter()
                .filter(|a| matches!(a, Action::SelectRelative(1)))
                .count()
        };
        // The first frame of a press acts; the frames of it being held don't.
        assert_eq!(go(&keyboard_asks(vec![vec![press(Key::Space)]])), 1);
        let held = |key| {
            vec![
                vec![press(key)],
                vec![press(key)], // a repeat, since the key is still down
                vec![press(key)],
            ]
        };
        assert_eq!(go(&keyboard_asks(held(Key::Space))), 0);
        assert_eq!(panic(&keyboard_asks(held(Key::Escape))), 0);
        // Let go and press again: that is a new press.
        assert_eq!(
            go(&keyboard_asks(vec![
                vec![press(Key::Space)],
                vec![release(Key::Space)],
                vec![press(Key::Space)],
            ])),
            1
        );
        // Moving down the list is meant to repeat.
        assert_eq!(down(&keyboard_asks(held(Key::ArrowDown))), 1);
    }

    /// A dialog that stays open until `release` is sent, then answers with it.
    fn blocked_dialog() -> (
        std::sync::mpsc::Sender<Option<PathBuf>>,
        impl FnOnce() -> Option<PathBuf> + Send + 'static,
    ) {
        let (release, wait) = std::sync::mpsc::channel::<Option<PathBuf>>();
        (release, move || wait.recv().ok().flatten())
    }

    fn poll_until_answered(show: &Show, st: &mut UiState) -> Vec<Action> {
        let ctx = egui::Context::default();
        let t0 = std::time::Instant::now();
        loop {
            let mut actions = Vec::new();
            poll_dialog(&ctx, show, st, &mut actions);
            if st.dialog.is_none() {
                return actions;
            }
            assert!(t0.elapsed().as_secs() < 10, "the dialog never answered");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn a_file_dialog_does_not_stop_the_show_while_it_is_open() {
        let mut show = Show::default();
        let mut c = show.make_cue(CueType::Wait, None, 0);
        if let CueKind::Wait(w) = &mut c.kind {
            w.duration = 5.0;
        }
        let wait = c.id;
        show.cues.push(c);
        let mut e = Engine::for_tests();
        let mut st = UiState::default();
        let (release, ask) = blocked_dialog();
        start_dialog_with(&mut st, &show, DialogFor::AddCue(CueType::Video), ask);
        assert!(st.dialog.is_some());
        // The dialog is open, and the show runs on behind it.
        e.fire(&show, wait);
        let ctx = egui::Context::default();
        let mut actions = Vec::new();
        for _ in 0..5 {
            e.tick(&show, 0.1);
            poll_dialog(&ctx, &show, &mut st, &mut actions);
        }
        assert!(actions.is_empty(), "nothing is chosen yet");
        assert!(
            matches!(e.status(wait), crate::engine::CueStatus::Playing { elapsed, .. } if elapsed > 0.4)
        );
        // A second request while one is open is ignored.
        let (_, ask2) = blocked_dialog();
        start_dialog_with(&mut st, &show, DialogFor::OpenShow, ask2);
        assert_eq!(
            st.dialog.as_ref().unwrap().purpose,
            DialogFor::AddCue(CueType::Video)
        );
        // Closing the dialog on a file asks for what it was for.
        release
            .send(Some(PathBuf::from("/media/clip.mp4")))
            .unwrap();
        let actions = poll_until_answered(&show, &mut st);
        assert!(matches!(
            actions.as_slice(),
            [Action::AddCueFile(CueType::Video, p)] if p == Path::new("/media/clip.mp4")
        ));
    }

    #[test]
    fn cancelling_a_file_dialog_asks_for_nothing() {
        let show = Show::default();
        let mut st = UiState::default();
        let (release, ask) = blocked_dialog();
        start_dialog_with(&mut st, &show, DialogFor::ExportCueSheet, ask);
        release.send(None).unwrap();
        assert!(poll_until_answered(&show, &mut st).is_empty());
        // And another can be opened after it.
        let (_, ask) = blocked_dialog();
        start_dialog_with(&mut st, &show, DialogFor::OpenShow, ask);
        assert!(st.dialog.is_some());
    }

    #[test]
    fn what_a_dialog_asks_for_depends_on_what_it_was_for() {
        let p = || PathBuf::from("/x/y");
        let id = CueId(4);
        assert!(matches!(
            DialogFor::OpenShow.answer(p(), true),
            Action::OpenShowFile(_, true)
        ));
        assert!(matches!(
            DialogFor::SaveShow.answer(p(), false),
            Action::SaveShowTo(_)
        ));
        assert!(matches!(
            DialogFor::AddCue(CueType::Audio).answer(p(), false),
            Action::AddCueFile(CueType::Audio, _)
        ));
        assert!(matches!(
            DialogFor::BrowseFile(id, CueType::Video).answer(p(), false),
            Action::SetCueFile(CueId(4), _)
        ));
        assert!(matches!(
            DialogFor::LocateFile(id, CueType::Video).answer(p(), false),
            Action::RelinkCue(CueId(4), _)
        ));
        assert!(matches!(
            DialogFor::ExportCueSheet.answer(p(), false),
            Action::ExportCueSheet(_)
        ));
        assert!(matches!(
            DialogFor::RelinkFolder.answer(p(), false),
            Action::RelinkFolder(_)
        ));
        assert!(matches!(
            DialogFor::CollectMedia.answer(p(), false),
            Action::CollectMedia(_)
        ));
    }

    #[test]
    fn editing_the_show_while_the_open_dialog_is_up_is_noticed() {
        let mut show = Show::default();
        let mut st = UiState::default();
        let (release, ask) = blocked_dialog();
        start_dialog_with(&mut st, &show, DialogFor::OpenShow, ask);
        // The operator edits the show while the dialog is open.
        show.name = "Edited meanwhile".into();
        release
            .send(Some(PathBuf::from("/shows/other.newq")))
            .unwrap();
        let actions = poll_until_answered(&show, &mut st);
        assert!(matches!(
            actions.as_slice(),
            [Action::OpenShowFile(_, true)]
        ));
        // Untouched, it isn't.
        let (release, ask) = blocked_dialog();
        start_dialog_with(&mut st, &show, DialogFor::OpenShow, ask);
        release
            .send(Some(PathBuf::from("/shows/other.newq")))
            .unwrap();
        let actions = poll_until_answered(&show, &mut st);
        assert!(matches!(
            actions.as_slice(),
            [Action::OpenShowFile(_, false)]
        ));
    }

    #[test]
    fn choosing_another_file_for_a_cue_points_it_at_the_file_and_forgets_what_it_knew() {
        let mut show = Show::default();
        let id = video_cue(&mut show, PathBuf::from("/old/a.mp4"));
        let mut e = Engine::for_tests();
        let mut st = UiState::default();
        apply(
            Action::SetCueFile(id, PathBuf::from("/new/b.mp4")),
            &mut show,
            &mut e,
            &mut st,
        );
        assert_eq!(file_of(&show, id), PathBuf::from("/new/b.mp4"));
        // A cue that has gone meanwhile is simply ignored.
        apply(
            Action::SetCueFile(CueId(999), PathBuf::from("/new/c.mp4")),
            &mut show,
            &mut e,
            &mut st,
        );
    }

    /// Poll the save in progress until it has finished.
    fn wait_for_save(engine: &mut Engine, st: &mut UiState) {
        let ctx = egui::Context::default();
        let t0 = std::time::Instant::now();
        while st.saving.is_some() {
            assert!(t0.elapsed().as_secs() < 10, "the save never finished");
            std::thread::sleep(std::time::Duration::from_millis(5));
            poll_save(&ctx, engine, st);
        }
    }

    fn save_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("newq_ui_save_{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn saving_is_written_in_the_background_and_then_the_show_is_clean() {
        let d = save_dir("ok");
        let path = d.join("a.newq");
        let mut show = Show {
            name: "Saved".into(),
            ..Show::default()
        };
        let (mut e, mut st) = (Engine::for_tests(), UiState::default());
        frame_end(&show, &mut st, 0.0, false);
        // An old recovery copy is there, as after unsaved edits.
        std::fs::write(recovery::path_for(Some(&path)), "{}").unwrap();
        apply(Action::SaveShowTo(path.clone()), &mut show, &mut e, &mut st);
        assert!(
            st.saving.is_some(),
            "the request returns with the write still to do"
        );
        // A second request while one runs is ignored.
        apply(
            Action::SaveShowTo(d.join("other.newq")),
            &mut show,
            &mut e,
            &mut st,
        );
        wait_for_save(&mut e, &mut st);
        assert!(st.saving.is_none());
        assert_eq!(Show::load(&path).unwrap().name, "Saved");
        assert!(!d.join("other.newq").exists());
        assert_eq!(st.show_path.as_deref(), Some(path.as_path()));
        assert!(
            !recovery::path_for(Some(&path)).exists(),
            "the recovery copy is cleared"
        );
        assert_eq!(st.saved_json, serde_json::to_string(&show).ok());
        // Save with a path already known saves there, without asking.
        show.name = "Saved again".into();
        apply(Action::Save, &mut show, &mut e, &mut st);
        wait_for_save(&mut e, &mut st);
        assert_eq!(Show::load(&path).unwrap().name, "Saved again");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_failed_save_is_logged_and_leaves_the_show_unsaved() {
        let d = save_dir("fail");
        let mut show = Show::default();
        let (mut e, mut st) = (Engine::for_tests(), UiState::default());
        let before = st.saved_json.clone();
        apply(
            Action::SaveShowTo(d.join("no/such/folder/a.newq")),
            &mut show,
            &mut e,
            &mut st,
        );
        wait_for_save(&mut e, &mut st);
        assert!(st.show_path.is_none(), "still untitled");
        assert_eq!(st.saved_json, before, "and still unsaved");
        assert!(
            e.messages.iter().any(|(_, m)| m.contains("Could not save")),
            "{:?}",
            e.messages
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn leaving_a_show_waits_for_a_save_that_is_still_being_written() {
        let d = save_dir("leave");
        let path = d.join("a.newq");
        let mut show = Show::default();
        let (mut e, mut st) = (Engine::for_tests(), UiState::default());
        frame_end(&show, &mut st, 0.0, false);
        show.name = "Edited".into();
        st.dirty = true;
        apply(Action::SaveShowTo(path.clone()), &mut show, &mut e, &mut st);
        // Quitting right away: it waits for the write, finds nothing unsaved,
        // and doesn't ask.
        assert!(confirm_discard(&show, &mut st));
        assert!(st.saving.is_none());
        assert_eq!(Show::load(&path).unwrap().name, "Edited");
        assert!(!st.dirty);
        let _ = std::fs::remove_dir_all(&d);
    }

    fn long_show(n: usize) -> Show {
        let mut show = Show::default();
        for i in 0..n {
            let mut c = show.make_cue(CueType::Wait, None, show.cues.len());
            c.name = format!("cue {i}");
            show.cues.push(c);
        }
        show
    }

    #[test]
    fn a_long_cue_list_draws_only_the_rows_in_view() {
        let ctx = egui::Context::default();
        let (mut e, mut st) = (Engine::for_tests(), UiState::default());
        let frame = |show: &Show, e: &mut Engine, st: &mut UiState, ctx: &egui::Context| {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 400.0))),
                time: Some(1.0),
                ..Default::default()
            };
            let mut actions = Vec::new();
            let mut out = ctx.run_ui(input, |ui| cue_list(ui, show, e, st, &mut actions));
            out.textures_delta.clear();
            out.shapes.len()
        };
        let few = frame(&long_show(30), &mut e, &mut st, &ctx);
        let many = frame(&long_show(2000), &mut e, &mut st, &egui::Context::default());
        // Drawing every row of 2,000 would be tens of thousands of shapes.
        assert!(many < 1500, "{many} shapes for 2,000 cues");
        assert!(many < few * 3, "{many} shapes for 2,000 cues, {few} for 30");
    }

    #[test]
    fn the_sync_window_draws_and_the_test_cue_loops() {
        let mut show = Show::default();
        let (mut e, mut st) = (Engine::for_tests(), UiState::default());
        st.show_sync = true;
        let ctx = egui::Context::default();
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 700.0))),
            time: Some(1.0),
            ..Default::default()
        };
        let mut actions = Vec::new();
        let mut out = ctx.run_ui(input, |ui| {
            sync_window(ui.ctx(), &mut show, &mut st, &mut actions)
        });
        out.textures_delta.clear();
        assert!(out.shapes.len() > 5, "the window has something in it");
        assert!(actions.is_empty() && st.show_sync);

        apply(
            Action::AddSyncCue(PathBuf::from("sync.mp4")),
            &mut show,
            &mut e,
            &mut st,
        );
        let cue = &show.cues[0];
        assert_eq!(cue.name, "Sync test");
        assert!(cue.kind.media().unwrap().loop_media);

        // Without ffmpeg there is nothing to make, and it says so.
        apply(Action::MakeSyncClip, &mut show, &mut e, &mut st);
        assert!(st.sync_note.contains("ffmpeg") && st.sync_clip.is_none());
    }

    #[test]
    fn scrolling_to_a_cue_far_down_a_long_list_brings_it_into_view() {
        let show = long_show(2000);
        let last = show.cues.last().unwrap().id;
        let ctx = egui::Context::default();
        let (mut e, mut st) = (Engine::for_tests(), UiState::default());
        st.selected = Some(last);
        st.scroll_to_selected = true;
        let mut t = 1.0;
        let mut run = |events: Vec<Event>, st: &mut UiState| {
            t += 1.0 / 60.0;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900.0, 400.0))),
                time: Some(t),
                events,
                ..Default::default()
            };
            let mut actions = Vec::new();
            let mut out = ctx.run_ui(input, |ui| cue_list(ui, &show, &mut e, st, &mut actions));
            out.textures_delta.clear();
            actions
        };
        for _ in 0..4 {
            run(vec![], &mut st);
        }
        assert!(!st.scroll_to_selected, "the request was taken");
        // Click down the list: the last cue is on screen somewhere.
        let mut hit = false;
        for y in [150.0, 190.0, 230.0, 270.0, 310.0, 350.0, 385.0] {
            let at = pos2(300.0, y);
            run(vec![Event::PointerMoved(at)], &mut st);
            run(vec![button(at, true)], &mut st);
            let actions = run(vec![button(at, false)], &mut st);
            hit |= actions
                .iter()
                .any(|a| matches!(a, Action::Select(id) if *id == last));
        }
        assert!(
            hit,
            "the last of 2,000 cues was not on screen after scrolling to it"
        );
    }

    #[test]
    fn the_show_is_serialised_when_something_may_have_changed_it_not_every_frame() {
        // Never, or told it changed, or doing something that edits: yes.
        assert!(json_due(false, false, false, 0.0), "the first time");
        assert!(json_due(true, true, false, 0.0), "told so");
        assert!(json_due(true, false, true, 0.0), "editing input");
        // Idle, or just the mouse about, a moment after the last time: no.
        assert!(!json_due(true, false, false, 0.016));
        assert!(!json_due(true, false, false, JSON_EVERY - 0.01));
        // But never unlooked at for long, whatever else is going on.
        assert!(json_due(true, false, false, JSON_EVERY));
    }

    #[test]
    fn only_input_that_can_edit_counts_as_editing() {
        let ctx = egui::Context::default();
        let frame = |events: Vec<Event>| {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 200.0))),
                time: Some(1.0),
                events,
                ..Default::default()
            };
            let mut seen = false;
            let mut out = ctx.run_ui(input, |ui| seen = ui.ctx().input(editing_input));
            out.textures_delta.clear();
            seen
        };
        assert!(!frame(vec![]), "nothing");
        assert!(
            !frame(vec![Event::PointerMoved(pos2(10.0, 10.0))]),
            "just moving"
        );
        assert!(frame(vec![Event::Text("a".into())]), "typing");
        assert!(frame(vec![press(Key::A)]), "a key");
        assert!(frame(vec![button(pos2(10.0, 10.0), true)]), "a click");
        // Still held on the next frame: a drag in progress.
        assert!(frame(vec![]), "the button is still down");
    }

    #[test]
    fn missing_files_are_found_on_a_thread_and_the_list_marks_them() {
        let d = media_dir("missing_check");
        let mut show = Show::default();
        let here = video_cue(&mut show, d.join("new/a.mp4"));
        let gone = video_cue(&mut show, d.join("new/gone.mp4"));
        let ctx = egui::Context::default();
        let mut st = UiState::default();
        // The first look starts a check and returns at once; nothing is marked yet.
        refresh_missing(&ctx, &show, &mut st);
        assert!(st.missing_check.is_some());
        let t0 = std::time::Instant::now();
        while st.missing_check.is_some() {
            assert!(t0.elapsed().as_secs() < 10, "the check never finished");
            std::thread::sleep(std::time::Duration::from_millis(5));
            refresh_missing(&ctx, &show, &mut st);
        }
        let _ = here;
        assert_eq!(st.missing, HashSet::from([file_of(&show, gone)]));
        // It isn't repeated until its time (egui's clock stood still here).
        refresh_missing(&ctx, &show, &mut st);
        assert!(st.missing_check.is_none(), "not due again yet");
        let _ = std::fs::remove_dir_all(&d);
    }
}
