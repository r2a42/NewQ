//! The editor's body: the ruler with its markers, the track headers, the
//! tracks with their clips, the playhead and the scroll bar; and the drags
//! (clips, their edges, the playhead, markers), shown as they go and asked for
//! when they are let go.

use super::ruler;
use super::*;
use crate::show::{CueKind, RunOut};
use crate::timeline::model::{Clip, LeadIn};
use crate::timeline::snap::{self, SnapPoints};
use egui::{Color32, FontId, Rect, Sense, Stroke, pos2, vec2};

/// What one frame of the editor works from.
pub(in crate::ui) struct Ctx<'a> {
    pub show: &'a Show,
    pub key: TimelineKey,
    pub host: Host,
    pub clips: &'a [Clip],
    pub tracks: &'a [TrackId],
    pub run: &'a RunView,
    /// Where the playhead is (the run's, or a group playing the old way).
    pub pos: f64,
    /// Clips the run has let run free of its clock, and those waiting to fade in.
    pub detached: Vec<CueId>,
    pub waiting: Vec<CueId>,
    /// Edits may be made (not show mode, no test running).
    pub editable: bool,
    /// The ruler can't move the playhead (show mode, see `editor`).
    pub ruler_locked: bool,
}

/// The track headers' width.
pub(super) const HEAD_W: f32 = 124.0;
pub(super) const RULER_H: f32 = 24.0;
const ROW_H: f32 = 38.0;
const CUES_ROW_H: f32 = 28.0;
const SCROLL_H: f32 = 9.0;
/// How wide the grip at each end of a clip is.
const EDGE_W: f32 = 6.0;
const MIN_PPS: f32 = 0.05;
const MAX_PPS: f32 = 4000.0;
/// How far past its start a clip of unknown or no end is drawn, at least.
const OPEN_TAIL: f64 = 4.0;
/// The shortest a trim leaves a clip.
const MIN_LEN: f64 = 0.05;
/// How near, in points, a dragged time snaps.
const SNAP_PX: f32 = 8.0;

pub(super) const PLAYHEAD: Color32 = Color32::from_rgb(235, 85, 70);
pub(super) const MARKER: Color32 = Color32::from_rgb(232, 184, 48);
const WARN: Color32 = Color32::from_rgb(240, 175, 60);
/// The hatching of a clip the run is waiting on to fade in.
const WAITING: Color32 = Color32::from_rgba_premultiplied(120, 190, 255, 200);

/// Screen x of a time and back.
#[derive(Clone, Copy, Debug)]
pub(super) struct Geo {
    /// Where time 0 is (scroll taken into account).
    pub x0: f32,
    pub pps: f32,
}

impl Geo {
    pub fn x(&self, t: f64) -> f32 {
        self.x0 + t as f32 * self.pps
    }

    pub fn t(&self, x: f32) -> f64 {
        f64::from((x - self.x0) / self.pps.max(1e-6))
    }
}

/// How many seconds there is to show: past the last clip, marker and the playhead.
fn span(ctx: &Ctx) -> f64 {
    let ends = ctx
        .clips
        .iter()
        .map(|c| c.start + c.len.unwrap_or(OPEN_TAIL).max(0.0));
    let marks = model::settings(ctx.show, ctx.key)
        .into_iter()
        .flat_map(|s| s.markers.iter().map(|m| m.time));
    let end = ends
        .chain(marks)
        .chain([ctx.pos])
        .filter(|t| t.is_finite())
        .fold(0.0, f64::max);
    (end * 1.1).max(10.0)
}

/// A time rounded to a hundredth, as the inspector shows it.
fn hundredth(t: f64) -> f64 {
    (t * 100.0).round() / 100.0
}

/// The colour of a clip, by its kind (a pause cue stands out).
fn kind_color(show: &Show, c: &Clip) -> Color32 {
    let pause = show.cue(c.id).is_some_and(|q| {
        matches!(&q.kind, CueKind::Control(k)
            if matches!(k.action, ControlAction::TimelinePause | ControlAction::Pause))
    });
    if pause {
        return Color32::from_rgb(196, 140, 30);
    }
    match c.ty {
        CueType::Video => Color32::from_rgb(63, 111, 181),
        CueType::Image => Color32::from_rgb(122, 92, 192),
        CueType::Audio | CueType::LiveAudio => Color32::from_rgb(47, 154, 122),
        CueType::Text | CueType::Clock | CueType::Timer => Color32::from_rgb(176, 120, 42),
        CueType::Shape => Color32::from_rgb(150, 98, 70),
        CueType::Camera => Color32::from_rgb(176, 74, 106),
        CueType::Fade | CueType::Stop => Color32::from_rgb(92, 104, 130),
        _ => Color32::from_rgb(107, 111, 120),
    }
}

