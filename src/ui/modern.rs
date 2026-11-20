//! The new interface: the same show and the same engine, arranged three ways
//! under one strip, as an alternative to the dockable panes. It is switched on
//! and off in the Window menu (`Prefs::new_layout`, per computer); nothing in
//! the show file changes.
//!
//! - **Editor**: the cue list with the inspector under it and what is playing
//!   beside it, for building a show.
//! - **Operator**: a calm screen for running one: the next cue large, a big GO,
//!   what is running with time left, the picture, and the state of the gear.
//! - **Patch**: a signal-flow view of cues, displays and audio outputs and the
//!   gear they reach; drag from a cue to a display or an audio output to route it.

use super::*;
use egui::{Pos2, Rect, pos2, vec2};

/// Which arrangement of the new interface is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Editor,
    Operator,
    Patch,
    /// The timeline workspace (when the timeline editor opens as one,
    /// `Prefs::timeline_layout`).
    Timeline,
}

impl View {
    pub const ALL: [View; 4] = [View::Editor, View::Operator, View::Patch, View::Timeline];

    pub fn label(self) -> &'static str {
        match self {
            View::Editor => "Editor",
            View::Operator => "Operator",
            View::Patch => "Patch",
            View::Timeline => "Timeline",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            View::Editor => "Build the show: cue list, inspector and what is playing",
            View::Operator => "Run the show: the next cue, a big GO and what is running",
            View::Patch => "See and change where cues go: displays, audio outputs and gear",
            View::Timeline => {
                "The timeline workspace: a bin, the picture, the inspector and the tracks"
            }
        }
    }
}

/// How wide the header's "what GO does next" read-out is.
const HEADER_NEXT_WIDTH: f32 = 260.0;

/// The cue GO would fire: the first armed one at or after the playhead. None
/// on a timeline list, where GO runs the timeline and the playhead means nothing.
fn next_cue<'a>(show: &'a Show, engine: &Engine) -> Option<&'a Cue> {
    if timeline::list_tab::active(show, engine).is_some() {
        return None;
    }
    let at = engine.playhead.and_then(|id| show.cue_index(id))?;
    show.armed_top_from(at).map(|i| &show.cues[i])
}

/// Up to `n` armed top-level cues that follow `after` in its list.
fn upcoming(show: &Show, after: CueId, n: usize) -> Vec<&Cue> {
    let mut out = Vec::new();
    let mut at = show.cue_index(after);
    while out.len() < n {
        let Some(i) = at.and_then(|i| show.next_top(i)) else {
            break;
        };
        if show.cues[i].armed {
            out.push(&show.cues[i]);
        }
        at = Some(i);
    }
    out
}

/// The whole new interface, under the menu bar and above the status bar.
pub fn draw(
    ui: &mut Ui,
    show: &mut Show,
    engine: &mut Engine,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) {
    egui::Panel::top("modern_strip")
        .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(8))
        .show(ui, |ui| strip(ui, show, engine, st, actions));
    egui::CentralPanel::default().show(ui, |ui| {
        // The settings take the place of the view while they are open, as they
        // take the place of the cue list in the pane layout.
        if st.settings_open() {
            let locked = st.show_mode;
            ui.add_enabled_ui(!locked, |ui| {
                settings::settings_view(ui, show, engine, st, actions)
            });
            return;
        }
        match st.modern_view {
            View::Editor => editor(ui, show, engine, st, actions),
            View::Operator => operator(ui, show, engine, st, actions),
            View::Patch => patch(ui, show, engine, st, actions),
            View::Timeline => timeline::workspace::draw(ui, show, engine, st, actions),
        }
    });
}

