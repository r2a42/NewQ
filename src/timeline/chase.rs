//! What a timeline has playing at a given time, worked out from the show by
//! playing it through in the mind (plan 2.6): the clips alive there and from
//! where in each, at what levels, and what is still to come. The engine's seek,
//! parking and Jump start the first and leave the rest waiting.
//!
//! The rules, as the engine plays a timeline:
//! - A clip comes in at its start, or with a lead-in (`model::lead_in`) where its
//!   fade-in begins; a dip's clip half way through the dip. Its own fade-in
//!   ramps from there.
//! - A picture coming in replaces the pictures on its display and layer (any
//!   layer, for a show from before layers were kept apart; everything, for a
//!   dip): a cut at once, a crossfade over its fade. Clips that start together
//!   at 0 don't replace each other.
//! - A clip is over at its start, lead-in and length (`Dur`, the content's).
//!   A video that holds its last frame (with no fade-out) stays, on that frame.
//! - Fade cues move the levels of what they reach from where they are; a stop
//!   cue (or a fade that stops) ends its target, and one aimed at a clip that has
//!   not come in yet means it never does.
//! - Control cues are not done again, except a Pause and Resume of a clip, which
//!   hold its content (and so move it off the clock). Pause cues of the timeline
//!   itself, how long the clock once stood, and send, wait and timecode cues
//!   change nothing here.
//! - What is exactly at `pos` has happened (a GO from a marker on a pause cue
//!   runs on), except at 0: that is the top, and everything is still to come.
//!
//! - A clip's opacity, motion and effect keyframes are read at its time by the
//!   timeline's clock (`pos` less where its time is 0), as the engine plays them,
//!   whatever its content did (frozen, or running on through a pause).
//!
//! Not modelled: a fade cue's matrix crosspoints, a fade on a clip while it is
//! held, a stop of everything (the clips before it go; what comes after still
//! does).

use super::TimelineKey;
use super::model::{self, Dur, LeadIn};
use crate::show::{
    ControlAction, Cue, CueId, CueKind, Curve, FadeCue, GeometryTarget, MIN_DB, MediaCue, Show,
    StopCue, Transition,
};

/// Times closer than this are the same time.
const SAME_TIME: f64 = 1e-6;

/// A level now, where it is going, the seconds left to get there, and how.
pub type Level = (f32, f32, f64, Curve);

/// A picture's position, scale and rotation moved by fade cues: where it is
/// now, where it is going, the seconds left and how. Fields a fade didn't name are None.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeometryRamp {
    pub now: GeometryTarget,
    pub to: GeometryTarget,
    pub left: f64,
    pub curve: Curve,
}

/// A clip alive at the chased time, and how to start it.
#[derive(Clone, Debug, PartialEq)]
pub struct Alive {
    pub id: CueId,
    /// Clip time (`elapsed`) to start at.
    pub from: f64,
    /// Lead-in V; content position counts from here.
    pub lead: f64,
    /// Frozen by a clip Pause cue not yet resumed.
    pub frozen: bool,
    /// Its content has left the clock (it was frozen for a while).
    pub detached: bool,
    /// A held video that had ended.
    pub at_last_frame: bool,
    /// Opacity and volume (dB) from fades in progress or done, or its own fade-in
    /// in progress; None: the cue's own (its keyframes, if it has any).
    pub opacity: Option<Level>,
    pub volume_db: Option<Level>,
    /// A fade cue has reached it (live level edits and its opacity keyframes no
    /// longer apply, as after any fade cue).
    pub faded: bool,
    /// Where fade cues have moved (or are moving) its picture.
    pub geometry: Option<GeometryRamp>,
    /// A fade-and-stop (a crossfade out, a fade cue that stops) in progress: it
    /// is gone this many seconds from now.
    pub stop_after: Option<f64>,
}

