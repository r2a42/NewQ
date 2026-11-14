//! Changes to a timeline, on the show: moving, trimming and splitting clips,
//! adding cues, tracks and markers. Each is pure and says whether it changed
//! anything. (Stubs for now: the editing library fills them in.)

use super::model::Dur;
use super::{TimelineKey, TimelineSettings, TrackId};
use crate::show::{Animated, ControlAction, CueId, CueKind, CueType, Show, Slot};
use std::path::PathBuf;

/// Which end of a clip is dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Left,
    Right,
}

/// Where a pause cue goes for a clip: as it comes in, or as it starts to fade out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseBefore {
    ClipIn,
    ClipFadeOut,
}

/// Move clips by `dt` (clamped so none starts before 0); `to` moves the anchor
/// (`ids[0]`) to that track and the others by the same track step where the kind allows.
pub fn move_clips(
    _show: &mut Show,
    _key: TimelineKey,
    _ids: &[CueId],
    _dt: f64,
    _to: Option<TrackId>,
) -> bool {
    false
}

/// Drag an edge of a clip to `to`, a time on the timeline.
pub fn trim(_show: &mut Show, _id: CueId, _edge: Edge, _to: f64, _dur: Dur) -> bool {
    false
}

/// Split clips at `at`. Returns the new halves.
pub fn split(
    _show: &mut Show,
    _key: TimelineKey,
    _ids: &[CueId],
    _at: f64,
    _dur: Dur,
) -> Vec<CueId> {
    Vec::new()
}

/// Remove clips, closing the gap with `ripple`. Returns the cues removed.
pub fn delete(
    _show: &mut Show,
    _key: TimelineKey,
    _ids: &[CueId],
    _ripple: bool,
    _dur: Dur,
) -> Vec<CueId> {
    Vec::new()
}

pub fn add_clip(
    _show: &mut Show,
    _key: TimelineKey,
    _ty: CueType,
    _file: Option<PathBuf>,
    _track: TrackId,
    _at: f64,
) -> Option<CueId> {
    None
}

pub fn add_control(
    _show: &mut Show,
    _key: TimelineKey,
    _action: ControlAction,
    _at: f64,
) -> Option<CueId> {
    None
}

/// A free-run `TimelinePause` at exactly the time `what` names for `clip`.
pub fn add_pause_before(
    _show: &mut Show,
    _key: TimelineKey,
    _clip: CueId,
    _what: PauseBefore,
    _dur: Dur,
) -> Option<CueId> {
    None
}

pub fn add_track(_show: &mut Show, _key: TimelineKey, _picture: bool) -> Option<TrackId> {
    None
}

/// Only if it is empty.
pub fn remove_track(_show: &mut Show, _key: TimelineKey, _t: TrackId) -> bool {
    false
}

pub fn add_marker(_show: &mut Show, _key: TimelineKey, _at: f64, _name: &str) -> Option<u64> {
    None
}

pub fn move_marker(_show: &mut Show, _key: TimelineKey, _id: u64, _to: f64) -> bool {
    false
}

pub fn rename_marker(_show: &mut Show, _key: TimelineKey, _id: u64, _name: &str) -> bool {
    false
}

pub fn remove_marker(_show: &mut Show, _key: TimelineKey, _id: u64) -> bool {
    false
}

/// Set a clip's keyframed speed (sanitised).
pub fn set_rate(_show: &mut Show, _id: CueId, _rate: Animated) -> bool {
    false
}

/// Put a timeline's clips in order of their starts (stable); the ops above call it.
pub fn sort_by_start(_show: &mut Show, _key: TimelineKey) {}

/// A new cue list that is a timeline, with the editor's settings. Returns its id.
pub fn new_timeline_list(show: &mut Show, name: &str) -> u32 {
    let id = show.add_list(name);
    if let Some(l) = show.lists.iter_mut().find(|l| l.id == id) {
        l.timeline = Some(TimelineSettings::for_editor());
    }
    id
}

/// A new timeline group at `at`, with the editor's settings.
pub fn new_timeline_group(show: &mut Show, at: Slot) -> CueId {
    let mut cue = show.make_cue(CueType::Group, None, at.at);
    if let CueKind::Group(g) = &mut cue.kind {
        g.timeline = Some(TimelineSettings::for_editor());
    }
    let id = cue.id;
    // Groups don't nest.
    show.insert_cue(
        Slot {
            at: at.at,
            parent: None,
            list: at.list,
        },
        cue,
    );
    id
}

/// Make a list a timeline or a plain list again. A list that holds groups
/// can't be a timeline, and the main list never is one.
pub fn set_list_timeline(show: &mut Show, list: u32, on: bool) -> Result<(), String> {
    let has_groups = show
        .cues
        .iter()
        .any(|c| c.list == list && matches!(c.kind, CueKind::Group(_)));
    let Some(l) = show.lists.iter_mut().find(|l| l.id == list) else {
        return Err("The main list can't be a timeline".into());
    };
    if !on {
        l.timeline = None;
    } else if has_groups {
        return Err(format!(
            "{} has groups in it: a timeline list can't hold groups",
            l.name
        ));
    } else if l.timeline.is_none() {
        l.timeline = Some(TimelineSettings::for_editor());
    }
    Ok(())
}
