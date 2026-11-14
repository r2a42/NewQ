//! The editor's toolbar: which timeline it is, its transport (Run, Pause,
//! Stop, the time, the next pause, what GO will do), then Snap, + Marker and
//! the zoom.

use super::tracks::Ctx;
use super::*;
use crate::show::{CueKind, Transition};

/// What GO will do on a paused timeline, for the read-out: "GO: crossfade to …".
pub(super) fn go_text(show: &Show, id: CueId) -> String {
    let Some(c) = show.cue(id) else {
        return "GO: run on".into();
    };
    let what = match &c.kind {
        CueKind::Fade(_) | CueKind::Stop(_) => cue_label(c),
        k => match k.media() {
            Some(m) if m.transition == Transition::Crossfade => {
                format!("crossfade to {}", cue_label(c))
            }
            Some(_) => format!("bring in {}", cue_label(c)),
            None => cue_label(c),
        },
    };
    format!("GO: {what}")
}

/// How the run is doing, in a few words.
pub(super) fn state_text(run: &RunView, pos: f64) -> String {
    match run.state {
        RunState::Stopped => "Stopped".into(),
        RunState::Cueing => "Cueing…".into(),
        RunState::Running => match run.next_pause {
            Some(p) if p >= pos => format!("Running · next pause in {}", fmt_time(p - pos)),
            _ => "Running".into(),
        },
        RunState::Paused {
            freeze: PauseFreeze::Nothing,
            ..
        } => "Paused · content runs on".into(),
        RunState::Paused { .. } if run.parked => "Parked · Run or GO plays from here".into(),
        RunState::Paused { .. } => "Paused".into(),
    }
}

/// Note, for the tests, where a toolbar button is.
pub(super) fn note(ui: &Ui, _host: Host, _name: &'static str, _r: &egui::Response) {
    #[cfg(test)]
    ui.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(&'static str, egui::Rect)>>(egui::Id::new((
            "timeline_buttons",
            _host,
        )))
        .push((_name, _r.rect))
    });
    #[cfg(not(test))]
    let _ = ui;
}

pub(super) fn draw(
    ui: &mut Ui,
    ctx: &Ctx,
    _engine: &mut Engine,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) {
    #[cfg(test)]
    ui.data_mut(|d| {
        d.remove::<Vec<(&'static str, egui::Rect)>>(egui::Id::new(("timeline_buttons", ctx.host)))
    });
    let key = ctx.key;
    let host = ctx.host;
    let push = |actions: &mut Vec<Action>, a| actions.push(Action::Timeline(a));
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("Timeline · {}", timeline_name(ctx.show, key))).strong());
        ui.separator();
        let r = ui
            .button("▶ Run")
            .on_hover_text("Run the timeline from its playhead (or carry on from a pause)");
        note(ui, host, "run", &r);
        if r.clicked() {
            push(actions, TimelineAction::Run(key));
        }
        let r = ui
            .button("⏸ Pause")
            .on_hover_text("Stop the clock and hold every clip (live inputs stay live)");
        note(ui, host, "pause", &r);
        if r.clicked() {
            push(actions, TimelineAction::Pause(key));
        }
        let r = ui
            .button("⏹ Stop")
            .on_hover_text("Stop its clips; the playhead stays where it is");
        note(ui, host, "stop", &r);
        if r.clicked() {
            push(actions, TimelineAction::Stop(key));
        }
        let length = ctx.run.length.map_or("–:––.–".to_string(), fmt_time);
        ui.label(RichText::new(format!("{} / {length}", fmt_time(ctx.pos))).monospace());
        ui.label(RichText::new(state_text(ctx.run, ctx.pos)).weak());
        if matches!(ctx.run.state, RunState::Paused { .. })
            && let Some(id) = ctx.run.on_go
        {
            ui.label(RichText::new(go_text(ctx.show, id)).color(super::super::PLAYHEAD));
        }
        ui.separator();
        let ed = &mut st.timeline;
        let r = ui
            .selectable_label(ed.snap, "Snap")
            .on_hover_text("Dragged clips, edges and markers snap to clip edges, fade-outs, markers and the playhead (hold Alt to drag freely)");
        if r.clicked() {
            ed.snap = !ed.snap;
        }
        let r = ui
            .add_enabled(ctx.editable, egui::Button::new("+ Marker"))
            .on_hover_text("Add a marker at the playhead");
        note(ui, host, "marker", &r);
        if r.clicked() {
            push(actions, TimelineAction::AddMarker { key, at: ctx.pos });
        }
        ui.separator();
        let now = if ed.shown_pps > 0.0 { ed.shown_pps } else { 50.0 };
        if ui.button("−").on_hover_text("Zoom out (Ctrl + wheel)").clicked() {
            ed.pps = (now / 1.5).max(0.05);
        }
        if ui.button("+").on_hover_text("Zoom in (Ctrl + wheel)").clicked() {
            ed.pps = (now * 1.5).min(4000.0);
        }
        if ui
            .selectable_label(ed.pps <= 0.0, "Fit")
            .on_hover_text("Show the whole timeline")
            .clicked()
        {
            ed.pps = 0.0;
            ed.scroll.x = 0.0;
        }
    });
}
