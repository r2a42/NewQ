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

    // The markers' flags, on top of the ruler. Those at one place (or nearer
    // than a few points) are one flag, as cues are one chip on the Cues track
    // (`tracks::chips`): "×N", the picked one (else the first) shown and acted
    // on, a click picking each in turn.
    let markers = model::settings(ctx.show, key).map_or(&[][..], |s| &s.markers[..]);
    let mut rects = Vec::new();
    let dragged = match st.timeline.drag_in(host) {
        Some(Drag::Marker { id, to, .. }) => Some((*id, *to)),
        _ => None,
    };
    let at = |m: &crate::timeline::Marker| match dragged {
        Some((id, to)) if id == m.id => to,
        _ => m.time,
    };
    let picked = st
        .timeline
        .marker_pick
        .filter(|(k, id)| *k == key && markers.iter().any(|m| m.id == *id))
        .map(|(_, id)| id);
    let items: Vec<(u64, f32, bool)> = markers.iter().map(|m| (m.id, geo.x(at(m)), true)).collect();
    let plan = super::tracks::chips(&items, rect.right());
    let marker = |id: u64| markers.iter().find(|m| m.id == id);
    // Drawn again, whole, over their neighbours: the hovered (or dragged) and the picked.
    let mut whole: Vec<(Rect, Color32, String, bool)> = Vec::new();
    #[cfg(test)]
    let mut notes: Vec<(Vec<u64>, Rect, String)> = Vec::new();
    let mut on_a_flag = false;
    for (_, chip) in &plan {
        let super::tracks::Chip::Lead { members, limit, .. } = chip else {
            continue;
        };
        // The one it shows: dragged, else picked, else the first.
        let Some(m) = members
            .iter()
            .find(|id| dragged.is_some_and(|(d, _)| d == **id))
            .or_else(|| members.iter().find(|id| picked == Some(**id)))
            .or(members.first())
            .and_then(|id| marker(*id))
        else {
            continue;
        };
        let x = geo.x(at(m));
        let n = members.len();
        let color = m.color.map_or(MARKER, |c| {
            let [r, g, b] = c.rgb();
            Color32::from_rgb(r, g, b)
        });
        let name = if n > 1 {
            format!("×{n} {}", m.name)
        } else {
            m.name.clone()
        };
        // As a cue's name on the Cues track: up to the next flag, cut short
        // with "…", or no name at all where there is no room for one.
        let next = limit + super::tracks::LABEL_GAP;
        let room = (next - FLAG_GAP).min(rect.right()) - x - 8.0;
        let galley =
            super::tracks::fitted(&painter, &name, 10.0, room, Color32::from_rgb(20, 20, 20));
        let w = galley
            .as_ref()
            .map_or(FLAG_MIN_W.min((next - x - 1.0).max(3.0)), |g| {
                g.size().x + 8.0
            });
        let flag = Rect::from_min_size(pos2(x, rect.top() + 1.0), vec2(w, 13.0));
        for id in members {
            rects.push((*id, flag));
        }
        #[cfg(test)]
        notes.push((members.clone(), flag, name.clone()));
        let hit = flag.intersect(rect);
        if hit.width() <= 0.0 {
            continue;
        }
        if n > 1 {
            // More than one here: a second flag peeps out from behind.
            let behind = flag.translate(vec2(2.0, -1.0));
            painter.rect_filled(behind, 2.0, color.gamma_multiply(0.6));
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
        let tip = if n > 1 {
            let lines: Vec<String> = members
                .iter()
                .filter_map(|id| marker(*id))
                .map(|m| format!("{} · {}", m.name, fmt_time(m.time)))
                .collect();
            format!(
                "{n} markers here (zoom in to part them; a click picks each in turn):\n\n{}\n\n\
                 Drag to move the one shown; double-click to rename it; right-click for more.",
                lines.join("\n")
            )
        } else {
            format!(
                "{} at {}. Drag to move it; double-click to rename; right-click for more.",
                m.name,
                fmt_time(m.time)
            )
        };
        let r = ui
            .interact(hit, ui.id().with(("marker", members[0])), sense)
            .on_hover_text(tip);
        on_a_flag |= r.contains_pointer();
        let is_picked = picked == Some(m.id);
        if r.hovered() || r.dragged() || is_picked {
            whole.push((flag, color, name.clone(), is_picked));
        }
        if r.clicked() {
            // A flag of several picks each in turn.
            let to = match members.iter().position(|id| *id == m.id) {
                Some(i) if n > 1 && is_picked => members[(i + 1) % n],
                _ => m.id,
            };
            st.timeline.marker_pick = Some((key, to));
        }
        if ctx.editable && r.drag_started() {
            // Where on the flag it was taken, so the marker doesn't jump to the pointer.
            let grab = ui
                .input(|i| i.pointer.press_origin())
                .map_or(0.0, |p| geo.t(p.x) - m.time);
            st.timeline.marker_pick = Some((key, m.id));
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
    // A press anywhere else lets go of the picked marker.
    if !on_a_flag
        && ui.input(|i| i.pointer.primary_pressed())
        && st.timeline.marker_pick.is_some_and(|(k, _)| k == key)
    {
        st.timeline.marker_pick = None;
    }

    // The picked one first, so the hovered is on top of it.
    whole.sort_by_key(|w| !w.3);
    #[cfg(test)]
    let whole_names: Vec<String> = whole.iter().map(|w| w.2.clone()).collect();
    #[cfg(test)]
    ui.data_mut(|d| d.insert_temp(egui::Id::new(("timeline_marker_whole", host)), whole_names));
    for (flag, color, name, picked) in whole {
        let g = painter.layout_no_wrap(name, FontId::proportional(10.0), Color32::BLACK);
        let full = Rect::from_min_size(flag.min, vec2(g.size().x + 8.0, 13.0));
        painter.rect_filled(full, 2.0, color);
        if picked {
            painter.rect_stroke(
                full,
                2.0,
                Stroke::new(1.0, Color32::WHITE),
                egui::StrokeKind::Inside,
            );
        }
        painter.galley(pos2(full.left() + 4.0, full.top()), g, Color32::BLACK);
    }
    #[cfg(test)]
    ui.data_mut(|d| d.insert_temp(egui::Id::new(("timeline_marker_flags", host)), notes));

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
