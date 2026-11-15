//! Changes to a timeline, on the show: moving, trimming and splitting clips,
//! adding cues, tracks and markers. Each is pure and says whether it changed
//! anything (or what it made).
//!
//! A clip's start is its pre-wait and its length is what `Dur` says (the
//! engine's `cue_duration`, a fake in tests), with its lead-in added as
//! `model::clips` does. Times that come in are checked: a time that isn't a
//! number changes nothing, and nothing starts before 0 or gets shorter than
//! `MIN_LEN`. (No `clamp` on bounds from the show: inverted or NaN ones panic.)

use super::model::{self, Dur, LeadIn};
use super::{Marker, TimelineKey, TimelineSettings, TrackId};
use crate::show::{
    Animated, ControlAction, ControlCue, Cue, CueId, CueKind, CueType, Keyframe, MAX_RATE, MIN_DB,
    MIN_RATE, MediaCue, Show, Slot, Transition,
};
use std::path::PathBuf;

/// The shortest a clip (its content, after any lead-in) can be made.
pub const MIN_LEN: f64 = 0.05;

/// Times closer than this are the same time.
const SAME_TIME: f64 = 1e-6;

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

// ----- Helpers -------------------------------------------------------------

/// Where a cue starts on its timeline (as `model` reads it).
fn start_of(cue: &Cue) -> f64 {
    if cue.pre_wait.is_finite() {
        cue.pre_wait.max(0.0)
    } else {
        0.0
    }
}

fn cue_mut(show: &mut Show, id: CueId) -> Option<&mut Cue> {
    show.cues.iter_mut().find(|c| c.id == id)
}

/// How long a clip's content waits for its fade-in, in seconds after its start
/// (what `model::clips` adds to the length).
fn lead_secs(show: &Show, key: TimelineKey, cue: &Cue) -> f64 {
    match model::lead_in(show, key, cue) {
        LeadIn::None => 0.0,
        LeadIn::At(v) => v.max(0.0),
        LeadIn::UntilFaded => model::fade_wake(show, key, cue).unwrap_or(0.0),
    }
}

/// The length of a clip's content (no lead-in): `Some(None)` open-ended, None not known.
fn content_len(cue: &Cue, dur: Dur) -> Option<Option<f64>> {
    match dur(cue)? {
        Some(d) if d.is_finite() => Some(Some(d.max(0.0))),
        Some(_) => None,
        None => Some(None),
    }
}

/// File seconds played from clip time 0 to `t` (the speed map's integral, or a
/// steady speed's product).
fn media_at(m: &MediaCue, t: f64) -> f64 {
    match m.speed_map() {
        Some(map) => map.media_at(t),
        None => t.max(0.0) * m.speed(),
    }
}

/// A video or sound file, whose in and out points are times in the file (an
/// image's `end` is how long it is held).
fn plays_file(kind: &CueKind) -> bool {
    matches!(kind, CueKind::Video(_) | CueKind::Audio(_))
}

/// Whether a kind can be trimmed at all (the model's rule, `Clip::trim_*`).
fn trims(kind: &CueKind) -> bool {
    let loops = kind.media().is_some_and(|m| m.loop_media);
    !loops
        && matches!(
            kind,
            CueKind::Video(_)
                | CueKind::Audio(_)
                | CueKind::Image(_)
                | CueKind::Text(_)
                | CueKind::Shape(_)
                | CueKind::Camera(_)
                | CueKind::LiveAudio(_)
                | CueKind::Fade(_)
                | CueKind::Wait(_)
        )
}

/// Keyframes moved `by` seconds earlier (later if `by` is negative). Those
/// that would come before 0 go, and a key at 0 with the value there takes
/// their place, so the curve from 0 on is what it was from `by` on.
pub(crate) fn shift_keys(a: &mut Animated, by: f64) {
    if !a.is_animated() || !by.is_finite() || by == 0.0 {
        return;
    }
    if by < 0.0 {
        for k in &mut a.keys {
            k.t -= by;
        }
        return;
    }
    let v0 = a.at(by);
    let dropped: Vec<Keyframe> = a.keys.iter().copied().filter(|k| k.t < by).collect();
    a.keys.retain(|k| k.t >= by);
    for k in &mut a.keys {
        k.t = (k.t - by).max(0.0);
    }
    if let Some(last) = dropped.last()
        && a.keys.first().is_none_or(|k| k.t > SAME_TIME)
    {
        a.keys.insert(
            0,
            Keyframe {
                t: 0.0,
                v: v0,
                curve: last.curve,
            },
        );
    }
}

/// Every keyframed number of a clip (speed, motion, effects) moved `by` earlier.
fn shift_all_keys(m: &mut MediaCue, by: f64) {
    shift_keys(&mut m.rate, by);
    for t in crate::show::MotionTrack::ALL {
        if let Some(a) = m.motion.track_mut(t) {
            shift_keys(a, by);
        }
    }
    for e in &mut m.effects {
        for p in &mut e.p {
            shift_keys(p, by);
        }
    }
}

/// Whether a cue may go on a track: pictures on picture tracks, sounds on
/// sound tracks, the rest on the cue track.
fn fits(cue_track: TrackId, to: TrackId) -> bool {
    matches!(
        (cue_track, to),
        (TrackId::Picture { .. }, TrackId::Picture { .. })
            | (TrackId::Sound { .. }, TrackId::Sound { .. })
            | (TrackId::Cues, TrackId::Cues)
    )
}

/// Put a cue on a track of its kind. Returns whether that changed anything.
fn set_track(cue: &mut Cue, to: TrackId) -> bool {
    if !fits(model::track_of(cue), to) || model::track_of(cue) == to {
        return false;
    }
    let Some(m) = cue.kind.media_mut() else {
        return false;
    };
    match to {
        TrackId::Picture { output, layer } => {
            // A clip moved to a display it was also on now lives there.
            if let Some(o) = output {
                m.also_on.retain(|a| *a != o);
            }
            m.output = output;
            m.layer = layer;
        }
        TrackId::Sound { output } => m.audio_output = output,
        TrackId::Cues => return false,
    }
    true
}

/// Where a new cue goes on a timeline: the end of the group, or of the list.
fn slot_for(show: &Show, key: TimelineKey) -> Option<Slot> {
    match key {
        TimelineKey::Group(g) => {
            let i = show.cue_index(g)?;
            Some(Slot {
                at: show.block_end(i),
                parent: Some(g),
                list: show.cues[i].list,
            })
        }
        TimelineKey::List(n) => Some(Slot {
            at: show.list_end(n),
            parent: None,
            list: n,
        }),
    }
}

/// Add a new cue at `at` on a timeline, sorted into place.
fn add_cue(show: &mut Show, key: TimelineKey, mut cue: Cue, at: f64) -> Option<CueId> {
    let slot = slot_for(show, key)?;
    cue.pre_wait = at.max(0.0);
    let id = cue.id;
    show.insert_cue(slot, cue);
    sort_by_start(show, key);
    Some(id)
}

// ----- Clips ---------------------------------------------------------------

/// Move clips by `dt` (clamped so none starts before 0); `to` moves the anchor
/// (`ids[0]`) to that track and the others by the same track step where the kind allows.
pub fn move_clips(
    show: &mut Show,
    key: TimelineKey,
    ids: &[CueId],
    dt: f64,
    to: Option<TrackId>,
) -> bool {
    if !model::is_timeline(show, key) || !dt.is_finite() {
        return false;
    }
    let moving: Vec<(CueId, f64, TrackId)> = model::members(show, key)
        .into_iter()
        .filter(|c| ids.contains(&c.id))
        .map(|c| (c.id, start_of(c), model::track_of(c)))
        .collect();
    if moving.is_empty() {
        return false;
    }
    // The whole selection stops where its earliest clip reaches 0.
    let earliest = moving.iter().map(|m| m.1).fold(f64::INFINITY, f64::min);
    let dt = dt.max(-earliest);
    // The track step: from the anchor's track to `to`, in the order drawn.
    let mut targets: Vec<(CueId, TrackId)> = Vec::new();
    if let Some(to) = to
        && let Some(&(_, _, anchor)) = moving.iter().find(|m| m.0 == ids[0])
        && fits(anchor, to)
    {
        let clips = model::clips(show, key, &mut |_| None);
        let mut order = model::tracks(show, key, &clips);
        if !order.contains(&to) {
            order.push(to);
        }
        let place = |t: TrackId| order.iter().position(|o| *o == t);
        if let (Some(a), Some(b)) = (place(anchor), place(to)) {
            let step = b as isize - a as isize;
            for &(id, _, own) in &moving {
                let target = if id == ids[0] {
                    Some(to)
                } else {
                    place(own)
                        .and_then(|p| p.checked_add_signed(step))
                        .and_then(|p| order.get(p).copied())
                        .filter(|t| fits(own, *t))
                };
                if let Some(t) = target {
                    targets.push((id, t));
                }
            }
        }
    }
    let mut changed = false;
    for &(id, start, _) in &moving {
        let Some(c) = cue_mut(show, id) else {
            continue;
        };
        if dt != 0.0 {
            c.pre_wait = (start + dt).max(0.0);
            changed = true;
        }
        if let Some(&(_, t)) = targets.iter().find(|t| t.0 == id) {
            changed |= set_track(c, t);
        }
    }
    if changed {
        sort_by_start(show, key);
    }
    changed
}

/// Drag an edge of a clip to `to`, a time on the timeline.
///
/// File clips move their in point (left) or out point (right), through a
/// steady speed or the speed map, less any lead-in; still and live kinds their
/// hold (`end`); fade and wait cues their duration. Not a looping clip, and no
/// left trim with a keyframed speed.
pub fn trim(show: &mut Show, id: CueId, edge: Edge, to: f64, dur: Dur) -> bool {
    if !to.is_finite() {
        return false;
    }
    let Some(key) = model::key_of(show, id) else {
        return false;
    };
    let Some(cue) = show.cue(id) else {
        return false;
    };
    if !trims(&cue.kind) {
        return false;
    }
    let file = plays_file(&cue.kind);
    if edge == Edge::Left && file && cue.kind.media().is_some_and(|m| m.rate.is_animated()) {
        return false;
    }
    let start = start_of(cue);
    let lead = lead_secs(show, key, cue);
    // The content's length: a file clip must have one; a still may be open-ended.
    let content = match content_len(cue, dur) {
        Some(c) => c,
        None if file => return false,
        None => None,
    };
    let Some(cue) = cue_mut(show, id) else {
        return false;
    };
    let changed = match edge {
        Edge::Right => trim_right(cue, lead, content, to - start),
        Edge::Left => trim_left(cue, start, lead, content, to - start),
    };
    if changed {
        sort_by_start(show, key);
    }
    changed
}

