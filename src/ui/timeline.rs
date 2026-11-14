//! The timeline editor: one component (`editor`) with its toolbar, ruler,
//! markers, tracks, clips and playhead, hosted as a pane (`pane`: the dock's
//! Timeline pane, and the bottom panel of the new interface's Editor view), and
//! later as a cue list's tab or a workspace; and the timeline rows of the
//! inspector (`control`).
//!
//! The editor never changes the show while it draws: what it is asked to do
//! comes back as `TimelineAction`s, which `apply` makes after the frame. A drag
//! is shown from `EditorState` while it lasts and asked for once, on release,
//! so it is one undo step.

// Some of the actions are for the packages that build on this one (a list's
// tab, the workspace, split and the speed lane).
#![allow(dead_code)]

mod control;
mod ruler;
mod toolbar;
mod tracks;

pub use control::{clip_fields, control_fields, describe_control, settings_fields};

use super::{Action, Pane, UiState, cue_label, fmt_time};
use crate::engine::{Engine, RunState, RunView};
pub use crate::prefs::TimelineLayout;
use crate::show::{Animated, ControlAction, CueId, CueType, PauseFreeze, Show};
use crate::timeline::edit::{self, Edge, PauseBefore};
use crate::timeline::{TimelineKey, TimelinePoint, TrackId, model};
use egui::{RichText, Ui, Vec2};
use std::path::PathBuf;

/// Where the editor is drawn. Also the salt of its egui ids, so two hosts
/// never share one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Host {
    Pane,
    ListTab,
    Workspace,
}

/// The editor's own state (`UiState::timeline`).
pub struct EditorState {
    /// The editor is wanted (pane or workspace).
    pub open: bool,
    /// Otherwise it follows the selection.
    pub pinned: Option<TimelineKey>,
    /// Points per second (zoom); 0 = fit.
    pub pps: f32,
    pub scroll: Vec2,
    pub snap: bool,
    pub ripple: bool,
    /// What is being dragged, and in which host.
    drag: Option<(Host, Drag)>,
    /// A marker whose name is being typed, and the name so far.
    renaming: Option<(Host, u64, String)>,
    /// The zoom last drawn (what "fit" came to), for the zoom buttons.
    shown_pps: f32,
}

impl Default for EditorState {
    fn default() -> EditorState {
        EditorState {
            open: false,
            pinned: None,
            pps: 0.0,
            scroll: Vec2::ZERO,
            // Snapping is what an editor is expected to do; Alt turns it off for a drag.
            snap: true,
            ripple: false,
            drag: None,
            renaming: None,
            shown_pps: 0.0,
        }
    }
}

/// A drag in progress, shown as it goes and asked for on release.
#[derive(Clone, Debug, PartialEq)]
enum Drag {
    /// Clips moved in time (and the first, the anchor, perhaps to another track).
    Clips {
        ids: Vec<CueId>,
        /// The time under the pointer when it was pressed.
        grab: f64,
        /// How far they have moved, snapped, and the track the anchor is over.
        dt: f64,
        to: Option<TrackId>,
    },
    /// An edge of a clip, now at `to` (a time on the timeline).
    Trim { id: CueId, edge: Edge, to: f64 },
    /// The playhead on the ruler, not yet let go.
    Playhead { pos: f64 },
    /// A marker, now at `to`.
    Marker { id: u64, to: f64, grab: f64 },
}

impl EditorState {
    /// Drop a drag and a marker's renaming half done (after a panic: drawing
    /// one of them may be what fell over). What is shown and how stays.
    pub fn let_go(&mut self) {
        self.drag = None;
        self.renaming = None;
    }

    /// The drag in progress in `host`.
    fn drag_in(&self, host: Host) -> Option<&Drag> {
        self.drag
            .as_ref()
            .filter(|(h, _)| *h == host)
            .map(|(_, d)| d)
    }
}

/// The timeline the editor should show: the active list if it is a timeline, else the
/// selected timeline group (or the group of the selected child), else the pinned one.
pub fn target(show: &Show, engine: &Engine, st: &UiState) -> Option<TimelineKey> {
    let list = TimelineKey::List(engine.active_list);
    if model::is_timeline(show, list) {
        return Some(list);
    }
    let selected = st.selected.and_then(|id| {
        let group = TimelineKey::Group(id);
        if model::is_timeline(show, group) {
            Some(group)
        } else {
            model::key_of(show, id).filter(|k| matches!(k, TimelineKey::Group(_)))
        }
    });
    selected.or(st.timeline.pinned.filter(|k| model::is_timeline(show, *k)))
}