/// The strip across the top: the views, GO and what it will fire, Stop all.
fn strip(
    ui: &mut Ui,
    show: &Show,
    engine: &mut Engine,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        if !st.settings_open() && st.modern_view != View::Operator {
            let go = egui::Button::new(
                RichText::new("GO")
                    .size(22.0)
                    .strong()
                    .color(Color32::WHITE),
            )
            .fill(GO_GREEN)
            .corner_radius(CornerRadius::same(8))
            .min_size(Vec2::new(90.0, 44.0));
            let on_timeline = timeline::list_tab::active(show, engine).is_some();
            if ui
                .add(go)
                .on_hover_text(if on_timeline {
                    "GO (0): run the timeline, or carry it on from a pause; Space plays and pauses it"
                } else {
                    "GO (0, or Space on a cue list): fire the cue at the playhead"
                })
                .clicked()
            {
                actions.push(Action::Go);
            }
            ui.add_space(4.0);
            // A fixed width, so the view tabs after it stay put as the read-out
            // changes (a long name is cut short; the whole of it on hover).
            ui.vertical(|ui| {
                ui.set_width(HEADER_NEXT_WIDTH);
                ui.label(
                    RichText::new(format!("NEXT · {}", show.list_name(engine.active_list)))
                        .small()
                        .weak(),
                );
                let (text, hover) = match (go_readout(show, engine), next_cue(show, engine)) {
                    (Some((what, then)), _) => {
                        let hover = match &then {
                            Some(then) => format!("GO: {what}, {then}"),
                            None => format!("GO: {what}"),
                        };
                        (RichText::new(what).strong().color(PLAYHEAD), hover)
                    }
                    (None, Some(c)) => {
                        let label = cue_label(c);
                        (RichText::new(&label).strong().color(PLAYHEAD), label)
                    }
                    (None, None) => (
                        RichText::new("End of cue list").weak(),
                        "End of cue list".into(),
                    ),
                };
                #[cfg(test)]
                ui.data_mut(|d| d.insert_temp(egui::Id::new("strip_readout"), hover.clone()));
                ui.add(egui::Label::new(text).truncate())
                    .on_hover_text(hover);
            });
            ui.add_space(12.0);
        }
        for v in View::ALL {
            // The Timeline view is there while the editor opens as a workspace.
            if v == View::Timeline && !timeline::workspace::chosen(st) {
                continue;
            }
            let here = st.modern_view == v && !st.settings_open();
            let r = ui.selectable_label(here, v.label()).on_hover_text(v.hint());
            #[cfg(test)]
            ui.data_mut(|d| {
                d.get_temp_mut_or_default::<Vec<(View, Rect)>>(egui::Id::new("modern_views"))
                    .push((v, r.rect))
            });
            if r.clicked() {
                if v == View::Timeline {
                    // Opening it is opening the editor (on what it showed last).
                    actions.push(Action::Timeline(timeline::TimelineAction::Open(None)));
                } else if st.modern_view == View::Timeline {
                    // Leaving it puts the editor away; the tab brings it back.
                    actions.push(Action::Timeline(timeline::TimelineAction::Close));
                }
                st.modern_view = v;
                if st.settings_open() {
                    actions.push(Action::CloseSettings);
                }
            }
        }
        let settings = st.settings_open();
        if ui.selectable_label(settings, "Settings").clicked() {
            actions.push(Action::ToggleSettings);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui
                .button("Classic interface")
                .on_hover_text("Go back to the panes: the same show, laid out as before")
                .clicked()
            {
                actions.push(Action::SetNewLayout(false));
            }
            let stop = egui::Button::new(RichText::new("STOP ALL").strong().color(Color32::WHITE))
                .fill(STOP_RED)
                .min_size(Vec2::new(90.0, 36.0));
            if ui
                .add(stop)
                .on_hover_text("Fade out everything (Esc). Press twice to stop instantly.")
                .clicked()
            {
                actions.push(Action::Panic);
            }
            let pause = if engine.paused { "Resume" } else { "Pause" };
            if ui
                .button(pause)
                .on_hover_text("Pause / resume everything (P)")
                .clicked()
            {
                actions.push(Action::TogglePause);
            }
            let lock = if st.show_mode {
                "Unlock editing"
            } else {
                "Lock editing"
            };
            if ui
                .selectable_label(st.show_mode, lock)
                .on_hover_text("Show mode: nothing can be edited by accident")
                .clicked()
            {
                actions.push(Action::ToggleShowMode);
            }
        });
    });
}

/// The editor: cue list in the middle, what is playing at the right, the
/// inspector of the selected cue underneath.
fn editor(
    ui: &mut Ui,
    show: &mut Show,
    engine: &mut Engine,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) {
    egui::Panel::bottom("modern_inspector")
        .resizable(true)
        .default_size(250.0)
        .min_size(90.0)
        .show(ui, |ui| {
            let title = st
                .selected
                .and_then(|id| show.cue(id))
                .map_or("Inspector".to_string(), |c| {
                    format!("Inspector · {}", cue_label(c))
                });
            ui.label(RichText::new(title).strong());
            ui.separator();
            let locked = st.show_mode;
            ui.add_enabled_ui(!locked, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| inspector(ui, show, engine, st, actions));
            });
        });
    // The timeline editor, above the inspector, while it is wanted (and not
    // in a workspace of its own).
    if st.timeline.open && !timeline::workspace::chosen(st) {
        egui::Panel::bottom("modern_timeline")
            .resizable(true)
            // Room for three tracks and the cue track; more scroll.
            .default_size(250.0)
            .min_size(120.0)
            .show(ui, |ui| {
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    if ui
                        .small_button("Close timeline")
                        .on_hover_text(
                            "Put the timeline editor away (Window ▸ Timeline brings it back)",
                        )
                        .clicked()
                    {
                        actions.push(Action::Timeline(timeline::TimelineAction::Close));
                    }
                });
                timeline::pane(ui, show, engine, st, actions);
            });
    }
    egui::Panel::right("modern_active")
        .resizable(true)
        .default_size(260.0)
        .min_size(160.0)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| active_panel(ui, show, engine, st, actions));
        });
    egui::CentralPanel::default().show(ui, |ui| cue_list(ui, show, engine, st, actions));
}

