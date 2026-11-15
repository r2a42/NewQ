//! A timeline read out of a show: which cues are its clips, where each starts
//! and how long it is, which track it is on, and the rules that need the whole
//! timeline to answer (a clip waiting for its fade-in, a clip whose end the
//! clock keeps). Pure: the lengths of cues come from the caller (`Dur`).

use super::{TimelineKey, TimelinePoint, TimelineSettings, TrackId};
use crate::show::{
    ControlAction, Cue, CueColor, CueId, CueKind, CueType, GroupMode, MIN_DB, OutputId,
    PauseFreeze, Show, Transition,
};

/// How long a cue's content is, if known: the engine's own length sum, or a fake in tests.
/// (`Some(None)` is open-ended, `None` not known yet, as `Engine::cue_duration`.)
pub type Dur<'a> = &'a mut dyn FnMut(&Cue) -> Option<Option<f64>>;

/// Times closer than this are the same time (a pause cue placed exactly at a
/// clip's end, by sums done another way).
const SAME_TIME: f64 = 1e-6;

/// How long after its start a clip's content waits, unseen, for its fade-in
/// (section 2.5 of the plan): its lead-in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LeadIn {
    /// It starts at its place.
    None,
    /// Its content starts this many seconds after its start, where its opacity
    /// keyframes rise from 0.
    At(f64),
    /// It is invisible (or silent) until a fade cue raises it.
    UntilFaded,
}

/// One clip of a timeline, as the editor draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub id: CueId,
    pub start: f64,
    /// Lead-in included; None = open-ended or not known yet.
    pub len: Option<f64>,
    /// The hatched "waiting to fade in" part.
    pub lead_in: LeadIn,
    /// For the handles and the snap points.
    pub fade_in: f64,
    pub fade_out: f64,
    pub track: TrackId,
    pub ty: CueType,
    pub name: String,
    pub armed: bool,
    pub color: Option<CueColor>,
    /// What the editor may drag.
    pub trim_left: bool,
    pub trim_right: bool,
    pub has_sound: bool,
    pub loops: bool,
    pub speed_keys: bool,
    /// A free-run pause or clip pause lies inside it.
    pub clocked: bool,
    /// Further displays; the track is the home display's.
    pub also_on: Vec<OutputId>,
}

/// What a timeline is called: its group's number and name, or its list's name.
pub fn name(show: &Show, key: TimelineKey) -> String {
    match key {
        TimelineKey::Group(id) => show
            .cue(id)
            .map_or_else(|| "(gone)".into(), |c| format!("{} · {}", c.number, c.name)),
        TimelineKey::List(n) => show.list_name(n),
    }
}

/// What a paused timeline's clock brings when GO runs it on, the cue `id`
/// being the first clip or fade it will reach: "crossfade to 3 · Still",
/// "bring in 4 · Song", or a fade or stop cue's own name. For the editor's
/// toolbar and the transports' GO read-out.
pub fn on_go_text(show: &Show, id: CueId) -> String {
    let Some(c) = show.cue(id) else {
        return "run on".into();
    };
    let label = format!("{} · {}", c.number, c.name);
    match &c.kind {
        CueKind::Fade(_) | CueKind::Stop(_) => label,
        k => match k.media() {
            Some(m) if m.transition == Transition::Crossfade => format!("crossfade to {label}"),
            Some(_) => format!("bring in {label}"),
            None => label,
        },
    }
}

/// Whether `key` is a timeline: a "start all together" group that is not a
/// slide show, or a cue list marked as one (never the main list).
pub fn is_timeline(show: &Show, key: TimelineKey) -> bool {
    match key {
        TimelineKey::Group(id) => show.cue(id).is_some_and(|c| {
            matches!(&c.kind, CueKind::Group(g) if g.mode == GroupMode::Timeline && g.slides.is_none())
        }),
        TimelineKey::List(n) => {
            n != 0 && show.lists.iter().any(|l| l.id == n && l.timeline.is_some())
        }
    }
}

/// The timeline a cue is on: its group's if that is a timeline, else its
/// list's if it is a top-level cue of a timeline list.
pub fn key_of(show: &Show, cue: CueId) -> Option<TimelineKey> {
    let c = show.cue(cue)?;
    let key = match c.parent {
        Some(g) => TimelineKey::Group(g),
        None => TimelineKey::List(c.list),
    };
    is_timeline(show, key).then_some(key)
}

/// The cues on a timeline, in show order: a group's children, or a list's
/// top-level cues. (By structure only: see `is_timeline` for whether it is one.)
pub fn members(show: &Show, key: TimelineKey) -> Vec<&Cue> {
    match key {
        TimelineKey::Group(id) => show.children(id).iter().collect(),
        TimelineKey::List(n) => show
            .cues
            .iter()
            .filter(|c| c.list == n && c.parent.is_none())
            .collect(),
    }
}

pub fn settings(show: &Show, key: TimelineKey) -> Option<&TimelineSettings> {
    match key {
        TimelineKey::Group(id) => match &show.cue(id)?.kind {
            CueKind::Group(g) => g.timeline.as_ref(),
            _ => None,
        },
        TimelineKey::List(n) => show
            .lists
            .iter()
            .find(|l| l.id == n)
            .and_then(|l| l.timeline.as_ref()),
    }
}

/// A timeline's settings to change. A timeline group without any gets the
/// defaults (`settle` takes them away again); a list that is not a timeline
/// has none (making it one is `edit::set_list_timeline`).
pub fn settings_mut(show: &mut Show, key: TimelineKey) -> Option<&mut TimelineSettings> {
    if !is_timeline(show, key) {
        return None;
    }
    match key {
        TimelineKey::Group(id) => match &mut show.cues.iter_mut().find(|c| c.id == id)?.kind {
            CueKind::Group(g) => Some(g.timeline.get_or_insert_with(TimelineSettings::default)),
            _ => None,
        },
        TimelineKey::List(n) => show
            .lists
            .iter_mut()
            .find(|l| l.id == n)
            .and_then(|l| l.timeline.as_mut()),
    }
}

