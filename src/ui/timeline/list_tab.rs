//! A cue list of the timeline type, shown as a tab of its own: the cue-list
//! area draws the tracks editor (`Host::ListTab`) in place of the rows, with a
//! switch back to the rows. Cues added while it is the active list become clips
//! at its playhead; GO, the transports' read-outs and the Operator view act on
//! its run (`Engine::go_plan`). The main list is never a timeline.

use super::*;
use crate::engine::{GoPlan, TimelineGo};
use crate::show::{Cue, CueKind};

/// How long a still, a text or a shape added to a timeline list is held.
pub const STILL_LEN: f64 = 5.0;

/// Why a group can't go into a timeline list.
pub const NO_GROUPS: &str = "A timeline list can't hold groups: its cues are clips on one clock";

/// The timeline list being worked, if the active list is one.
pub fn active(show: &Show, engine: &Engine) -> Option<TimelineKey> {
    let key = TimelineKey::List(engine.active_list);
    model::is_timeline(show, key).then_some(key)
}

/// Whether the list `list` is a timeline (and so takes no groups).
pub fn is_timeline_list(show: &Show, list: u32) -> bool {
    model::is_timeline(show, TimelineKey::List(list))
}

/// Whether the cue-list area shows the active list's tracks (rather than its rows).
/// Where the editor opens as a workspace, the tab keeps its rows and its
/// Tracks button opens the workspace.
pub fn shows_tracks(show: &Show, engine: &Engine, st: &UiState) -> bool {
    active(show, engine).is_some() && !st.timeline.list_rows && !workspace::chosen(st)
}

/// The top of the cue list when the active list is a timeline: the Tracks /
/// Cue rows switch, the offer to keep pictures on their own layers, and the
/// tracks editor filling the rest. Whether the tracks were drawn (the rows
/// are not, then).
pub fn draw(
    ui: &mut Ui,
    show: &Show,
    engine: &mut Engine,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) -> bool {
    let Some(key) = active(show, engine) else {
        return false;
    };
    let rows = st.timeline.list_rows || workspace::chosen(st);
    ui.horizontal(|ui| {
        ui.label(RichText::new("TIMELINE").small().weak());
        let r = ui
            .selectable_label(!rows, "Tracks")
            .on_hover_text(if workspace::chosen(st) {
                "Open the list's tracks in the timeline workspace"
            } else {
                "The list's cues as clips on tracks, on one clock"
            });
        note_button(ui, "tracks", &r);
        if r.clicked() {
            if workspace::chosen(st) {
                actions.push(Action::Timeline(TimelineAction::Open(Some(key))));
            } else {
                st.timeline.list_rows = false;
            }
        }
        let r = ui.selectable_label(rows, "Cue rows").on_hover_text(
            "The same cues as rows, to number, name and arm them (the Timeline pane, \
                 Window menu, shows the tracks under them)",
        );
        note_button(ui, "rows", &r);
        if r.clicked() {
            st.timeline.list_rows = true;
        }
    });
    if !show.own_layers {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(
                    "⚠ In this show a picture replaces everything on its display, so the \
                     tracks don't keep pictures over each other.",
                )
                .color(super::tracks::MARKER),
            );
            let r = ui
                .add_enabled(
                    can_edit(st),
                    egui::Button::new("Replace only on its own layer").small(),
                )
                .on_hover_text(
                    "Turn on Settings ▸ Show ▸ Layers for this show: a picture then replaces only \
                     what is on its own layer of its display",
                );
            note_button(ui, "own_layers", &r);
            if r.clicked() {
                actions.push(Action::Timeline(TimelineAction::UseOwnLayers));
            }
        });
    }
    if rows {
        return false;
    }
    editor(ui, show, engine, st, key, Host::ListTab, actions);
    true
}

/// Note, for the tests, where one of the tab's buttons is.
fn note_button(_ui: &Ui, _name: &'static str, _r: &egui::Response) {
    #[cfg(test)]
    _ui.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(&'static str, egui::Rect)>>(egui::Id::new(
            "list_tab_buttons",
        ))
        .push((_name, _r.rect))
    });
}

/// Add cues to the timeline list `key` as clips at its playhead, one after
/// the other where their lengths are known, each on a track of its kind with
/// nothing on it there (`fitting_track`). The last is selected. A group is
/// refused.
pub fn add_clips(
    show: &mut Show,
    engine: &mut Engine,
    st: &mut UiState,
    key: TimelineKey,
    cues: Vec<(CueType, Option<PathBuf>)>,
) -> Vec<CueId> {
    let mut at = engine.timeline(show, key).pos;
    let mut added = Vec::new();
    for (ty, file) in cues {
        if ty == CueType::Group {
            engine.log(NO_GROUPS);
            continue;
        }
        let clips = model::clips(show, key, &mut |c| engine.cue_duration(show, c));
        let track = fitting_track(show, key, ty, at, &clips);
        let Some(id) = edit::add_clip(show, key, ty, file, track, at) else {
            continue;
        };
        added.push(id);
        // A still, a text or a shape would hold for ever and fill its track:
        // on a timeline it gets a length to trim.
        if matches!(
            ty,
            CueType::Image | CueType::Text | CueType::Shape | CueType::Clock | CueType::Timer
        ) && let Some(i) = show.cue_index(id)
            && let Some(m) = show.cues[i].kind.media_mut()
            && m.end.is_none()
        {
            m.end = Some(STILL_LEN);
        }
        // The next one after this one, if its length is known yet (a file not
        // looked at yet has none: the next goes at the same time, on another track).
        if let Some(len) = show
            .cue(id)
            .cloned()
            .and_then(|c| engine.cue_duration(show, &c).flatten())
        {
            at += len;
        }
    }
    if let Some(&last) = added.last() {
        super::super::select_new(show, st, engine, None, last);
    }
    added
}

