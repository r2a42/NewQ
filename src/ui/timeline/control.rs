//! The timeline rows of the inspector: a control cue's timeline action (which
//! timeline, from or to where, what a pause holds), a clip's run-out and
//! lead-in choices, a timeline's own two rules; and a control cue's text in
//! the cue list.

use super::*;
use crate::show::{ControlCue, Cue, MediaCue, RunOut, TIMELINE_STOP_FADE};
use crate::timeline::{TimelinePoint, TimelineSettings};

/// What a clip whose end the clock keeps can do when its content runs out,
/// in the order offered.
pub(super) const RUN_OUT_CHOICES: [(RunOut, &str, &str); 3] = [
    (
        RunOut::End,
        "End, fading out",
        "It fades out as its content ends (the default)",
    ),
    (
        RunOut::Loop,
        "Loop",
        "It goes round again from its in point until the timeline ends it",
    ),
    (
        RunOut::Hold,
        "Hold its last frame",
        "It stays on its last frame (a sound: silence) until the timeline ends it",
    ),
];

/// The freeze choices of a timeline pause.
const FREEZE_CHOICES: [(PauseFreeze, &str, &str); 3] = [
    (
        PauseFreeze::Nothing,
        "Nothing: content runs on",
        "Only the clock stops: clips playing go on playing or looping until GO",
    ),
    (
        PauseFreeze::Everything,
        "Everything (pause on a frame)",
        "Every clip of the timeline holds its frame and its sound stops",
    ),
    (
        PauseFreeze::Clips,
        "These clips",
        "Only the clips ticked below hold; the rest run on",
    ),
];

fn point_text(show: &Show, key: Option<TimelineKey>, p: TimelinePoint) -> String {
    match p {
        TimelinePoint::Time(t) => fmt_time(t),
        TimelinePoint::Marker(id) => key
            .and_then(|k| model::settings(show, k))
            .and_then(|s| s.markers.iter().find(|m| m.id == id))
            .map_or_else(
                || "a marker that has gone".into(),
                |m| format!("marker {}", m.name),
            ),
        TimelinePoint::Cue(id) => show.cue(id).map_or_else(
            || "a cue that has gone".into(),
            |c| format!("the start of {}", cue_label(c)),
        ),
    }
}

/// The cue-list row text of a control cue.
pub fn describe_control(show: &Show, cue: &Cue, c: &ControlCue) -> String {
    if !c.action.is_timeline() {
        // As it always was: the target of the actions that have one.
        if !c.action.needs_target() {
            return String::new();
        }
        let target = c
            .target
            .and_then(|t| show.cue(t))
            .map(cue_label)
            .unwrap_or_else(|| "(no cue)".into());
        return format!("{} » {target}", c.action.label());
    }
    let key = c.timeline.or_else(|| model::key_of(show, cue.id));
    let mut s = c.action.label().to_string();
    if let Some(k) = c.timeline {
        s += &format!(" » {}", timeline_name(show, k));
    }
    match c.action {
        ControlAction::TimelinePause if c.is_loop() => {
            if let Some(p) = c.loop_from {
                s += &format!(" · loops from {} until GO", point_text(show, key, p));
            }
            if c.leave_at_once {
                s += ", which leaves at once";
            }
        }
        ControlAction::TimelinePause => {
            s += match c.freeze {
                PauseFreeze::Nothing => " · content runs on",
                PauseFreeze::Everything => " · holds everything",
                PauseFreeze::Clips => " · holds",
            };
            if c.freeze == PauseFreeze::Clips {
                let n = c.clips.len();
                s += &format!(" {n} clip{}", if n == 1 { "" } else { "s" });
            }
            if c.freeze_live && c.freeze != PauseFreeze::Nothing {
                s += " and live inputs";
            }
        }
        ControlAction::TimelineRun => {
            if let Some(p) = c.at {
                s += &format!(" from {}", point_text(show, key, p));
            }
        }
        ControlAction::TimelineJump => match c.at {
            Some(p) => s += &format!(" to {}", point_text(show, key, p)),
            None => s += " (nowhere chosen)",
        },
        ControlAction::TimelineStop => match c.stop_fade() {
            f if f > 0.0 => s += &format!(" · fades out over {f:.1} s"),
            _ => s += " · cuts",
        },
        _ => {}
    }
    if key.is_none() {
        s += " (not on a timeline)";
    }
    s
}