/// A group's settings that are back to the defaults go, so the group is
/// written as it was before it had any. (A list's stay: they mark it a timeline.)
pub fn settle(show: &mut Show, key: TimelineKey) {
    if let TimelineKey::Group(id) = key
        && let Some(c) = show.cues.iter_mut().find(|c| c.id == id)
        && let CueKind::Group(g) = &mut c.kind
        && g.timeline
            .as_ref()
            .is_some_and(TimelineSettings::is_default)
    {
        g.timeline = None;
    }
}

/// Cues that put a picture on a display.
pub fn is_picture(kind: &CueKind) -> bool {
    matches!(
        kind,
        CueKind::Video(_)
            | CueKind::Image(_)
            | CueKind::Text(_)
            | CueKind::Shape(_)
            | CueKind::Camera(_)
    )
}

/// Cues that are only sound.
pub fn is_sound(kind: &CueKind) -> bool {
    matches!(kind, CueKind::Audio(_) | CueKind::LiveAudio(_))
}

/// The track a cue is drawn on: a picture's display and layer, a sound's audio
/// output (a video's own sound is part of its picture clip), or the cue track.
pub fn track_of(cue: &Cue) -> TrackId {
    match cue.kind.media() {
        Some(m) if is_picture(&cue.kind) => TrackId::Picture {
            output: m.output,
            layer: m.layer,
        },
        Some(m) if is_sound(&cue.kind) => TrackId::Sound {
            output: m.audio_output,
        },
        _ => TrackId::Cues,
    }
}

/// Where a cue starts on its timeline: its pre-wait (never before 0).
fn start_of(cue: &Cue) -> f64 {
    if cue.pre_wait.is_finite() {
        cue.pre_wait.max(0.0)
    } else {
        0.0
    }
}

/// A clip's lead-in (section 2.5): only on a timeline with
/// `start_on_fade_in`, and not for a clip that `runs_unseen`. A fade-in by the
/// cue's own transition never defers it; opacity keyframes that are 0 from the
/// start defer it to the last of those keys, and a still opacity of 0 (a sound
/// at silence) until a fade cue raises it.
pub fn lead_in(show: &Show, key: TimelineKey, cue: &Cue) -> LeadIn {
    if !settings(show, key).is_some_and(|s| s.start_on_fade_in) {
        return LeadIn::None;
    }
    let Some(m) = cue.kind.media() else {
        return LeadIn::None;
    };
    if m.runs_unseen {
        return LeadIn::None;
    }
    if is_picture(&cue.kind) {
        match &m.motion.opacity {
            Some(a) if a.is_animated() => {
                // The keys in order, while they are all 0: the rise starts from the last.
                let mut from = None;
                for k in &a.keys {
                    if k.v > 0.0 {
                        return match from {
                            Some(t) if t > 0.0 => LeadIn::At(t),
                            _ => LeadIn::None,
                        };
                    }
                    from = Some(k.t);
                }
                // Never rises by itself.
                LeadIn::UntilFaded
            }
            _ if m.opacity <= 0.0 => LeadIn::UntilFaded,
            _ => LeadIn::None,
        }
    } else if is_sound(&cue.kind) && m.volume_db <= MIN_DB {
        LeadIn::UntilFaded
    } else {
        LeadIn::None
    }
}

/// For a clip that waits until it is faded up (`LeadIn::UntilFaded`): how
/// long after its start the first fade cue of the timeline that raises it (or
/// its group) comes, if one does.
pub fn fade_wake(show: &Show, key: TimelineKey, cue: &Cue) -> Option<f64> {
    let start = start_of(cue);
    let sound = is_sound(&cue.kind);
    members(show, key)
        .into_iter()
        .filter(|f| f.armed)
        .filter_map(|f| match &f.kind {
            CueKind::Fade(fade) => Some((start_of(f), fade)),
            _ => None,
        })
        .filter(|(t, fade)| {
            let aims =
                fade.target.is_some() && (fade.target == Some(cue.id) || fade.target == cue.parent);
            let raises = if sound {
                fade.volume_db.is_some_and(|v| v > MIN_DB)
            } else {
                fade.opacity.is_some_and(|o| o > 0.0)
            };
            aims && raises && *t + SAME_TIME >= start
        })
        .map(|(t, _)| (t - start).max(0.0))
        .min_by(f64::total_cmp)
}

/// Whether a clip's end is kept by the timeline's clock rather than by its
/// content (section 2.3): a free-run timeline pause (one that doesn't hold
/// this clip) or a Pause of this clip lies inside it, start to end inclusive.
/// `len` is the clip's length on the timeline (None = open-ended).
pub fn clocked(show: &Show, key: TimelineKey, cue: &Cue, len: Option<f64>) -> bool {
    if cue.kind.media().is_none() {
        return false;
    }
    let start = start_of(cue);
    let end = len.map(|l| start + l.max(0.0));
    members(show, key)
        .into_iter()
        .filter(|p| p.armed && p.id != cue.id)
        .any(|p| {
            let CueKind::Control(c) = &p.kind else {
                return false;
            };
            let t = start_of(p);
            let inside = t + SAME_TIME >= start && end.is_none_or(|e| t <= e + SAME_TIME);
            inside
                && match c.action {
                    ControlAction::TimelinePause => {
                        c.timeline.is_none_or(|k| k == key)
                            && match c.freeze {
                                PauseFreeze::Nothing => true,
                                PauseFreeze::Everything => false,
                                PauseFreeze::Clips => !c.clips.contains(&cue.id),
                            }
                    }
                    ControlAction::Pause => c.target == Some(cue.id),
                    _ => false,
                }
        })
}

