//! The ruler: times along the top, the playhead's handle, and the markers as
//! flags (dragged, renamed, and a menu with "Add Pause cue here").

use super::tracks::{Ctx, Geo, LOOP, MARKER};
use super::*;
use egui::{Color32, FontId, Rect, Sense, Stroke, pos2, vec2};

/// A marker's flag stops this short of the next; with no room for a name it
/// is a tab this wide (or narrower, to stay clear of the next).
const FLAG_GAP: f32 = 3.0;
const FLAG_MIN_W: f32 = 8.0;

/// The times to mark between screen x `left` and `right`: (time, a labelled one).
pub(super) fn ticks(geo: Geo, left: f32, right: f32) -> Vec<(f64, bool)> {
    let (from, to) = (geo.t(left).max(0.0), geo.t(right));
    let width = f64::from((right - left).max(1.0));
    if to.is_nan() || to <= from {
        return Vec::new();
    }
    let major = crate::ui::timeline_step(to - from, width / 90.0);
    let minor = major / 5.0;
    let mut out = Vec::new();
    let mut i = (from / minor).floor() as i64;
    loop {
        let t = i as f64 * minor;
        if t > to || out.len() > 2000 {
            break;
        }
        if t >= 0.0 {
            out.push((t, i % 5 == 0));
        }
        i += 1;
    }
    out
}

