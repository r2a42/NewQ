//! The timeline editor: one component (`editor`) with its toolbar, ruler,
//! markers, tracks, clips and playhead, hosted as a pane, a cue list's tab or a
//! workspace; and the timeline rows of the inspector. (Stubs for now: nothing
//! opens it yet, and none of it is in a menu.)

// Used once the editor is built and wired in.
#![allow(dead_code)]

use super::{Action, UiState};
use crate::engine::Engine;
pub use crate::prefs::TimelineLayout;
use crate::show::{Animated, ControlAction, ControlCue, Cue, CueId, CueType, MediaCue, Show};
use crate::timeline::edit::{Edge, PauseBefore};
use crate::timeline::{TimelineKey, TimelineSettings, TrackId, model};
use egui::{Ui, Vec2};
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
#[derive(Default)]
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
}

/// The timeline the editor should show: the active list if it is a timeline, else the
/// selected timeline group (or the group of the selected child), else the pinned one.
pub fn target(show: &Show, engine: &Engine, st: &UiState) -> Option<TimelineKey> {
    let list = TimelineKey::List(engine.active_list);
    if model::is_timeline(show, list) {
        return Some(list);
    }
    let selected = st.selected?;
    let group = TimelineKey::Group(selected);
    if model::is_timeline(show, group) {
        return Some(group);
    }
    // (The pinned one comes with `UiState::timeline`.)
    model::key_of(show, selected).filter(|k| matches!(k, TimelineKey::Group(_)))
}

/// THE component: toolbar, ruler, markers, tracks, clips, playhead.
pub fn editor(
    _ui: &mut Ui,
    _show: &Show,
    _engine: &mut Engine,
    _st: &mut UiState,
    _key: TimelineKey,
    _host: Host,
    _actions: &mut Vec<Action>,
) {
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
        None => {
            ui.label("Select a timeline group to see its timeline here.");
        }
    }
}

/// The control cue's timeline fields, called from the inspector's Control arm.
pub fn control_fields(_ui: &mut Ui, _show: &Show, _cue_id: CueId, _c: &mut ControlCue) {}

/// A clip's timeline rows in the media inspector: run_out, its fade, runs_unseen.
pub fn clip_fields(_ui: &mut Ui, _show: &Show, _cue_id: CueId, _m: &mut MediaCue) {}

/// A timeline's own settings (the two flags), in the group's or list's inspector.
pub fn settings_fields(_ui: &mut Ui, _s: &mut TimelineSettings) {}

/// The cue-list row text of a control cue.
pub fn describe_control(_show: &Show, _cue: &Cue, c: &ControlCue) -> String {
    c.action.label().to_string()
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

/// Make what the editor asked for.
pub(super) fn apply(_a: TimelineAction, _show: &mut Show, _engine: &mut Engine, _st: &mut UiState) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_are_refused_in_show_mode_and_the_transport_is_not() {
        let key = TimelineKey::List(1);
        assert!(TimelineAction::AddMarker { key, at: 1.0 }.edits_the_show());
        assert!(TimelineAction::NewTimelineGroup.edits_the_show());
        assert!(!TimelineAction::Run(key).edits_the_show());
        assert!(!TimelineAction::Seek { key, pos: 2.0 }.edits_the_show());
        assert!(!TimelineAction::SetLayout(TimelineLayout::Workspace).edits_the_show());
    }

    #[test]
    fn the_editor_follows_a_selected_timeline_group_or_its_child() {
        let mut show = Show::default();
        let g = show.make_cue(CueType::Group, None, 0);
        let gid = g.id;
        show.cues.push(g);
        let mut child = show.make_cue(CueType::Video, None, 1);
        child.parent = Some(gid);
        let cid = child.id;
        show.cues.push(child);
        let engine = Engine::for_tests();
        let mut st = UiState::default();
        assert_eq!(target(&show, &engine, &st), None);
        st.selected = Some(gid);
        assert_eq!(target(&show, &engine, &st), Some(TimelineKey::Group(gid)));
        st.selected = Some(cid);
        assert_eq!(target(&show, &engine, &st), Some(TimelineKey::Group(gid)));
    }
}
