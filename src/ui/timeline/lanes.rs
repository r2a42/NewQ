//! The keyframe lanes under a clip: its speed and its opacity, each a line
//! across the clip's width with a diamond for each key. Double-click adds a
//! key, a drag moves one (shown as it goes, asked for on release) and a
//! right-click takes one out; the last going leaves a steady value.
//!
//! Both lanes are drawn on the timeline's own time axis, a key at `t` at the
//! clip's start plus `t`. What follows what differs (ruling L2): speed keys
//! follow the clip's content, which plays on through a free-run pause, while
//! opacity keys follow the timeline's clock and stand still at a pause.

use super::tracks::{Ctx, Geo};
use super::*;
use crate::show::{Cue, MAX_RATE, MIN_RATE};
use crate::timeline::model::Clip;
use egui::{Color32, FontId, Pos2, Rect, Sense, Stroke, pos2};

/// How tall a lane is.
pub(super) const LANE_H: f32 = 44.0;
/// How near, in points, the pointer must be to take a key.
const GRAB: f32 = 8.0;
/// The lanes' line and keys.
const SPEED: Color32 = Color32::from_rgb(120, 200, 255);
const OPACITY: Color32 = Color32::from_rgb(240, 200, 90);

impl Lane {
    pub(super) fn label(self) -> &'static str {
        match self {
            Lane::Speed => "Speed",
            Lane::Opacity => "Opacity",
        }
    }

    fn color(self) -> Color32 {
        match self {
            Lane::Speed => SPEED,
            Lane::Opacity => OPACITY,
        }
    }

    /// Where a value sits in the lane, 0 (bottom) to 1 (top): speed on a
    /// doubling scale with normal speed in the middle, opacity straight.
    fn frac(self, v: f32) -> f32 {
        let f = match self {
            Lane::Speed => {
                let lo = (MIN_RATE as f32).log2();
                let hi = (MAX_RATE as f32).log2();
                (v.max(MIN_RATE as f32).log2() - lo) / (hi - lo)
            }
            Lane::Opacity => v,
        };
        if f.is_finite() {
            f.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// The value at `f` up the lane, rounded as the inspector shows it.
    pub(super) fn value(self, f: f32) -> f32 {
        let f = if f.is_finite() {
            f.clamp(0.0, 1.0)
        } else {
            0.5
        };
        match self {
            Lane::Speed => {
                let lo = (MIN_RATE as f32).log2();
                let hi = (MAX_RATE as f32).log2();
                let v = (lo + f * (hi - lo)).exp2();
                // Snap to normal speed near the middle.
                if (v - 1.0).abs() < 0.04 {
                    1.0
                } else {
                    (v * 100.0).round() / 100.0
                }
            }
            // Snap to unseen and full near the ends: a key a pixel off 0
            // would end a lead-in (anything above 0 is seen).
            Lane::Opacity if f < 0.05 => 0.0,
            Lane::Opacity if f > 0.95 => 1.0,
            Lane::Opacity => (f * 100.0).round() / 100.0,
        }
    }

    fn text(self, v: f32) -> String {
        match self {
            Lane::Speed => format!("{v:.2}×"),
            Lane::Opacity => format!("{:.0}%", v * 100.0),
        }
    }
}

/// Whether a cue has a lane of this kind: speed for a video or sound file,
/// opacity for a picture.
pub(super) fn has(cue: &Cue, lane: Lane) -> bool {
    match lane {
        Lane::Speed => matches!(cue.kind, CueKind::Video(_) | CueKind::Audio(_)),
        Lane::Opacity => model::is_picture(&cue.kind),
    }
}

/// The keys a lane edits, as the show has them (a steady value: no keys).
pub(super) fn keys_of(cue: &Cue, lane: Lane) -> Option<Animated> {
    if !has(cue, lane) {
        return None;
    }
    let m = cue.kind.media()?;
    Some(match lane {
        Lane::Speed => m.rate.clone(),
        Lane::Opacity => m
            .motion
            .opacity
            .clone()
            .unwrap_or_else(|| Animated::new(m.opacity)),
    })
}

/// Put a lane's keys in the show (the edit `apply` makes).
fn put(show: &mut Show, id: CueId, lane: Lane, keys: Animated) -> bool {
    match lane {
        Lane::Speed => edit::set_rate(show, id, keys),
        Lane::Opacity => edit::set_opacity(show, id, keys),
    }
}

/// What to ask for to give a clip these keys.
pub(super) fn ask(id: CueId, lane: Lane, keys: Animated) -> TimelineAction {
    match lane {
        Lane::Speed => TimelineAction::SetRate { id, rate: keys },
        Lane::Opacity => TimelineAction::SetOpacity { id, opacity: keys },
    }
}

/// While a lane's key is dragged: the show with the keys as they are now, so
/// the editor draws the clip's length and lead-in as they will be.
pub(super) fn preview(show: &Show, drag: Option<&Drag>) -> Option<Show> {
    let Some(Drag::Key { id, lane, keys, .. }) = drag else {
        return None;
    };
    let mut copy = show.clone();
    put(&mut copy, *id, *lane, keys.clone()).then_some(copy)
}

/// Keys with `i` taken out; the last going leaves the value it had, steady.
fn without(a: &Animated, i: usize) -> Animated {
    let mut a = a.clone();
    super::super::remove_key(&mut a, i);
    a
}

/// What the lane's header says: the value at the playhead, then (smaller)
/// for speed the clip's length that comes of it, and the clip's name.
pub(super) fn readout(ctx: &Ctx, c: &Clip, lane: Lane) -> (String, String) {
    let Some(a) = ctx.show.cue(c.id).and_then(|q| keys_of(q, lane)) else {
        return (lane.label().into(), c.name.clone());
    };
    let t = ctx.pos - c.start;
    let inside = t >= 0.0 && c.len.is_none_or(|l| t <= l);
    let now = if inside { a.at(t) } else { a.at(0.0) };
    let value = format!("{} {}", lane.label(), lane.text(now));
    let more = match (lane, c.len) {
        (Lane::Speed, Some(l)) => format!("{} · {}", fmt_time(l), c.name),
        _ => c.name.clone(),
    };
    (value, more)
}

/// What the lane's header says on hover.
pub(super) fn tip(lane: Lane) -> &'static str {
    match lane {
        Lane::Speed => {
            "The clip's playback speed, from a quarter to four times (normal in the middle). \
             Speed keys follow the clip's content: through a free-run pause the content plays \
             on, and so do they. The clip's length follows the speed. Double-click to add a \
             key, drag one to move it, right-click to take it out."
        }
        Lane::Opacity => {
            "The clip's opacity. Opacity keys follow the timeline's clock: at a pause they \
             stand still, and a ramp after a pause cue waits for GO. Keys at 0 before the \
             opacity rises are the clip's lead-in: its content starts as it begins to show. \
             Double-click to add a key, drag one to move it, right-click to take it out."
        }
    }
}