/// Clips of these tracks can be dragged between them.
fn same_kind(a: TrackId, b: TrackId) -> bool {
    matches!(
        (a, b),
        (TrackId::Picture { .. }, TrackId::Picture { .. })
            | (TrackId::Sound { .. }, TrackId::Sound { .. })
    )
}

/// Diagonal lines over `rect`, for the parts of a clip that aren't playing
/// its content.
fn hatch(painter: &egui::Painter, rect: Rect, color: Color32) {
    let p = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let h = rect.height();
    let mut x = rect.left() - h;
    while x < rect.right() {
        p.line_segment(
            [pos2(x, rect.bottom()), pos2(x + h, rect.top())],
            Stroke::new(1.0, color),
        );
        x += 7.0;
    }
}

/// The lead-in of a clip in seconds, and whether it waits for a fade cue that
/// isn't there.
fn lead_of(ctx: &Ctx, c: &Clip) -> (f64, bool) {
    match c.lead_in {
        LeadIn::None => (0.0, false),
        LeadIn::At(v) => (v, false),
        LeadIn::UntilFaded => match ctx
            .show
            .cue(c.id)
            .and_then(|q| model::fade_wake(ctx.show, ctx.key, q))
        {
            Some(v) => (v, false),
            None => (0.0, true),
        },
    }
}

/// A clip whose end the clock keeps, left to end when its content runs out:
/// it fades out if the pause in it lasts longer than it does.
pub(super) fn fades_if_held(show: &Show, c: &Clip) -> bool {
    c.clocked
        && !c.loops
        && show
            .cue(c.id)
            .and_then(|q| q.kind.media())
            .is_some_and(|m| m.run_out == RunOut::End)
}

/// Where each track's row is, top to bottom, scrolled.
fn rows(ctx: &Ctx, lanes: Rect, scroll_y: f32) -> Vec<(TrackId, Rect)> {
    let mut y = lanes.top() - scroll_y;
    ctx.tracks
        .iter()
        .map(|&t| {
            let h = if t == TrackId::Cues {
                CUES_ROW_H
            } else {
                ROW_H
            };
            let r = Rect::from_min_max(pos2(lanes.left(), y), pos2(lanes.right(), y + h));
            y += h;
            (t, r)
        })
        .collect()
}

fn rows_height(ctx: &Ctx) -> f32 {
    ctx.tracks
        .iter()
        .map(|t| {
            if *t == TrackId::Cues {
                CUES_ROW_H
            } else {
                ROW_H
            }
        })
        .sum()
}

