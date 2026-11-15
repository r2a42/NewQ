//! The editor's right-click menus: on a clip (pauses before it, split,
//! delete, its lanes, freeze), on an empty spot of a track (a new clip or cue
//! there, the tracks), and the cue track's "+" (a cue at the playhead).

use super::tracks::Ctx;
use super::*;
use crate::timeline::model::Clip;

/// What the clip menu's edits act on: the picked clips of this timeline when
/// the clicked one is among them, else the clicked one alone.
fn acted_on(ctx: &Ctx, picked: &[CueId], c: &Clip) -> Vec<CueId> {
    if picked.contains(&c.id) {
        picked
            .iter()
            .copied()
            .filter(|id| ctx.clips.iter().any(|k| k.id == *id))
            .collect()
    } else {
        vec![c.id]
    }
}

/// What a split says on hover (plan 2.13, 7: clips start on the engine's tick).
pub(super) const SPLIT_TIP: &str = "Cut the clip in two at the playhead (Ctrl/Cmd+K): the second \
     half takes up where the first stops. A sound split in two may click or gap a little at the \
     join, since each half starts on its own.";

/// A clip's right-click menu. `at` is the time that was right-clicked.
#[allow(clippy::too_many_arguments)]
pub(super) fn clip(
    ui: &mut Ui,
    ctx: &Ctx,
    engine: &Engine,
    c: &Clip,
    picked: &[CueId],
    at: Option<f64>,
    lanes_open: &[(CueId, Lane)],
    actions: &mut Vec<Action>,
) {
    let key = ctx.key;
    let host = ctx.host;
    let cue = ctx.show.cue(c.id);
    let media = cue.is_some_and(|q| q.kind.media().is_some());
    let push = |actions: &mut Vec<Action>, a| actions.push(Action::Timeline(a));
    let ids = acted_on(ctx, picked, c);
    let inside = |t: f64| t > c.start && c.len.is_some_and(|l| t < c.start + l);
    ui.add_enabled_ui(ctx.editable, |ui| {
        let r = ui
            .button("Pause before this clip comes in")
            .on_hover_text("A pause cue at its start: GO brings it in");
        super::toolbar::note(ui, host, "pause_in", &r);
        if r.clicked() {
            push(
                actions,
                TimelineAction::AddPauseBefore {
                    key,
                    clip: c.id,
                    what: PauseBefore::ClipIn,
                },
            );
            ui.close();
        }
        if ui
            .add_enabled(
                media && c.len.is_some(),
                egui::Button::new("Pause before this clip fades out"),
            )
            .on_hover_text("A pause cue where its fade-out starts: GO lets it fade out")
            .clicked()
        {
            push(
                actions,
                TimelineAction::AddPauseBefore {
                    key,
                    clip: c.id,
                    what: PauseBefore::ClipFadeOut,
                },
            );
            ui.close();
        }
        ui.separator();
        let splits = media && !c.loops;
        let r = ui
            .add_enabled(
                splits && inside(ctx.pos),
                egui::Button::new("Split at the playhead").shortcut_text("Ctrl+K"),
            )
            .on_hover_text(SPLIT_TIP)
            .on_disabled_hover_text(
                "Put the playhead inside the clip to split it there (a looping clip isn't split)",
            );
        super::toolbar::note(ui, host, "split_playhead", &r);
        if r.clicked() {
            let ids = ids.to_vec();
            push(
                actions,
                TimelineAction::Split {
                    key,
                    ids,
                    at: ctx.pos,
                },
            );
            ui.close();
        }
        if let Some(t) = at.filter(|t| splits && inside(*t)) {
            let r = ui
                .button(format!("Split here ({})", fmt_time(t)))
                .on_hover_text(SPLIT_TIP);
            super::toolbar::note(ui, host, "split_here", &r);
            if r.clicked() {
                push(
                    actions,
                    TimelineAction::Split {
                        key,
                        ids: vec![c.id],
                        at: t,
                    },
                );
                ui.close();
            }
        }
        let n = ids.len();
        let what = if n == 1 {
            "clip".to_string()
        } else {
            format!("{n} clips")
        };
        let r = ui
            .add(egui::Button::new(format!("Delete {what}")).shortcut_text("Delete"))
            .on_hover_text("Take it off the timeline; the clips after it stay where they are");
        super::toolbar::note(ui, host, "delete", &r);
        if r.clicked() {
            push(
                actions,
                TimelineAction::Delete {
                    key,
                    ids: ids.clone(),
                    ripple: false,
                },
            );
            ui.close();
        }
        let r = ui
            .add(egui::Button::new(format!("Ripple delete {what}")).shortcut_text("Shift+Delete"))
            .on_hover_text("Take it off and move the later clips on its track up to close the gap");
        super::toolbar::note(ui, host, "ripple_delete", &r);
        if r.clicked() {
            push(
                actions,
                TimelineAction::Delete {
                    key,
                    ids: ids.clone(),
                    ripple: true,
                },
            );
            ui.close();
        }
    });
    // The lanes are a view: open them in show mode too.
    let lanes: Vec<Lane> = [Lane::Speed, Lane::Opacity]
        .into_iter()
        .filter(|l| cue.is_some_and(|q| lanes::has(q, *l)))
        .collect();
    if !lanes.is_empty() {
        ui.separator();
        for lane in lanes {
            let open = lanes_open.contains(&(c.id, lane));
            let mut on = open;
            let r = ui
                .checkbox(&mut on, format!("{} lane", lane.label()))
                .on_hover_text(lanes::tip(lane));
            super::toolbar::note(ui, host, lane_name(lane), &r);
            if r.changed() {
                push(actions, TimelineAction::ShowLane { id: c.id, lane, on });
                ui.close();
            }
        }
    }
    ui.separator();
    let playing = matches!(
        engine.status(c.id),
        crate::engine::CueStatus::Playing { .. }
    );
    ui.add_enabled_ui(media && playing, |ui| {
        if ui
            .button("Freeze clip")
            .on_hover_text("Hold this clip where it is while the timeline carries on")
            .clicked()
        {
            push(actions, TimelineAction::FreezeClip(c.id, true));
            ui.close();
        }
        if ui.button("Resume clip").clicked() {
            push(actions, TimelineAction::FreezeClip(c.id, false));
            ui.close();
        }
    });
}