/// Draw the ruler in `rect` and let it move the playhead; returns where the
/// marker flags are.
pub(super) fn draw(
    ui: &mut Ui,
    ctx: &Ctx,
    st: &mut UiState,
    geo: Geo,
    rect: Rect,
    ticks: &[(f64, bool)],
    actions: &mut Vec<Action>,
) -> Vec<(u64, Rect)> {
    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, visuals.faint_bg_color);
    // Each looped section, under the ticks: shaded, with a bracket along the
    // bottom from where it starts to its loop pause.
    for (_id, from, to) in super::tracks::loop_sections(ctx) {
        let (a, b) = (geo.x(from), geo.x(to));
        let band = Rect::from_x_y_ranges(a..=b, rect.y_range());
        painter.rect_filled(band, 0.0, LOOP.gamma_multiply(0.14));
        let y = rect.bottom() - 2.0;
        painter.hline(a..=b, y, Stroke::new(2.0, LOOP));
        painter.vline(a, (y - 6.0)..=y, Stroke::new(2.0, LOOP));
        painter.vline(b, (y - 6.0)..=y, Stroke::new(2.0, LOOP));
        #[cfg(test)]
        ui.data_mut(|d| {
            d.get_temp_mut_or_default::<Vec<(CueId, Rect)>>(egui::Id::new((
                "timeline_loop_ruler",
                ctx.host,
            )))
            .push((_id, band))
        });
    }
    let weak = visuals.weak_text_color();
    for &(t, major) in ticks {
        let x = geo.x(t);
        let h = if major { 9.0 } else { 4.0 };
        painter.vline(
            x,
            (rect.bottom() - h)..=rect.bottom(),
            Stroke::new(1.0, weak.gamma_multiply(0.6)),
        );
        if major {
            painter.text(
                pos2(x + 3.0, rect.bottom() - 10.0),
                egui::Align2::LEFT_BOTTOM,
                fmt_time(t),
                FontId::monospace(10.0),
                weak,
            );
        }
    }
    painter.hline(
        rect.x_range(),
        rect.bottom(),
        Stroke::new(1.0, visuals.widgets.noninteractive.bg_stroke.color),
    );

    let host = ctx.host;
    let key = ctx.key;
    let resp = ui.interact(rect, ui.id().with("ruler"), Sense::click_and_drag());
    if ctx.ruler_locked {
        resp.on_hover_text(
            "Show mode: a timeline group's playhead can't be moved from here (it would put \
             its picture on the outputs). Run, Pause and Stop still work.",
        );
    } else {
        let resp = resp.on_hover_text(
            "Click or drag to move the playhead; where you let go, the timeline is parked \
             with its picture on the outputs.",
        );
        if resp.is_pointer_button_down_on() && st.timeline.drag_in(host).is_none() {
            let pos = ui
                .input(|i| i.pointer.interact_pos())
                .map_or(ctx.pos, |p| geo.t(p.x).max(0.0));
            st.timeline.drag = Some((
                host,
                Drag::Playhead {
                    pos: Some(pos),
                    mark: ctx.run.mark,
                },
            ));
        } else if resp.clicked()
            && st.timeline.drag_in(host).is_none()
            && let Some(p) = resp.interact_pointer_pos()
        {
            // Pressed and let go within one frame (a quick tap, or a busy
            // frame): no drag was seen, so seek here and now.
            actions.push(Action::Timeline(TimelineAction::Seek {
                key,
                pos: geo.t(p.x).max(0.0),
            }));
        }
    }

    // The markers' flags, on top of the ruler.
    let markers = model::settings(ctx.show, key).map_or(&[][..], |s| &s.markers[..]);
    let mut rects = Vec::new();
    let at = |m: &crate::timeline::Marker| match st.timeline.drag_in(host) {
        Some(Drag::Marker { id, to, .. }) if *id == m.id => *to,
        _ => m.time,
    };
    let xs: Vec<f32> = markers.iter().map(|m| geo.x(at(m))).collect();
    // The flag under the pointer is drawn again, whole, over its neighbours.
    let mut whole: Option<(Rect, Color32, String)> = None;
    for (i, m) in markers.iter().enumerate() {
        let x = xs[i];
        let color = m.color.map_or(MARKER, |c| {
            let [r, g, b] = c.rgb();
            Color32::from_rgb(r, g, b)
        });
        // As a cue's name on the Cues track: up to the next flag, cut short
        // with "…", or no name at all where there is no room for one.
        let next = xs
            .iter()
            .filter(|o| **o > x + 0.01)
            .fold(f32::INFINITY, |a, b| a.min(*b));
        let room = (next - FLAG_GAP).min(rect.right()) - x - 8.0;
        let galley =
            super::tracks::fitted(&painter, &m.name, 10.0, room, Color32::from_rgb(20, 20, 20));
        let w = galley
            .as_ref()
            .map_or(FLAG_MIN_W.min((next - x - 1.0).max(3.0)), |g| {
                g.size().x + 8.0
            });
        let flag = Rect::from_min_size(pos2(x, rect.top() + 1.0), vec2(w, 13.0));
        rects.push((m.id, flag));
        let hit = flag.intersect(rect);
        if hit.width() <= 0.0 {
            continue;
        }
        painter.rect_filled(flag, 2.0, color);
        if let Some(galley) = galley {
            painter.galley(pos2(flag.left() + 4.0, flag.top()), galley, Color32::BLACK);
        }
        let sense = if ctx.editable {
            Sense::click_and_drag()
        } else {
            Sense::click()
        };
        let r = ui
            .interact(hit, ui.id().with(("marker", m.id)), sense)
            .on_hover_text(format!(
                "{} at {}. Drag to move it; double-click to rename; right-click for more.",
                m.name,
                fmt_time(m.time)
            ));
        if r.hovered() || r.dragged() {
            whole = Some((flag, color, m.name.clone()));
        }
        if ctx.editable && r.drag_started() {
            // Where on the flag it was taken, so the marker doesn't jump to the pointer.
            let grab = ui
                .input(|i| i.pointer.press_origin())
                .map_or(0.0, |p| geo.t(p.x) - m.time);
            st.timeline.drag = Some((
                host,
                Drag::Marker {
                    id: m.id,
                    to: m.time,
                    grab,
                },
            ));
        }
        if ctx.editable && r.double_clicked() {
            st.timeline.renaming = Some((host, m.id, m.name.clone()));
        }
        r.context_menu(|ui| {
            ui.add_enabled_ui(ctx.editable, |ui| {
                if ui.button("Rename…").clicked() {
                    st.timeline.renaming = Some((host, m.id, m.name.clone()));
                    ui.close();
                }
                let r = ui
                    .button("Add Pause cue here")
                    .on_hover_text("A pause cue at this marker: the timeline waits here for GO");
                super::toolbar::note(ui, host, "pause_here", &r);
                if r.clicked() {
                    actions.push(Action::Timeline(TimelineAction::AddControl {
                        key,
                        action: ControlAction::TimelinePause,
                        at: m.time,
                    }));
                    ui.close();
                }
            });
            if ui
                .add_enabled(!ctx.ruler_locked, egui::Button::new("Jump here"))
                .on_hover_text("Move the playhead to this marker")
                .clicked()
            {
                actions.push(Action::Timeline(TimelineAction::JumpToMarker {
                    key,
                    id: m.id,
                }));
                ui.close();
            }
            ui.separator();
            if ui
                .add_enabled(ctx.editable, egui::Button::new("Remove marker"))
                .clicked()
            {
                actions.push(Action::Timeline(TimelineAction::RemoveMarker {
                    key,
                    id: m.id,
                }));
                ui.close();
            }
        });
    }

    if let Some((flag, color, name)) = whole {
        let g = painter.layout_no_wrap(name, FontId::proportional(10.0), Color32::BLACK);
        let full = Rect::from_min_size(flag.min, vec2(g.size().x + 8.0, 13.0));
        painter.rect_filled(full, 2.0, color);
        painter.galley(pos2(full.left() + 4.0, full.top()), g, Color32::BLACK);
    }

    // A marker's name being typed, over its flag.
    let renaming = match &st.timeline.renaming {
        Some((h, id, _)) if *h == host => Some(*id),
        _ => None,
    };
    if let Some(id) = renaming {
        let at = rects.iter().find(|(m, _)| *m == id).map(|(_, r)| r.min);
        match (at, &mut st.timeline.renaming) {
            (Some(at), Some((_, _, text))) => {
                let box_rect = Rect::from_min_size(at, vec2(150.0, 18.0));
                let te = ui.put(
                    box_rect,
                    egui::TextEdit::singleline(text).font(FontId::proportional(11.0)),
                );
                if te.lost_focus() {
                    let keep = !ui.input(|i| i.key_pressed(egui::Key::Escape));
                    let name = text.trim().to_string();
                    let was = markers.iter().find(|m| m.id == id).map(|m| m.name.clone());
                    if keep && !name.is_empty() && was.as_deref() != Some(name.as_str()) {
                        actions.push(Action::Timeline(TimelineAction::RenameMarker {
                            key,
                            id,
                            name,
                        }));
                    }
                    st.timeline.renaming = None;
                } else if !te.has_focus() {
                    te.request_focus();
                }
            }
            // The marker has gone.
            _ => st.timeline.renaming = None,
        }
    }
    rects
}