/// The timeline rows for the cue at `idx`, at the end of its inspector: a
/// control cue's timeline action, a clip's run-out and lead-in choices. (They
/// need the show and the cue at once, so they edit a copy of the cue's part
/// and put it back if it changed.)
pub(super) fn inspector_rows(ui: &mut Ui, show: &mut Show, idx: usize) {
    let Some(cue) = show.cues.get(idx) else {
        return;
    };
    let id = cue.id;
    match &cue.kind {
        crate::show::CueKind::Control(c) if c.action.is_timeline() => {
            let mut edited = c.clone();
            ui.separator();
            control_fields(ui, show, id, &mut edited);
            if let crate::show::CueKind::Control(c) = &mut show.cues[idx].kind
                && *c != edited
            {
                *c = edited;
            }
        }
        k if k.media().is_some() && model::key_of(show, id).is_some() => {
            let Some(mut edited) = k.media().cloned() else {
                return;
            };
            ui.separator();
            clip_fields(ui, show, id, &mut edited);
            if let Some(m) = show.cues[idx].kind.media_mut()
                && *m != edited
            {
                *m = edited;
            }
        }
        _ => {}
    }
}

/// What a timeline is called: its group's number and name, or its list's name.
fn timeline_name(show: &Show, key: TimelineKey) -> String {
    match key {
        TimelineKey::Group(id) => show.cue(id).map_or_else(|| "(gone)".into(), cue_label),
        TimelineKey::List(n) => show.list_name(n),
    }
}

/// Every timeline in the show: the timeline groups, then the timeline lists.
fn all_timelines(show: &Show) -> Vec<TimelineKey> {
    let groups = show
        .cues
        .iter()
        .map(|c| TimelineKey::Group(c.id))
        .filter(|k| model::is_timeline(show, *k));
    let lists = show
        .lists
        .iter()
        .map(|l| TimelineKey::List(l.id))
        .filter(|k| model::is_timeline(show, *k));
    groups.chain(lists).collect()
}

/// Edits are refused while the show is locked (show mode) or a test runs.
fn can_edit(st: &UiState) -> bool {
    !st.show_mode && !st.test_locked
}

/// THE component: toolbar, ruler, markers, tracks, clips, playhead.
pub fn editor(
    ui: &mut Ui,
    show: &Show,
    engine: &mut Engine,
    st: &mut UiState,
    key: TimelineKey,
    host: Host,
    actions: &mut Vec<Action>,
) {
    let clips = model::clips(show, key, &mut |c| engine.cue_duration(show, c));
    let tracks = model::tracks(show, key, &clips);
    let run = engine.timeline(show, key);
    // The playhead: the run's, or while the group plays the old way (before
    // the engine runs timelines) where the group has got to.
    let pos = match (run.state, key) {
        (RunState::Stopped, TimelineKey::Group(g)) => match engine.status(g) {
            crate::engine::CueStatus::Playing { elapsed, .. } => elapsed,
            _ => run.pos,
        },
        _ => run.pos,
    };
    // What it shows stays when the selection moves off it.
    st.timeline.pinned = Some(key);
    let ctx = tracks::Ctx {
        show,
        key,
        host,
        clips: &clips,
        tracks: &tracks,
        run: &run,
        pos,
        detached: engine.timeline_detached(key),
        waiting: engine.timeline_waiting(key),
        editable: can_edit(st),
        // In show mode only the active list's timeline may be parked from its
        // ruler: parking puts picture on the real outputs (plan 2.7).
        ruler_locked: st.show_mode && key != TimelineKey::List(engine.active_list),
    };
    ui.push_id(("timeline_editor", host), |ui| {
        toolbar::draw(ui, &ctx, engine, st, actions);
        ui.separator();
        tracks::draw(ui, &ctx, engine, st, actions);
    });
}

/// Option A's body: `editor` for `target(..)`, or the empty state.
pub fn pane(
    ui: &mut Ui,
    show: &Show,
    engine: &mut Engine,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) {
    match target(show, engine, st) {
        Some(key) => editor(ui, show, engine, st, key, Host::Pane, actions),
        None => empty_state(ui, st, actions),
    }
}

/// What the pane shows with no timeline to edit.
fn empty_state(ui: &mut Ui, st: &UiState, actions: &mut Vec<Action>) {
    #[cfg(test)]
    ui.data_mut(|d| d.insert_temp(egui::Id::new("timeline_empty"), true));
    ui.add_space(8.0);
    ui.label("Select a timeline group to see its tracks here.");
    ui.label(
        RichText::new(
            "A timeline group is a group set to \u{201c}Start all together\u{201d}: each cue in it \
             starts at its pre-wait on one clock.",
        )
        .weak(),
    );
    ui.add_space(4.0);
    if ui
        .add_enabled(can_edit(st), egui::Button::new("New timeline group"))
        .on_hover_text("Add an empty timeline group after the selected cue")
        .clicked()
    {
        actions.push(Action::Timeline(TimelineAction::NewTimelineGroup));
    }
}

