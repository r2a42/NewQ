//! The editor's keys and what comes from outside it: Ctrl/Cmd+K splits at
//! the playhead, Delete (Shift+Delete to ripple) removes the picked clips,
//! files dropped on a track become clips there, and a file chosen in a
//! dialog for a new clip.
//!
//! The window's own keys (`ui::keyboard`) are read before anything is drawn,
//! so Delete has already been turned into the cue list's `Action::Delete`
//! when the editor sees it: the editor that holds the keys (the last clicked,
//! or under the pointer) puts its own delete in its place when every picked
//! cue is one of its clips. The same for a drop: `Action::AddFiles` gives way
//! to clips on the track the files were let go over.

use super::tracks::{Ctx, Geo};
use super::*;
use egui::{Key, Modifiers, Rect};

/// The clips a split at the playhead cuts: the picked clips of this timeline,
/// or with none picked every clip the playhead is in.
pub(super) fn split_ids(ctx: &Ctx, st: &UiState) -> Vec<CueId> {
    let picked: Vec<CueId> = st
        .selection(ctx.show)
        .into_iter()
        .filter(|id| ctx.clips.iter().any(|c| c.id == *id))
        .collect();
    if !picked.is_empty() {
        return picked;
    }
    ctx.clips
        .iter()
        .filter(|c| c.start < ctx.pos && c.len.is_some_and(|l| c.start + l > ctx.pos))
        .map(|c| c.id)
        .collect()
}

/// The keys, for the editor that holds them (`hovered`: the pointer is over it).
pub(super) fn handle(
    ui: &Ui,
    ctx: &Ctx,
    st: &mut UiState,
    hovered: bool,
    actions: &mut Vec<Action>,
) {
    if !(st.timeline.focus == Some(ctx.host) || hovered) || ui.ctx().egui_wants_keyboard_input() {
        return;
    }
    let key = ctx.key;
    if ctx.editable && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::K)) {
        let ids = split_ids(ctx, st);
        actions.push(Action::Timeline(TimelineAction::Split {
            key,
            ids,
            at: ctx.pos,
        }));
    }
    // Delete, as the window's keys read it: ours when every picked cue is a
    // clip here (Shift, or the Ripple toggle, closes the gaps).
    if let Some(at) = actions.iter().position(|a| matches!(a, Action::Delete)) {
        let picked = st.selection(ctx.show);
        let ids: Vec<CueId> = picked
            .iter()
            .copied()
            .filter(|id| ctx.clips.iter().any(|c| c.id == *id))
            .collect();
        if ctx.editable && !ids.is_empty() && ids.len() == picked.len() {
            let ripple = st.timeline.ripple || ui.input(|i| i.modifiers.shift);
            actions[at] = Action::Timeline(TimelineAction::Delete { key, ids, ripple });
        }
    }
}

/// Files let go over a track become clips there, at the time under the
/// pointer (each on a track of its kind: a sound dropped on a picture track
/// goes to the first sound track). They then don't go into the cue list too.
pub(super) fn dropped(
    ui: &Ui,
    ctx: &Ctx,
    geo: Geo,
    bars: &[(TrackId, Rect)],
    actions: &mut Vec<Action>,
) {
    let (files, at) = ui.input(|i| {
        (
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect::<Vec<PathBuf>>(),
            i.pointer.hover_pos(),
        )
    });
    let Some(p) = at else {
        return;
    };
    let Some(&(track, _)) = bars.iter().find(|(_, r)| r.contains(p)) else {
        return;
    };
    if files.is_empty() || !ctx.editable {
        return;
    }
    let mut asked = false;
    for f in files {
        let Some(ty) = crate::show::guess_cue_type(&f) else {
            continue;
        };
        let to = fitting_track(ctx, ty, track);
        actions.push(Action::Timeline(TimelineAction::AddClip {
            key: ctx.key,
            ty,
            file: Some(f),
            track: to,
            at: geo.t(p.x).max(0.0),
        }));
        asked = true;
    }
    if asked {
        actions.retain(|a| !matches!(a, Action::AddFiles(_)));
    }
}

/// The track a new clip of `ty` goes on when asked for on `track`: that one
/// if it is of its kind, else the first of its kind (a new one if none).
pub(super) fn fitting_track(ctx: &Ctx, ty: CueType, track: TrackId) -> TrackId {
    let picture = matches!(
        ty,
        CueType::Video
            | CueType::Image
            | CueType::Text
            | CueType::Shape
            | CueType::Camera
            | CueType::Clock
            | CueType::Timer
    );
    let sound = matches!(ty, CueType::Audio | CueType::LiveAudio);
    let fits = |t: &TrackId| match t {
        TrackId::Picture { .. } => picture,
        TrackId::Sound { .. } => sound,
        TrackId::Cues => !picture && !sound,
    };
    if fits(&track) {
        return track;
    }
    ctx.tracks.iter().copied().find(fits).unwrap_or(if picture {
        TrackId::Picture {
            output: ctx.show.outputs.first().map(|o| o.id),
            layer: 0,
        }
    } else if sound {
        TrackId::Sound { output: None }
    } else {
        TrackId::Cues
    })
}

/// Open a file dialog for a new clip on a thread of its own (one at a time);
/// `picked` takes its answer.
pub(super) fn pick_file(st: &mut UiState, key: TimelineKey, ty: CueType, track: TrackId, at: f64) {
    if st.timeline.pick.is_some() {
        return;
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("timeline-file-dialog".into())
        .spawn(move || {
            let file = super::super::media_dialog(ty).pick_file();
            let _ = tx.send((key, ty, track, at, file));
        });
    if spawned.is_ok() {
        st.timeline.pick = Some(rx);
    }
}

/// The file chosen for a new clip, once the dialog has closed.
pub(super) fn picked(st: &mut UiState, actions: &mut Vec<Action>) {
    let Some(rx) = &st.timeline.pick else {
        return;
    };
    match rx.try_recv() {
        Ok((key, ty, track, at, file)) => {
            st.timeline.pick = None;
            if let Some(file) = file {
                actions.push(Action::Timeline(TimelineAction::AddClip {
                    key,
                    ty,
                    file: Some(file),
                    track,
                    at,
                }));
            }
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => st.timeline.pick = None,
        Err(std::sync::mpsc::TryRecvError::Empty) => {}
    }
}