/// The track a new cue of type `ty` goes on at `at`: for a picture, the first
/// picture track (top layer first) with nothing on it at `at`, else a new layer
/// above the highest of its display, so nothing is replaced; for a sound, the
/// timeline's first sound track; anything else, the cue track.
pub fn fitting_track(
    show: &Show,
    key: TimelineKey,
    ty: CueType,
    at: f64,
    clips: &[model::Clip],
) -> TrackId {
    // What a cue of that type is made with in this show (its display, layer 0).
    let mut probe_show = Show {
        outputs: show.outputs.clone(),
        ..Show::default()
    };
    let natural = model::track_of(&probe_show.make_cue(ty, None, 0));
    let tracks = model::tracks(show, key, clips);
    match natural {
        TrackId::Picture { output, .. } => {
            let busy = |t: &TrackId| {
                clips.iter().any(|c| {
                    c.track == *t
                        && c.armed
                        && c.start <= at + 1e-6
                        && c.len.is_none_or(|l| at < c.start + l - 1e-6)
                })
            };
            let pictures: Vec<TrackId> = tracks
                .iter()
                .copied()
                .filter(|t| matches!(t, TrackId::Picture { .. }))
                .collect();
            if pictures.is_empty() {
                return natural;
            }
            if let Some(free) = pictures.iter().find(|t| !busy(t)) {
                return *free;
            }
            // All taken: a layer above the highest on the display it would be on.
            let home = match pictures[0] {
                TrackId::Picture { output: o, .. } => o.or(output),
                _ => output,
            };
            let top = pictures
                .iter()
                .filter_map(|t| match t {
                    TrackId::Picture { output: o, layer } if *o == home => Some(*layer),
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            TrackId::Picture {
                output: home,
                layer: top + 1,
            }
        }
        TrackId::Sound { .. } => tracks
            .iter()
            .copied()
            .find(|t| matches!(t, TrackId::Sound { .. }))
            .unwrap_or(natural),
        TrackId::Cues => TrackId::Cues,
    }
}

/// The transports' GO read-out (`Engine::go_plan`, which GO follows), with
/// what only a timeline's run knows: a running timeline list's next pause.
pub fn go_readout(show: &Show, engine: &mut Engine) -> Option<(String, Option<String>)> {
    let plan = engine.go_plan(show);
    let said = plan.describe(show)?;
    let GoPlan::Timeline { key, does, .. } = plan else {
        return Some(said);
    };
    let run = engine.timeline(show, key);
    match does {
        TimelineGo::Running => {
            // A loop pause ahead comes before the next pause or the end it is
            // before: GO means something there.
            let next_loop = model::members(show, key)
                .into_iter()
                .filter(|c| c.armed && c.pre_wait.is_finite() && c.pre_wait >= run.pos)
                .filter(|c| matches!(&c.kind, CueKind::Control(ctl) if ctl.is_loop()))
                .map(|c| c.pre_wait)
                .min_by(f64::total_cmp)
                .filter(|l| run.next_pause.is_none_or(|p| *l < p));
            let (what, then) = match (run.next_pause, run.length) {
                _ if let Some(l) = next_loop => (
                    format!("RUNNING · loops back in {}", fmt_time(l - run.pos)),
                    "GO does nothing until it loops",
                ),
                (Some(p), _) if p >= run.pos => (
                    format!("RUNNING · next pause in {}", fmt_time(p - run.pos)),
                    "GO does nothing until it pauses",
                ),
                (_, Some(l)) if l >= run.pos => (
                    format!("RUNNING · ends in {}", fmt_time(l - run.pos)),
                    "GO does nothing until it ends",
                ),
                _ => ("RUNNING".to_string(), "GO does nothing while it runs"),
            };
            Some((what, Some(then.into())))
        }
        _ => Some(said),
    }
}

/// The Operator view's line for a timeline list being worked: how its run is
/// doing and where its clock is.
pub fn operator_state(ui: &mut Ui, show: &Show, engine: &mut Engine) {
    let Some(key) = active(show, engine) else {
        return;
    };
    let run = engine.timeline(show, key);
    let length = run.length.map_or("–:––.–".to_string(), fmt_time);
    let state = if run.at_end {
        "At its end".to_string()
    } else {
        toolbar::state_text(&run, run.pos)
    };
    #[cfg(test)]
    ui.data_mut(|d| d.insert_temp(egui::Id::new("operator_timeline"), state.clone()));
    ui.label(
        RichText::new(format!(
            "TIMELINE · {} · {state} · {} / {length}",
            model::name(show, key),
            fmt_time(run.pos)
        ))
        .size(18.0),
    );
}

/// Whether a cue can go into the list `list`: no group into a timeline list.
pub fn refuses(show: &Show, list: u32, cue: &Cue) -> bool {
    matches!(cue.kind, CueKind::Group(_)) && is_timeline_list(show, list)
}

#[cfg(test)]
mod tests;