/// The clip ends `t` seconds after its start (clip time, lead-in included).
fn trim_right(cue: &mut Cue, lead: f64, content: Option<f64>, t: f64) -> bool {
    let t = t.max(lead + MIN_LEN);
    match &mut cue.kind {
        CueKind::Video(m) | CueKind::Audio(m) => {
            let Some(len) = content else {
                return false;
            };
            // Played to the file's end: it can be made shorter, not longer.
            let t = if m.end.is_none() {
                t.min(lead + len)
            } else {
                t
            };
            let end = m.start + (media_at(m, t) - media_at(m, lead)).max(0.0);
            if m.end == Some(end) || (m.end.is_none() && (t - (lead + len)).abs() < SAME_TIME) {
                return false;
            }
            m.end = Some(end);
            true
        }
        CueKind::Fade(f) => set_if(&mut f.duration, t),
        CueKind::Wait(w) => set_if(&mut w.duration, t),
        kind => {
            let Some(m) = kind.media_mut() else {
                return false;
            };
            let hold = Some(t - lead);
            if m.end == hold {
                return false;
            }
            m.end = hold;
            true
        }
    }
}

/// The clip starts `d` seconds later (earlier if negative), its end staying put.
fn trim_left(cue: &mut Cue, start: f64, lead: f64, content: Option<f64>, d: f64) -> bool {
    // Never before 0, never shorter than the least.
    let mut d = d.max(-start);
    if let Some(len) = content {
        d = d.min(lead + len - MIN_LEN);
    }
    match &cue.kind {
        CueKind::Fade(f) => d = d.min(f.duration - MIN_LEN),
        CueKind::Wait(w) => d = d.min(w.duration - MIN_LEN),
        // Without a lead-in to grow, the in point can't go before the file's start.
        CueKind::Video(m) | CueKind::Audio(m) if lead <= 0.0 => d = d.max(-m.start / m.speed()),
        _ => {}
    }
    // Content cut from (or put back at) the front: what the lead-in doesn't absorb.
    let cut = if d >= 0.0 {
        (d - lead).max(0.0)
    } else if lead > 0.0 {
        0.0
    } else {
        d
    };
    if d.abs() < SAME_TIME {
        return false;
    }
    cue.pre_wait = (start + d).max(0.0);
    match &mut cue.kind {
        CueKind::Fade(f) => f.duration -= d,
        CueKind::Wait(w) => w.duration -= d,
        CueKind::Video(m) | CueKind::Audio(m) => {
            m.start = (m.start + cut * m.speed()).max(0.0);
            shift_all_keys(m, d);
        }
        kind => {
            if let Some(m) = kind.media_mut() {
                if let Some(e) = &mut m.end {
                    *e = (*e - cut).max(MIN_LEN);
                }
                shift_all_keys(m, d);
            }
        }
    }
    true
}

fn set_if(v: &mut f64, to: f64) -> bool {
    if (*v - to).abs() < SAME_TIME {
        return false;
    }
    *v = to;
    true
}

/// Split clips at `at`. Returns the new halves.
///
/// The first half keeps its fade-in and loses its fade-out; the second starts
/// at `at` with a cut and no fade-in, its content and keyframes taken up where
/// the first's stop. A looping clip isn't split, nor one `at` isn't inside.
/// Fade cues stay aimed at the first half, and a second half that would be
/// unseen without them starts visible (`starts_visible`).
pub fn split(show: &mut Show, key: TimelineKey, ids: &[CueId], at: f64, dur: Dur) -> Vec<CueId> {
    let mut made = Vec::new();
    if !model::is_timeline(show, key) || !at.is_finite() {
        return made;
    }
    let picked: Vec<CueId> = model::members(show, key)
        .into_iter()
        .filter(|c| ids.contains(&c.id))
        .map(|c| c.id)
        .collect();
    for id in picked {
        let Some(cue) = show.cue(id) else {
            continue;
        };
        let Some(m) = cue.kind.media() else {
            continue;
        };
        if m.loop_media {
            continue;
        }
        let file = plays_file(&cue.kind);
        let start = start_of(cue);
        let lead = lead_secs(show, key, cue);
        let content = match content_len(cue, dur) {
            Some(c) => c,
            None => continue,
        };
        if file && content.is_none() {
            continue;
        }
        let off = at - start;
        // Inside the content, leaving each half its least length.
        if off < lead + MIN_LEN || content.is_some_and(|l| off > lead + l - MIN_LEN) {
            continue;
        }
        let mut second = cue.clone();
        let Some(first) = cue_mut(show, id) else {
            continue;
        };
        let Some(fm) = first.kind.media_mut() else {
            continue;
        };
        let played = off - lead;
        let file_at = if file {
            fm.start + (media_at(fm, off) - media_at(fm, lead)).max(0.0)
        } else {
            0.0
        };
        // The first half ends at the split.
        fm.fade_out = 0.0;
        fm.hold_last_frame = false;
        fm.end = Some(if file { file_at } else { played });
        // The second takes up from there.
        second.id = CueId(show.alloc_id());
        second.number = show.next_cue_number();
        second.hotkey = None;
        second.midi_note = None;
        second.pre_wait = at;
        if let Some(sm) = second.kind.media_mut() {
            sm.transition = Transition::Cut;
            sm.fade_in = 0.0;
            if file {
                sm.start = file_at;
            } else if let Some(e) = &mut sm.end {
                *e = (*e - played).max(MIN_LEN);
            }
            shift_all_keys(sm, off);
        }
        // Fade cues stay aimed at the first half; the second starts visible.
        starts_visible(show, key, id, at, &mut second);
        // Its content goes straight on: it doesn't wait for a fade-in of its own.
        if model::lead_in(show, key, &second) != LeadIn::None
            && let Some(sm) = second.kind.media_mut()
        {
            sm.runs_unseen = true;
        }
        let new = second.id;
        let Some(i) = show.cue_index(id) else {
            continue;
        };
        show.cues.insert(i + 1, second);
        made.push(new);
    }
    if !made.is_empty() {
        sort_by_start(show, key);
    }
    made
}

/// The second half of a split clip that is unseen by itself (a picture at
/// opacity 0, or keys that stay there; a sound at silence) and was brought up
/// by fade cues: those stay aimed at the first half (`first`), so the second
/// is made visible at the level the last of them before the split (`at`)
/// takes it to, else the first of them, else full.
fn starts_visible(show: &Show, key: TimelineKey, first: CueId, at: f64, second: &mut Cue) {
    let picture = model::is_picture(&second.kind);
    let sound = model::is_sound(&second.kind);
    let Some(m) = second.kind.media_mut() else {
        return;
    };
    let unseen = if picture {
        match &m.motion.opacity {
            Some(a) if a.is_animated() => a.keys.iter().all(|k| k.v <= 0.0),
            _ => m.opacity <= 0.0,
        }
    } else {
        sound && m.volume_db <= MIN_DB
    };
    if !unseen {
        return;
    }
    let fades: Vec<(f64, f32)> = model::members(show, key)
        .into_iter()
        .filter(|f| f.armed)
        .filter_map(|f| match &f.kind {
            CueKind::Fade(fade) if fade.target == Some(first) => {
                let level = if picture {
                    fade.opacity.filter(|o| *o > 0.0)
                } else {
                    fade.volume_db.filter(|v| *v > MIN_DB)
                };
                level.map(|l| (start_of(f), l))
            }
            _ => None,
        })
        .collect();
    let before = fades
        .iter()
        .filter(|(t, _)| *t <= at + SAME_TIME)
        .max_by(|a, b| a.0.total_cmp(&b.0));
    let first_one = fades.iter().min_by(|a, b| a.0.total_cmp(&b.0));
    let level = before.or(first_one).map(|f| f.1);
    if picture {
        m.motion.opacity = None;
        m.opacity = level.unwrap_or(1.0);
    } else {
        m.volume_db = level.unwrap_or(0.0);
    }
}

/// Remove clips, closing the gap with `ripple`. Returns the cues removed.
///
/// A ripple moves up the later clips on the tracks the removed clips were on,
/// by the time removed before each; markers move only when every track with
/// clips on it rippled, by the least any of them moved there.
pub fn delete(
    show: &mut Show,
    key: TimelineKey,
    ids: &[CueId],
    ripple: bool,
    dur: Dur,
) -> Vec<CueId> {
    if !model::is_timeline(show, key) {
        return Vec::new();
    }
    let clips = model::clips(show, key, dur);
    let gone: Vec<CueId> = clips
        .iter()
        .filter(|c| ids.contains(&c.id))
        .map(|c| c.id)
        .collect();
    if gone.is_empty() {
        return Vec::new();
    }
    let removed = show.delete_many(&gone).map(|(_, r)| r).unwrap_or_default();
    if ripple {
        // The gaps of each track, joined where they overlap.
        let mut gaps: Vec<(TrackId, Vec<(f64, f64)>)> = Vec::new();
        for c in clips.iter().filter(|c| gone.contains(&c.id)) {
            let Some(len) = c.len else {
                continue;
            };
            let span = (c.start, c.start + len);
            match gaps.iter_mut().find(|g| g.0 == c.track) {
                Some(g) => g.1.push(span),
                None => gaps.push((c.track, vec![span])),
            }
        }
        for g in &mut gaps {
            g.1 = union(std::mem::take(&mut g.1));
        }
        let before = |spans: &[(f64, f64)], t: f64| -> f64 {
            spans
                .iter()
                .filter(|s| s.1 <= t + SAME_TIME)
                .map(|s| s.1 - s.0)
                .sum()
        };
        let kept: Vec<(CueId, f64, TrackId)> = clips
            .iter()
            .filter(|c| !gone.contains(&c.id))
            .map(|c| (c.id, c.start, c.track))
            .collect();
        for (id, start, track) in &kept {
            let Some(g) = gaps.iter().find(|g| g.0 == *track) else {
                continue;
            };
            let by = before(&g.1, *start);
            if by > 0.0
                && let Some(c) = cue_mut(show, *id)
            {
                c.pre_wait = (start - by).max(0.0);
            }
        }
        let used: Vec<TrackId> = clips.iter().map(|c| c.track).fold(Vec::new(), |mut v, t| {
            if !v.contains(&t) {
                v.push(t);
            }
            v
        });
        let all = !gaps.is_empty() && used.iter().all(|t| gaps.iter().any(|g| g.0 == *t));
        if all && let Some(s) = model::settings_mut(show, key) {
            for mk in &mut s.markers {
                let by = gaps
                    .iter()
                    .map(|g| before(&g.1, mk.time))
                    .fold(f64::INFINITY, f64::min);
                if by.is_finite() && by > 0.0 {
                    mk.time = (mk.time - by).max(0.0);
                }
            }
            sort_markers(&mut s.markers);
        }
    }
    sort_by_start(show, key);
    model::settle(show, key);
    removed
}