/// The ruler, the tracks, the clips and the playhead, in what is left of `ui`.
pub(super) fn draw(
    ui: &mut Ui,
    ctx: &Ctx,
    engine: &mut Engine,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) {
    let area = ui.available_rect_before_wrap();
    let area = Rect::from_min_size(
        area.min,
        vec2(
            area.width().max(HEAD_W + 120.0),
            area.height().max(RULER_H + CUES_ROW_H + SCROLL_H),
        ),
    );
    ui.allocate_rect(area, Sense::hover());
    let ruler_rect = Rect::from_min_max(
        pos2(area.left() + HEAD_W, area.top()),
        pos2(area.right(), area.top() + RULER_H),
    );
    let lanes = Rect::from_min_max(
        pos2(area.left() + HEAD_W, ruler_rect.bottom()),
        pos2(area.right(), area.bottom() - SCROLL_H),
    );
    let heads = Rect::from_min_max(
        pos2(area.left(), lanes.top()),
        pos2(lanes.left(), lanes.bottom()),
    );
    let base = ui.id().with("tracks");
    let span = span(ctx);
    let lane_w = lanes.width().max(1.0);

    // Zoom and scroll.
    let ed = &mut st.timeline;
    let fit = lane_w / span as f32;
    let mut pps = if ed.pps > 0.0 { ed.pps } else { fit };
    let hovered = ui.rect_contains_pointer(area);
    if hovered {
        let (zoom, scroll, pointer) =
            ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta, i.pointer.hover_pos()));
        if (zoom - 1.0).abs() > 1e-4 {
            // The time under the pointer stays where it is.
            let px = pointer.map_or(lanes.center().x, |p| p.x.max(lanes.left()));
            let t = (px - lanes.left() + ed.scroll.x) / pps;
            let new = (pps * zoom).clamp(MIN_PPS, MAX_PPS);
            ed.scroll.x = t * new - (px - lanes.left());
            ed.pps = new;
            pps = new;
        } else {
            ed.scroll.x -= scroll.x;
            ed.scroll.y -= scroll.y;
        }
    }
    let content_w = (span as f32 * pps).max(lane_w);
    ed.scroll.x = ed.scroll.x.max(0.0).min((content_w - lane_w).max(0.0));
    if ed.pps <= 0.0 {
        ed.scroll.x = 0.0;
    }
    ed.scroll.y = ed
        .scroll
        .y
        .max(0.0)
        .min((rows_height(ctx) - lanes.height()).max(0.0));
    ed.shown_pps = pps;
    let geo = Geo {
        x0: lanes.left() - ed.scroll.x,
        pps,
    };
    let rows = rows(ctx, lanes, ed.scroll.y);

    let painter = ui.painter_at(area);
    let visuals = ui.visuals().clone();
    let lane_painter = painter.with_clip_rect(lanes);
    // Rows and the time grid.
    for (i, (_, r)) in rows.iter().enumerate() {
        let shade = if i % 2 == 0 { 0.04 } else { 0.0 };
        lane_painter.rect_filled(
            *r,
            0.0,
            visuals
                .extreme_bg_color
                .lerp_to_gamma(Color32::WHITE, shade),
        );
        lane_painter.hline(
            lanes.x_range(),
            r.bottom(),
            Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
        );
    }
    let ticks = ruler::ticks(geo, lanes.left(), lanes.right());
    for (t, major) in &ticks {
        if *major {
            lane_painter.vline(
                geo.x(*t),
                lanes.y_range(),
                Stroke::new(0.5, visuals.weak_text_color().gamma_multiply(0.25)),
            );
        }
    }

    let picked = st.selection(ctx.show);
    let snapping = st.timeline.snap && !ui.input(|i| i.modifiers.alt);
    let markers: &[crate::timeline::Marker] =
        model::settings(ctx.show, ctx.key).map_or(&[], |s| &s.markers);
    #[cfg(test)]
    let mut clip_rects = Vec::new();

    for c in ctx.clips {
        let drag = st.timeline.drag_in(ctx.host).cloned();
        let (mut start, mut len, mut track) = (c.start, c.len, c.track);
        match &drag {
            Some(Drag::Clips { ids, dt, to, .. }) if ids.contains(&c.id) => {
                start += dt;
                if ids.first() == Some(&c.id)
                    && let Some(t) = to
                {
                    track = *t;
                }
            }
            Some(Drag::Trim { id, edge, to }) if *id == c.id => match edge {
                Edge::Left => {
                    len = len.map(|l| l + start - to);
                    start = *to;
                }
                Edge::Right => len = Some(to - start),
            },
            _ => {}
        }
        let Some((_, row)) = rows.iter().find(|(t, _)| *t == track) else {
            continue;
        };
        let left = geo.x(start);
        let right = match len {
            Some(l) => geo.x(start + l),
            None => geo.x(start + OPEN_TAIL).max(geo.x(span)),
        };
        let rect = Rect::from_min_max(
            pos2(left, row.top() + 3.0),
            pos2(right.max(left + 9.0), row.bottom() - 3.0),
        );
        #[cfg(test)]
        clip_rects.push((c.id, rect));
        let hit = rect.intersect(lanes);
        if hit.width() <= 0.0 || hit.height() <= 0.0 {
            continue;
        }
        let selected = picked.contains(&c.id);
        draw_clip(&lane_painter, ctx, c, rect, geo, start, selected);

        let sense = if ctx.editable {
            Sense::click_and_drag()
        } else {
            Sense::click()
        };
        let resp = ui.interact(hit, base.with(("clip", c.id.0)), sense);
        if resp.clicked() {
            let m = ui.input(|i| i.modifiers);
            actions.push(if m.command {
                Action::ToggleSelect(c.id)
            } else if m.shift {
                Action::SelectRange(c.id)
            } else {
                Action::Select(c.id)
            });
        }
        if ctx.editable && resp.drag_started() {
            // Dragging a clip of the selection takes the selection's clips with it.
            let ids = if selected {
                let mut ids = vec![c.id];
                ids.extend(
                    picked
                        .iter()
                        .copied()
                        .filter(|id| *id != c.id && ctx.clips.iter().any(|k| k.id == *id)),
                );
                ids
            } else {
                actions.push(Action::Select(c.id));
                vec![c.id]
            };
            let grab = ui
                .input(|i| i.pointer.press_origin())
                .map_or(c.start, |p| geo.t(p.x));
            st.timeline.drag = Some((
                ctx.host,
                Drag::Clips {
                    ids,
                    grab,
                    dt: 0.0,
                    to: None,
                },
            ));
        }
        let resp = resp.on_hover_text(clip_tip(ctx, c));
        resp.context_menu(|ui| clip_menu(ui, ctx, engine, c, actions));

        // The grips at its ends.
        if ctx.editable && rect.width() > EDGE_W * 3.0 {
            for (edge, allowed, x) in [
                (Edge::Left, c.trim_left, rect.left()),
                (Edge::Right, c.trim_right && c.len.is_some(), rect.right()),
            ] {
                if !allowed {
                    continue;
                }
                let grip =
                    Rect::from_center_size(pos2(x, rect.center().y), vec2(EDGE_W, rect.height()))
                        .intersect(lanes);
                if grip.width() <= 0.0 {
                    continue;
                }
                let r = ui
                    .interact(
                        grip,
                        base.with(("edge", c.id.0, edge == Edge::Left)),
                        Sense::drag(),
                    )
                    .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                if r.drag_started() {
                    let to = match edge {
                        Edge::Left => c.start,
                        Edge::Right => c.start + c.len.unwrap_or(0.0),
                    };
                    st.timeline.drag = Some((ctx.host, Drag::Trim { id: c.id, edge, to }));
                }
            }
        }
    }

    // Markers through the tracks.
    for m in markers {
        let t = match st.timeline.drag_in(ctx.host) {
            Some(Drag::Marker { id, to, .. }) if *id == m.id => *to,
            _ => m.time,
        };
        let x = geo.x(t);
        if lanes.x_range().contains(x) {
            let color = m.color.map_or(MARKER, |c| {
                let [r, g, b] = c.rgb();
                Color32::from_rgb(r, g, b)
            });
            lane_painter.add(egui::Shape::dashed_line(
                &[pos2(x, lanes.top()), pos2(x, lanes.bottom())],
                Stroke::new(1.0, color),
                4.0,
                3.0,
            ));
        }
    }

    // Track headers.
    let head_painter = painter.with_clip_rect(heads);
    head_painter.rect_filled(heads, 0.0, visuals.panel_fill);
    for (t, r) in &rows {
        let r = Rect::from_min_max(pos2(heads.left(), r.top()), pos2(heads.right(), r.bottom()));
        head_painter.hline(
            heads.x_range(),
            r.bottom(),
            Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
        );
        head_painter.text(
            pos2(r.left() + 8.0, r.center().y),
            egui::Align2::LEFT_CENTER,
            model::track_label(ctx.show, *t),
            FontId::proportional(12.0),
            visuals.text_color(),
        );
    }
    head_painter.vline(
        heads.right(),
        heads.y_range(),
        Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
    );

    // The ruler, its markers and the playhead.
    let marker_rects = ruler::draw(ui, ctx, st, geo, ruler_rect, &ticks, actions);
    let shown = match st.timeline.drag_in(ctx.host) {
        Some(Drag::Playhead { pos }) => *pos,
        _ => ctx.pos,
    };
    let px = geo.x(shown);
    let playhead_x = (lanes.x_range().contains(px)).then_some(px);
    if let Some(x) = playhead_x {
        painter.vline(
            x,
            ruler_rect.top()..=lanes.bottom(),
            Stroke::new(1.5, PLAYHEAD),
        );
        painter.add(egui::Shape::convex_polygon(
            vec![
                pos2(x - 5.0, ruler_rect.top()),
                pos2(x + 5.0, ruler_rect.top()),
                pos2(x, ruler_rect.top() + 7.0),
            ],
            PLAYHEAD,
            Stroke::NONE,
        ));
    }

    scroll_bar(ui, base, &mut st.timeline, lanes, area, content_w);
    // More tracks than fit (the wheel scrolls them): a thin bar says where.
    let all_h = rows_height(ctx);
    if all_h > lanes.height() + 0.5 {
        let h = (lanes.height() * lanes.height() / all_h).max(12.0);
        let top = lanes.top()
            + (lanes.height() - h) * st.timeline.scroll.y / (all_h - lanes.height()).max(1.0);
        let thumb = Rect::from_min_size(pos2(lanes.right() - 5.0, top), vec2(4.0, h));
        painter.rect_filled(thumb, 2.0, visuals.weak_text_color().gamma_multiply(0.8));
    }
    follow_drag(ui, ctx, st, geo, &rows, markers, snapping, actions);

    #[cfg(test)]
    ui.data_mut(|d| {
        d.insert_temp(
            egui::Id::new(("timeline_rects", ctx.host)),
            TimelineRects {
                ruler: ruler_rect,
                clips: clip_rects,
                tracks: rows.clone(),
                markers: marker_rects,
                playhead_x,
                x0: geo.x0,
                pps: geo.pps,
            },
        )
    });
    #[cfg(not(test))]
    let _ = marker_rects;
}