/// One lane, in `rect` (its row; drawn and used where it is inside `view`).
#[allow(clippy::too_many_arguments)]
pub(super) fn draw(
    ui: &mut Ui,
    ctx: &Ctx,
    st: &mut UiState,
    geo: Geo,
    rect: Rect,
    view: Rect,
    c: &Clip,
    lane: Lane,
    actions: &mut Vec<Action>,
    keys_drawn: &mut Vec<(CueId, Lane, Pos2)>,
) {
    let hit = rect.intersect(view);
    if hit.width() <= 0.0 || hit.height() <= 0.0 {
        return;
    }
    let Some(shown) = ctx.show.cue(c.id).and_then(|q| keys_of(q, lane)) else {
        return;
    };
    let p = ui.painter().with_clip_rect(hit);
    let inner = rect.shrink2(egui::vec2(0.0, 5.0));
    let y = |v: f32| inner.bottom() - lane.frac(v) * inner.height();
    let x = |t: f64| geo.x(c.start + t);
    let end = c
        .len
        .unwrap_or_else(|| geo.t(view.right()) - c.start)
        .max(0.0);
    let span = Rect::from_min_max(pos2(x(0.0), rect.top()), pos2(x(end), rect.bottom()));
    let visuals = ui.visuals().clone();
    p.rect_filled(
        span,
        2.0,
        visuals.extreme_bg_color.lerp_to_gamma(lane.color(), 0.10),
    );
    // Normal speed, and half and full opacity, as faint lines.
    let marks: &[f32] = match lane {
        Lane::Speed => &[1.0],
        Lane::Opacity => &[0.5, 1.0],
    };
    for &v in marks {
        p.hline(
            span.x_range(),
            y(v),
            Stroke::new(0.5, visuals.weak_text_color().gamma_multiply(0.5)),
        );
    }
    // The pointer: where it is in the lane's time and value.
    let at = |pt: Pos2| -> (f64, f32) {
        let t = (geo.t(pt.x) - c.start).max(0.0);
        let f = (inner.bottom() - pt.y) / inner.height().max(1.0);
        (t, lane.value(f))
    };
    let near = |a: &Animated, pt: Pos2| {
        a.keys
            .iter()
            .enumerate()
            .map(|(i, k)| (i, pos2(x(k.t), y(k.v)).distance(pt)))
            .filter(|(_, d)| *d <= GRAB)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };

    // A key being dragged follows the pointer (time snapped as clips are).
    let id = ui.id().with(("lane", c.id.0, lane));
    let sense = if ctx.editable {
        Sense::click_and_drag()
    } else {
        Sense::hover()
    };
    let resp = ui.interact(hit, id, sense).on_hover_text(tip(lane));
    let pointer = ui.input(|i| i.pointer.interact_pos());
    if let Some(Drag::Key {
        id: kid,
        lane: kl,
        keys,
        i,
        grab,
        v0,
    }) = st
        .timeline
        .drag
        .as_mut()
        .filter(|d| d.0 == ctx.host)
        .map(|d| &mut d.1)
        && *kid == c.id
        && *kl == lane
        && let Some(pt) = pointer
    {
        let (t, v) = at(pt);
        // Moved along the lane, not up or down it: the value stays.
        let v = if (pt.y - grab.y).abs() < 6.0 { *v0 } else { v };
        let markers = model::settings(ctx.show, ctx.key).map_or(&[][..], |s| &s.markers[..]);
        let snapping = st.timeline.snap && !ui.input(|i| i.modifiers.alt);
        let pts = super::tracks::points(ctx, markers, &[], snapping);
        let tolerance = f64::from(8.0 / geo.pps.max(1e-6));
        let t = (super::tracks::snapped(c.start + t, &pts, tolerance) - c.start).max(0.0);
        *i = keys.move_key(*i, t, v);
    }
    let a = match st.timeline.drag_in(ctx.host) {
        Some(Drag::Key {
            id: kid,
            lane: kl,
            keys,
            ..
        }) if *kid == c.id && *kl == lane => keys.clone(),
        _ => shown.clone(),
    };
    if ctx.editable {
        if resp.drag_started()
            && let Some(pt) = ui.input(|i| i.pointer.press_origin())
            && let Some(i) = near(&a, pt)
        {
            st.timeline.drag = Some((
                ctx.host,
                Drag::Key {
                    id: c.id,
                    lane,
                    keys: a.clone(),
                    i,
                    grab: pt,
                    v0: a.keys[i].v,
                },
            ));
        }
        if resp.double_clicked()
            && let Some(pt) = resp.interact_pointer_pos()
            && near(&a, pt).is_none()
        {
            let (t, v) = at(pt);
            let mut keys = a.clone();
            // A steady value becomes keys: the value it had, from 0.
            if !keys.is_animated() {
                keys.set_key(0.0, keys.value);
            }
            keys.set_key(t, v);
            actions.push(Action::Timeline(ask(c.id, lane, keys)));
        }
        if resp.secondary_clicked()
            && let Some(pt) = resp.interact_pointer_pos()
            && let Some(i) = near(&a, pt)
        {
            actions.push(Action::Timeline(ask(c.id, lane, without(&a, i))));
        }
    }

    // The line the keys make, and the keys.
    let color = lane.color();
    let line: Vec<Pos2> = if a.is_animated() {
        let n = ((span.width() / 3.0) as usize).clamp(8, 400);
        (0..=n)
            .map(|k| {
                let t = end * k as f64 / n as f64;
                pos2(x(t), y(a.at(t)))
            })
            .collect()
    } else {
        vec![
            pos2(span.left(), y(a.value)),
            pos2(span.right(), y(a.value)),
        ]
    };
    p.add(egui::Shape::line(line, Stroke::new(1.5, color)));
    let hover = resp.hover_pos();
    let mut label: Option<(usize, Pos2, String)> = None;
    for (i, k) in a.keys.iter().enumerate() {
        let ctr = pos2(x(k.t), y(k.v));
        keys_drawn.push((c.id, lane, ctr));
        let held = matches!(st.timeline.drag_in(ctx.host),
            Some(Drag::Key { id: kid, lane: kl, i: ki, .. }) if *kid == c.id && *kl == lane && *ki == i);
        let hot = held || hover.is_some_and(|h| h.distance(ctr) <= GRAB);
        let r = if hot { 6.0 } else { 4.5 };
        p.add(egui::Shape::convex_polygon(
            vec![
                pos2(ctr.x, ctr.y - r),
                pos2(ctr.x + r, ctr.y),
                pos2(ctr.x, ctr.y + r),
                pos2(ctr.x - r, ctr.y),
            ],
            if held { color } else { Color32::from_gray(30) },
            Stroke::new(1.5, if hot { Color32::WHITE } else { color }),
        ));
        if held {
            label = Some((i, ctr, format!("{} · {}", fmt_time(k.t), lane.text(k.v))));
        }
    }
    // The held key's time and value, drawn over everything on a dark chip:
    // on the side of the lane away from the key, and on the side of the key
    // away from the next one when that one is close.
    if let Some((i, ctr, text)) = label {
        let galley = p.layout_no_wrap(text, FontId::proportional(10.0), Color32::WHITE);
        let size = galley.size();
        let y = if ctr.y > rect.center().y {
            rect.top() + 2.0
        } else {
            rect.bottom() - 2.0 - size.y
        };
        let next_close = a
            .keys
            .get(i + 1)
            .is_some_and(|n| x(n.t) - ctr.x < size.x + 24.0);
        let left = next_close && ctr.x - 10.0 - size.x >= rect.left();
        let x0 = if left {
            ctr.x - 10.0 - size.x
        } else {
            ctr.x + 10.0
        };
        let at = pos2(x0, y);
        p.rect_filled(
            Rect::from_min_size(at, size).expand(2.0),
            3.0,
            Color32::from_black_alpha(190),
        );
        p.galley(at, galley, Color32::WHITE);
    }
    if !a.is_animated() && span.width() > 120.0 {
        p.text(
            pos2(span.left() + 6.0, rect.top() + 2.0),
            egui::Align2::LEFT_TOP,
            format!("{} steady · double-click to add a key", lane.text(a.value)),
            FontId::proportional(10.0),
            visuals.weak_text_color(),
        );
    }
}
