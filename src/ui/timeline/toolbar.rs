//! The editor's toolbar: which timeline it is, its transport (Run, Pause,
//! Stop, the time), Snap, + Marker and the zoom, then how the run is doing
//! (the next pause, what GO will do), last so that its changing length moves
//! nothing.

use super::tracks::Ctx;
use super::*;

/// What GO will do on a paused timeline, for the read-out: "GO: crossfade to …".
pub(super) fn go_text(show: &Show, id: CueId) -> String {
    format!("GO: {}", model::on_go_text(show, id))
}

/// How the run is doing, in a few words.
pub(super) fn state_text(run: &RunView, pos: f64) -> String {
    match run.state {
        RunState::Stopped if run.fading => "Stopping · fading out".into(),
        RunState::Stopped => "Stopped".into(),
        RunState::Running | RunState::Cueing if run.looping.is_some() => {
            match run.looping.map(|l| (l.last_pass, l.at_once)) {
                Some((true, _)) => "Looping · last pass, then on".into(),
                Some((false, true)) => "Looping · GO leaves the loop".into(),
                _ => "Looping · GO continues".into(),
            }
        }
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
    // One row: what comes last (the run state, GO's read-out) is cut short
    // rather than wrapped onto a row that would push the tracks down.
    ui.horizontal(|ui| {
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
        // Padded, so that the buttons after it stay put as the time grows.
        let length = ctx.run.length.map_or("–:––.–".to_string(), fmt_time);
        ui.label(RichText::new(format!("{:>7} / {length:>7}", fmt_time(ctx.pos))).monospace());
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
        let ids = super::keys::split_ids(ctx, st);
        let r = ui
            .add_enabled(ctx.editable && !ids.is_empty(), egui::Button::new("✂ Split"))
            .on_hover_text(super::menus::SPLIT_TIP)
            .on_disabled_hover_text(
                "Put the playhead inside a clip (and pick clips to split only those) to split there",
            );
        note(ui, host, "split", &r);
        if r.clicked() {
            push(
                actions,
                TimelineAction::Split {
                    key,
                    ids,
                    at: ctx.pos,
                },
            );
        }
        let ed = &mut st.timeline;
        let r = ui.selectable_label(ed.ripple, "Ripple").on_hover_text(
            "Deleting a clip moves the later clips on its track up to close the gap \
             (Shift+Delete ripples either way)",
        );
        note(ui, host, "ripple", &r);
        if r.clicked() {
            ed.ripple = !ed.ripple;
        }
        let r = ui
            .add_enabled_ui(ctx.editable, |ui| {
                ui.menu_button("+ Track", |ui| {
                    if ui.button("Picture track").clicked() {
                        push(actions, TimelineAction::AddTrack { key, picture: true });
                        ui.close();
                    }
                    if ui.button("Sound track").clicked() {
                        push(
                            actions,
                            TimelineAction::AddTrack {
                                key,
                                picture: false,
                            },
                        );
                        ui.close();
                    }
                })
            })
            .inner
            .response
            .on_hover_text("Add an empty track (right-click a track for more)");
        note(ui, host, "add_track", &r);
        ui.separator();
        let ed = &mut st.timeline;
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
        // How the run is doing comes last, its length changing as it runs
        // ("Running · next pause in …"): nothing after it to push about, and
        // cut short rather than wrapped onto a row of its own.
        ui.separator();
        let state = state_text(ctx.run, ctx.pos);
        #[cfg(test)]
        ui.data_mut(|d| d.insert_temp(egui::Id::new(("timeline_state", host)), state.clone()));
        ui.add(egui::Label::new(RichText::new(&state).weak()).truncate())
            .on_hover_text(state);
        if matches!(ctx.run.state, RunState::Paused { .. })
            && let Some(id) = ctx.run.on_go
        {
            let go = go_text(ctx.show, id);
            ui.add(
                egui::Label::new(RichText::new(&go).color(super::super::PLAYHEAD)).truncate(),
            )
            .on_hover_text(go);
        }
    });
}