/// The clips of a timeline, in show order.
pub fn clips(show: &Show, key: TimelineKey, dur: Dur) -> Vec<Clip> {
    members(show, key)
        .into_iter()
        .map(|cue| {
            let lead = lead_in(show, key, cue);
            let waits = match lead {
                LeadIn::None => 0.0,
                LeadIn::At(v) => v,
                LeadIn::UntilFaded => fade_wake(show, key, cue).unwrap_or(0.0),
            };
            let len = match dur(cue) {
                Some(Some(d)) => Some(d.max(0.0) + waits),
                _ => None,
            };
            let media = cue.kind.media();
            let file = cue.kind.file_media();
            let loops = media.is_some_and(|m| m.loop_media);
            let speed_keys = file.is_some_and(|m| m.rate.is_animated());
            let trims = !loops
                && matches!(
                    cue.kind,
                    CueKind::Video(_)
                        | CueKind::Audio(_)
                        | CueKind::Image(_)
                        | CueKind::Text(_)
                        | CueKind::Shape(_)
                        | CueKind::Camera(_)
                        | CueKind::LiveAudio(_)
                        | CueKind::Fade(_)
                        | CueKind::Wait(_)
                );
            Clip {
                id: cue.id,
                start: start_of(cue),
                len,
                lead_in: lead,
                fade_in: media.map_or(0.0, |m| m.fade_in.max(0.0)),
                fade_out: media.map_or(0.0, |m| m.fade_out.max(0.0)),
                track: track_of(cue),
                ty: cue.kind.cue_type(),
                name: cue.name.clone(),
                armed: cue.armed,
                color: cue.color,
                trim_left: trims && !speed_keys,
                trim_right: trims,
                has_sound: matches!(
                    cue.kind,
                    CueKind::Video(_) | CueKind::Audio(_) | CueKind::LiveAudio(_)
                ),
                loops,
                speed_keys,
                clocked: clocked(show, key, cue, len),
                also_on: media.map_or_else(Vec::new, |m| m.also_on.clone()),
            }
        })
        .collect()
}

/// The tracks to draw, top to bottom: pictures by display order then layer
/// high to low, sounds by output order, the cue track last (always there).
/// Those of the clips (and of their ghosts on the other displays a clip is
/// also on, `ghosts`) and the ones the settings keep, each once.
pub fn tracks(show: &Show, key: TimelineKey, clips: &[Clip]) -> Vec<TrackId> {
    let kept = settings(show, key).map_or(&[][..], |s| &s.tracks[..]);
    let mut out: Vec<TrackId> = Vec::new();
    for t in clips
        .iter()
        .map(|c| c.track)
        .chain(ghosts(clips).into_iter().map(|g| g.1))
        .chain(kept.iter().copied())
        .chain([TrackId::Cues])
    {
        if !out.contains(&t) {
            out.push(t);
        }
    }
    // A track with no display (or one the show no longer has) after those it has.
    let display = |o: Option<crate::show::OutputId>| {
        o.and_then(|o| show.outputs.iter().position(|d| d.id == o))
            .unwrap_or(usize::MAX)
    };
    let audio = |o: Option<crate::show::AudioOutputId>| match o {
        None => 0,
        Some(a) => show
            .audio_outputs
            .iter()
            .position(|d| d.id == a)
            .unwrap_or(usize::MAX),
    };
    out.sort_by_key(|t| match *t {
        TrackId::Picture { output, layer } => (0, display(output), -i64::from(layer)),
        TrackId::Sound { output } => (1, audio(output), 0),
        TrackId::Cues => (2, 0, 0),
    });
    out
}

/// Where a clip that is also on other displays (`Clip::also_on`) shows there:
/// a ghost of it on the same layer's track of each (clip, that track). Its
/// home display, and a display named twice, give none.
pub fn ghosts(clips: &[Clip]) -> Vec<(CueId, TrackId)> {
    let mut out = Vec::new();
    for c in clips {
        let TrackId::Picture { output, layer } = c.track else {
            continue;
        };
        for o in &c.also_on {
            let t = TrackId::Picture {
                output: Some(*o),
                layer,
            };
            if Some(*o) != output && !out.contains(&(c.id, t)) {
                out.push((c.id, t));
            }
        }
    }
    out
}

/// A clip that waits for its fade-in and enters by a cut while another clip
/// on its track is still showing then (plan 2.13, 5): the cut takes that one
/// off at once, and this one fades up from nothing. Returns the clip it cuts.
pub fn cuts_over(show: &Show, key: TimelineKey, clips: &[Clip], id: CueId) -> Option<CueId> {
    let c = clips.iter().find(|c| c.id == id)?;
    let cue = show.cue(id)?;
    let m = cue.kind.media()?;
    if !is_picture(&cue.kind) || m.transition != Transition::Cut {
        return None;
    }
    let enters = c.start
        + match c.lead_in {
            LeadIn::None => return None,
            LeadIn::At(v) => v,
            LeadIn::UntilFaded => fade_wake(show, key, cue)?,
        };
    clips
        .iter()
        .filter(|o| o.id != id && o.armed && o.track == c.track)
        .find(|o| o.start < enters - SAME_TIME && o.len.is_none_or(|l| o.start + l > enters))
        .map(|o| o.id)
}

/// How long the timeline runs: to the end of its last armed clip. None if
/// any is open-ended or not known yet.
pub fn length(clips: &[Clip]) -> Option<f64> {
    clips
        .iter()
        .filter(|c| c.armed)
        .try_fold(0.0_f64, |end, c| c.len.map(|l| end.max(c.start + l)))
}