/// The operator's screen.
fn operator(
    ui: &mut Ui,
    show: &mut Show,
    engine: &mut Engine,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) {
    egui::Panel::right("modern_running")
        .resizable(true)
        .default_size(380.0)
        .min_size(240.0)
        .show(ui, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    running(ui, show, engine, actions);
                    ui.add_space(10.0);
                    output_previews(ui, show, st, false, crate::dock::OPERATOR_PREVIEW);
                    ui.add_space(10.0);
                    gear(ui, show, engine, st);
                });
        });
    egui::CentralPanel::default().show(ui, |ui| {
        let next = next_cue(show, engine);
        ui.label(
            RichText::new(format!("NEXT · {}", show.list_name(engine.active_list)))
                .small()
                .weak(),
        );
        let readout = go_readout(show, engine);
        match next {
            _ if let Some((what, then)) = &readout => {
                ui.label(RichText::new(what).size(40.0).strong().color(PLAYHEAD));
                if let Some(then) = then {
                    ui.label(RichText::new(then).size(18.0).italics());
                }
                timeline::list_tab::operator_state(ui, show, engine);
            }
            Some(c) => {
                ui.label(
                    RichText::new(cue_label(c))
                        .size(40.0)
                        .strong()
                        .color(PLAYHEAD),
                );
                if !c.notes.trim().is_empty() {
                    ui.label(RichText::new(c.notes.trim()).size(18.0).italics());
                }
            }
            None => {
                ui.label(RichText::new("End of cue list").size(40.0).weak());
            }
        }
        ui.add_space(12.0);
        let go = egui::Button::new(
            RichText::new("GO")
                .size(56.0)
                .strong()
                .color(Color32::WHITE),
        )
        .fill(GO_GREEN)
        .corner_radius(CornerRadius::same(14));
        if ui
            .add_sized([ui.available_width(), 130.0], go)
            .on_hover_text(if timeline::list_tab::active(show, engine).is_some() {
                "GO (0): run the timeline, or carry it on from a pause; Space plays and pauses it"
            } else {
                "GO (0, or Space on a cue list): fire the cue at the playhead"
            })
            .clicked()
        {
            actions.push(Action::Go);
        }
        ui.add_space(14.0);
        if let Some(c) = next {
            // GO carries a timeline on first: the next cue comes after it.
            let mut later = upcoming(show, c.id, 6);
            if readout.is_some() {
                later.insert(0, c);
                later.truncate(6);
            }
            if !later.is_empty() {
                ui.label(RichText::new("THEN").small().weak());
                for c in later {
                    ui.label(RichText::new(cue_label(c)).size(18.0));
                }
            }
        }
    });
}

/// What is running, with the time left, big enough to read from a distance.
fn running(ui: &mut Ui, show: &Show, engine: &Engine, actions: &mut Vec<Action>) {
    ui.label(RichText::new("RUNNING").small().weak());
    if !engine.is_active() {
        ui.label(RichText::new("Nothing playing").weak());
    }
    for p in &engine.pending {
        let name = show.cue(p.cue_id).map(cue_label).unwrap_or_default();
        ui.horizontal(|ui| {
            ui.label(name);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(
                    RichText::new(fmt_time(p.remaining))
                        .monospace()
                        .color(PREWAIT),
                );
            });
        });
    }
    for a in engine.active.iter().rev() {
        let name = show
            .cue(a.cue_id)
            .map(cue_label)
            .unwrap_or_else(|| "(deleted cue)".into());
        // Time left (a timeline clip whose end the clock keeps: on that clock).
        let (elapsed, duration) = a.panel_times(show);
        let (shown, frac) = match duration {
            Some(d) => (
                fmt_time((d - elapsed).max(0.0)),
                (elapsed / d.max(1e-6)).clamp(0.0, 1.0),
            ),
            None => (
                "∞".to_string(),
                match (a.pass_len, a.pass_number()) {
                    (Some(len), Some(_)) => (a.bar_position() / len.max(1e-6)).clamp(0.0, 1.0),
                    _ => 1.0,
                },
            ),
        };
        ui.horizontal(|ui| {
            ui.label(RichText::new(name).strong());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Stop").clicked() {
                    actions.push(Action::StopInstance(a.instance, 0.0));
                }
                if ui.small_button("Fade out").clicked() {
                    actions.push(Action::FadeOutInstance(a.instance));
                }
                ui.label(RichText::new(shown).size(20.0).monospace());
            });
        });
        let fill = if a.stopping { PREWAIT } else { PLAYING };
        ui.add(
            egui::ProgressBar::new(frac as f32)
                .fill(fill)
                .desired_height(8.0),
        );
        ui.add_space(4.0);
    }
}