/// The test name of a lane's menu item.
fn lane_name(lane: Lane) -> &'static str {
    match lane {
        Lane::Speed => "speed_lane",
        Lane::Opacity => "opacity_lane",
    }
}

/// What can be added on a track, from its menu: (type, its words, a file to
/// choose for it).
fn new_on(track: TrackId) -> &'static [(CueType, &'static str, bool)] {
    match track {
        TrackId::Picture { .. } => &[
            (CueType::Video, "Video…", true),
            (CueType::Image, "Image…", true),
            (CueType::Text, "Text", false),
            (CueType::Shape, "Shape", false),
            (CueType::Camera, "Camera", false),
        ],
        TrackId::Sound { .. } => &[
            (CueType::Audio, "Sound file…", true),
            (CueType::LiveAudio, "Live sound", false),
        ],
        TrackId::Cues => &[
            (CueType::Fade, "Fade cue", false),
            (CueType::Send, "Send cue (OSC, MIDI…)", false),
            (CueType::Wait, "Wait", false),
        ],
    }
}

/// The new clips and cues of a track's menu, at `at`.
fn add_items(ui: &mut Ui, ctx: &Ctx, track: TrackId, at: f64, actions: &mut Vec<Action>) {
    let key = ctx.key;
    let push = |actions: &mut Vec<Action>, a| actions.push(Action::Timeline(a));
    ui.label(
        RichText::new(format!("Add at {}", fmt_time(at)))
            .weak()
            .small(),
    );
    if track == TrackId::Cues {
        for (action, words, name) in [
            (
                ControlAction::TimelinePause,
                "Pause cue (waits for GO)",
                "add_pause",
            ),
            (ControlAction::TimelineStop, "Stop the timeline", "add_stop"),
        ] {
            let r = ui.button(words);
            super::toolbar::note(ui, ctx.host, name, &r);
            if r.clicked() {
                push(actions, TimelineAction::AddControl { key, action, at });
                ui.close();
            }
        }
    }
    for &(ty, words, file) in new_on(track) {
        let r = ui.button(words);
        super::toolbar::note(ui, ctx.host, ty.label(), &r);
        if r.clicked() {
            push(
                actions,
                if file {
                    TimelineAction::PickClip { key, ty, track, at }
                } else {
                    TimelineAction::AddClip {
                        key,
                        ty,
                        file: None,
                        track,
                        at,
                    }
                },
            );
            ui.close();
        }
    }
}