/// What the editor asks for; made after the frame by `apply`.
#[derive(Clone, Debug, PartialEq)]
pub enum TimelineAction {
    // edits (refused in show mode)
    MoveClips {
        key: TimelineKey,
        ids: Vec<CueId>,
        dt: f64,
        to: Option<TrackId>,
    },
    Trim {
        id: CueId,
        edge: Edge,
        to: f64,
    },
    Split {
        key: TimelineKey,
        ids: Vec<CueId>,
        at: f64,
    },
    Delete {
        key: TimelineKey,
        ids: Vec<CueId>,
        ripple: bool,
    },
    AddClip {
        key: TimelineKey,
        ty: CueType,
        file: Option<PathBuf>,
        track: TrackId,
        at: f64,
    },
    AddControl {
        key: TimelineKey,
        action: ControlAction,
        at: f64,
    },
    AddPauseBefore {
        key: TimelineKey,
        clip: CueId,
        what: PauseBefore,
    },
    AddTrack {
        key: TimelineKey,
        picture: bool,
    },
    RemoveTrack {
        key: TimelineKey,
        track: TrackId,
    },
    AddMarker {
        key: TimelineKey,
        at: f64,
    },
    MoveMarker {
        key: TimelineKey,
        id: u64,
        to: f64,
    },
    RenameMarker {
        key: TimelineKey,
        id: u64,
        name: String,
    },
    RemoveMarker {
        key: TimelineKey,
        id: u64,
    },
    SetRate {
        id: CueId,
        rate: Animated,
    },
    NewTimelineList,
    NewTimelineGroup,
    SetListTimeline {
        list: u32,
        on: bool,
    },
    // transport (allowed in show mode)
    Run(TimelineKey),
    Pause(TimelineKey),
    Stop(TimelineKey),
    /// On release of the playhead.
    Seek {
        key: TimelineKey,
        pos: f64,
    },
    JumpToMarker {
        key: TimelineKey,
        id: u64,
    },
    FreezeClip(CueId, bool),
    // view
    Open(Option<TimelineKey>),
    Close,
    SetLayout(TimelineLayout),
}

impl TimelineAction {
    /// Whether it changes the show, and so is refused in show mode.
    pub fn edits_the_show(&self) -> bool {
        use TimelineAction as A;
        match self {
            A::MoveClips { .. }
            | A::Trim { .. }
            | A::Split { .. }
            | A::Delete { .. }
            | A::AddClip { .. }
            | A::AddControl { .. }
            | A::AddPauseBefore { .. }
            | A::AddTrack { .. }
            | A::RemoveTrack { .. }
            | A::AddMarker { .. }
            | A::MoveMarker { .. }
            | A::RenameMarker { .. }
            | A::RemoveMarker { .. }
            | A::SetRate { .. }
            | A::NewTimelineList
            | A::NewTimelineGroup
            | A::SetListTimeline { .. } => true,
            A::Run(_)
            | A::Pause(_)
            | A::Stop(_)
            | A::Seek { .. }
            | A::JumpToMarker { .. }
            | A::FreezeClip(..)
            | A::Open(_)
            | A::Close
            | A::SetLayout(_) => false,
        }
    }
}