/// Lights for the things a show depends on.
fn gear(ui: &mut Ui, show: &Show, engine: &Engine, st: &UiState) {
    ui.label(RichText::new("GEAR").small().weak());
    let light = |ui: &mut Ui, ok: bool, text: String| {
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
            ui.painter()
                .circle_filled(rect.center(), 5.0, if ok { GO_GREEN } else { STOP_RED });
            ui.label(text);
        });
    };
    light(ui, engine.ff.is_some(), "FFmpeg".into());
    let audio = engine.audio.available();
    light(
        ui,
        audio,
        if audio {
            "Audio output".into()
        } else {
            "No audio output".to_string()
        },
    );
    let displays: Vec<&OutputConfig> = show
        .outputs
        .iter()
        .filter(|o| o.enabled && !o.is_sub_display() && o.monitor.is_some())
        .collect();
    for o in displays {
        let ok = matches!(
            st.display_states.get(&o.id),
            Some(DisplayState::Connected(_) | DisplayState::OnMainWindowDisplay(_))
        );
        light(
            ui,
            ok,
            format!("{}{}", o.name, if ok { "" } else { " is not connected" }),
        );
    }
    for o in &show.audio_outputs {
        if let Some(s) = engine.audio.device_status(Some(o.id)) {
            light(
                ui,
                s.error.is_none(),
                match s.error {
                    None => format!("{} · {}", o.name, s.name),
                    Some(e) => format!("{}: {e}", o.name),
                },
            );
        }
    }
}

// ---- The patch view ----

const ROW: f32 = 58.0;
const NODE_H: f32 = 46.0;
const MAX_CUES: usize = 40;

/// What a cue can be routed to.
fn routes_picture(k: &CueKind) -> bool {
    matches!(
        k,
        CueKind::Video(_)
            | CueKind::Image(_)
            | CueKind::Text(_)
            | CueKind::Shape(_)
            | CueKind::Camera(_)
    )
}

fn routes_sound(k: &CueKind) -> bool {
    matches!(
        k,
        CueKind::Video(_) | CueKind::Audio(_) | CueKind::LiveAudio(_)
    )
}

/// Where something ends up, for the right-hand column of the patch view.
fn gear_line(o: &OutputConfig) -> String {
    let screen = match &o.monitor {
        Some(m) => m.name.clone(),
        None => "Not assigned to a monitor".to_string(),
    };
    match o.stream.as_ref().filter(|s| s.enabled) {
        Some(s) => format!("{screen}\n+ {}", stream_label(&s.kind)),
        None => screen,
    }
}

fn stream_label(k: &crate::show::StreamKind) -> &'static str {
    use crate::show::StreamKind::*;
    match k {
        VirtualCamera { .. } => "virtual camera",
        Udp { .. } => "network stream",
        Custom { .. } => "custom FFmpeg output",
        Ndi { .. } => "NDI source",
        DeckLink { .. } => "Blackmagic output",
        Record { .. } => "recording",
        Server { .. } => "streaming server",
    }
}

/// A rounded box with a title and a second line, drawn into `painter`.
fn node(
    painter: &egui::Painter,
    rect: Rect,
    title: &str,
    sub: &str,
    fill: Color32,
    stroke: Stroke,
    text: Color32,
) {
    painter.rect(
        rect,
        CornerRadius::same(8),
        fill,
        stroke,
        egui::StrokeKind::Inside,
    );
    let line = |at: Pos2, s: &str, size: f32, color: Color32| {
        // One line, cut with an ellipsis where the box ends.
        let mut job = egui::text::LayoutJob::simple_singleline(
            s.to_string(),
            egui::FontId::proportional(size),
            color,
        );
        job.wrap.max_width = rect.width() - 20.0;
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        painter.galley(at, painter.layout_job(job), color);
    };
    line(rect.left_top() + vec2(10.0, 8.0), title, 14.0, text);
    if !sub.is_empty() {
        line(
            rect.left_top() + vec2(10.0, 27.0),
            sub,
            11.5,
            text.gamma_multiply(0.65),
        );
    }
}