/// The menu of a track (its header, or an empty spot on it at `at`).
pub(super) fn track(ui: &mut Ui, ctx: &Ctx, track: TrackId, at: f64, actions: &mut Vec<Action>) {
    let key = ctx.key;
    let push = |actions: &mut Vec<Action>, a| actions.push(Action::Timeline(a));
    ui.add_enabled_ui(ctx.editable, |ui| {
        add_items(ui, ctx, track, at, actions);
        ui.separator();
        let r = ui
            .button("Add a picture track")
            .on_hover_text("An empty track a layer above the top one");
        super::toolbar::note(ui, ctx.host, "add_picture_track", &r);
        if r.clicked() {
            push(actions, TimelineAction::AddTrack { key, picture: true });
            ui.close();
        }
        if ui.button("Add a sound track").clicked() {
            push(
                actions,
                TimelineAction::AddTrack {
                    key,
                    picture: false,
                },
            );
            ui.close();
        }
        if let TrackId::Picture { .. } = track {
            let order: Vec<TrackId> = ctx
                .tracks
                .iter()
                .copied()
                .filter(|t| same_display(*t, track))
                .collect();
            let i = order.iter().position(|t| *t == track);
            let r = ui
                .add_enabled(i.is_some_and(|i| i > 0), egui::Button::new("Move track up"))
                .on_hover_text("Its clips go on top of the track above (they swap layers)");
            super::toolbar::note(ui, ctx.host, "track_up", &r);
            if r.clicked() {
                push(
                    actions,
                    TimelineAction::MoveTrack {
                        key,
                        track,
                        up: true,
                    },
                );
                ui.close();
            }
            if ui
                .add_enabled(
                    i.is_some_and(|i| i + 1 < order.len()),
                    egui::Button::new("Move track down"),
                )
                .on_hover_text("Its clips go under the track below (they swap layers)")
                .clicked()
            {
                push(
                    actions,
                    TimelineAction::MoveTrack {
                        key,
                        track,
                        up: false,
                    },
                );
                ui.close();
            }
        }
        let empty = !ctx.clips.iter().any(|c| c.track == track)
            && !model::ghosts(ctx.clips).iter().any(|g| g.1 == track);
        let r = ui
            .add_enabled(
                empty && track != TrackId::Cues,
                egui::Button::new("Remove this track"),
            )
            .on_disabled_hover_text("Only an empty track can be removed (the cue track stays)");
        super::toolbar::note(ui, ctx.host, "remove_track", &r);
        if r.clicked() {
            push(actions, TimelineAction::RemoveTrack { key, track });
            ui.close();
        }
    });
}

/// The cue track's "+": a cue at the playhead.
pub(super) fn cue_plus(ui: &mut Ui, ctx: &Ctx, actions: &mut Vec<Action>) {
    ui.add_enabled_ui(ctx.editable, |ui| {
        add_items(ui, ctx, TrackId::Cues, ctx.pos, actions);
    });
}

/// Two picture tracks on one display.
fn same_display(a: TrackId, b: TrackId) -> bool {
    matches!((a, b), (TrackId::Picture { output: x, .. }, TrackId::Picture { output: y, .. }) if x == y)
}