/// Make what the editor asked for. (`ui::apply` has already refused an edit
/// in show mode or during a test.)
pub(super) fn apply(a: TimelineAction, show: &mut Show, engine: &mut Engine, st: &mut UiState) {
    use TimelineAction as A;
    // The edits that need lengths get them from the engine, on the show as it
    // was before the edit.
    let before = || show.clone();
    match a {
        A::MoveClips { key, ids, dt, to } => {
            edit::move_clips(show, key, &ids, dt, to);
        }
        A::Trim { id, edge, to } => {
            let was = before();
            edit::trim(show, id, edge, to, &mut |c| engine.cue_duration(&was, c));
        }
        A::Split { key, ids, at } => {
            let was = before();
            edit::split(show, key, &ids, at, &mut |c| engine.cue_duration(&was, c));
        }
        A::Delete { key, ids, ripple } => {
            let was = before();
            let gone = edit::delete(show, key, &ids, ripple, &mut |c| {
                engine.cue_duration(&was, c)
            });
            if st.selected.is_some_and(|s| gone.contains(&s)) {
                st.selected = None;
            }
        }
        A::AddClip {
            key,
            ty,
            file,
            track,
            at,
        } => {
            if let Some(id) = edit::add_clip(show, key, ty, file, track, at) {
                super::apply(Action::Select(id), show, engine, st);
            }
        }
        A::AddControl { key, action, at } => {
            if let Some(id) = edit::add_control(show, key, action, at) {
                super::apply(Action::Select(id), show, engine, st);
            }
        }
        A::AddPauseBefore { key, clip, what } => {
            let was = before();
            if edit::add_pause_before(show, key, clip, what, &mut |c| engine.cue_duration(&was, c))
                .is_some()
            {
                engine.log("Added a pause cue: GO runs the timeline on from it");
            }
        }
        A::AddTrack { key, picture } => {
            edit::add_track(show, key, picture);
        }
        A::RemoveTrack { key, track } => {
            edit::remove_track(show, key, track);
        }
        A::AddMarker { key, at } => {
            let n = model::settings(show, key).map_or(0, |s| s.markers.len()) + 1;
            edit::add_marker(show, key, at, &format!("Marker {n}"));
        }
        A::MoveMarker { key, id, to } => {
            edit::move_marker(show, key, id, to);
        }
        A::RenameMarker { key, id, name } => {
            edit::rename_marker(show, key, id, &name);
        }
        A::RemoveMarker { key, id } => {
            edit::remove_marker(show, key, id);
        }
        A::SetRate { id, rate } => {
            edit::set_rate(show, id, rate);
        }
        A::NewTimelineList => {
            let n = show.lists.iter().filter(|l| l.timeline.is_some()).count() + 1;
            let id = edit::new_timeline_list(show, &format!("Timeline {n}"));
            super::apply(Action::SelectList(id), show, engine, st);
        }
        A::NewTimelineGroup => {
            let slot = show.insert_slot(st.selected, true, engine.active_list);
            let id = edit::new_timeline_group(show, slot);
            if let Some(i) = show.cue_index(id) {
                show.cues[i].name = "Timeline".into();
            }
            super::select_new(show, st, engine, None, id);
            st.timeline.pinned = Some(TimelineKey::Group(id));
        }
        A::SetListTimeline { list, on } => {
            if let Err(e) = edit::set_list_timeline(show, list, on) {
                engine.log(e);
            }
        }
        A::Run(key) => engine.timeline_run(show, key),
        // The editor's Pause holds everything (as the transport's Pause does)
        // and leaves live inputs live; its Run carries on.
        A::Pause(key) => {
            engine.timeline_pause(show, key, PauseFreeze::Everything, &[], false, false)
        }
        // As a timeline Stop cue does: a fade, and a second Stop cuts.
        A::Stop(key) => engine.timeline_stop(show, key, crate::show::TIMELINE_STOP_FADE),
        A::Seek { key, pos } => engine.timeline_seek(show, key, pos.max(0.0), true),
        A::JumpToMarker { key, id } => {
            if let Some(t) = model::resolve(show, key, TimelinePoint::Marker(id)) {
                engine.timeline_seek(show, key, t, true);
            }
        }
        A::FreezeClip(id, on) => {
            let action = if on {
                ControlAction::Pause
            } else {
                ControlAction::Resume
            };
            engine.control_target(show, action, id, "the timeline editor");
        }
        A::Open(key) => {
            st.timeline.open = true;
            if key.is_some() {
                st.timeline.pinned = key;
            }
            // The new interface has its own place for it (the Editor view's
            // bottom panel); the panes get the Timeline pane.
            if !st.prefs.new_layout {
                st.layout.open(Pane::Timeline);
            }
        }
        A::Close => {
            st.timeline.open = false;
            if !st.prefs.new_layout {
                st.layout.close(Pane::Timeline);
            }
        }
        // Kept for this computer. (The workspace isn't built yet: the editor
        // stays a pane until it is.)
        A::SetLayout(layout) => {
            st.prefs.timeline_layout = layout;
            if let Some(dir) = &st.layout_dir
                && let Err(e) = st.prefs.save(dir)
            {
                log::warn!("could not save the settings of this computer: {e}");
            }
        }
    }
}

/// Where the editor drew things, for tests to aim the mouse at; stored at
/// `egui::Id::new(("timeline_rects", host))`.
#[cfg(test)]
#[derive(Clone, Debug)]
pub struct TimelineRects {
    pub ruler: egui::Rect,
    pub clips: Vec<(CueId, egui::Rect)>,
    pub tracks: Vec<(TrackId, egui::Rect)>,
    pub markers: Vec<(u64, egui::Rect)>,
    pub playhead_x: Option<f32>,
    pub x0: f32,
    pub pps: f32,
}

/// Note, for the tests, that an inspector row was drawn.
#[cfg(test)]
pub(super) fn note_row(ui: &Ui, row: &'static str) {
    ui.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<&'static str>>(egui::Id::new("timeline_rows"))
            .push(row)
    });
}

#[cfg(not(test))]
fn note_row(_ui: &Ui, _row: &'static str) {}

#[cfg(test)]
mod tests;