/// The time on the timeline a point stands for, if it is one there.
pub fn resolve(show: &Show, key: TimelineKey, p: TimelinePoint) -> Option<f64> {
    match p {
        TimelinePoint::Time(t) => t.is_finite().then_some(t.max(0.0)),
        TimelinePoint::Marker(id) => settings(show, key)?
            .markers
            .iter()
            .find(|m| m.id == id)
            .map(|m| m.time),
        TimelinePoint::Cue(id) => members(show, key)
            .into_iter()
            .find(|c| c.id == id)
            .map(start_of),
    }
}

/// What a track is called: "V2 · Layer 1" (with more than one display, its
/// name first), "A1 · Main", "Cues".
pub fn track_label(show: &Show, t: TrackId) -> String {
    match t {
        TrackId::Picture { output, layer } => {
            let track = format!("V{} · Layer {layer}", i64::from(layer) + 1);
            let only = show.outputs.len() == 1 && output == show.outputs.first().map(|o| o.id);
            if only {
                track
            } else {
                let name = output
                    .and_then(|o| show.output(o))
                    .map_or_else(|| "No display".to_string(), |o| o.name.clone());
                format!("{name} · {track}")
            }
        }
        TrackId::Sound { output } => {
            let (n, name) = match output {
                None => (
                    0,
                    show.audio_outputs
                        .first()
                        .map_or_else(|| "Default".to_string(), |o| o.name.clone()),
                ),
                Some(a) => match show.audio_outputs.iter().position(|o| o.id == a) {
                    Some(i) => (i, show.audio_outputs[i].name.clone()),
                    None => (show.audio_outputs.len(), "Missing output".to_string()),
                },
            };
            format!("A{} · {name}", n + 1)
        }
        TrackId::Cues => "Cues".to_string(),
    }
}