/// Spans joined where they overlap or touch, in order.
fn union(mut spans: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(f64, f64)> = Vec::new();
    for s in spans {
        match out.last_mut() {
            Some(l) if s.0 <= l.1 + SAME_TIME => l.1 = l.1.max(s.1),
            _ => out.push(s),
        }
    }
    out
}

/// A new clip of type `ty` on `track` at `at` (a file for one that plays one).
/// None if the track isn't for that kind of cue, or for a group.
pub fn add_clip(
    show: &mut Show,
    key: TimelineKey,
    ty: CueType,
    file: Option<PathBuf>,
    track: TrackId,
    at: f64,
) -> Option<CueId> {
    if !model::is_timeline(show, key) || !at.is_finite() || ty == CueType::Group {
        return None;
    }
    let slot = slot_for(show, key)?;
    let mut cue = show.make_cue(ty, file, slot.at);
    if !fits(model::track_of(&cue), track) {
        return None;
    }
    set_track(&mut cue, track);
    add_cue(show, key, cue, at)
}

/// Move the clips `ids` of a timeline in time so the earliest starts at `at`,
/// keeping their spacing and tracks (cues pasted or moved in).
pub fn place_at(show: &mut Show, key: TimelineKey, ids: &[CueId], at: f64) -> bool {
    if !at.is_finite() {
        return false;
    }
    let earliest = model::members(show, key)
        .into_iter()
        .filter(|c| ids.contains(&c.id))
        .map(start_of)
        .fold(f64::INFINITY, f64::min);
    if !earliest.is_finite() {
        return false;
    }
    move_clips(show, key, ids, at.max(0.0) - earliest, None)
}

/// Paste copies of cues (`Show::copy_cues`) into a timeline as clips, the
/// earliest at `at` and the others as far after it as they were, each on the
/// track it had (a display or output the show lacks mapped as `paste_cues`
/// does, which also aims a fade, stop or control cue at the copy of what it
/// aimed at). A group isn't pasted into a timeline: nothing is, then. Returns
/// the new clips.
pub fn paste_clips(
    show: &mut Show,
    key: TimelineKey,
    cues: &[Cue],
    at: f64,
    same_show: bool,
    keep_ids: bool,
) -> Vec<CueId> {
    if !model::is_timeline(show, key)
        || !at.is_finite()
        || cues.is_empty()
        || cues.iter().any(|c| matches!(c.kind, CueKind::Group(_)))
    {
        return Vec::new();
    }
    let (selected, list) = match key {
        TimelineKey::Group(g) => match show.cue(g) {
            Some(c) => (Some(g), c.list),
            None => return Vec::new(),
        },
        TimelineKey::List(n) => (None, n),
    };
    let heads = show.paste_cues(cues, selected, list, same_show, keep_ids);
    if !place_at(show, key, &heads, at) {
        sort_by_start(show, key);
    }
    heads
}

/// Copies of the clips `ids` of a timeline, just after them: the copies start
/// where the last of them ends (an open-ended one counts from its start), so
/// they don't overlap what they copy. Returns the copies.
pub fn duplicate_clips(show: &mut Show, key: TimelineKey, ids: &[CueId], dur: Dur) -> Vec<CueId> {
    let clips = model::clips(show, key, dur);
    let picked: Vec<&model::Clip> = clips.iter().filter(|c| ids.contains(&c.id)).collect();
    if picked.is_empty() {
        return Vec::new();
    }
    let end = picked
        .iter()
        .map(|c| c.start + c.len.unwrap_or(0.0))
        .fold(0.0, f64::max);
    let own: Vec<CueId> = picked.iter().map(|c| c.id).collect();
    let copies = show.copy_cues(&own);
    paste_clips(show, key, &copies, end, true, false)
}

/// A control cue doing `action` at `at` on the timeline's cue track.
pub fn add_control(
    show: &mut Show,
    key: TimelineKey,
    action: ControlAction,
    at: f64,
) -> Option<CueId> {
    if !model::is_timeline(show, key) || !at.is_finite() {
        return None;
    }
    let slot = slot_for(show, key)?;
    let mut cue = show.make_cue(CueType::Control, None, slot.at);
    cue.name = action.label().to_string();
    cue.kind = CueKind::Control(ControlCue {
        action,
        ..ControlCue::default()
    });
    add_cue(show, key, cue, at)
}

/// A free-run `TimelinePause` at exactly the time `what` names for `clip`.
pub fn add_pause_before(
    show: &mut Show,
    key: TimelineKey,
    clip: CueId,
    what: PauseBefore,
    dur: Dur,
) -> Option<CueId> {
    if !model::is_timeline(show, key) {
        return None;
    }
    let c = model::clips(show, key, dur)
        .into_iter()
        .find(|c| c.id == clip)?;
    let (at, name) = match what {
        PauseBefore::ClipIn => (c.start, format!("Pause before {}", c.name)),
        PauseBefore::ClipFadeOut => {
            let end = c.start + c.len?;
            (
                (end - c.fade_out).max(c.start),
                format!("Pause before {} fades out", c.name),
            )
        }
    };
    let id = add_control(show, key, ControlAction::TimelinePause, at)?;
    if let Some(p) = cue_mut(show, id) {
        p.name = name;
    }
    Some(id)
}

// ----- Tracks --------------------------------------------------------------

/// An empty track kept on the timeline: a picture track a layer above the
/// highest on the first picture track's display, or a sound track on the
/// first audio output without one. Returns it (None if there's no output left).
pub fn add_track(show: &mut Show, key: TimelineKey, picture: bool) -> Option<TrackId> {
    if !model::is_timeline(show, key) {
        return None;
    }
    let clips = model::clips(show, key, &mut |_| None);
    let have = model::tracks(show, key, &clips);
    let t = if picture {
        let output = have
            .iter()
            .find_map(|t| match t {
                TrackId::Picture { output, .. } => Some(*output),
                _ => None,
            })
            .unwrap_or_else(|| show.outputs.first().map(|o| o.id));
        let top = have
            .iter()
            .filter_map(|t| match t {
                TrackId::Picture { output: o, layer } if *o == output => Some(*layer),
                _ => None,
            })
            .max();
        TrackId::Picture {
            output,
            layer: top.map_or(0, |l| l.saturating_add(1)),
        }
    } else {
        std::iter::once(None)
            .chain(show.audio_outputs.iter().map(|o| Some(o.id)))
            .map(|output| TrackId::Sound { output })
            .find(|t| !have.contains(t))?
    };
    model::settings_mut(show, key)?.tracks.push(t);
    Some(t)
}

/// Only if it is empty (and not the cue track, which is always there).
pub fn remove_track(show: &mut Show, key: TimelineKey, t: TrackId) -> bool {
    if !model::is_timeline(show, key) || t == TrackId::Cues {
        return false;
    }
    if model::members(show, key)
        .iter()
        .any(|c| model::track_of(c) == t)
    {
        return false;
    }
    let Some(s) = model::settings_mut(show, key) else {
        return false;
    };
    let before = s.tracks.len();
    s.tracks.retain(|k| *k != t);
    let changed = s.tracks.len() != before;
    model::settle(show, key);
    changed
}

// ----- Markers -------------------------------------------------------------

fn sort_markers(markers: &mut [Marker]) {
    markers.sort_by(|a, b| a.time.total_cmp(&b.time));
}

/// A marker at `at`, named `name`. Its id is new to the show, so a cue that
/// pointed at a removed marker never finds another in its place.
pub fn add_marker(show: &mut Show, key: TimelineKey, at: f64, name: &str) -> Option<u64> {
    if !model::is_timeline(show, key) || !at.is_finite() {
        return None;
    }
    let id = show.alloc_id();
    let s = model::settings_mut(show, key)?;
    s.markers.push(Marker {
        id,
        time: at.max(0.0),
        name: name.to_string(),
        color: None,
    });
    sort_markers(&mut s.markers);
    Some(id)
}

pub fn move_marker(show: &mut Show, key: TimelineKey, id: u64, to: f64) -> bool {
    if !to.is_finite() {
        return false;
    }
    let Some(s) = model::settings_mut(show, key) else {
        return false;
    };
    let Some(m) = s.markers.iter_mut().find(|m| m.id == id) else {
        model::settle(show, key);
        return false;
    };
    let to = to.max(0.0);
    let changed = m.time != to;
    m.time = to;
    sort_markers(&mut s.markers);
    changed
}

pub fn rename_marker(show: &mut Show, key: TimelineKey, id: u64, name: &str) -> bool {
    let Some(s) = model::settings_mut(show, key) else {
        return false;
    };
    let Some(m) = s.markers.iter_mut().find(|m| m.id == id) else {
        model::settle(show, key);
        return false;
    };
    let changed = m.name != name;
    m.name = name.to_string();
    changed
}

pub fn remove_marker(show: &mut Show, key: TimelineKey, id: u64) -> bool {
    let Some(s) = model::settings_mut(show, key) else {
        return false;
    };
    let before = s.markers.len();
    s.markers.retain(|m| m.id != id);
    let changed = s.markers.len() != before;
    model::settle(show, key);
    changed
}

// ----- Speed ---------------------------------------------------------------