/// One clip: its colour, label and badges, fades, lead-in and warnings.
fn draw_clip(
    p: &egui::Painter,
    ctx: &Ctx,
    c: &Clip,
    rect: Rect,
    geo: Geo,
    start: f64,
    selected: bool,
) {
    let base = kind_color(ctx.show, c);
    let fill = if c.armed {
        base
    } else {
        base.gamma_multiply(0.35)
    };
    p.rect_filled(rect, 3.0, fill);
    if let Some(col) = c.color {
        let [r, g, b] = col.rgb();
        let stripe = Rect::from_min_size(rect.min, vec2(rect.width(), 3.0));
        p.rect_filled(stripe, 0.0, Color32::from_rgb(r, g, b));
    }
    let inner = p.with_clip_rect(rect.intersect(p.clip_rect()));
    // The lead-in: waiting, unseen, for its fade-in.
    let (lead, never) = lead_of(ctx, c);
    if never {
        hatch(&inner, rect, Color32::from_white_alpha(70));
    } else if lead > 0.0 {
        let r = Rect::from_min_max(
            rect.min,
            pos2(geo.x(start + lead).min(rect.right()), rect.bottom()),
        );
        inner.rect_filled(r, 0.0, Color32::from_black_alpha(70));
        hatch(&inner, r, Color32::from_white_alpha(60));
    }
    // The run has reached it and it is waiting, now, for its fade-in (the
    // engine's word, `timeline_waiting`): its lead-in, or all of it when it
    // waits for a fade cue, hatched brighter.
    if ctx.waiting.contains(&c.id) {
        let r = if lead > 0.0 && !never {
            Rect::from_min_max(
                rect.min,
                pos2(geo.x(start + lead).min(rect.right()), rect.bottom()),
            )
        } else {
            rect
        };
        hatch(&inner, r, WAITING);
        #[cfg(test)]
        p.ctx().data_mut(|d| {
            d.get_temp_mut_or_default::<Vec<CueId>>(egui::Id::new(("timeline_waiting", ctx.host)))
                .push(c.id)
        });
    }
    // The fades, as a ramp in each end.
    let content_left = if never {
        rect.left()
    } else {
        geo.x(start + lead)
    };
    let ramp = Stroke::new(1.0, Color32::from_white_alpha(170));
    if c.fade_in > 0.0 {
        let w = (c.fade_in as f32 * geo.pps).min(rect.width());
        inner.add(egui::Shape::convex_polygon(
            vec![
                pos2(content_left, rect.top()),
                pos2(content_left + w, rect.top()),
                pos2(content_left, rect.bottom()),
            ],
            Color32::from_black_alpha(80),
            Stroke::NONE,
        ));
        inner.line_segment(
            [
                pos2(content_left, rect.bottom()),
                pos2(content_left + w, rect.top()),
            ],
            ramp,
        );
    }
    if c.fade_out > 0.0 && c.len.is_some() {
        let w = (c.fade_out as f32 * geo.pps).min(rect.width());
        inner.add(egui::Shape::convex_polygon(
            vec![
                pos2(rect.right() - w, rect.top()),
                pos2(rect.right(), rect.top()),
                pos2(rect.right(), rect.bottom()),
            ],
            Color32::from_black_alpha(80),
            Stroke::NONE,
        ));
        inner.line_segment(
            [
                pos2(rect.right() - w, rect.top()),
                pos2(rect.right(), rect.bottom()),
            ],
            ramp,
        );
    }
    if ctx.detached.contains(&c.id) {
        hatch(
            &inner,
            rect,
            Color32::from_rgba_unmultiplied(255, 220, 120, 90),
        );
    }
    // Name, then the badges.
    let text = if c.armed {
        Color32::WHITE
    } else {
        Color32::from_white_alpha(140)
    };
    let mut name = c.name.clone();
    if c.ty == CueType::Control && name.is_empty() {
        name = "Control".into();
    }
    // A cue that takes no time (a pause, a control or stop cue) is a narrow
    // chip with its name beside it.
    let instant = c.len.is_some_and(|l| l <= 0.0);
    let name_rect = if instant {
        p.text(
            pos2(rect.right() + 4.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            name,
            FontId::proportional(11.0),
            text.gamma_multiply(0.85),
        )
    } else {
        inner.text(
            pos2(rect.left() + 5.0, rect.top() + 3.0),
            egui::Align2::LEFT_TOP,
            name,
            FontId::proportional(11.0),
            text,
        )
    };
    let mut notes: Vec<&str> = Vec::new();
    if ctx.waiting.contains(&c.id) {
        notes.push("waiting to fade in");
    } else if never {
        notes.push("waits for a fade cue");
    } else if lead > 0.0 {
        notes.push("waits to fade in");
    }
    if ctx.detached.contains(&c.id) {
        notes.push("free-running");
    }
    if !c.armed {
        notes.push("disarmed");
    }
    if !notes.is_empty() && rect.height() > 24.0 {
        inner.text(
            pos2(rect.left() + 5.0, rect.bottom() - 2.0),
            egui::Align2::LEFT_BOTTOM,
            notes.join(" · "),
            FontId::proportional(9.5),
            Color32::from_white_alpha(200),
        );
    }
    // Badges after the name (the clip's far end may be under the next one).
    let mut x = name_rect.right() + 6.0;
    if fades_if_held(ctx.show, c) {
        x = inner
            .text(
                pos2(x, rect.top() + 3.0),
                egui::Align2::LEFT_TOP,
                "⚠",
                FontId::proportional(11.0),
                WARN,
            )
            .right()
            + 4.0;
    }
    let mut badges: Vec<&str> = Vec::new();
    if c.loops {
        badges.push("🔁");
    }
    if c.speed_keys {
        badges.push("⏩");
    }
    if c.ty == CueType::Video && c.has_sound {
        badges.push("🔊");
    }
    // Open-ended, or its length isn't known yet.
    if c.len.is_none() {
        badges.push("∞");
    }
    let badges = badges.join(" ");
    if !badges.is_empty() {
        inner.text(
            pos2(x, rect.top() + 3.0),
            egui::Align2::LEFT_TOP,
            badges,
            FontId::proportional(11.0),
            text,
        );
    }
    let outline = if selected {
        Stroke::new(2.0, Color32::WHITE)
    } else {
        Stroke::new(1.0, Color32::from_black_alpha(120))
    };
    p.rect_stroke(rect, 3.0, outline, egui::StrokeKind::Inside);
}

/// The clip's tooltip: what it is, when, and what to look out for.
fn clip_tip(ctx: &Ctx, c: &Clip) -> String {
    let mut s = format!("{}\nStarts at {}", c.name, fmt_time(c.start));
    match c.len {
        Some(l) => s += &format!(", lasts {}", fmt_time(l)),
        None if ctx.show.cue(c.id).is_some_and(|q| q.kind.media().is_some()) => {
            s += ", runs until something ends it (or its length isn't known yet)"
        }
        None => {}
    }
    s += &format!("\nTrack: {}", model::track_label(ctx.show, c.track));
    let (lead, never) = lead_of(ctx, c);
    if never {
        s += "\nAt opacity 0 and no fade cue raises it: it waits for one and never starts.";
    } else if lead > 0.0 {
        s += &format!(
            "\nWaits, unseen, {} for its fade-in; its content starts then.",
            fmt_time(lead)
        );
    }
    if ctx.waiting.contains(&c.id) {
        s += "\nWaiting to fade in.";
    }
    if ctx.detached.contains(&c.id) {
        s += "\nFree-running: no longer in step with the timeline's clock.";
    }
    if fades_if_held(ctx.show, c) {
        s += "\n⚠ A pause lies inside this clip: it will fade out if the pause outlasts it. \
              Choose Loop or Hold under \u{201c}If it runs out\u{201d} in the inspector to keep it up.";
    }
    s += "\nDrag to move; drag an end to trim; right-click for more.";
    s
}

/// A clip's right-click menu.
fn clip_menu(ui: &mut Ui, ctx: &Ctx, engine: &Engine, c: &Clip, actions: &mut Vec<Action>) {
    let key = ctx.key;
    let media = ctx.show.cue(c.id).is_some_and(|q| q.kind.media().is_some());
    ui.add_enabled_ui(ctx.editable, |ui| {
        let r = ui
            .button("Pause before this clip comes in")
            .on_hover_text("A pause cue at its start: GO brings it in");
        super::toolbar::note(ui, ctx.host, "pause_in", &r);
        if r.clicked() {
            actions.push(Action::Timeline(TimelineAction::AddPauseBefore {
                key,
                clip: c.id,
                what: PauseBefore::ClipIn,
            }));
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
            actions.push(Action::Timeline(TimelineAction::AddPauseBefore {
                key,
                clip: c.id,
                what: PauseBefore::ClipFadeOut,
            }));
            ui.close();
        }
    });
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
            actions.push(Action::Timeline(TimelineAction::FreezeClip(c.id, true)));
            ui.close();
        }
        if ui.button("Resume clip").clicked() {
            actions.push(Action::Timeline(TimelineAction::FreezeClip(c.id, false)));
            ui.close();
        }
    });
}