fn curve(painter: &egui::Painter, from: Pos2, to: Pos2, stroke: Stroke) {
    let dx = ((to.x - from.x) * 0.5).max(24.0);
    painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
        [from, from + vec2(dx, 0.0), to - vec2(dx, 0.0), to],
        false,
        Color32::TRANSPARENT,
        stroke,
    ));
}

/// Cues, displays and audio outputs, and the gear they reach.
fn patch(
    ui: &mut Ui,
    show: &mut Show,
    engine: &mut Engine,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) {
    let _ = actions;
    let blue = Color32::from_rgb(55, 138, 221);
    let teal = Color32::from_rgb(29, 158, 117);
    let locked = st.show_mode;
    if locked {
        ui.label(
            RichText::new("Show mode: routes can't be changed.")
                .small()
                .weak(),
        );
    } else {
        ui.label(
            RichText::new(
                "Drag from the dot on a cue to a display or an audio output to route it.",
            )
            .small()
            .weak(),
        );
    }

    // The middle column: displays, then audio outputs ("None" goes to the
    // first of each, as the engine does).
    struct Mid {
        title: String,
        sub: String,
        display: Option<OutputId>,
        audio: Option<Option<AudioOutputId>>,
        gear: String,
        gear_ok: bool,
        level: f32,
    }
    let mut mids: Vec<Mid> = Vec::new();
    for o in show.outputs.iter().filter(|o| !o.is_sub_display()) {
        let connected = matches!(
            st.display_states.get(&o.id),
            Some(DisplayState::Connected(_) | DisplayState::OnMainWindowDisplay(_))
        );
        mids.push(Mid {
            title: o.name.clone(),
            sub: if o.enabled {
                "display".into()
            } else {
                "display · off".to_string()
            },
            display: Some(o.id),
            audio: None,
            gear: gear_line(o),
            gear_ok: o.monitor.is_none() || connected,
            level: 0.0,
        });
    }
    if show.audio_outputs.is_empty() {
        let s = engine.audio.device_status(None);
        mids.push(Mid {
            title: "Default audio output".into(),
            sub: "audio".into(),
            display: None,
            audio: Some(None),
            gear: s
                .as_ref()
                .map_or("No audio device".into(), |s| s.name.clone()),
            gear_ok: s.as_ref().is_some_and(|s| s.error.is_none()),
            level: engine.audio.meters(None).into_iter().fold(0.0, f32::max),
        });
    }
    for o in &show.audio_outputs {
        let s = engine.audio.device_status(Some(o.id));
        mids.push(Mid {
            title: o.name.clone(),
            sub: "audio".into(),
            display: None,
            audio: Some(Some(o.id)),
            gear: s.as_ref().map_or_else(
                || o.device.clone().unwrap_or_else(|| "Default device".into()),
                |s| match &s.error {
                    Some(e) => format!("{}\n{e}", s.name),
                    None => s.name.clone(),
                },
            ),
            gear_ok: s.as_ref().is_some_and(|s| s.error.is_none()),
            level: engine
                .audio
                .meters(Some(o.id))
                .into_iter()
                .fold(0.0, f32::max),
        });
    }
    let first_display = show
        .outputs
        .iter()
        .find(|o| !o.is_sub_display())
        .map(|o| o.id);
    let first_audio = mids.iter().position(|m| m.audio.is_some());

    let list = engine.active_list;
    let cue_ids: Vec<CueId> = show
        .cues
        .iter()
        .filter(|c| c.list == list && c.kind.media().is_some())
        .map(|c| c.id)
        .take(MAX_CUES)
        .collect();
    let rows = cue_ids.len().max(mids.len()).max(1);
    let playing: Vec<CueId> = engine.active.iter().map(|a| a.cue_id).collect();
    let next = next_cue(show, engine).map(|c| c.id);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let width = ui.available_width();
            let (area, _) =
                ui.allocate_exact_size(vec2(width, rows as f32 * ROW + 34.0), egui::Sense::hover());
            let col_w = (width * 0.27).clamp(150.0, 320.0);
            let xs = [
                area.left(),
                area.center().x - col_w / 2.0,
                area.right() - col_w,
            ];
            let node_rect = |col: usize, row: usize| {
                Rect::from_min_size(
                    pos2(xs[col], area.top() + 26.0 + row as f32 * ROW),
                    vec2(col_w, NODE_H),
                )
            };
            let painter = ui.painter_at(area);
            let visuals = ui.visuals().clone();
            let fill = visuals.faint_bg_color;
            let line = visuals.widgets.noninteractive.bg_stroke;
            let text = visuals.text_color();
            for (col, name) in ["Cues", "Displays and audio outputs", "Gear"]
                .into_iter()
                .enumerate()
            {
                painter.text(
                    pos2(xs[col] + 2.0, area.top() + 4.0),
                    egui::Align2::LEFT_TOP,
                    name,
                    egui::FontId::proportional(12.0),
                    text.gamma_multiply(0.6),
                );
            }

            // Gear column and the links from the middle to it.
            for (i, m) in mids.iter().enumerate() {
                let mid = node_rect(1, i);
                let gear_rect = node_rect(2, i);
                let (first, rest) = m.gear.split_once('\n').unwrap_or((&m.gear, ""));
                node(
                    &painter,
                    gear_rect,
                    first,
                    rest,
                    fill,
                    Stroke::new(1.0, if m.gear_ok { line.color } else { STOP_RED }),
                    text,
                );
                curve(
                    &painter,
                    mid.right_center(),
                    gear_rect.left_center(),
                    Stroke::new(
                        1.5,
                        if m.gear_ok {
                            visuals.widgets.inactive.fg_stroke.color
                        } else {
                            STOP_RED
                        },
                    ),
                );
                if m.level > 0.0 {
                    let w = (gear_rect.width() - 20.0) * m.level.clamp(0.0, 1.0);
                    painter.rect_filled(
                        Rect::from_min_size(
                            gear_rect.left_bottom() + vec2(10.0, -7.0),
                            vec2(w, 3.0),
                        ),
                        CornerRadius::same(1),
                        if m.level > 0.95 { STOP_RED } else { GO_GREEN },
                    );
                }
            }

            // Cues first, so a drag that ends on a middle node is known.
            let mut ports: Vec<(CueId, Rect, Rect)> = Vec::new();
            for (row, id) in cue_ids.iter().enumerate() {
                let rect = node_rect(0, row);
                let port = Rect::from_center_size(rect.right_center(), Vec2::splat(18.0));
                ports.push((*id, rect, port));
            }
            let mid_hit = |p: Pos2| (0..mids.len()).find(|i| node_rect(1, *i).contains(p));
            #[cfg(test)]
            ui.data_mut(|m| {
                m.insert_temp(
                    egui::Id::new("modern_patch_rects"),
                    PatchRects {
                        ports: ports.iter().map(|(id, _, p)| (*id, p.center())).collect(),
                        mids: (0..mids.len())
                            .map(|i| (mids[i].title.clone(), node_rect(1, i)))
                            .collect(),
                    },
                )
            });

            let mut route: Option<(CueId, usize)> = None;
            let mut pick: Option<CueId> = None;
            for (id, rect, port) in &ports {
                let Some(c) = show.cue(*id) else { continue };
                let is_playing = playing.contains(id);
                let selected = st.selected == Some(*id);
                let stroke = if selected {
                    Stroke::new(2.0, PLAYHEAD)
                } else if is_playing {
                    Stroke::new(1.5, GO_GREEN)
                } else {
                    line
                };
                let sub = if is_playing {
                    "playing"
                } else if next == Some(*id) {
                    "next"
                } else {
                    ""
                };
                node(&painter, *rect, &cue_label(c), sub, fill, stroke, text);
                let body = ui.interact(
                    *rect,
                    ui.id().with(("patch_cue", id.0)),
                    egui::Sense::click(),
                );
                if body.clicked() {
                    pick = Some(*id);
                }
                // The links of this cue.
                if let Some(m) = c.kind.media() {
                    let width = if is_playing { 2.5 } else { 1.5 };
                    if routes_picture(&c.kind)
                        && let Some(target) = m.output.or(first_display)
                        && let Some(i) = mids.iter().position(|x| x.display == Some(target))
                    {
                        curve(
                            &painter,
                            port.center(),
                            node_rect(1, i).left_center(),
                            Stroke::new(width, blue),
                        );
                    }
                    if routes_sound(&c.kind) {
                        let want = m.audio_output;
                        let i = mids
                            .iter()
                            .position(|x| x.audio == Some(want))
                            .or(first_audio);
                        if let Some(i) = i {
                            curve(
                                &painter,
                                port.center(),
                                node_rect(1, i).left_center(),
                                Stroke::new(width, teal),
                            );
                        }
                    }
                }
                // The port to drag from.
                let drag = ui.interact(
                    *port,
                    ui.id().with(("patch_port", id.0)),
                    if locked {
                        egui::Sense::hover()
                    } else {
                        egui::Sense::drag()
                    },
                );
                let hot = drag.hovered() || drag.dragged();
                painter.circle(
                    port.center(),
                    if hot { 7.0 } else { 5.0 },
                    if hot {
                        text
                    } else {
                        visuals.widgets.inactive.fg_stroke.color
                    },
                    Stroke::new(1.0, visuals.window_fill),
                );
                if drag.dragged()
                    && let Some(p) = ui.input(|i| i.pointer.interact_pos())
                {
                    curve(&painter, port.center(), p, Stroke::new(2.0, text));
                    if let Some(i) = mid_hit(p) {
                        painter.rect_stroke(
                            node_rect(1, i),
                            CornerRadius::same(8),
                            Stroke::new(2.0, text),
                            egui::StrokeKind::Outside,
                        );
                    }
                }
                if drag.drag_stopped()
                    && let Some(p) = ui.input(|i| i.pointer.interact_pos())
                    && let Some(i) = mid_hit(p)
                {
                    route = Some((*id, i));
                }
            }

            // The middle column on top of the lines.
            for (i, m) in mids.iter().enumerate() {
                node(
                    &painter,
                    node_rect(1, i),
                    &m.title,
                    &m.sub,
                    visuals.extreme_bg_color,
                    line,
                    text,
                );
            }

            if let Some((id, i)) = route
                && let Some(c) = show.cues.iter_mut().find(|c| c.id == id)
            {
                let picture = routes_picture(&c.kind);
                let sound = routes_sound(&c.kind);
                if let Some(m) = c.kind.media_mut() {
                    if let Some(d) = mids[i].display
                        && picture
                    {
                        m.output = Some(d);
                    } else if let Some(a) = mids[i].audio
                        && sound
                    {
                        m.audio_output = a;
                    }
                }
            }
            if let Some(id) = pick {
                select(show, st, engine, Some(id));
            }
        });
}