/// Set a clip's keyframed speed (sanitised: numbers that are numbers, keys in
/// order from 0, every value within `MIN_RATE`..`MAX_RATE`). Video and sound
/// files only.
pub fn set_rate(show: &mut Show, id: CueId, mut rate: Animated) -> bool {
    rate.sanitize(1.0);
    let keep = |v: f32| v.max(MIN_RATE as f32).min(MAX_RATE as f32);
    rate.value = keep(rate.value);
    for k in &mut rate.keys {
        k.v = keep(k.v);
        k.t = k.t.max(0.0);
    }
    let Some(c) = cue_mut(show, id) else {
        return false;
    };
    let (CueKind::Video(m) | CueKind::Audio(m)) = &mut c.kind else {
        return false;
    };
    if m.rate == rate {
        return false;
    }
    m.rate = rate;
    true
}

// ----- Opacity and fades ---------------------------------------------------

/// Set a picture clip's opacity keyframes (`Motion`'s opacity track): with
/// keys, the track (values kept to 0..1, keys from 0, in order); with none,
/// the steady opacity `value` and no track. Pictures only.
pub fn set_opacity(show: &mut Show, id: CueId, mut opacity: Animated) -> bool {
    opacity.sanitize(1.0);
    let keep = |v: f32| v.clamp(0.0, 1.0);
    opacity.value = keep(opacity.value);
    for k in &mut opacity.keys {
        k.v = keep(k.v);
        k.t = k.t.max(0.0);
    }
    let Some(c) = cue_mut(show, id) else {
        return false;
    };
    if !model::is_picture(&c.kind) {
        return false;
    }
    let Some(m) = c.kind.media_mut() else {
        return false;
    };
    let (track, steady) = if opacity.is_animated() {
        // The steady value under keys is never read; keep the first key's.
        opacity.value = opacity.keys[0].v;
        (Some(opacity), m.opacity)
    } else {
        (None, opacity.value)
    };
    if m.motion.opacity == track && m.opacity == steady {
        return false;
    }
    m.motion.opacity = track;
    m.opacity = steady;
    true
}

/// Set how long a clip fades in (`Edge::Left`) or out (`Edge::Right`), in
/// seconds: never below 0, nor so long that the two fades overlap within the
/// clip's content (`dur`). A picture that cut in and is given a fade-in now
/// fades in over what is under it.
pub fn set_fade(show: &mut Show, id: CueId, edge: Edge, secs: f64, dur: Dur) -> bool {
    if !secs.is_finite() {
        return false;
    }
    let Some(cue) = show.cue(id) else {
        return false;
    };
    let picture = model::is_picture(&cue.kind);
    let content = content_len(cue, dur).flatten();
    let Some(m) = cue_mut(show, id).and_then(|c| c.kind.media_mut()) else {
        return false;
    };
    let other = match edge {
        Edge::Left => m.fade_out,
        Edge::Right => m.fade_in,
    };
    let mut secs = secs.max(0.0);
    if let Some(len) = content {
        secs = secs.min((len - other.max(0.0)).max(0.0));
    }
    let secs = (secs * 1000.0).round() / 1000.0;
    let field = match edge {
        Edge::Left => &mut m.fade_in,
        Edge::Right => &mut m.fade_out,
    };
    if (*field - secs).abs() < SAME_TIME {
        return false;
    }
    *field = secs;
    if edge == Edge::Left && picture && secs > 0.0 && m.transition == Transition::Cut {
        m.transition = Transition::FadeIn;
    }
    true
}

/// Move a picture track above (`up`) or below the next picture track of its
/// display: the clips on the two swap layers, as do the empty tracks the
/// timeline keeps. Refused at the top or bottom of the display's tracks.
pub fn move_track(show: &mut Show, key: TimelineKey, t: TrackId, up: bool) -> bool {
    let TrackId::Picture { output, layer } = t else {
        return false;
    };
    if !model::is_timeline(show, key) {
        return false;
    }
    let clips = model::clips(show, key, &mut |_| None);
    let order = model::tracks(show, key, &clips);
    // Layers of this display, top to bottom (as drawn).
    let layers: Vec<i32> = order
        .iter()
        .filter_map(|o| match *o {
            TrackId::Picture { output: d, layer } if d == output => Some(layer),
            _ => None,
        })
        .collect();
    let Some(i) = layers.iter().position(|l| *l == layer) else {
        return false;
    };
    let j = if up { i.checked_sub(1) } else { Some(i + 1) };
    let Some(&other) = j.and_then(|j| layers.get(j)) else {
        return false;
    };
    let ids: Vec<(CueId, i32)> = model::members(show, key)
        .into_iter()
        .filter_map(|c| match model::track_of(c) {
            TrackId::Picture {
                output: d,
                layer: l,
            } if d == output => Some((c.id, l)),
            _ => None,
        })
        .collect();
    for (id, l) in ids {
        let to = if l == layer {
            other
        } else if l == other {
            layer
        } else {
            continue;
        };
        if let Some(m) = cue_mut(show, id).and_then(|c| c.kind.media_mut()) {
            m.layer = to;
        }
    }
    if let Some(s) = model::settings_mut(show, key) {
        for k in &mut s.tracks {
            if let TrackId::Picture {
                output: d,
                layer: l,
            } = k
                && *d == output
            {
                if *l == layer {
                    *l = other;
                } else if *l == other {
                    *l = layer;
                }
            }
        }
    }
    model::settle(show, key);
    true
}

// ----- Order and new timelines --------------------------------------------