/// The bar under the tracks that scrolls them sideways.
fn scroll_bar(
    ui: &mut Ui,
    base: egui::Id,
    ed: &mut EditorState,
    lanes: Rect,
    area: Rect,
    content_w: f32,
) {
    let lane_w = lanes.width().max(1.0);
    if content_w <= lane_w + 0.5 {
        return;
    }
    let track = Rect::from_min_max(
        pos2(lanes.left(), lanes.bottom() + 1.0),
        pos2(lanes.right(), area.bottom()),
    );
    let w = (lane_w * lane_w / content_w).max(20.0);
    let travel = (track.width() - w).max(1.0);
    let max_scroll = (content_w - lane_w).max(1.0);
    let x = track.left() + travel * (ed.scroll.x / max_scroll);
    let thumb = Rect::from_min_size(pos2(x, track.top()), vec2(w, track.height()));
    let r = ui.interact(thumb, base.with("hscroll"), Sense::drag());
    if r.dragged() {
        ed.scroll.x = (ed.scroll.x + r.drag_delta().x * max_scroll / travel)
            .max(0.0)
            .min(max_scroll);
    }
    let visuals = ui.visuals();
    let p = ui.painter();
    p.rect_filled(track, 3.0, visuals.extreme_bg_color);
    let fill = if r.hovered() || r.dragged() {
        visuals.widgets.hovered.bg_fill
    } else {
        visuals.widgets.inactive.bg_fill
    };
    p.rect_filled(thumb, 3.0, fill);
}