/// A clip still to come: at a time, or (`at` None) waiting for a fade cue.
#[derive(Clone, Debug, PartialEq)]
pub struct Waiting {
    pub id: CueId,
    /// When the run starts it (a clip's start plus its lead-in).
    pub at: Option<f64>,
    /// Its lead-in (for one waiting for a fade cue, the first fade's time, if known).
    pub lead: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Chase {
    /// In the order they came in.
    pub alive: Vec<Alive>,
    /// In time order.
    pub upcoming: Vec<Waiting>,
    pub passed: Vec<CueId>,
}

/// A level's ramp: `from` at `t0` to `to` over `dur` seconds.
#[derive(Clone, Copy, Debug)]
struct Track {
    from: f32,
    to: f32,
    t0: f64,
    dur: f64,
    curve: Curve,
}

impl Track {
    fn new(from: f32, to: f32, t0: f64, dur: f64, curve: Curve) -> Track {
        let dur = if dur.is_finite() { dur.max(0.0) } else { 0.0 };
        Track {
            from,
            to,
            t0,
            dur,
            curve,
        }
    }

    fn at(&self, t: f64) -> f32 {
        if self.dur <= 0.0 || t >= self.t0 + self.dur {
            self.to
        } else if t <= self.t0 {
            self.from
        } else {
            let k = self.curve.apply((t - self.t0) / self.dur) as f32;
            self.from + (self.to - self.from) * k
        }
    }

    fn running(&self, t: f64) -> bool {
        t < self.t0 + self.dur - SAME_TIME
    }

    fn left(&self, t: f64) -> f64 {
        (self.t0 + self.dur - t).max(0.0)
    }

    fn level(&self, t: f64) -> Level {
        (self.at(t), self.to, self.left(t), self.curve)
    }
}

/// One clip as the chase plays it.
struct Clip<'a> {
    cue: &'a Cue,
    m: &'a MediaCue,
    start: f64,
    /// Where its clip time is 0 (its start, or half way through a dip).
    zero: f64,
    lead: f64,
    /// Its content's length, if known and not open-ended.
    len: Option<f64>,
    picture: bool,
    dip: bool,
    /// Waits for a fade cue that comes after `pos`.
    waits_fade: bool,
    /// Started together with its brothers at 0 (they don't replace each other).
    at_top: bool,
    entered: bool,
    /// A stop cue reached it before it came in: it never does.
    cancelled: bool,
    /// When it is (or was) gone, by a replacement or a stop.
    gone: Option<f64>,
    opacity: Option<Track>,
    volume: Option<Track>,
    faded: bool,
    geometry: [Option<Track>; 5],
    frozen_since: Option<f64>,
    frozen_for: f64,
    detached: bool,
}

impl Clip<'_> {
    /// Its opacity at `t`: a ramp's, else the cue's own (with its keyframes).
    fn opacity_at(&self, t: f64) -> f32 {
        match self.opacity {
            Some(tr) if self.faded || tr.running(t) => tr.at(t),
            _ => self.m.motion.opacity_at(self.m.opacity, t - self.zero),
        }
    }

    fn volume_at(&self, t: f64) -> f32 {
        match self.volume {
            Some(tr) if self.faded || tr.running(t) => tr.at(t),
            _ => self.m.volume_db,
        }
    }

    /// A video that keeps its last frame when it ends (and doesn't fade out first).
    fn holds_last(&self) -> bool {
        matches!(self.cue.kind, CueKind::Video(_))
            && self.m.hold_last_frame
            && self.m.fade_out <= 0.0
    }

    /// Its content (and the clock's slot for it) has run out by `t`.
    fn ended_by(&self, t: f64) -> bool {
        self.len
            .is_some_and(|l| t - self.zero - self.lead >= l - SAME_TIME)
    }

    /// On screen (or sounding) at `t`, and not on its way out.
    fn up_at(&self, t: f64) -> bool {
        self.entered
            && !self.cancelled
            && self.gone.is_none()
            && (!self.ended_by(t) || self.holds_last())
    }

    /// Aimed at by a fade or stop cue's target (the clip or its group).
    fn aimed(&self, target: Option<CueId>) -> bool {
        target.is_some() && (target == Some(self.cue.id) || target == self.cue.parent)
    }

    fn fades_out(&mut self, t: f64, secs: f64, curve: Curve) {
        self.opacity = Some(Track::new(self.opacity_at(t), 0.0, t, secs, curve));
        self.volume = Some(Track::new(self.volume_at(t), MIN_DB, t, secs, curve));
        self.gone = Some(t + secs.max(0.0));
    }
}