/// Put a timeline's clips in order of their starts (stable); the ops above call it.
/// A group's children stay right after the group, a list's cues inside the
/// list (and a group in a list with its children).
pub fn sort_by_start(show: &mut Show, key: TimelineKey) {
    let range = match key {
        TimelineKey::Group(g) => match show.cue_index(g) {
            Some(i) if show.is_group(i) => i + 1..show.block_end(i),
            _ => return,
        },
        TimelineKey::List(n) => {
            if n == 0 || !show.has_list(n) {
                return;
            }
            show.list_range(n)
        }
    };
    // Blocks: a cue, or a group with its children (only in a list).
    let mut blocks: Vec<Vec<Cue>> = Vec::new();
    for c in show.cues.drain(range.clone()) {
        match (c.parent, blocks.last_mut()) {
            (Some(p), Some(b)) if matches!(key, TimelineKey::List(_)) && b[0].id == p => b.push(c),
            _ => blocks.push(vec![c]),
        }
    }
    // At the same time, in the order they are due (control, fade and stop, clips).
    blocks.sort_by(|a, b| {
        start_of(&a[0])
            .total_cmp(&start_of(&b[0]))
            .then(model::due_order(&a[0]).cmp(&model::due_order(&b[0])))
    });
    let at = range.start;
    show.cues
        .splice(at..at, blocks.into_iter().flatten())
        .for_each(drop);
}

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
    if list == 0 {
        return Err("The main list can't be a timeline".into());
    }
    let has_groups = show
        .cues
        .iter()
        .any(|c| c.list == list && matches!(c.kind, CueKind::Group(_)));
    let Some(l) = show.lists.iter_mut().find(|l| l.id == list) else {
        return Err("There is no such list".into());
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::{AudioOutputId, Curve, Effect, EffectKind, OutputId};
    use crate::speed::SpeedMap;

    /// Every file in these tests is this long.
    const FILE: f64 = 20.0;

    /// The engine's sum (`Engine::cue_duration`) with every file `FILE` long.
    fn fake_dur(c: &Cue) -> Option<Option<f64>> {
        Some(match &c.kind {
            CueKind::Video(m) | CueKind::Audio(m) => {
                if m.loop_media {
                    return Some(None);
                }
                let file = (m.end.unwrap_or(FILE).min(FILE) - m.start).max(0.0);
                Some(match m.speed_map() {
                    Some(map) => map.time_for(file),
                    None => file / m.speed(),
                })
            }
            CueKind::Fade(f) => Some(f.duration),
            CueKind::Wait(w) => Some(w.duration),
            CueKind::Stop(_) | CueKind::Control(_) | CueKind::Send(_) => Some(0.0),
            kind => kind.media().and_then(|m| m.end.filter(|e| *e > 0.0)),
        })
    }

    /// A show with cues before and after a timeline group (made in the editor,
    /// in the main list), a timeline list between two plain lists, a second
    /// display and an audio output.
    struct Fixture {
        show: Show,
        group: TimelineKey,
        list: TimelineKey,
        audio: AudioOutputId,
    }

    fn push_to(show: &mut Show, list: u32) {
        let c = show.make_cue(CueType::Wait, None, show.cues.len());
        let at = show.list_end(list);
        show.insert_cue(
            Slot {
                at,
                parent: None,
                list,
            },
            c,
        );
    }

    fn fixture() -> Fixture {
        let mut show = Show::default();
        show.add_display();
        let audio = show.add_audio_output();
        push_to(&mut show, 0);
        let slot = show.insert_slot(None, true, 0);
        let g = new_timeline_group(&mut show, slot);
        push_to(&mut show, 0);
        let before = show.add_list("Before");
        push_to(&mut show, before);
        let list = new_timeline_list(&mut show, "Timeline");
        let after = show.add_list("After");
        push_to(&mut show, after);
        Fixture {
            show,
            group: TimelineKey::Group(g),
            list: TimelineKey::List(list),
            audio,
        }
    }

    fn display(show: &Show, n: usize) -> Option<OutputId> {
        show.outputs.get(n).map(|o| o.id)
    }

    fn picture(show: &Show, n: usize, layer: i32) -> TrackId {
        TrackId::Picture {
            output: display(show, n),
            layer,
        }
    }

    fn clip(show: &mut Show, key: TimelineKey, ty: CueType, track: TrackId, at: f64) -> CueId {
        add_clip(show, key, ty, Some(PathBuf::from("a.mp4")), track, at).unwrap()
    }

    fn media(show: &mut Show, id: CueId) -> &mut MediaCue {
        cue_mut(show, id).unwrap().kind.media_mut().unwrap()
    }

    fn start(show: &Show, id: CueId) -> f64 {
        show.cue(id).unwrap().pre_wait
    }

    fn ids(show: &Show, key: TimelineKey) -> Vec<CueId> {
        model::members(show, key).iter().map(|c| c.id).collect()
    }

    fn len(show: &Show, key: TimelineKey, id: CueId) -> Option<f64> {
        model::clips(show, key, &mut fake_dur)
            .into_iter()
            .find(|c| c.id == id)
            .and_then(|c| c.len)
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    /// The show as a file would hold it.
    fn json(show: &Show) -> String {
        serde_json::to_string(show).unwrap()
    }

    /// What the loader repairs finds nothing to repair.
    fn assert_sound(show: &Show) {
        let mut copy = show.clone();
        copy.normalize_groups();
        copy.normalize_lists();
        assert_eq!(json(&copy), json(show), "the structure needed repair");
    }

    fn keys(f: &Fixture) -> [TimelineKey; 2] {
        [f.group, f.list]
    }

    #[test]
    fn clips_are_added_on_their_tracks_in_order_of_their_starts() {
        for n in 0..2 {
            let mut f = fixture();
            let key = keys(&f)[n];
            let sound = TrackId::Sound {
                output: Some(f.audio),
            };
            let show = &mut f.show;
            let v1 = picture(show, 0, 0);
            let a = clip(show, key, CueType::Video, v1, 4.0);
            let b = clip(show, key, CueType::Image, picture(show, 1, 2), 1.0);
            let s = clip(show, key, CueType::Audio, sound, 2.0);
            let p = add_control(show, key, ControlAction::TimelinePause, 3.0).unwrap();
            // A cue is refused a track of another kind, and a group anywhere.
            assert!(add_clip(show, key, CueType::Audio, None, v1, 0.0).is_none());
            assert!(add_clip(show, key, CueType::Fade, None, v1, 0.0).is_none());
            assert!(add_clip(show, key, CueType::Group, None, TrackId::Cues, 0.0).is_none());
            assert!(add_clip(show, key, CueType::Video, None, v1, f64::NAN).is_none());
            assert_eq!(ids(show, key), vec![b, s, p, a]);
            assert_eq!(model::track_of(show.cue(b).unwrap()), picture(show, 1, 2));
            assert_eq!(model::track_of(show.cue(s).unwrap()), sound);
            assert_eq!(model::key_of(show, p), Some(key));
            let CueKind::Control(c) = &show.cue(p).unwrap().kind else {
                panic!("not a control cue");
            };
            assert_eq!(c.action, ControlAction::TimelinePause);
            assert_eq!(show.cue(p).unwrap().name, "Pause timeline");
            assert_sound(show);
            // Not on something that isn't a timeline.
            let plain = TimelineKey::List(show.lists[0].id);
            assert!(add_control(show, plain, ControlAction::TimelineRun, 0.0).is_none());
        }
    }

    #[test]
    fn moving_clamps_the_whole_selection_at_zero_and_changes_tracks_by_kind() {
        for n in 0..2 {
            let mut f = fixture();
            let key = keys(&f)[n];
            let show = &mut f.show;
            let (v0, v1) = (picture(show, 0, 0), picture(show, 0, 1));
            let a = clip(show, key, CueType::Video, v0, 1.0);
            let b = clip(show, key, CueType::Image, v1, 3.0);
            let s = clip(
                show,
                key,
                CueType::Audio,
                TrackId::Sound { output: None },
                2.0,
            );
            let other = clip(show, key, CueType::Video, v0, 6.0);
            let top = clip(show, key, CueType::Image, picture(show, 0, 2), 9.0);
            assert!(move_clips(show, key, &[a, b, s], -2.0, None));
            assert_eq!(
                (start(show, a), start(show, s), start(show, b)),
                (0.0, 1.0, 2.0)
            );
            assert_eq!(start(show, other), 6.0, "not picked");
            assert!(!move_clips(show, key, &[a, b], -1.0, None), "already at 0");
            assert!(!move_clips(show, key, &[a], f64::NAN, None));
            // The anchor (a, on layer 0) goes up a row to layer 1; b goes up a
            // row too, to layer 2; the sound can't go to a picture track.
            assert!(move_clips(show, key, &[a, b, s], 0.5, Some(v1)));
            assert_eq!(model::track_of(show.cue(a).unwrap()), v1);
            assert_eq!(model::track_of(show.cue(b).unwrap()), picture(show, 0, 2));
            assert_eq!(
                model::track_of(show.cue(s).unwrap()),
                TrackId::Sound { output: None }
            );
            assert_eq!(start(show, a), 0.5);
            // A track of another kind is not the anchor's to go to.
            assert!(!move_clips(show, key, &[a], 0.0, Some(TrackId::Cues)));
            // To the other display.
            assert!(move_clips(show, key, &[a], 0.0, Some(picture(show, 1, 0))));
            let second = display(show, 1);
            assert_eq!(media(show, a).output, second);
            let _ = top;
            assert_sound(show);
        }
    }

    #[test]
    fn trimming_a_file_follows_a_steady_speed_and_the_file_s_end() {
        let mut f = fixture();
        let key = f.group;
        let show = &mut f.show;
        let v = clip(show, key, CueType::Video, picture(show, 0, 0), 2.0);
        media(show, v).start = 1.0;
        media(show, v).rate = Animated::new(2.0);
        // 19 s of file at double speed.
        assert!(near(len(show, key, v).unwrap(), 9.5));
        assert!(trim(show, v, Edge::Right, 6.0, &mut fake_dur));
        assert_eq!(media(show, v).end, Some(1.0 + 4.0 * 2.0));
        assert!(near(len(show, key, v).unwrap(), 4.0));
        // Left: 1 s later on the timeline is 2 s of file.
        assert!(trim(show, v, Edge::Left, 3.0, &mut fake_dur));
        assert_eq!((start(show, v), media(show, v).start), (3.0, 3.0));
        assert!(near(len(show, key, v).unwrap(), 3.0), "the end stays");
        // Back further than the file goes: only to its start (1.5 s at double speed).
        assert!(trim(show, v, Edge::Left, 0.0, &mut fake_dur));
        assert_eq!((start(show, v), media(show, v).start), (1.5, 0.0));
        // Never shorter than the least, never inside out.
        assert!(trim(show, v, Edge::Right, 0.0, &mut fake_dur));
        assert!(near(len(show, key, v).unwrap(), MIN_LEN));
        assert!(media(show, v).end.unwrap() > media(show, v).start);
        assert!(!trim(show, v, Edge::Left, 100.0, &mut fake_dur), "no room");
        assert!(near(len(show, key, v).unwrap(), MIN_LEN));
        assert!(!trim(show, v, Edge::Right, f64::NAN, &mut fake_dur));
        // A file played to its end can't be made longer than it is.
        let w = clip(
            show,
            key,
            CueType::Audio,
            TrackId::Sound { output: None },
            0.0,
        );
        assert!(!trim(show, w, Edge::Right, 30.0, &mut fake_dur));
        assert_eq!(media(show, w).end, None);
        // Not a loop; not without a length.
        media(show, w).loop_media = true;
        assert!(!trim(show, w, Edge::Right, 5.0, &mut fake_dur));
        media(show, w).loop_media = false;
        assert!(!trim(show, w, Edge::Right, 5.0, &mut |_| None));
        assert_sound(show);
    }

    #[test]
    fn trimming_with_a_speed_ramp_follows_the_speed_map() {
        let mut f = fixture();
        let key = f.list;
        let show = &mut f.show;
        let v = clip(show, key, CueType::Video, picture(show, 0, 0), 1.0);
        let mut ramp = Animated::new(1.0);
        ramp.set_key(0.0, 1.0);
        ramp.set_key(4.0, 3.0);
        assert!(set_rate(show, v, ramp.clone()));
        assert!(trim(show, v, Edge::Right, 6.0, &mut fake_dur));
        let map = SpeedMap::new(&ramp);
        assert!(near(media(show, v).end.unwrap(), map.media_at(5.0)));
        assert!((len(show, key, v).unwrap() - 5.0).abs() < 1e-3);
        // No left trim with keyframed speed.
        assert!(!trim(show, v, Edge::Left, 2.0, &mut fake_dur));
        assert_eq!(start(show, v), 1.0);
    }

    #[test]
    fn trimming_leaves_a_lead_in_out_of_the_content() {
        let mut f = fixture();
        let key = f.group;
        let show = &mut f.show;
        let v = clip(show, key, CueType::Video, picture(show, 0, 0), 1.0);
        let mut op = Animated::new(1.0);
        op.set_key(0.0, 0.0);
        op.set_key(2.0, 0.0);
        op.set_key(3.0, 1.0);
        media(show, v).motion.opacity = Some(op);
        let lead = |show: &Show| model::lead_in(show, key, show.cue(v).unwrap());
        assert_eq!(lead(show), LeadIn::At(2.0));
        assert!(trim(show, v, Edge::Right, 7.0, &mut fake_dur));
        assert_eq!(
            media(show, v).end,
            Some(4.0),
            "6 s of clip less the 2 s lead-in"
        );
        assert!(near(len(show, key, v).unwrap(), 6.0));
        // Left inside the lead-in: the keys move, the content doesn't.
        assert!(trim(show, v, Edge::Left, 2.0, &mut fake_dur));
        assert_eq!((start(show, v), media(show, v).start), (2.0, 0.0));
        assert_eq!(lead(show), LeadIn::At(1.0));
        assert!(near(len(show, key, v).unwrap(), 5.0));
        // Past it: the content is cut by what is past.
        assert!(trim(show, v, Edge::Left, 4.0, &mut fake_dur));
        assert_eq!(start(show, v), 4.0);
        assert!(near(media(show, v).start, 1.0));
        assert_eq!(lead(show), LeadIn::None);
        assert!(near(len(show, key, v).unwrap(), 3.0));
        assert_sound(show);
    }

    #[test]
    fn trimming_stills_fades_and_waits_moves_their_hold_or_duration() {
        let mut f = fixture();
        let key = f.list;
        let show = &mut f.show;
        let img = clip(show, key, CueType::Image, picture(show, 0, 0), 1.0);
        assert_eq!(len(show, key, img), None, "open-ended");
        assert!(trim(show, img, Edge::Right, 4.0, &mut fake_dur));
        assert_eq!(media(show, img).end, Some(3.0));
        assert!(trim(show, img, Edge::Left, 2.0, &mut fake_dur));
        assert_eq!((start(show, img), media(show, img).end), (2.0, Some(2.0)));
        let fade = add_clip(show, key, CueType::Fade, None, TrackId::Cues, 5.0).unwrap();
        assert!(trim(show, fade, Edge::Right, 6.0, &mut fake_dur));
        assert!(trim(show, fade, Edge::Left, 5.5, &mut fake_dur));
        let CueKind::Fade(fc) = &show.cue(fade).unwrap().kind else {
            panic!("not a fade")
        };
        assert!(near(fc.duration, 0.5) && start(show, fade) == 5.5);
        let wait = add_clip(show, key, CueType::Wait, None, TrackId::Cues, 0.0).unwrap();
        assert!(trim(show, wait, Edge::Right, 0.0, &mut fake_dur));
        assert!(near(len(show, key, wait).unwrap(), MIN_LEN));
        // A control cue has nothing to trim.
        let p = add_control(show, key, ControlAction::TimelinePause, 1.0).unwrap();
        assert!(!trim(show, p, Edge::Right, 3.0, &mut fake_dur));
        assert_sound(show);
    }

    #[test]
    fn splitting_keeps_the_length_and_takes_up_content_and_keys_where_it_stopped() {
        for n in 0..2 {
            let mut f = fixture();
            let key = keys(&f)[n];
            let show = &mut f.show;
            let v = clip(show, key, CueType::Video, picture(show, 0, 0), 1.0);
            {
                let m = media(show, v);
                m.fade_in = 1.0;
                m.fade_out = 1.5;
                m.transition = Transition::Crossfade;
                let mut x = Animated::new(0.0);
                x.set_key(0.0, 0.0);
                x.set_key(10.0, 1.0);
                m.motion.x = Some(x.clone());
                let mut blur = Effect::new(EffectKind::Blur);
                blur.p[0] = x;
                m.effects.push(blur);
            }
            let whole = len(show, key, v).unwrap();
            assert_eq!(whole, FILE);
            let made = split(show, key, &[v], 6.0, &mut fake_dur);
            assert_eq!(made.len(), 1);
            let w = made[0];
            assert!(near(
                len(show, key, v).unwrap() + len(show, key, w).unwrap(),
                whole
            ));
            assert!(near(start(show, w), 6.0));
            let (first, second) = (media(show, v).clone(), media(show, w).clone());
            assert_eq!(
                (first.fade_in, first.fade_out, first.end),
                (1.0, 0.0, Some(5.0))
            );
            assert_eq!((second.fade_in, second.fade_out), (0.0, 1.5));
            assert_eq!(second.transition, Transition::Cut);
            assert_eq!((second.start, second.end), (5.0, None));
            // The keys from the split on, from 0.
            for a in [second.motion.x.as_ref().unwrap(), &second.effects[0].p[0]] {
                assert!(near(f64::from(a.at(0.0)), 0.5));
                assert!(near(f64::from(a.at(5.0)), 1.0));
                assert_eq!(a.keys[0].t, 0.0);
            }
            // It comes straight after the first, a new cue of its own.
            assert_eq!(ids(show, key), vec![v, w]);
            assert_ne!(show.cue(w).unwrap().number, show.cue(v).unwrap().number);
            // Not outside a clip, not a loop.
            assert!(split(show, key, &[v], 0.5, &mut fake_dur).is_empty());
            assert!(split(show, key, &[w], 30.0, &mut fake_dur).is_empty());
            media(show, w).loop_media = true;
            assert!(split(show, key, &[w], 10.0, &mut fake_dur).is_empty());
            assert_sound(show);
        }
    }

    #[test]
    fn splitting_a_speed_ramp_and_stills() {
        let mut f = fixture();
        let key = f.group;
        let show = &mut f.show;
        let v = clip(show, key, CueType::Video, picture(show, 0, 0), 0.0);
        let mut ramp = Animated::new(1.0);
        ramp.set_key(0.0, 1.0);
        ramp.set_key(4.0, 2.0);
        set_rate(show, v, ramp.clone());
        let whole = len(show, key, v).unwrap();
        let w = split(show, key, &[v], 2.0, &mut fake_dur)[0];
        let map = SpeedMap::new(&ramp);
        assert!(near(media(show, w).start, map.media_at(2.0)));
        assert!(near(media(show, v).end.unwrap(), map.media_at(2.0)));
        // The second half's speed goes on from where it was (a linear ramp
        // re-anchors exactly).
        assert!(near(f64::from(media(show, w).rate.at(0.0)), 1.5));
        assert!(near(f64::from(media(show, w).rate.at(2.0)), 2.0));
        assert!((len(show, key, v).unwrap() + len(show, key, w).unwrap() - whole).abs() < 1e-3);

        let img = clip(show, key, CueType::Image, picture(show, 0, 1), 1.0);
        media(show, img).end = Some(6.0);
        let j = split(show, key, &[img], 3.0, &mut fake_dur)[0];
        assert_eq!(
            (media(show, img).end, media(show, j).end),
            (Some(2.0), Some(4.0))
        );
        // Open-ended stays open-ended.
        let open = clip(show, key, CueType::Text, picture(show, 0, 2), 0.0);
        let o2 = split(show, key, &[open], 5.0, &mut fake_dur)[0];
        assert_eq!(
            (media(show, open).end, media(show, o2).end),
            (Some(5.0), None)
        );
        assert_sound(show);
    }

    #[test]
    fn a_split_after_a_lead_in_goes_straight_on() {
        let mut f = fixture();
        let key = f.list;
        let show = &mut f.show;
        let v = clip(show, key, CueType::Video, picture(show, 0, 0), 0.0);
        let mut op = Animated::new(1.0);
        op.set_key(0.0, 0.0);
        op.set_key(2.0, 0.0);
        op.set_key(4.0, 1.0);
        media(show, v).motion.opacity = Some(op);
        // A list made with `new_timeline_list` waits for fade-ins.
        let whole = len(show, key, v).unwrap();
        assert_eq!(whole, FILE + 2.0);
        // Not inside the lead-in.
        assert!(split(show, key, &[v], 1.0, &mut fake_dur).is_empty());
        let w = split(show, key, &[v], 3.0, &mut fake_dur)[0];
        assert_eq!(media(show, v).end, Some(1.0));
        assert_eq!(media(show, w).start, 1.0);
        assert_eq!(
            model::lead_in(show, key, show.cue(w).unwrap()),
            LeadIn::None
        );
        assert!(near(
            len(show, key, v).unwrap() + len(show, key, w).unwrap(),
            whole
        ));
    }

    #[test]
    fn a_split_leaves_the_fade_cue_on_the_first_half_and_the_second_starts_visible() {
        for (n, ty) in [
            (0, CueType::Video),
            (1, CueType::Video),
            (0, CueType::Audio),
        ] {
            let mut f = fixture();
            let key = keys(&f)[n];
            let sound = ty == CueType::Audio;
            let show = &mut f.show;
            let track = if sound {
                TrackId::Sound { output: None }
            } else {
                picture(show, 0, 0)
            };
            let v = clip(show, key, ty, track, 0.0);
            if sound {
                media(show, v).volume_db = MIN_DB;
            } else {
                media(show, v).opacity = 0.0;
            }
            // Brought up by a fade at 1 s, to 0.8 (or -6 dB).
            let fade = add_clip(show, key, CueType::Fade, None, TrackId::Cues, 1.0).unwrap();
            let i = show.cue_index(fade).unwrap();
            if let CueKind::Fade(fc) = &mut show.cues[i].kind {
                fc.target = Some(v);
                fc.opacity = (!sound).then_some(0.8);
                fc.volume_db = sound.then_some(-6.0);
            }
            let w = split(show, key, &[v], 4.0, &mut fake_dur)[0];
            let i = show.cue_index(fade).unwrap();
            let CueKind::Fade(fc) = &show.cues[i].kind else {
                panic!()
            };
            assert_eq!(fc.target, Some(v), "the fade stays on the first half");
            let second = media(show, w).clone();
            if sound {
                assert_eq!(second.volume_db, -6.0);
            } else {
                assert_eq!(
                    (second.opacity, second.motion.opacity.is_none()),
                    (0.8, true)
                );
            }
            // It doesn't wait for a fade-in of its own: it starts as it comes.
            let cue = show.cue(w).unwrap();
            assert_eq!(model::lead_in(show, key, cue), LeadIn::None);
            assert!(!second.runs_unseen);
            // The first half is as it was.
            let first = media(show, v).clone();
            assert!(if sound {
                first.volume_db == MIN_DB
            } else {
                first.opacity == 0.0
            });
        }
        // Keys that stay at 0, and no fade cue: full.
        let mut f = fixture();
        let key = f.list;
        let show = &mut f.show;
        let v = clip(show, key, CueType::Video, picture(show, 0, 0), 0.0);
        let mut op = Animated::new(1.0);
        op.set_key(0.0, 0.0);
        op.set_key(9.0, 0.0);
        media(show, v).motion.opacity = Some(op);
        media(show, v).runs_unseen = true;
        let w = split(show, key, &[v], 4.0, &mut fake_dur)[0];
        let second = media(show, w).clone();
        assert_eq!(
            (second.opacity, second.motion.opacity.is_none()),
            (1.0, true)
        );
        // A clip seen already is left as it is.
        let u = clip(show, key, CueType::Video, picture(show, 0, 1), 0.0);
        media(show, u).opacity = 0.5;
        let x = split(show, key, &[u], 4.0, &mut fake_dur)[0];
        assert_eq!(media(show, x).opacity, 0.5);
    }

    #[test]
    fn pasted_clips_land_at_the_time_on_their_tracks_with_their_spacing_and_aims() {
        for n in 0..2 {
            let mut f = fixture();
            let key = keys(&f)[n];
            let show = &mut f.show;
            let v = clip(show, key, CueType::Video, picture(show, 1, 2), 3.0);
            let a = clip(
                show,
                key,
                CueType::Audio,
                TrackId::Sound { output: None },
                5.0,
            );
            let fade = add_clip(show, key, CueType::Fade, None, TrackId::Cues, 4.0).unwrap();
            let i = show.cue_index(fade).unwrap();
            if let CueKind::Fade(fc) = &mut show.cues[i].kind {
                fc.target = Some(v);
            }
            // Copied in the show's order, which is their starts'.
            let copies = show.copy_cues(&[v, a, fade]);
            let made = paste_clips(show, key, &copies, 10.0, true, false);
            assert_eq!(made.len(), 3);
            let (v2, f2, a2) = (made[0], made[1], made[2]);
            assert!(near(start(show, v2), 10.0));
            assert!(near(start(show, f2), 11.0));
            assert!(near(start(show, a2), 12.0));
            let clips = model::clips(show, key, &mut fake_dur);
            let track = |id| clips.iter().find(|c| c.id == id).unwrap().track;
            assert_eq!(track(v2), picture(show, 1, 2));
            assert_eq!(track(a2), TrackId::Sound { output: None });
            // The copied fade aims at the copy, the old one at the old clip.
            let aim = |show: &Show, id| match &show.cue(id).unwrap().kind {
                CueKind::Fade(fc) => fc.target,
                _ => None,
            };
            assert_eq!(aim(show, f2), Some(v2));
            assert_eq!(aim(show, fade), Some(v));
            assert!(ids(show, key).contains(&v2));
            // A group isn't pasted into a timeline.
            let g = show.make_cue(CueType::Group, None, 0);
            assert!(paste_clips(show, key, &[g], 0.0, true, false).is_empty());
            assert!(paste_clips(show, key, &copies, f64::NAN, true, false).is_empty());
            assert_sound(show);
        }
    }

    #[test]
    fn a_duplicate_comes_right_after_what_it_copies_and_moved_in_cues_go_to_a_time() {
        for n in 0..2 {
            let mut f = fixture();
            let key = keys(&f)[n];
            let show = &mut f.show;
            let v = clip(show, key, CueType::Video, picture(show, 0, 0), 1.0);
            let w = clip(show, key, CueType::Video, picture(show, 0, 1), 4.0);
            // FILE long each: the pair ends at 24 s.
            let made = duplicate_clips(show, key, &[v, w], &mut fake_dur);
            assert_eq!(made.len(), 2);
            assert!(near(start(show, made[0]), 24.0));
            assert!(near(start(show, made[1]), 27.0));
            assert!(duplicate_clips(show, key, &[CueId(9999)], &mut fake_dur).is_empty());
            // Placing keeps the spacing.
            assert!(place_at(show, key, &[v, w], 2.0));
            assert!(near(start(show, v), 2.0) && near(start(show, w), 5.0));
            assert!(!place_at(show, key, &[v, w], f64::INFINITY));
            assert_sound(show);
        }
    }

    #[test]
    fn ripple_closes_the_gap_on_the_deleted_clips_tracks_only() {
        for n in 0..2 {
            let mut f = fixture();
            let key = keys(&f)[n];
            let show = &mut f.show;
            let (v0, v1) = (picture(show, 0, 0), picture(show, 0, 1));
            let gone = clip(show, key, CueType::Image, v0, 2.0);
            media(show, gone).end = Some(3.0);
            let later = clip(show, key, CueType::Image, v0, 7.0);
            let earlier = clip(show, key, CueType::Image, v0, 0.0);
            media(show, earlier).end = Some(1.0);
            let other = clip(show, key, CueType::Image, v1, 7.0);
            let m = add_marker(show, key, 8.0, "Chorus").unwrap();
            let removed = delete(show, key, &[gone], true, &mut fake_dur);
            assert_eq!(removed, vec![gone]);
            assert!(show.cue(gone).is_none());
            assert_eq!(start(show, later), 4.0);
            assert_eq!((start(show, earlier), start(show, other)), (0.0, 7.0));
            let marker = |show: &Show| model::settings(show, key).unwrap().markers[0].time;
            assert_eq!(marker(show), 8.0, "not every track rippled");
            assert_sound(show);
            // Every track with clips rippling moves the markers after the gap too,
            // by the least any track moved.
            let a = clip(show, key, CueType::Image, v0, 2.0);
            media(show, a).end = Some(1.0);
            let b = clip(show, key, CueType::Image, v1, 4.0);
            media(show, b).end = Some(2.0);
            delete(show, key, &[a, b], true, &mut fake_dur);
            assert_eq!(start(show, other), 5.0);
            assert_eq!(start(show, later), 3.0);
            assert_eq!(marker(show), 7.0, "one second on V1, two on V2");
            assert_eq!(model::settings(show, key).unwrap().markers[0].id, m);
            // Without ripple nothing moves.
            delete(show, key, &[earlier], false, &mut fake_dur);
            assert_eq!(start(show, later), 3.0);
            assert!(delete(show, key, &[gone], true, &mut fake_dur).is_empty());
            assert_sound(show);
        }
    }

    #[test]
    fn a_pause_before_a_clip_lands_on_its_exact_time() {
        for n in 0..2 {
            let mut f = fixture();
            let key = keys(&f)[n];
            let show = &mut f.show;
            let v = clip(show, key, CueType::Video, picture(show, 0, 0), 1.3);
            media(show, v).end = Some(7.1);
            media(show, v).fade_out = 0.7;
            let c = model::clips(show, key, &mut fake_dur)
                .into_iter()
                .find(|c| c.id == v)
                .unwrap();
            let p = add_pause_before(show, key, v, PauseBefore::ClipIn, &mut fake_dur).unwrap();
            let q =
                add_pause_before(show, key, v, PauseBefore::ClipFadeOut, &mut fake_dur).unwrap();
            assert_eq!(start(show, p), c.start);
            assert_eq!(start(show, q), c.start + c.len.unwrap() - c.fade_out);
            for id in [p, q] {
                let pc = show.cue(id).unwrap();
                let CueKind::Control(ctl) = &pc.kind else {
                    panic!("not a control cue")
                };
                assert_eq!(ctl.action, ControlAction::TimelinePause);
                assert!(ctl.freeze.is_nothing() && ctl.timeline.is_none());
                assert!(pc.name.contains("a"), "{}", pc.name);
            }
            assert_eq!(show.cue(p).unwrap().name, "Pause before a");
            assert_eq!(show.cue(q).unwrap().name, "Pause before a fades out");
            // The pause comes before the clip at the same time, and the clip is
            // now one whose end the clock keeps.
            assert_eq!(ids(show, key)[0], p);
            assert!(model::clocked(show, key, show.cue(v).unwrap(), c.len));
            // Without a length there is no fade-out to pause before.
            let img = clip(show, key, CueType::Image, picture(show, 0, 1), 0.0);
            let none = add_pause_before(show, key, img, PauseBefore::ClipFadeOut, &mut fake_dur);
            assert!(none.is_none());
            assert_sound(show);
        }
    }

    #[test]
    fn tracks_are_added_and_removed_only_when_empty() {
        for n in 0..2 {
            let mut f = fixture();
            let key = keys(&f)[n];
            let audio = f.audio;
            let show = &mut f.show;
            clip(show, key, CueType::Video, picture(show, 0, 1), 0.0);
            let t = add_track(show, key, true).unwrap();
            assert_eq!(t, picture(show, 0, 2));
            let s1 = add_track(show, key, false).unwrap();
            let s2 = add_track(show, key, false).unwrap();
            assert_eq!(s1, TrackId::Sound { output: None });
            assert_eq!(
                s2,
                TrackId::Sound {
                    output: Some(audio)
                }
            );
            assert!(add_track(show, key, false).is_none(), "no output left");
            let drawn = model::tracks(show, key, &model::clips(show, key, &mut |_| None));
            assert!(drawn.contains(&t) && drawn.contains(&s2));
            assert!(!remove_track(show, key, picture(show, 0, 1)), "has a clip");
            assert!(!remove_track(show, key, TrackId::Cues));
            assert!(remove_track(show, key, t));
            assert!(!remove_track(show, key, t));
            assert!(remove_track(show, key, s1) && remove_track(show, key, s2));
            assert!(model::settings(show, key).unwrap().tracks.is_empty());
            assert_sound(show);
        }
    }

    #[test]
    fn markers_are_added_moved_renamed_and_removed_in_time_order() {
        for n in 0..2 {
            let mut f = fixture();
            let key = keys(&f)[n];
            let show = &mut f.show;
            let a = add_marker(show, key, 10.0, "B").unwrap();
            let b = add_marker(show, key, -3.0, "A").unwrap();
            assert_ne!(a, b);
            assert!(add_marker(show, key, f64::INFINITY, "x").is_none());
            let names = |show: &Show| {
                model::settings(show, key)
                    .unwrap()
                    .markers
                    .iter()
                    .map(|m| (m.name.clone(), m.time))
                    .collect::<Vec<_>>()
            };
            assert_eq!(names(show), vec![("A".into(), 0.0), ("B".into(), 10.0)]);
            assert!(move_marker(show, key, b, 12.0));
            assert!(!move_marker(show, key, b, 12.0));
            assert!(!move_marker(show, key, b, f64::NAN));
            assert!(rename_marker(show, key, b, "C"));
            assert!(!rename_marker(show, key, 9999, "C"));
            assert_eq!(names(show), vec![("B".into(), 10.0), ("C".into(), 12.0)]);
            assert_eq!(
                model::resolve(show, key, crate::timeline::TimelinePoint::Marker(b)),
                Some(12.0)
            );
            assert!(remove_marker(show, key, a) && !remove_marker(show, key, a));
            // A new marker never takes the id of one removed.
            let c = add_marker(show, key, 1.0, "D").unwrap();
            assert!(c != a && c != b);
            assert_sound(show);
        }
    }

    #[test]
    fn a_plain_group_s_settings_go_again_when_its_last_marker_does() {
        let mut show = Show::default();
        let g = show.make_cue(CueType::Group, None, 0);
        let key = TimelineKey::Group(g.id);
        show.cues.push(g);
        let m = add_marker(&mut show, key, 1.0, "M").unwrap();
        assert!(model::settings(&show, key).is_some());
        assert!(remove_marker(&mut show, key, m));
        assert!(model::settings(&show, key).is_none());
        assert!(!json(&show).contains("timeline"));
    }

    #[test]
    fn a_speed_is_kept_to_what_plays() {
        let mut f = fixture();
        let key = f.group;
        let show = &mut f.show;
        let v = clip(show, key, CueType::Video, picture(show, 0, 0), 0.0);
        let key_at = |t: f64, v: f32| Keyframe {
            t,
            v,
            curve: Curve::Linear,
        };
        let mut r = Animated::new(f32::NAN);
        r.keys = vec![key_at(3.0, 9.0), key_at(1.0, 0.0), key_at(f64::NAN, 1.0)];
        assert!(set_rate(show, v, r.clone()));
        let rate = &media(show, v).rate;
        assert_eq!(rate.value, 1.0);
        let ks: Vec<(f64, f32)> = rate.keys.iter().map(|k| (k.t, k.v)).collect();
        assert_eq!(ks, vec![(1.0, MIN_RATE as f32), (3.0, MAX_RATE as f32)]);
        assert!(!set_rate(show, v, r), "the same again");
        // Only a file plays at a speed.
        let img = clip(show, key, CueType::Image, picture(show, 0, 1), 0.0);
        assert!(!set_rate(show, img, Animated::new(2.0)));
    }

    #[test]
    fn sorting_is_stable_and_keeps_groups_and_lists_whole() {
        let mut f = fixture();
        let (group, list) = (f.group, f.list);
        let show = &mut f.show;
        for key in [group, list] {
            let v0 = picture(show, 0, 0);
            let a = clip(show, key, CueType::Image, v0, 2.0);
            let b = clip(show, key, CueType::Image, v0, 1.0);
            let c = clip(show, key, CueType::Image, v0, 2.0);
            assert_eq!(ids(show, key), vec![b, a, c], "ties stay as they were");
            cue_mut(show, b).unwrap().pre_wait = 5.0;
            sort_by_start(show, key);
            assert_eq!(ids(show, key), vec![a, c, b]);
        }
        // A group's children follow it, a list's cues are in the list.
        let TimelineKey::Group(g) = group else {
            unreachable!()
        };
        let gi = show.cue_index(g).unwrap();
        assert_eq!(show.block_end(gi) - gi, 4);
        let TimelineKey::List(n) = list else {
            unreachable!()
        };
        assert!(show.cues[show.list_range(n)].iter().all(|c| c.list == n));
        assert_sound(show);
        // An old list timeline that holds a group (not made any more) keeps it whole.
        let slot = show.insert_slot(None, true, n);
        let grp = show.make_cue(CueType::Group, None, slot.at);
        let gid = grp.id;
        show.insert_cue(slot, grp);
        let mut child = show.make_cue(CueType::Wait, None, 0);
        child.parent = Some(gid);
        child.list = n;
        let at = show.list_end(n);
        show.cues.insert(at, child);
        sort_by_start(show, list);
        assert_eq!(model::members(show, list)[0].id, gid);
        assert_sound(show);
    }

    #[test]
    fn lists_become_timelines_unless_they_hold_groups_and_never_the_main_one() {
        let mut show = Show::default();
        let l = show.add_list("Plain");
        assert!(set_list_timeline(&mut show, l, true).is_ok());
        assert_eq!(show.lists[0].timeline, Some(TimelineSettings::for_editor()));
        assert!(model::is_timeline(&show, TimelineKey::List(l)));
        assert!(set_list_timeline(&mut show, l, false).is_ok());
        assert!(!model::is_timeline(&show, TimelineKey::List(l)));
        // The main list stays a cue list (the owner's ruling E1).
        assert!(set_list_timeline(&mut show, 0, true).is_err());
        assert!(set_list_timeline(&mut show, 42, true).is_err());
        let slot = show.insert_slot(None, true, l);
        let g = show.make_cue(CueType::Group, None, slot.at);
        show.insert_cue(slot, g);
        let err = set_list_timeline(&mut show, l, true).unwrap_err();
        assert!(err.contains("groups"), "{err}");
        // New timelines are the editor's.
        let n = new_timeline_list(&mut show, "T");
        assert_eq!(
            model::settings(&show, TimelineKey::List(n)),
            Some(&TimelineSettings::for_editor())
        );
        assert_sound(&show);
    }

    #[test]
    fn new_timelines_crossfade_their_overlaps_and_an_old_group_does_not() {
        let mut show = Show::default();
        let slot = show.insert_slot(None, true, 0);
        let g = new_timeline_group(&mut show, slot);
        let on = |show: &Show, k| model::settings(show, k).is_some_and(|s| s.crossfade_overlaps);
        assert!(on(&show, TimelineKey::Group(g)));
        let n = new_timeline_list(&mut show, "T");
        assert!(on(&show, TimelineKey::List(n)));
        // A "start all together" group from an older show has no settings.
        let slot = show.insert_slot(None, true, 0);
        let mut old = show.make_cue(CueType::Group, None, slot.at);
        if let CueKind::Group(gc) = &mut old.kind {
            gc.mode = crate::show::GroupMode::Timeline;
        }
        let old_id = old.id;
        show.insert_cue(slot, old);
        assert!(!on(&show, TimelineKey::Group(old_id)));
    }

    #[test]
    fn keys_move_and_are_re_anchored_at_zero() {
        let mut a = Animated::new(0.0);
        a.set_key(1.0, 0.0);
        a.set_key(3.0, 2.0);
        let mut later = a.clone();
        shift_keys(&mut later, -2.0);
        assert_eq!(later.keys[0].t, 3.0);
        let mut cut = a.clone();
        shift_keys(&mut cut, 2.0);
        assert_eq!(cut.keys.len(), 2);
        assert_eq!((cut.keys[0].t, cut.keys[0].v), (0.0, 1.0));
        assert_eq!(cut.keys[1].t, 1.0);
        // Past every key: one key holding the last value.
        let mut past = a.clone();
        shift_keys(&mut past, 10.0);
        assert_eq!(past.keys.len(), 1);
        assert_eq!(past.at(0.0), 2.0);
        // Before the first: nothing is dropped and the values keep their times.
        let mut early = a.clone();
        shift_keys(&mut early, 0.5);
        assert_eq!(early.keys.len(), 2);
        assert_eq!(early.at(0.5), a.at(1.0));
        let mut steady = Animated::new(3.0);
        shift_keys(&mut steady, 4.0);
        assert_eq!(steady, Animated::new(3.0));
    }

    #[test]
    fn opacity_keys_are_set_kept_to_0_to_1_and_none_leave_a_steady_opacity() {
        let mut f = fixture();
        let key = f.group;
        let show = &mut f.show;
        let v = clip(show, key, CueType::Video, picture(show, 0, 0), 0.0);
        let mut a = Animated::new(1.0);
        a.set_key(2.0, 3.0);
        a.set_key(0.0, -1.0);
        assert!(set_opacity(show, v, a.clone()));
        let o = media(show, v).motion.opacity.clone().unwrap();
        let ks: Vec<(f64, f32)> = o.keys.iter().map(|k| (k.t, k.v)).collect();
        assert_eq!(ks, vec![(0.0, 0.0), (2.0, 1.0)]);
        assert!(!set_opacity(show, v, a), "the same again");
        // A first key at 0 rising later is a lead-in the model reads.
        let mut b = Animated::new(1.0);
        b.set_key(0.0, 0.0);
        b.set_key(1.5, 0.0);
        b.set_key(2.5, 1.0);
        assert!(set_opacity(show, v, b));
        assert_eq!(
            model::lead_in(show, key, show.cue(v).unwrap()),
            LeadIn::At(1.5)
        );
        // No keys: the steady opacity, no track.
        assert!(set_opacity(show, v, Animated::new(0.6)));
        assert!(media(show, v).motion.opacity.is_none());
        assert_eq!(media(show, v).opacity, 0.6);
        // Only pictures have an opacity.
        let s = clip(
            show,
            key,
            CueType::Audio,
            TrackId::Sound { output: None },
            0.0,
        );
        assert!(!set_opacity(show, s, Animated::new(0.5)));
    }

    #[test]
    fn fades_are_set_within_the_clip_and_a_cut_picture_then_fades_in() {
        let mut f = fixture();
        let key = f.group;
        let show = &mut f.show;
        let v = clip(show, key, CueType::Video, picture(show, 0, 0), 0.0);
        media(show, v).transition = Transition::Cut;
        media(show, v).fade_out = 5.0;
        assert!(set_fade(show, v, Edge::Left, 2.5, &mut fake_dur));
        assert_eq!(media(show, v).fade_in, 2.5);
        assert_eq!(media(show, v).transition, Transition::FadeIn);
        // Never into the fade-out (20 s of file, 5 s of fade-out).
        assert!(set_fade(show, v, Edge::Left, 30.0, &mut fake_dur));
        assert_eq!(media(show, v).fade_in, 15.0);
        assert!(set_fade(show, v, Edge::Right, -1.0, &mut fake_dur));
        assert_eq!(media(show, v).fade_out, 0.0);
        assert!(!set_fade(show, v, Edge::Right, 0.0, &mut fake_dur));
        assert!(!set_fade(show, v, Edge::Right, f64::NAN, &mut fake_dur));
        // A sound has no transition to change.
        let s = clip(
            show,
            key,
            CueType::Audio,
            TrackId::Sound { output: None },
            0.0,
        );
        assert!(set_fade(show, s, Edge::Left, 1.0, &mut fake_dur));
        assert_eq!(media(show, s).fade_in, 1.0);
    }

    #[test]
    fn a_track_moves_above_or_below_the_next_on_its_display() {
        let mut f = fixture();
        let key = f.group;
        let show = &mut f.show;
        let (v0, v1) = (picture(show, 0, 0), picture(show, 0, 1));
        let a = clip(show, key, CueType::Image, v0, 0.0);
        let b = clip(show, key, CueType::Image, v1, 0.0);
        let other = clip(show, key, CueType::Image, picture(show, 1, 0), 0.0);
        // v1 is the top one of its display.
        assert!(!move_track(show, key, v1, true));
        assert!(!move_track(show, key, v0, false));
        assert!(!move_track(show, key, TrackId::Cues, true));
        assert!(move_track(show, key, v0, true));
        assert_eq!(model::track_of(show.cue(a).unwrap()), v1);
        assert_eq!(model::track_of(show.cue(b).unwrap()), v0);
        assert_eq!(
            model::track_of(show.cue(other).unwrap()),
            picture(show, 1, 0),
            "another display's tracks stay"
        );
        // An empty track the timeline keeps moves too.
        let top = add_track(show, key, true).unwrap();
        assert_eq!(top, picture(show, 0, 2));
        assert!(move_track(show, key, top, false));
        assert_eq!(model::settings(show, key).unwrap().tracks, vec![v1]);
        assert_eq!(model::track_of(show.cue(a).unwrap()), picture(show, 0, 2));
        assert_sound(show);
    }

    #[test]
    fn a_clip_moved_to_a_display_it_was_also_on_lives_there() {
        let mut f = fixture();
        let key = f.group;
        let show = &mut f.show;
        let a = clip(show, key, CueType::Image, picture(show, 0, 0), 0.0);
        let second = display(show, 1).unwrap();
        media(show, a).also_on = vec![second];
        assert!(move_clips(show, key, &[a], 0.0, Some(picture(show, 1, 0))));
        assert_eq!(media(show, a).output, Some(second));
        assert!(media(show, a).also_on.is_empty());
    }
}