/// The order of things due at the same time: control, then fade and stop, then clips.
pub fn due_order(cue: &Cue) -> u8 {
    match cue.kind {
        CueKind::Control(_) => 0,
        CueKind::Fade(_) | CueKind::Stop(_) => 1,
        _ => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::{
        Animated, AudioOutputConfig, AudioOutputId, ControlCue, CueType, Keyframe, Marker,
        OutputId, SlideShow,
    };

    /// A show with a timeline group (made in the editor) and a timeline list.
    struct Fixture {
        show: Show,
        group: CueId,
        list: u32,
    }

    fn add(show: &mut Show, ty: CueType, parent: Option<CueId>, list: u32, at: f64) -> CueId {
        let mut c = show.make_cue(ty, None, show.cues.len());
        c.parent = parent;
        c.list = list;
        c.pre_wait = at;
        let id = c.id;
        show.cues.push(c);
        id
    }

    fn cue_mut(show: &mut Show, id: CueId) -> &mut Cue {
        show.cues.iter_mut().find(|c| c.id == id).unwrap()
    }

    fn media_of(show: &mut Show, id: CueId) -> &mut crate::show::MediaCue {
        cue_mut(show, id).kind.media_mut().unwrap()
    }

    fn fixture() -> Fixture {
        let mut show = Show::default();
        let group = add(&mut show, CueType::Group, None, 0, 0.0);
        if let CueKind::Group(g) = &mut cue_mut(&mut show, group).kind {
            g.timeline = Some(TimelineSettings::for_editor());
        }
        let list = show.add_list("Timeline");
        show.lists[0].timeline = Some(TimelineSettings::default());
        Fixture { show, group, list }
    }

    fn control(show: &mut Show, id: CueId, f: impl FnOnce(&mut ControlCue)) {
        let mut c = ControlCue::default();
        f(&mut c);
        cue_mut(show, id).kind = CueKind::Control(c);
    }

    #[test]
    fn what_is_a_timeline_and_which_one_a_cue_is_on() {
        let Fixture {
            mut show,
            group,
            list,
        } = fixture();
        let child = add(&mut show, CueType::Video, Some(group), 0, 1.0);
        let plain = add(&mut show, CueType::Video, None, 0, 0.0);
        let in_list = add(&mut show, CueType::Audio, None, list, 2.0);
        let g = TimelineKey::Group(group);
        let l = TimelineKey::List(list);
        assert!(is_timeline(&show, g) && is_timeline(&show, l));
        assert!(
            !is_timeline(&show, TimelineKey::List(0)),
            "never the main list"
        );
        assert!(!is_timeline(&show, TimelineKey::Group(plain)));
        assert_eq!(key_of(&show, child), Some(g));
        assert_eq!(key_of(&show, in_list), Some(l));
        assert_eq!(key_of(&show, plain), None);
        assert_eq!(key_of(&show, group), None, "the group is in the main list");
        let ids = |v: Vec<&Cue>| v.iter().map(|c| c.id).collect::<Vec<_>>();
        assert_eq!(ids(members(&show, g)), vec![child]);
        assert_eq!(ids(members(&show, l)), vec![in_list]);
        // A play-in-order group and a slide show are not timelines.
        if let CueKind::Group(gc) = &mut cue_mut(&mut show, group).kind {
            gc.mode = GroupMode::Playlist;
        }
        assert!(!is_timeline(&show, g) && key_of(&show, child).is_none());
        if let CueKind::Group(gc) = &mut cue_mut(&mut show, group).kind {
            gc.mode = GroupMode::Timeline;
            gc.slides = Some(SlideShow::default());
        }
        assert!(!is_timeline(&show, g));
        // A list without settings is a plain list.
        show.lists[0].timeline = None;
        assert!(!is_timeline(&show, l) && key_of(&show, in_list).is_none());
    }

    #[test]
    fn settings_are_made_for_a_group_and_go_again_when_they_are_the_defaults() {
        let Fixture {
            mut show,
            group,
            list,
        } = fixture();
        let g = TimelineKey::Group(group);
        if let CueKind::Group(gc) = &mut cue_mut(&mut show, group).kind {
            gc.timeline = None;
        }
        assert!(settings(&show, g).is_none());
        settings_mut(&mut show, g).unwrap().markers.push(Marker {
            id: 1,
            time: 3.0,
            name: "A".into(),
            color: None,
        });
        settle(&mut show, g);
        assert_eq!(settings(&show, g).unwrap().markers.len(), 1);
        settings_mut(&mut show, g).unwrap().markers.clear();
        settle(&mut show, g);
        assert!(settings(&show, g).is_none(), "back to none");
        let json = serde_json::to_string(&show).unwrap();
        assert!(
            !json.contains("\"timeline\":null") && json.matches("\"timeline\":").count() == 1,
            "only the list's: {json}"
        );
        // A list keeps its settings, even at the defaults: they make it a timeline.
        let l = TimelineKey::List(list);
        settle(&mut show, l);
        assert!(settings(&show, l).is_some());
        // And a list that is not a timeline doesn't become one this way, nor
        // does the main list.
        let other = show.add_list("Plain");
        assert!(settings_mut(&mut show, TimelineKey::List(other)).is_none());
        assert!(!is_timeline(&show, TimelineKey::List(other)));
        assert!(settings_mut(&mut show, TimelineKey::List(0)).is_none());
        assert!(!is_timeline(&show, TimelineKey::List(0)));
    }

    #[test]
    fn a_clip_carries_its_further_displays() {
        let Fixture {
            mut show, group, ..
        } = fixture();
        let d1 = show.outputs[0].id;
        let d2 = show.add_display();
        let v = add(&mut show, CueType::Video, Some(group), 0, 0.0);
        let m = media_of(&mut show, v);
        m.output = Some(d1);
        m.also_on = vec![d2];
        let w = add(&mut show, CueType::Wait, Some(group), 0, 0.0);
        let cl = clips(&show, TimelineKey::Group(group), &mut |_| Some(Some(1.0)));
        assert_eq!(cl[0].also_on, vec![d2]);
        assert_eq!(
            cl[0].track,
            TrackId::Picture {
                output: Some(d1),
                layer: 0
            },
            "on its home display's track"
        );
        assert_eq!((cl[1].id, cl[1].also_on.len()), (w, 0));
    }

    #[test]
    fn a_clip_on_other_displays_has_a_ghost_on_each_and_their_tracks_are_drawn() {
        let Fixture {
            mut show, group, ..
        } = fixture();
        let d1 = show.outputs[0].id;
        let d2 = show.add_display();
        let d3 = show.add_display();
        let v = add(&mut show, CueType::Video, Some(group), 0, 0.0);
        let m = media_of(&mut show, v);
        m.output = Some(d1);
        m.layer = 2;
        // Its home and a display named twice give no more ghosts.
        m.also_on = vec![d3, d1, d3];
        let key = TimelineKey::Group(group);
        let cl = clips(&show, key, &mut |_| Some(Some(1.0)));
        let on = |d| TrackId::Picture {
            output: Some(d),
            layer: 2,
        };
        assert_eq!(ghosts(&cl), vec![(v, on(d3))]);
        assert_eq!(tracks(&show, key, &cl), vec![on(d1), on(d3), TrackId::Cues]);
        let _ = d2;
    }

    #[test]
    fn a_waiting_clip_that_cuts_in_over_another_on_its_track_is_found() {
        let Fixture {
            mut show, group, ..
        } = fixture();
        let key = TimelineKey::Group(group);
        let under = add(&mut show, CueType::Image, Some(group), 0, 0.0);
        media_of(&mut show, under).end = Some(10.0);
        let over = add(&mut show, CueType::Image, Some(group), 0, 2.0);
        let m = media_of(&mut show, over);
        m.end = Some(5.0);
        m.transition = Transition::Cut;
        m.motion.opacity = Some(keys(&[(0.0, 0.0), (3.0, 0.0), (4.0, 1.0)]));
        let found = |show: &Show| {
            let cl = clips(show, key, &mut |c| {
                Some(c.kind.media().and_then(|m| m.end).or(Some(0.0)))
            });
            cuts_over(show, key, &cl, over)
        };
        // It enters at 5 s, while the one under it shows until 10 s.
        assert_eq!(found(&show), Some(under));
        // Not with a fade-in transition, nor once the other has ended.
        media_of(&mut show, over).transition = Transition::FadeIn;
        assert_eq!(found(&show), None);
        media_of(&mut show, over).transition = Transition::Cut;
        media_of(&mut show, under).end = Some(4.0);
        assert_eq!(found(&show), None);
        // Nor without a lead-in (it shows from its start, as any cut does).
        media_of(&mut show, under).end = Some(10.0);
        media_of(&mut show, over).motion.opacity = None;
        assert_eq!(found(&show), None);
    }

    #[test]
    fn every_kind_of_cue_has_its_track() {
        let mut show = Show::default();
        let out = show.outputs[0].id;
        for ty in [
            CueType::Video,
            CueType::Image,
            CueType::Text,
            CueType::Clock,
            CueType::Timer,
            CueType::Shape,
            CueType::Camera,
        ] {
            let mut c = show.make_cue(ty, None, 0);
            c.kind.media_mut().unwrap().layer = 3;
            c.kind.media_mut().unwrap().output = Some(out);
            assert_eq!(
                track_of(&c),
                TrackId::Picture {
                    output: Some(out),
                    layer: 3
                },
                "{ty:?}"
            );
        }
        for ty in [CueType::Audio, CueType::LiveAudio] {
            let mut c = show.make_cue(ty, None, 0);
            c.kind.media_mut().unwrap().audio_output = Some(AudioOutputId(8));
            assert_eq!(
                track_of(&c),
                TrackId::Sound {
                    output: Some(AudioOutputId(8))
                },
                "{ty:?}"
            );
        }
        for ty in [
            CueType::Fade,
            CueType::Stop,
            CueType::Wait,
            CueType::Group,
            CueType::Send,
            CueType::Control,
            CueType::Timecode,
        ] {
            assert_eq!(
                track_of(&show.make_cue(ty, None, 0)),
                TrackId::Cues,
                "{ty:?}"
            );
        }
    }

    #[test]
    fn tracks_go_pictures_by_display_and_layer_then_sounds_then_cues() {
        let Fixture {
            mut show, group, ..
        } = fixture();
        let d1 = show.outputs[0].id;
        show.add_display();
        let d2 = show.outputs.last().unwrap().id;
        assert_ne!(d1, d2);
        show.audio_outputs = vec![
            AudioOutputConfig {
                id: AudioOutputId(50),
                name: "Main".into(),
                device: None,
                delay_ms: 0,
            },
            AudioOutputConfig {
                id: AudioOutputId(51),
                name: "Foldback".into(),
                device: None,
                delay_ms: 0,
            },
        ];
        let pic = |show: &mut Show, out: OutputId, layer: i32| {
            let id = add(show, CueType::Image, Some(group), 0, 0.0);
            let m = cue_mut(show, id).kind.media_mut().unwrap();
            m.output = Some(out);
            m.layer = layer;
        };
        pic(&mut show, d2, 0);
        pic(&mut show, d1, 0);
        pic(&mut show, d1, 2);
        let a = add(&mut show, CueType::Audio, Some(group), 0, 0.0);
        cue_mut(&mut show, a).kind.media_mut().unwrap().audio_output = Some(AudioOutputId(51));
        add(&mut show, CueType::Audio, Some(group), 0, 0.0);
        add(&mut show, CueType::Fade, Some(group), 0, 0.0);
        let key = TimelineKey::Group(group);
        // A kept empty track comes in its place.
        settings_mut(&mut show, key)
            .unwrap()
            .tracks
            .push(TrackId::Picture {
                output: Some(d1),
                layer: 1,
            });
        let cl = clips(&show, key, &mut |_| Some(Some(1.0)));
        let order = tracks(&show, key, &cl);
        assert_eq!(
            order,
            vec![
                TrackId::Picture {
                    output: Some(d1),
                    layer: 2
                },
                TrackId::Picture {
                    output: Some(d1),
                    layer: 1
                },
                TrackId::Picture {
                    output: Some(d1),
                    layer: 0
                },
                TrackId::Picture {
                    output: Some(d2),
                    layer: 0
                },
                TrackId::Sound { output: None },
                TrackId::Sound {
                    output: Some(AudioOutputId(51))
                },
                TrackId::Cues,
            ]
        );
        assert_eq!(
            track_label(&show, order[0]),
            format!("{} · V3 · Layer 2", show.outputs[0].name)
        );
        assert_eq!(track_label(&show, order[4]), "A1 · Main");
        assert_eq!(track_label(&show, order[5]), "A2 · Foldback");
        assert_eq!(track_label(&show, TrackId::Cues), "Cues");
        // With one display its name isn't needed.
        let one = Show::default();
        let only = TrackId::Picture {
            output: Some(one.outputs[0].id),
            layer: 1,
        };
        assert_eq!(track_label(&one, only), "V2 · Layer 1");
        assert_eq!(
            track_label(&one, TrackId::Sound { output: None }),
            "A1 · Default"
        );
        // An empty timeline still has the cue track.
        let empty = fixture();
        assert_eq!(
            tracks(&empty.show, TimelineKey::Group(empty.group), &[]),
            vec![TrackId::Cues]
        );
    }

    #[test]
    fn clips_take_their_lengths_from_the_caller_and_their_flags_from_the_cue() {
        let Fixture {
            mut show, group, ..
        } = fixture();
        let v = add(&mut show, CueType::Video, Some(group), 0, 2.0);
        let looped = add(&mut show, CueType::Video, Some(group), 0, 1.0);
        cue_mut(&mut show, looped)
            .kind
            .media_mut()
            .unwrap()
            .loop_media = true;
        let ramp = add(&mut show, CueType::Audio, Some(group), 0, 0.5);
        let mut rate = Animated::new(1.0);
        rate.set_key(0.0, 1.0);
        rate.set_key(2.0, 2.0);
        cue_mut(&mut show, ramp).kind.media_mut().unwrap().rate = rate;
        let open = add(&mut show, CueType::Image, Some(group), 0, 4.0);
        let unknown = add(&mut show, CueType::Video, Some(group), 0, 3.0);
        let wait = add(&mut show, CueType::Wait, Some(group), 0, -1.0);
        cue_mut(&mut show, wait).armed = false;
        let key = TimelineKey::Group(group);
        let mut asked = Vec::new();
        let cl = clips(&show, key, &mut |c| {
            asked.push(c.id);
            if c.id == open {
                Some(None)
            } else if c.id == unknown {
                None
            } else {
                Some(Some(5.0))
            }
        });
        assert_eq!(asked, vec![v, looped, ramp, open, unknown, wait]);
        let get = |id| cl.iter().find(|c| c.id == id).unwrap();
        assert_eq!((get(v).start, get(v).len), (2.0, Some(5.0)));
        assert!(get(v).trim_left && get(v).trim_right && get(v).has_sound);
        assert!(get(looped).loops && !get(looped).trim_left && !get(looped).trim_right);
        assert!(get(ramp).speed_keys && !get(ramp).trim_left && get(ramp).trim_right);
        assert_eq!(get(open).len, None);
        assert!(!get(open).has_sound && get(open).trim_right);
        assert_eq!(get(unknown).len, None);
        assert_eq!(get(wait).start, 0.0, "never before the start");
        assert!(!get(wait).armed && get(wait).track == TrackId::Cues);
        assert_eq!(get(v).ty, CueType::Video);
        // Open-ended: no length. Known: to the end of the last armed clip.
        assert_eq!(length(&cl), None);
        let known: Vec<Clip> = cl.iter().filter(|c| c.len.is_some()).cloned().collect();
        assert_eq!(length(&known), Some(7.0));
        assert_eq!(length(&[]), Some(0.0));
    }

    #[test]
    fn points_resolve_to_a_time_a_marker_or_a_clips_start() {
        let Fixture {
            mut show, group, ..
        } = fixture();
        let key = TimelineKey::Group(group);
        let c = add(&mut show, CueType::Video, Some(group), 0, 6.5);
        let elsewhere = add(&mut show, CueType::Video, None, 0, 1.0);
        settings_mut(&mut show, key).unwrap().markers.push(Marker {
            id: 4,
            time: 12.0,
            name: "Chorus".into(),
            color: None,
        });
        assert_eq!(resolve(&show, key, TimelinePoint::Time(3.0)), Some(3.0));
        assert_eq!(resolve(&show, key, TimelinePoint::Time(-3.0)), Some(0.0));
        assert_eq!(resolve(&show, key, TimelinePoint::Time(f64::NAN)), None);
        assert_eq!(resolve(&show, key, TimelinePoint::Marker(4)), Some(12.0));
        assert_eq!(resolve(&show, key, TimelinePoint::Marker(5)), None);
        assert_eq!(resolve(&show, key, TimelinePoint::Cue(c)), Some(6.5));
        assert_eq!(resolve(&show, key, TimelinePoint::Cue(elsewhere)), None);
    }

    fn keys(pairs: &[(f64, f32)]) -> Animated {
        Animated {
            value: 1.0,
            keys: pairs
                .iter()
                .map(|&(t, v)| Keyframe {
                    t,
                    v,
                    curve: crate::show::Curve::Linear,
                })
                .collect(),
        }
    }

    #[test]
    fn a_clip_waits_for_its_fade_in_only_where_the_timeline_says_so() {
        let Fixture {
            mut show,
            group,
            list,
        } = fixture();
        let key = TimelineKey::Group(group);
        let clip = add(&mut show, CueType::Video, Some(group), 0, 2.0);
        let lead = |show: &Show| lead_in(show, key, show.cue(clip).unwrap());
        // Visible, with or without a fade-in by its transition: it starts at its place.
        assert_eq!(lead(&show), LeadIn::None);
        media_of(&mut show, clip).fade_in = 3.0;
        assert_eq!(lead(&show), LeadIn::None, "a transition never defers it");
        // Opacity keys at 0 until 4 s, then rising.
        media_of(&mut show, clip).motion.opacity =
            Some(keys(&[(0.0, 0.0), (4.0, 0.0), (5.0, 1.0)]));
        assert_eq!(lead(&show), LeadIn::At(4.0));
        // Rising from the very start is no lead-in.
        media_of(&mut show, clip).motion.opacity = Some(keys(&[(0.0, 0.0), (1.0, 1.0)]));
        assert_eq!(lead(&show), LeadIn::None);
        // Visible from the start, then down and up again: no lead-in.
        media_of(&mut show, clip).motion.opacity =
            Some(keys(&[(0.0, 1.0), (2.0, 0.0), (3.0, 1.0)]));
        assert_eq!(lead(&show), LeadIn::None);
        // Keys that stay at 0: until a fade cue.
        media_of(&mut show, clip).motion.opacity = Some(keys(&[(1.0, 0.0), (3.0, 0.0)]));
        assert_eq!(lead(&show), LeadIn::UntilFaded);
        // A still opacity of 0.
        media_of(&mut show, clip).motion.opacity = None;
        media_of(&mut show, clip).opacity = 0.0;
        assert_eq!(lead(&show), LeadIn::UntilFaded);
        // A clip that runs unseen starts at its place.
        media_of(&mut show, clip).runs_unseen = true;
        assert_eq!(lead(&show), LeadIn::None);
        media_of(&mut show, clip).runs_unseen = false;
        // The flag off (an old group): never.
        settings_mut(&mut show, key).unwrap().start_on_fade_in = false;
        assert_eq!(lead(&show), LeadIn::None);
        settings_mut(&mut show, key).unwrap().start_on_fade_in = true;
        // A sound at silence waits for a fade cue; at any other level it doesn't.
        let snd = add(&mut show, CueType::Audio, Some(group), 0, 0.0);
        assert_eq!(lead_in(&show, key, show.cue(snd).unwrap()), LeadIn::None);
        cue_mut(&mut show, snd).kind.media_mut().unwrap().volume_db = MIN_DB;
        assert_eq!(
            lead_in(&show, key, show.cue(snd).unwrap()),
            LeadIn::UntilFaded
        );
        // Anything else: no lead-in.
        let wait = add(&mut show, CueType::Wait, Some(group), 0, 0.0);
        assert_eq!(lead_in(&show, key, show.cue(wait).unwrap()), LeadIn::None);
        // A timeline list without the flag.
        let in_list = add(&mut show, CueType::Image, None, list, 0.0);
        cue_mut(&mut show, in_list)
            .kind
            .media_mut()
            .unwrap()
            .opacity = 0.0;
        assert_eq!(
            lead_in(&show, TimelineKey::List(list), show.cue(in_list).unwrap()),
            LeadIn::None
        );
    }

    #[test]
    fn a_waiting_clips_bar_includes_its_lead_in() {
        let Fixture {
            mut show, group, ..
        } = fixture();
        let key = TimelineKey::Group(group);
        let keyed = add(&mut show, CueType::Video, Some(group), 0, 2.0);
        cue_mut(&mut show, keyed)
            .kind
            .media_mut()
            .unwrap()
            .motion
            .opacity = Some(keys(&[(0.0, 0.0), (4.0, 0.0), (5.0, 1.0)]));
        let still = add(&mut show, CueType::Image, Some(group), 0, 1.0);
        cue_mut(&mut show, still).kind.media_mut().unwrap().opacity = 0.0;
        let never = add(&mut show, CueType::Image, Some(group), 0, 1.0);
        cue_mut(&mut show, never).kind.media_mut().unwrap().opacity = 0.0;
        // A fade cue that raises `still` at 7 s, one that lowers `never`, and
        // one before `still` starts.
        let fade = |show: &mut Show, at: f64, target: CueId, opacity: f32| {
            let f = add(show, CueType::Fade, Some(group), 0, at);
            if let CueKind::Fade(fc) = &mut cue_mut(show, f).kind {
                fc.target = Some(target);
                fc.opacity = Some(opacity);
            }
        };
        fade(&mut show, 0.5, still, 1.0);
        fade(&mut show, 7.0, still, 1.0);
        fade(&mut show, 9.0, still, 0.5);
        fade(&mut show, 3.0, never, 0.0);
        let cl = clips(&show, key, &mut |_| Some(Some(10.0)));
        let get = |id| cl.iter().find(|c| c.id == id).unwrap();
        assert_eq!(get(keyed).lead_in, LeadIn::At(4.0));
        assert_eq!(get(keyed).len, Some(14.0));
        assert_eq!(get(still).lead_in, LeadIn::UntilFaded);
        assert_eq!(get(still).len, Some(16.0), "6 s waiting, then 10");
        assert_eq!(fade_wake(&show, key, show.cue(still).unwrap()), Some(6.0));
        assert_eq!(get(never).len, Some(10.0), "nothing raises it");
        assert_eq!(fade_wake(&show, key, show.cue(never).unwrap()), None);
    }

    #[test]
    fn a_clip_is_clocked_when_a_free_run_pause_or_its_own_pause_lies_inside_it() {
        let Fixture {
            mut show, group, ..
        } = fixture();
        let key = TimelineKey::Group(group);
        let clip = add(&mut show, CueType::Video, Some(group), 0, 2.0);
        let len = Some(5.0);
        let pause = add(&mut show, CueType::Control, Some(group), 0, 4.0);
        control(&mut show, pause, |c| {
            c.action = ControlAction::TimelinePause
        });
        let is = |show: &Show| clocked(show, key, show.cue(clip).unwrap(), len);
        assert!(is(&show), "inside");
        for (at, inside) in [(2.0, true), (7.0, true), (1.9, false), (7.1, false)] {
            cue_mut(&mut show, pause).pre_wait = at;
            assert_eq!(is(&show), inside, "a pause at {at}");
        }
        // Open-ended: everything after its start is inside.
        cue_mut(&mut show, pause).pre_wait = 100.0;
        assert!(clocked(&show, key, show.cue(clip).unwrap(), None));
        cue_mut(&mut show, pause).pre_wait = 4.0;
        // A pause that holds everything, or this clip, keeps it with the clock.
        control(&mut show, pause, |c| {
            c.action = ControlAction::TimelinePause;
            c.freeze = PauseFreeze::Everything;
        });
        assert!(!is(&show));
        control(&mut show, pause, |c| {
            c.action = ControlAction::TimelinePause;
            c.freeze = PauseFreeze::Clips;
            c.clips = vec![clip];
        });
        assert!(!is(&show));
        control(&mut show, pause, |c| {
            c.action = ControlAction::TimelinePause;
            c.freeze = PauseFreeze::Clips;
            c.clips = vec![CueId(999)];
        });
        assert!(is(&show), "it is not one of the clips held");
        // A pause of another timeline, or disarmed, doesn't count.
        control(&mut show, pause, |c| {
            c.action = ControlAction::TimelinePause;
            c.timeline = Some(TimelineKey::List(5));
        });
        assert!(!is(&show));
        control(&mut show, pause, |c| {
            c.action = ControlAction::TimelinePause
        });
        cue_mut(&mut show, pause).armed = false;
        assert!(!is(&show));
        cue_mut(&mut show, pause).armed = true;
        // A Pause of this clip counts; of another, or another action, doesn't.
        control(&mut show, pause, |c| {
            c.action = ControlAction::Pause;
            c.target = Some(clip);
        });
        assert!(is(&show));
        control(&mut show, pause, |c| {
            c.action = ControlAction::Pause;
            c.target = Some(CueId(999));
        });
        assert!(!is(&show));
        control(&mut show, pause, |c| {
            c.action = ControlAction::TimelineStop;
        });
        assert!(!is(&show));
        // A cue that isn't a clip is never clocked.
        control(&mut show, pause, |c| {
            c.action = ControlAction::TimelinePause
        });
        let wait = add(&mut show, CueType::Wait, Some(group), 0, 3.0);
        assert!(!clocked(&show, key, show.cue(wait).unwrap(), Some(5.0)));
        // And `clips` says so.
        let cl = clips(&show, key, &mut |_| Some(Some(5.0)));
        assert!(cl.iter().find(|c| c.id == clip).unwrap().clocked);
    }

    #[test]
    fn control_cues_come_first_then_fades_and_stops_then_clips() {
        let show = Show::default();
        let order = |ty| due_order(&show.clone().make_cue(ty, None, 0));
        assert_eq!(order(CueType::Control), 0);
        assert_eq!(order(CueType::Fade), 1);
        assert_eq!(order(CueType::Stop), 1);
        for ty in [CueType::Video, CueType::Audio, CueType::Wait, CueType::Send] {
            assert_eq!(order(ty), 2);
        }
    }
}