/// Cues that put a picture on a display.
fn is_picture(kind: &CueKind) -> bool {
    matches!(
        kind,
        CueKind::Video(_)
            | CueKind::Image(_)
            | CueKind::Text(_)
            | CueKind::Shape(_)
            | CueKind::Camera(_)
    )
}

/// Where a cue starts on its timeline: its pre-wait (never before 0).
fn start_of(cue: &Cue) -> f64 {
    if cue.pre_wait.is_finite() {
        cue.pre_wait.max(0.0)
    } else {
        0.0
    }
}

/// Each geometry field a fade can move, as the clip has it at clip time `t`
/// (its motion keyframes, read at its time by the timeline's clock).
fn geometry_base(m: &MediaCue, t: f64) -> [f32; 5] {
    let t = m.motion.transform_at(&m.transform, t);
    [t.x, t.y, t.scale_x, t.scale_y, t.rotation]
}

fn geometry_fields(g: &GeometryTarget) -> [Option<f32>; 5] {
    [g.x, g.y, g.scale_x, g.scale_y, g.rotation]
}

fn geometry_from(v: [Option<f32>; 5]) -> GeometryTarget {
    GeometryTarget {
        x: v[0],
        y: v[1],
        scale_x: v[2],
        scale_y: v[3],
        rotation: v[4],
    }
}