/// Where the patch view put things, for the tests to aim at.
#[cfg(test)]
#[derive(Clone)]
struct PatchRects {
    ports: Vec<(CueId, Pos2)>,
    mids: Vec<(String, Rect)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton, RawInput};

    fn click(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// Press on `from`, drag to `to` and let go, drawing the patch view each frame.
    fn drag_in_patch(
        ctx: &egui::Context,
        show: &mut Show,
        engine: &mut Engine,
        st: &mut UiState,
        t: &mut f64,
        from: Pos2,
        to: Pos2,
    ) {
        let mut step = |events: Vec<Event>| {
            *t += 1.0 / 60.0;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))),
                time: Some(*t),
                events,
                ..Default::default()
            };
            let mut actions = Vec::new();
            let mut out = ctx.run_ui(input, |ui| {
                egui::CentralPanel::default()
                    .show(ui, |ui| patch(ui, show, engine, st, &mut actions));
            });
            out.textures_delta.clear();
        };
        step(vec![Event::PointerMoved(from)]);
        step(vec![click(from, true)]);
        for i in 1..8 {
            step(vec![Event::PointerMoved(
                from + (to - from) * (i as f32 / 8.0),
            )]);
        }
        step(vec![Event::PointerMoved(to), click(to, false)]);
        step(vec![]);
    }

    fn rects(ctx: &egui::Context) -> PatchRects {
        ctx.data(|d| d.get_temp::<PatchRects>(egui::Id::new("modern_patch_rects")))
            .expect("the patch view was drawn")
    }

    #[test]
    fn dragging_from_a_cue_to_a_display_or_an_audio_output_routes_it() {
        let mut show = Show::default();
        let first = show
            .outputs
            .first()
            .map(|o| o.id)
            .unwrap_or_else(|| show.add_display());
        let second = show.add_display();
        let (a1, a2) = (show.add_audio_output(), show.add_audio_output());
        let video = show.make_cue(CueType::Video, None, 0);
        let image = show.make_cue(CueType::Image, None, 0);
        let (vid, iid) = (video.id, image.id);
        show.cues.push(video);
        show.cues.push(image);
        let (mut e, mut st) = (Engine::for_tests(), UiState::default());
        let ctx = egui::Context::default();
        let mut t = 0.0;
        // Draw once to find where things are.
        drag_in_patch(
            &ctx,
            &mut show,
            &mut e,
            &mut st,
            &mut t,
            pos2(1.0, 1.0),
            pos2(1.0, 1.0),
        );
        let r = rects(&ctx);
        let port = |id| r.ports.iter().find(|(c, _)| *c == id).unwrap().1;
        let mid = |name: &str| {
            r.mids
                .iter()
                .find(|(n, _)| n == name)
                .unwrap_or_else(|| {
                    panic!(
                        "{name}: {:?}",
                        r.mids.iter().map(|m| &m.0).collect::<Vec<_>>()
                    )
                })
                .1
                .center()
        };
        let name_of = |show: &Show, id| show.output(id).unwrap().name.clone();
        let second_name = name_of(&show, second);
        // A picture goes to the display it is dropped on.
        drag_in_patch(
            &ctx,
            &mut show,
            &mut e,
            &mut st,
            &mut t,
            port(iid),
            mid(&second_name),
        );
        assert_eq!(
            show.cue(iid).unwrap().kind.media().unwrap().output,
            Some(second)
        );
        // It can go back.
        let first_name = name_of(&show, first);
        drag_in_patch(
            &ctx,
            &mut show,
            &mut e,
            &mut st,
            &mut t,
            port(iid),
            mid(&first_name),
        );
        assert_eq!(
            show.cue(iid).unwrap().kind.media().unwrap().output,
            Some(first)
        );
        // A picture dropped on an audio output is left as it was: it has no sound.
        let a2_name = show
            .audio_outputs
            .iter()
            .find(|o| o.id == a2)
            .unwrap()
            .name
            .clone();
        drag_in_patch(
            &ctx,
            &mut show,
            &mut e,
            &mut st,
            &mut t,
            port(iid),
            mid(&a2_name),
        );
        assert_eq!(
            show.cue(iid).unwrap().kind.media().unwrap().audio_output,
            None
        );
        // A video has both: its sound goes where it is dropped, its picture does not move.
        let picture_before = show.cue(vid).unwrap().kind.media().unwrap().output;
        drag_in_patch(
            &ctx,
            &mut show,
            &mut e,
            &mut st,
            &mut t,
            port(vid),
            mid(&a2_name),
        );
        let m = show.cue(vid).unwrap().kind.media().unwrap();
        assert_eq!(m.audio_output, Some(a2));
        assert_eq!(m.output, picture_before);
        let _ = a1;
        // In show mode nothing is routed.
        st.show_mode = true;
        drag_in_patch(
            &ctx,
            &mut show,
            &mut e,
            &mut st,
            &mut t,
            port(vid),
            mid(&second_name),
        );
        assert_eq!(
            show.cue(vid).unwrap().kind.media().unwrap().output,
            picture_before
        );
    }

    #[test]
    fn every_view_of_the_new_interface_draws_and_the_toggle_is_kept_per_computer() {
        let mut show = Show::default();
        show.add_display();
        show.add_audio_output();
        for ty in [
            CueType::Video,
            CueType::Audio,
            CueType::Image,
            CueType::Wait,
        ] {
            let c = show.make_cue(ty, None, 0);
            show.cues.push(c);
        }
        let (mut e, mut st) = (Engine::for_tests(), UiState::default());
        let ctx = egui::Context::default();
        let mut t = 0.0;
        let mut frame = |show: &mut Show, e: &mut Engine, st: &mut UiState| {
            t += 1.0 / 60.0;
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 900.0))),
                time: Some(t),
                ..Default::default()
            };
            let mut actions = Vec::new();
            let mut out = ctx.run_ui(input, |ui| draw(ui, show, e, st, &mut actions));
            out.textures_delta.clear();
            actions
        };
        for view in View::ALL {
            st.modern_view = view;
            for _ in 0..3 {
                frame(&mut show, &mut e, &mut st);
            }
            // Show mode is drawn too (the inspector and routes are locked).
            st.show_mode = true;
            frame(&mut show, &mut e, &mut st);
            st.show_mode = false;
        }
        // The settings take the place of the view while they are open.
        st.layout.open(Pane::Settings);
        frame(&mut show, &mut e, &mut st);
        st.layout.close(Pane::Settings);
        // Switching is an action that is kept, and off by default.
        assert!(!st.prefs.new_layout);
        apply(Action::SetNewLayout(true), &mut show, &mut e, &mut st);
        assert!(st.prefs.new_layout);
        apply(Action::SetNewLayout(false), &mut show, &mut e, &mut st);
        assert!(!st.prefs.new_layout);
    }
}