/// The control cue's timeline fields, called from the inspector's Control arm.
pub fn control_fields(ui: &mut Ui, show: &Show, cue_id: CueId, c: &mut ControlCue) {
    if !c.action.is_timeline() {
        return;
    }
    let own = model::key_of(show, cue_id);
    let timelines = all_timelines(show);
    egui::Grid::new(("control_timeline", cue_id.0))
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            note_row(ui, "timeline");
            ui.label("Timeline");
            let shown = match c.timeline {
                Some(k) => timeline_name(show, k),
                None => match own {
                    Some(k) => format!("This cue's ({})", timeline_name(show, k)),
                    None => "This cue's (it is on none)".into(),
                },
            };
            egui::ComboBox::from_id_salt(("control_timeline_key", cue_id.0))
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut c.timeline, None, "This cue's timeline");
                    for k in &timelines {
                        ui.selectable_value(&mut c.timeline, Some(*k), timeline_name(show, *k));
                    }
                });
            ui.end_row();
            let key = c.timeline.or(own);
            match c.action {
                ControlAction::TimelineRun | ControlAction::TimelineJump => {
                    let run = c.action == ControlAction::TimelineRun;
                    note_row(ui, "point");
                    ui.label(if run { "From" } else { "To" });
                    point_picker(ui, show, key, cue_id, &mut c.at, run);
                    ui.end_row();
                }
                ControlAction::TimelinePause => {
                    // A loop pause never stands the clock, so it holds nothing.
                    let holds = !c.is_loop();
                    if holds {
                        note_row(ui, "freeze");
                        ui.label("Holds");
                        let label = FREEZE_CHOICES
                            .iter()
                            .find(|f| f.0 == c.freeze)
                            .map_or("", |f| f.1);
                        egui::ComboBox::from_id_salt(("control_freeze", cue_id.0))
                            .selected_text(label)
                            .show_ui(ui, |ui| {
                                for (f, name, tip) in FREEZE_CHOICES {
                                    ui.selectable_value(&mut c.freeze, f, name)
                                        .on_hover_text(tip);
                                }
                            });
                        ui.end_row();
                        if c.freeze == PauseFreeze::Clips {
                            note_row(ui, "clips");
                            ui.label("Clips");
                            ui.vertical(|ui| clip_ticks(ui, show, key, cue_id, &mut c.clips));
                            ui.end_row();
                        }
                    }
                    note_row(ui, "loop");
                    ui.label("Loop");
                    let mut looping = c.loop_from.is_some();
                    if ui
                        .checkbox(&mut looping, "Play a section again until GO")
                        .on_hover_text(
                            "Reached, the timeline jumps back to where the section starts and \
                             plays it again, round and round, until GO. Not sample-accurate: \
                             the clips start again each time, so a sound that must not break \
                             is better as a clip that loops by itself.",
                        )
                        .changed()
                    {
                        c.loop_from = looping.then_some(TimelinePoint::Time(0.0));
                    }
                    ui.end_row();
                    if c.loop_from.is_some() {
                        note_row(ui, "loop_from");
                        ui.label("Section from");
                        point_picker(ui, show, key, cue_id, &mut c.loop_from, false);
                        ui.end_row();
                        note_row(ui, "leave_at_once");
                        ui.label("On GO");
                        ui.checkbox(&mut c.leave_at_once, "Leave the loop at once")
                            .on_hover_text(
                                "Off: GO lets the pass that is playing finish, and the \
                                 timeline carries straight on past this cue. On: GO leaves \
                                 the loop at once and goes on from this cue.",
                            );
                        ui.end_row();
                    }
                    if holds && c.freeze != PauseFreeze::Nothing {
                        note_row(ui, "freeze_live");
                        ui.label("Live inputs");
                        ui.checkbox(&mut c.freeze_live, "Live inputs freeze too")
                            .on_hover_text(
                                "Off: cameras, NDI sources and live sound carry on live while \
                                 the rest holds. On: a camera holds its last frame and live \
                                 sound is muted, until GO.",
                            );
                        ui.end_row();
                    }
                }
                ControlAction::TimelineStop => {
                    note_row(ui, "fade");
                    ui.label("Fade out");
                    let mut t = c.stop_fade();
                    if ui
                        .add(
                            egui::DragValue::new(&mut t)
                                .speed(0.05)
                                .range(0.0..=3_600.0)
                                .fixed_decimals(2)
                                .suffix(" s"),
                        )
                        .on_hover_text(
                            "What the timeline plays fades out over this time, then it stops. \
                             0 is a cut; a second Stop during the fade cuts at once.",
                        )
                        .changed()
                    {
                        // The default is left out of the file.
                        c.fade = ((t - TIMELINE_STOP_FADE).abs() > 1e-9).then_some(t);
                    }
                    ui.end_row();
                }
                _ => {}
            }
        });
    if model::key_of(show, cue_id).is_none() && c.timeline.is_none() {
        ui.label(
            RichText::new("This cue is not on a timeline: choose the timeline it is for.")
                .color(super::super::PLAYHEAD),
        );
    }
    if c.action == ControlAction::TimelineJump && c.at.is_none() {
        ui.label(RichText::new("Choose where it jumps to.").color(super::super::PLAYHEAD));
    }
}