/// Something that happens on the timeline, in the order it happens.
enum Event<'a> {
    /// A clip comes in (its index).
    Enter(usize),
    Fade(&'a FadeCue),
    Stop(&'a StopCue),
    /// A Pause (true) or Resume of a clip.
    Hold(CueId, bool),
}

/// What plays on the timeline `key` at `pos` (seconds from its start). `dur`
/// gives a cue's content length (`Engine::cue_duration`): it is asked only of
/// the clips that have come in by `pos`. Disarmed cues are left out.
pub fn chase(show: &Show, key: TimelineKey, pos: f64, dur: Dur) -> Chase {
    let pos = if pos.is_finite() { pos } else { 0.0 };
    // The top: everything is still to come, even what is at 0.
    let pos = if pos <= 0.0 { -1.0 } else { pos };
    let members: Vec<&Cue> = model::members(show, key)
        .into_iter()
        .filter(|c| c.armed)
        .collect();
    let mut out = Chase::default();
    let mut clips: Vec<Clip> = Vec::new();
    let mut events: Vec<((f64, u8, usize, u8), Event)> = Vec::new();
    for (idx, cue) in members.iter().copied().enumerate() {
        let start = start_of(cue);
        let Some(m) = cue.kind.media() else {
            // Fades, stops, control, wait, send and timecode cues (and a group
            // in a list): what is behind has been passed; only some do anything.
            if start > pos + SAME_TIME {
                out.upcoming.push(Waiting {
                    id: cue.id,
                    at: Some(start),
                    lead: 0.0,
                });
                continue;
            }
            out.passed.push(cue.id);
            let order = model::due_order(cue);
            match &cue.kind {
                CueKind::Fade(f) => events.push(((start, order, idx, 1), Event::Fade(f))),
                CueKind::Stop(s) => events.push(((start, order, idx, 1), Event::Stop(s))),
                CueKind::Control(c)
                    if matches!(c.action, ControlAction::Pause | ControlAction::Resume) =>
                {
                    if let Some(t) = c.target {
                        let hold = c.action == ControlAction::Pause;
                        events.push(((start, order, idx, 1), Event::Hold(t, hold)));
                    }
                }
                _ => {}
            }
            continue;
        };
        let picture = is_picture(&cue.kind);
        let dip = picture && m.transition.dip_color().is_some();
        let half = if dip { (m.fade_in / 2.0).max(0.0) } else { 0.0 };
        let (lead, woken) = match model::lead_in(show, key, cue) {
            LeadIn::None => (0.0, false),
            LeadIn::At(v) => (v.max(0.0), false),
            LeadIn::UntilFaded => match model::fade_wake(show, key, cue) {
                Some(w) => (w, true),
                // Nothing ever raises it: it never starts.
                None => {
                    out.passed.push(cue.id);
                    continue;
                }
            },
        };
        let zero = start + half;
        let entry = start + lead + half;
        let i = clips.len();
        clips.push(Clip {
            cue,
            m,
            start,
            zero,
            lead,
            len: None,
            picture,
            dip,
            waits_fade: woken && start + lead > pos + SAME_TIME,
            at_top: cue.pre_wait <= 0.0 && lead <= 0.0 && !dip,
            entered: false,
            cancelled: false,
            gone: None,
            opacity: None,
            volume: None,
            faded: false,
            geometry: [None; 5],
            frozen_since: None,
            frozen_for: 0.0,
            detached: false,
        });
        if start + lead <= pos + SAME_TIME && entry <= pos + SAME_TIME {
            // One woken by a fade comes in just before that fade is applied.
            let order = if woken { (1, 0, 0) } else { (2, idx, 0) };
            events.push(((entry, order.0, order.1, order.2), Event::Enter(i)));
        }
    }
    events.sort_by(|(a, _), (b, _)| {
        a.0.total_cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
            .then(a.3.cmp(&b.3))
    });

    for ((t, ..), ev) in events {
        match ev {
            Event::Enter(i) => enter(show, &mut clips, i, t, dur),
            Event::Fade(f) => {
                for c in clips.iter_mut() {
                    if !c.aimed(f.target) {
                        continue;
                    }
                    if !c.entered {
                        // A fade that stops its target cancels what of it is still to come.
                        if f.stop_target {
                            c.cancelled = true;
                        }
                        continue;
                    }
                    if !c.up_at(t) {
                        continue;
                    }
                    c.faded = true;
                    if let Some(o) = f.opacity {
                        c.opacity = Some(Track::new(c.opacity_at(t), o, t, f.duration, f.curve));
                    }
                    if let Some(v) = f.volume_db {
                        c.volume = Some(Track::new(c.volume_at(t), v, t, f.duration, f.curve));
                    }
                    let base = geometry_base(c.m, t - c.zero);
                    let named = geometry_fields(&f.geometry);
                    // The first fade that moves its picture keeps it where its
                    // keyframes had it then, in what it doesn't name too (as the
                    // engine's `LiveTransform` does).
                    if c.geometry.iter().all(Option::is_none)
                        && named.iter().any(|v| v.is_some_and(f32::is_finite))
                    {
                        for (k, g) in c.geometry.iter_mut().enumerate() {
                            *g = Some(Track::new(base[k], base[k], t, 0.0, f.curve));
                        }
                    }
                    for (k, to) in named.into_iter().enumerate() {
                        if let Some(to) = to.filter(|v| v.is_finite()) {
                            let from = c.geometry[k].map_or(base[k], |g| g.at(t));
                            c.geometry[k] = Some(Track::new(from, to, t, f.duration, f.curve));
                        }
                    }
                    if f.stop_target {
                        c.gone = Some(t + f.duration.max(0.0));
                    }
                }
            }
            Event::Stop(s) => {
                for c in clips.iter_mut() {
                    let all = s.target.is_none();
                    if !all && !c.aimed(s.target) {
                        continue;
                    }
                    if c.entered {
                        // A stop's fade still going is taken as done.
                        if c.gone.is_none_or(|g| g > t) && !c.cancelled {
                            c.gone = Some(t);
                        }
                    } else if !all {
                        c.cancelled = true;
                    }
                }
            }
            Event::Hold(target, hold) => {
                if let Some(c) = clips.iter_mut().find(|c| c.cue.id == target)
                    && c.up_at(t)
                {
                    if hold {
                        c.frozen_since.get_or_insert(t);
                        c.detached = true;
                    } else if let Some(f) = c.frozen_since.take() {
                        c.frozen_for += t - f;
                    }
                }
            }
        }
    }

    for c in &clips {
        if c.cancelled {
            out.passed.push(c.cue.id);
            continue;
        }
        if !c.entered {
            out.upcoming.push(Waiting {
                id: c.cue.id,
                // Waiting for its place, or for its fade cue.
                at: (!c.waits_fade).then_some(c.start + c.lead),
                lead: c.lead,
            });
            continue;
        }
        if c.gone.is_some_and(|g| g <= pos + SAME_TIME) {
            out.passed.push(c.cue.id);
            continue;
        }
        let ended = c.ended_by(pos);
        if ended && !c.holds_last() {
            out.passed.push(c.cue.id);
            continue;
        }
        let held = c.frozen_for + c.frozen_since.map_or(0.0, |f| pos - f);
        let from = if ended {
            c.lead + c.len.unwrap_or(0.0)
        } else {
            (pos - c.zero - held).max(c.lead)
        };
        let opacity = c
            .opacity
            .filter(|tr| c.faded || tr.running(pos))
            .map(|tr| tr.level(pos));
        let volume_db = c
            .volume
            .filter(|tr| c.faded || tr.running(pos))
            .map(|tr| tr.level(pos));
        let geometry = c.geometry.iter().any(Option::is_some).then(|| {
            let now = c.geometry.map(|g| g.map(|g| g.at(pos)));
            let to = c.geometry.map(|g| g.map(|g| g.to));
            let last = c
                .geometry
                .iter()
                .flatten()
                .max_by(|a, b| (a.t0 + a.dur).total_cmp(&(b.t0 + b.dur)))
                .copied();
            GeometryRamp {
                now: geometry_from(now),
                to: geometry_from(to),
                left: last.map_or(0.0, |g| g.left(pos)),
                curve: last.map_or(Curve::Linear, |g| g.curve),
            }
        });
        out.alive.push(Alive {
            id: c.cue.id,
            from,
            lead: c.lead,
            frozen: c.frozen_since.is_some(),
            detached: c.detached,
            at_last_frame: ended,
            opacity,
            volume_db,
            faded: c.faded,
            geometry,
            stop_after: c.gone.map(|g| g - pos),
        });
    }
    out.upcoming.sort_by(|a, b| {
        a.at.unwrap_or(f64::INFINITY)
            .total_cmp(&b.at.unwrap_or(f64::INFINITY))
    });
    out
}

/// The clip `i` comes in at `t`: what it replaces goes, and its own fade-in starts.
fn enter(show: &Show, clips: &mut [Clip], i: usize, t: f64, dur: Dur) {
    if clips[i].cancelled {
        return;
    }
    clips[i].len = match dur(clips[i].cue) {
        Some(Some(d)) if d.is_finite() => Some(d.max(0.0)),
        // Open-ended, or not known yet (it is started, and ends by itself).
        _ => None,
    };
    let m = clips[i].m;
    if clips[i].picture {
        let (output, layer, dip, top) = (m.output, m.layer, clips[i].dip, clips[i].at_top);
        let crossfade = (m.transition == Transition::Crossfade).then_some(m.fade_in.max(0.0));
        let cut = m.transition == Transition::Cut || crossfade == Some(0.0);
        for (j, o) in clips.iter_mut().enumerate() {
            if j == i || !o.picture || !o.up_at(t) || o.m.output != output {
                continue;
            }
            let brothers = show.own_layers && top && o.at_top;
            if dip {
                // A dip clears the display.
                o.gone = Some(t);
                continue;
            }
            if brothers || (show.own_layers && o.m.layer != layer) {
                continue;
            }
            if cut {
                o.gone = Some(t);
            } else if let Some(secs) = crossfade {
                o.fades_out(t, secs, m.curve);
            }
        }
    }
    let c = &mut clips[i];
    c.entered = true;
    // Its own transition: (seconds for opacity, for volume).
    let half = (m.fade_in / 2.0).max(0.0);
    let (fade_op, fade_vol) = if c.dip {
        (0.0, half)
    } else if c.picture && m.transition == Transition::Cut {
        (0.0, 0.0)
    } else {
        (m.fade_in.max(0.0), m.fade_in.max(0.0))
    };
    if fade_op > 0.0 && c.picture {
        c.opacity = Some(Track::new(0.0, m.opacity, t, fade_op, m.curve));
    }
    if fade_vol > 0.0 {
        c.volume = Some(Track::new(MIN_DB, m.volume_db, t, fade_vol, m.curve));
    }
}

#[cfg(test)]
mod tests;