/// The points a drag snaps to (none with snapping off or Alt held).
fn points(
    ctx: &Ctx,
    markers: &[crate::timeline::Marker],
    moving: &[CueId],
    on: bool,
) -> SnapPoints {
    if on {
        snap::points(ctx.clips, markers, Some(ctx.pos), moving)
    } else {
        SnapPoints::default()
    }
}

/// `t` snapped, or else to a hundredth.
fn snapped(t: f64, pts: &SnapPoints, tolerance: f64) -> f64 {
    match snap::snap(t, pts, tolerance) {
        (s, Some(_)) => s,
        _ => hundredth(t),
    }
}

/// Keep the drag in progress under the pointer, and ask for what it did when
/// the button is let go.
#[allow(clippy::too_many_arguments)]
fn follow_drag(
    ui: &Ui,
    ctx: &Ctx,
    st: &mut UiState,
    geo: Geo,
    rows: &[(TrackId, Rect)],
    markers: &[crate::timeline::Marker],
    snapping: bool,
    actions: &mut Vec<Action>,
) {
    let Some((host, drag)) = &mut st.timeline.drag else {
        return;
    };
    if *host != ctx.host {
        return;
    }
    let (pointer, down, released) = ui.input(|i| {
        (
            i.pointer.interact_pos(),
            i.pointer.primary_down(),
            i.pointer.primary_released(),
        )
    });
    let tolerance = f64::from(SNAP_PX / geo.pps.max(1e-6));
    let clip = |id: CueId| ctx.clips.iter().find(|c| c.id == id);
    if let Some(p) = pointer {
        let t = geo.t(p.x);
        match drag {
            Drag::Clips { ids, grab, dt, to } => {
                if let Some(anchor) = ids.first().and_then(|&id| clip(id)) {
                    let pts = points(ctx, markers, ids, snapping);
                    let raw = anchor.start + (t - *grab);
                    let start = match snap::snap_span(raw, anchor.len, &pts, tolerance) {
                        (s, Some(_)) => s,
                        _ => hundredth(raw),
                    };
                    // None starts before the timeline does.
                    let first = ids
                        .iter()
                        .filter_map(|&id| clip(id))
                        .map(|c| c.start)
                        .fold(f64::INFINITY, f64::min);
                    *dt = (start - anchor.start).max(-first.min(anchor.start));
                    *to = rows
                        .iter()
                        .find(|(_, r)| r.y_range().contains(p.y))
                        .map(|(t, _)| *t)
                        .filter(|t| *t != anchor.track && same_kind(*t, anchor.track));
                }
            }
            Drag::Trim { id, edge, to } => {
                if let Some(c) = clip(*id) {
                    let pts = points(ctx, markers, &[*id], snapping);
                    let s = snapped(t, &pts, tolerance);
                    *to = match edge {
                        Edge::Left => {
                            let end = c.len.map_or(f64::INFINITY, |l| c.start + l);
                            s.max(0.0).min((end - MIN_LEN).max(0.0))
                        }
                        Edge::Right => s.max(c.start + MIN_LEN),
                    };
                }
            }
            Drag::Playhead { pos } => {
                let pts = points(ctx, markers, &[], snapping);
                *pos = snapped(t.max(0.0), &pts, tolerance).max(0.0);
            }
            Drag::Marker { id, to, grab } => {
                let others: Vec<_> = markers.iter().filter(|m| m.id != *id).cloned().collect();
                let pts = points(ctx, &others, &[], snapping);
                *to = snapped((t - *grab).max(0.0), &pts, tolerance).max(0.0);
            }
        }
    }
    if !(released || !down) {
        return;
    }
    let key = ctx.key;
    let done = match drag.clone() {
        Drag::Clips { ids, dt, to, .. } => (ctx.editable && (dt.abs() > 1e-9 || to.is_some()))
            .then_some(TimelineAction::MoveClips { key, ids, dt, to }),
        Drag::Trim { id, edge, to } => clip(id)
            .filter(|c| {
                let was = match edge {
                    Edge::Left => c.start,
                    Edge::Right => c.start + c.len.unwrap_or(0.0),
                };
                ctx.editable && (to - was).abs() > 1e-9
            })
            .map(|_| TimelineAction::Trim { id, edge, to }),
        Drag::Playhead { pos } => (!ctx.ruler_locked).then_some(TimelineAction::Seek { key, pos }),
        Drag::Marker { id, to, .. } => markers
            .iter()
            .find(|m| m.id == id)
            .filter(|m| ctx.editable && (m.time - to).abs() > 1e-9)
            .map(|_| TimelineAction::MoveMarker { key, id, to }),
    };
    st.timeline.drag = None;
    if let Some(a) = done {
        actions.push(Action::Timeline(a));
    }
}