/// Where a Run starts or a Jump goes: the playhead (Run only), a time, a
/// marker or the start of a clip.
fn point_picker(
    ui: &mut Ui,
    show: &Show,
    key: Option<TimelineKey>,
    cue_id: CueId,
    at: &mut Option<TimelinePoint>,
    playhead_allowed: bool,
) {
    let markers: Vec<(u64, String)> = key
        .and_then(|k| model::settings(show, k))
        .map(|s| s.markers.iter().map(|m| (m.id, m.name.clone())).collect())
        .unwrap_or_default();
    let clips: Vec<(CueId, String)> = key
        .map(|k| {
            model::members(show, k)
                .into_iter()
                .filter(|c| c.id != cue_id)
                .map(|c| (c.id, cue_label(c)))
                .collect()
        })
        .unwrap_or_default();
    ui.horizontal(|ui| {
        let kind = match at {
            None => 0,
            Some(TimelinePoint::Time(_)) => 1,
            Some(TimelinePoint::Marker(_)) => 2,
            Some(TimelinePoint::Cue(_)) => 3,
        };
        let names = [
            if playhead_allowed {
                "Where its playhead is"
            } else {
                "(choose)"
            },
            "A time",
            "A marker",
            "The start of a clip",
        ];
        let mut chosen = kind;
        egui::ComboBox::from_id_salt(("control_point", cue_id.0))
            .selected_text(names[kind])
            .show_ui(ui, |ui| {
                if playhead_allowed {
                    ui.selectable_value(&mut chosen, 0, names[0]);
                }
                ui.selectable_value(&mut chosen, 1, names[1]);
                ui.add_enabled_ui(!markers.is_empty(), |ui| {
                    ui.selectable_value(&mut chosen, 2, names[2]);
                });
                ui.add_enabled_ui(!clips.is_empty(), |ui| {
                    ui.selectable_value(&mut chosen, 3, names[3]);
                });
            });
        if chosen != kind {
            let now = at.and_then(|p| key.and_then(|k| model::resolve(show, k, p)));
            *at = match chosen {
                1 => Some(TimelinePoint::Time(now.unwrap_or(0.0))),
                2 => markers.first().map(|m| TimelinePoint::Marker(m.0)),
                3 => clips.first().map(|c| TimelinePoint::Cue(c.0)),
                _ => None,
            };
        }
        match at {
            Some(TimelinePoint::Time(t)) => {
                ui.add(
                    egui::DragValue::new(t)
                        .speed(0.05)
                        .range(0.0..=36_000.0)
                        .fixed_decimals(2)
                        .suffix(" s"),
                );
            }
            Some(TimelinePoint::Marker(id)) => {
                let name = markers
                    .iter()
                    .find(|m| m.0 == *id)
                    .map_or("(gone)".to_string(), |m| m.1.clone());
                egui::ComboBox::from_id_salt(("control_marker", cue_id.0))
                    .selected_text(name)
                    .show_ui(ui, |ui| {
                        for (mid, n) in &markers {
                            ui.selectable_value(id, *mid, n);
                        }
                    });
            }
            Some(TimelinePoint::Cue(id)) => {
                let name = clips
                    .iter()
                    .find(|c| c.0 == *id)
                    .map_or("(gone)".to_string(), |c| c.1.clone());
                egui::ComboBox::from_id_salt(("control_clip", cue_id.0))
                    .selected_text(name)
                    .show_ui(ui, |ui| {
                        for (cid, n) in &clips {
                            ui.selectable_value(id, *cid, n);
                        }
                    });
            }
            None => {}
        }
    });
}

/// A tick for each clip of the timeline that a pause can hold.
fn clip_ticks(
    ui: &mut Ui,
    show: &Show,
    key: Option<TimelineKey>,
    cue_id: CueId,
    clips: &mut Vec<CueId>,
) {
    let Some(key) = key else {
        ui.label(RichText::new("(not on a timeline)").weak());
        return;
    };
    let members: Vec<&Cue> = model::members(show, key)
        .into_iter()
        .filter(|c| c.id != cue_id && c.kind.media().is_some())
        .collect();
    if members.is_empty() {
        ui.label(RichText::new("(the timeline has no clips)").weak());
    }
    for c in members {
        let mut on = clips.contains(&c.id);
        if ui.checkbox(&mut on, cue_label(c)).changed() {
            if on {
                clips.push(c.id);
            } else {
                clips.retain(|id| *id != c.id);
            }
        }
    }
}

/// A clip's timeline rows in the media inspector: run_out, its fade, runs_unseen.
pub fn clip_fields(ui: &mut Ui, show: &Show, cue_id: CueId, m: &mut MediaCue) {
    let Some(key) = model::key_of(show, cue_id) else {
        return;
    };
    let waits = model::settings(show, key).is_some_and(|s| s.start_on_fade_in);
    ui.label(RichText::new("On the timeline").strong());
    egui::Grid::new(("clip_timeline", cue_id.0))
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            note_row(ui, "run_out");
            ui.label("If it runs out").on_hover_text(
                "For a clip whose end the timeline's clock keeps (a pause lies inside it): \
                 what it does if its content ends before the timeline moves on",
            );
            let label = RUN_OUT_CHOICES
                .iter()
                .find(|r| r.0 == m.run_out)
                .map_or("", |r| r.1);
            egui::ComboBox::from_id_salt(("clip_run_out", cue_id.0))
                .selected_text(label)
                .show_ui(ui, |ui| {
                    for (r, name, tip) in RUN_OUT_CHOICES {
                        ui.selectable_value(&mut m.run_out, r, name)
                            .on_hover_text(tip);
                    }
                });
            ui.end_row();
            if m.run_out == RunOut::End {
                note_row(ui, "run_out_fade");
                ui.label("Fading over");
                ui.horizontal(|ui| {
                    let mut own = m.run_out_fade.is_none();
                    let own_time = m.manual_fade_out();
                    if ui
                        .checkbox(&mut own, format!("Its fade-out ({})", fmt_time(own_time)))
                        .on_hover_text(
                            "Its own fade-out time, or the default fade when it has none",
                        )
                        .changed()
                    {
                        m.run_out_fade = if own { None } else { Some(own_time) };
                    }
                    if let Some(t) = &mut m.run_out_fade {
                        ui.add(
                            egui::DragValue::new(t)
                                .speed(0.05)
                                .range(0.0..=3_600.0)
                                .fixed_decimals(2)
                                .suffix(" s"),
                        )
                        .on_hover_text("0 is a cut");
                    }
                });
                ui.end_row();
            }
            note_row(ui, "runs_unseen");
            ui.label("Unseen start");
            ui.add_enabled(
                waits,
                egui::Checkbox::new(&mut m.runs_unseen, "Already running when it fades in"),
            )
            .on_hover_text(if waits {
                "A clip at opacity 0 waits for its fade-in before its content starts. Tick this \
                 for one that should be rolling, unseen, from its start."
            } else {
                "This timeline starts clips at opacity 0 at their place anyway (see the \
                 group's \u{201c}Clips at opacity 0 wait for their fade-in\u{201d})."
            });
            ui.end_row();
        });
}

/// A timeline's own settings (the two flags), in the group's or list's inspector.
pub fn settings_fields(ui: &mut Ui, s: &mut TimelineSettings) {
    note_row(ui, "settings");
    ui.checkbox(
        &mut s.start_on_fade_in,
        "Clips at opacity 0 wait for their fade-in",
    )
    .on_hover_text(
        "A clip that is invisible (or silent) when the playhead reaches it starts its content \
         when it begins to fade in, not before. Each clip can opt out.",
    );
    ui.checkbox(&mut s.crossfade_overlaps, "Overlapping clips crossfade")
        .on_hover_text(
            "A clip that starts while an earlier one on its track is still playing crossfades \
             over it for as long as they overlap. Off: it comes in by its own transition.",
        );
}
