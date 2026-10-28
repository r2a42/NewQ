//! The settings page: every setting of the show in one place, in a sidebar of
//! pages grouped by what they are for. It takes the place of the cue list
//! while it is open, so the transport (GO, Stop All) and the live preview stay
//! where they are and a setting can be tried while the show runs.

use super::*;

/// A page of the settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SettingsPage {
    #[default]
    General,
    Media,
    Displays,
    SubDisplays,
    Audio,
    Sync,
    Osc,
    Midi,
    Web,
}

/// What the pages are grouped under in the sidebar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsGroup {
    Show,
    Output,
    Control,
}

impl SettingsGroup {
    pub const ALL: [SettingsGroup; 3] = [
        SettingsGroup::Show,
        SettingsGroup::Output,
        SettingsGroup::Control,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingsGroup::Show => "SHOW",
            SettingsGroup::Output => "PICTURE AND SOUND",
            SettingsGroup::Control => "REMOTE CONTROL",
        }
    }
}

impl SettingsPage {
    /// In the order the sidebar lists them.
    pub const ALL: [SettingsPage; 9] = [
        SettingsPage::General,
        SettingsPage::Media,
        SettingsPage::Displays,
        SettingsPage::SubDisplays,
        SettingsPage::Audio,
        SettingsPage::Sync,
        SettingsPage::Osc,
        SettingsPage::Midi,
        SettingsPage::Web,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SettingsPage::General => "General",
            SettingsPage::Media => "Media",
            SettingsPage::Displays => "Displays",
            SettingsPage::SubDisplays => "Sub displays",
            SettingsPage::Audio => "Audio outputs",
            SettingsPage::Sync => "Sync",
            SettingsPage::Osc => "OSC",
            SettingsPage::Midi => "MIDI",
            SettingsPage::Web => "Web remote",
        }
    }

    pub fn group(self) -> SettingsGroup {
        match self {
            SettingsPage::General | SettingsPage::Media => SettingsGroup::Show,
            SettingsPage::Displays
            | SettingsPage::SubDisplays
            | SettingsPage::Audio
            | SettingsPage::Sync => SettingsGroup::Output,
            SettingsPage::Osc | SettingsPage::Midi | SettingsPage::Web => SettingsGroup::Control,
        }
    }

    /// One line under the page's title.
    pub fn blurb(self) -> &'static str {
        match self {
            SettingsPage::General => "The show's name and how Stop All behaves.",
            SettingsPage::Media => "The files the cues play, and whether they can all be found.",
            SettingsPage::Displays => {
                "Which monitor each display is on, and how it is shaped, tested and sent out."
            }
            SettingsPage::SubDisplays => {
                "Areas of a display that cues can be sent to as if they were displays."
            }
            SettingsPage::Audio => "Where sound goes: the devices cues can play on.",
            SettingsPage::Sync => "Line the sound up with the picture.",
            SettingsPage::Osc => "Run NewQ from a lighting desk, Companion or other software.",
            SettingsPage::Midi => "Run NewQ from a MIDI controller, or follow MIDI Time Code.",
            SettingsPage::Web => "Run the show from a phone or tablet.",
        }
    }
}

/// What the sidebar shows beside a page: a count, a state, or a warning.
#[derive(Clone, Debug, PartialEq)]
pub struct Badge {
    pub text: String,
    pub color: Color32,
}

fn badge(text: impl Into<String>, color: Color32) -> Option<Badge> {
    Some(Badge {
        text: text.into(),
        color,
    })
}

/// The badge for `page`: a warning in red when something needs attention, a
/// green mark for what is on and working, grey for a count.
pub fn page_badge(page: SettingsPage, show: &Show, st: &UiState) -> Option<Badge> {
    let weak = Color32::from_gray(140);
    match page {
        SettingsPage::General => None,
        SettingsPage::Media => {
            let missing = relink::missing_among(show, &st.missing).len();
            (missing > 0).then(|| Badge {
                text: format!("⚠ {missing}"),
                color: STOP_RED,
            })
        }
        SettingsPage::Displays => {
            let displays: Vec<&OutputConfig> = show
                .outputs
                .iter()
                .filter(|o| !o.is_sub_display() && o.enabled)
                .collect();
            let trouble = displays.iter().any(|d| {
                matches!(
                    st.display_states.get(&d.id),
                    Some(DisplayState::Missing | DisplayState::OnMainWindowDisplay(_))
                )
            });
            if trouble {
                badge("⚠", STOP_RED)
            } else {
                badge(displays.len().to_string(), weak)
            }
        }
        SettingsPage::SubDisplays => {
            let n = show.outputs.iter().filter(|o| o.is_sub_display()).count();
            (n > 0).then(|| Badge {
                text: n.to_string(),
                color: weak,
            })
        }
        SettingsPage::Audio => {
            if show.audio_outputs.is_empty() {
                return None;
            }
            badge(show.audio_outputs.len().to_string(), weak)
        }
        SettingsPage::Sync => {
            let set = show.sync.audio_ms > 0
                || show.sync.video_ms > 0
                || show.audio_outputs.iter().any(|o| o.delay_ms > 0)
                || show.outputs.iter().any(|o| o.delay_ms > 0);
            set.then(|| Badge {
                text: "set".into(),
                color: PLAYHEAD,
            })
        }
        SettingsPage::Osc => remote_badge(
            show.remote.osc,
            st.remote_status.osc_port.is_some(),
            st.remote_status.osc_error.is_some(),
        ),
        SettingsPage::Midi => remote_badge(
            show.remote.midi,
            st.remote_status.midi_port.is_some(),
            st.remote_status.midi_error.is_some(),
        ),
        SettingsPage::Web => remote_badge(
            show.remote.web,
            st.remote_status.web_port.is_some(),
            st.remote_status.web_error.is_some(),
        ),
    }
}

/// Off shows nothing; on and listening is green; a problem is red.
fn remote_badge(enabled: bool, listening: bool, error: bool) -> Option<Badge> {
    match (enabled, error, listening) {
        (false, _, _) => None,
        (true, true, _) => badge("⚠", STOP_RED),
        (true, false, true) => badge("on", GO_GREEN),
        (true, false, false) => badge("…", Color32::from_gray(140)),
    }
}

/// The settings, in place of the cue list.
pub fn settings_view(
    ui: &mut Ui,
    show: &mut Show,
    engine: &mut Engine,
    st: &mut UiState,
    actions: &mut Vec<Action>,
) {
    egui::Panel::left("settings_nav")
        .resizable(false)
        .exact_size(214.0)
        .show(ui, |ui| sidebar(ui, show, st, actions));
    egui::CentralPanel::default().show(ui, |ui| {
        let page = st.settings_page;
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading(page.label());
                ui.label(RichText::new(page.blurb()).weak());
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui
                    .button("Close settings")
                    .on_hover_text("Close the settings (Ctrl+,)")
                    .clicked()
                {
                    actions.push(Action::CloseSettings);
                }
            });
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // A comfortable width and no more, with room for the frames'
                // edges and the scroll bar so nothing is clipped.
                ui.set_max_width((ui.available_width() - 20.0).clamp(300.0, 640.0));
                match page {
                    SettingsPage::General => general_page(ui, show, st),
                    SettingsPage::Media => media_page(ui, show, st, actions),
                    SettingsPage::Displays => displays_panel(ui, show, engine, st, actions),
                    SettingsPage::SubDisplays => sub_displays_panel(ui, show, st, actions),
                    SettingsPage::Audio => audio_panel(ui, show, engine, st, actions),
                    SettingsPage::Sync => sync_page(ui, show, st, actions),
                    SettingsPage::Osc => osc_page(ui, show, st),
                    SettingsPage::Midi => midi_page(ui, show, st, engine),
                    SettingsPage::Web => web_page(ui, show, st),
                }
            });
    });
}

/// The pages, grouped, each a row with its status on the right.
fn sidebar(ui: &mut Ui, show: &Show, st: &mut UiState, actions: &mut Vec<Action>) {
    ui.add_space(6.0);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for group in SettingsGroup::ALL {
                ui.add_space(8.0);
                ui.label(
                    RichText::new(group.label())
                        .small()
                        .strong()
                        .color(Color32::from_gray(130)),
                );
                ui.add_space(2.0);
                for page in SettingsPage::ALL.into_iter().filter(|p| p.group() == group) {
                    let b = page_badge(page, show, st);
                    if nav_row(ui, page.label(), st.settings_page == page, b.as_ref()) {
                        actions.push(Action::OpenSettings(page));
                    }
                }
            }
        });
}

/// One row of the sidebar. True when it was clicked.
fn nav_row(ui: &mut Ui, label: &str, selected: bool, badge: Option<&Badge>) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().visuals.clone();
        if selected {
            ui.painter()
                .rect_filled(rect, 4.0, visuals.selection.bg_fill);
        } else if response.hovered() {
            ui.painter()
                .rect_filled(rect, 4.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let text_color = if selected {
            visuals.selection.stroke.color
        } else {
            visuals.text_color()
        };
        ui.painter().text(
            rect.left_center() + Vec2::new(10.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            egui::FontId::proportional(14.0),
            text_color,
        );
        if let Some(b) = badge {
            ui.painter().text(
                rect.right_center() - Vec2::new(10.0, 0.0),
                egui::Align2::RIGHT_CENTER,
                &b.text,
                egui::FontId::proportional(12.0),
                b.color,
            );
        }
    }
    response.clicked()
}

/// A titled block of a page.
fn section(ui: &mut Ui, title: &str, body: impl FnOnce(&mut Ui)) {
    ui.add_space(6.0);
    ui.label(RichText::new(title).strong());
    ui.add_space(2.0);
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        body(ui);
    });
}

fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).weak().small());
}

fn warning(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).small().color(PREWAIT));
}

fn general_page(ui: &mut Ui, show: &mut Show, st: &UiState) {
    section(ui, "Show", |ui| {
        egui::Grid::new("settings_show")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut show.name).desired_width(260.0));
                ui.end_row();
                ui.label("File");
                match &st.show_path {
                    Some(p) => ui.label(RichText::new(p.display().to_string()).small()),
                    None => ui.label(RichText::new("Not saved yet").weak()),
                };
                ui.end_row();
            });
        hint(
            ui,
            "The name is the one the file is offered under when saving.",
        );
    });
    section(ui, "Stop All", |ui| {
        ui.horizontal(|ui| {
            ui.label("Fade out over");
            secs(ui, &mut show.panic_fade);
        });
        hint(
            ui,
            "How long the Stop All button and Esc take to fade everything out. Pressing Esc \
             twice stops everything at once.",
        );
    });
    section(ui, "Recovery and logs", |ui| {
        ui.label(format!(
            "Unsaved changes are copied to a recovery file every {} seconds, and offered \
             back if NewQ didn't close properly.",
            crate::recovery::INTERVAL.as_secs()
        ));
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Log folder");
            ui.label(RichText::new(crate::crash::log_dir().display().to_string()).small());
        });
        hint(
            ui,
            "newq.log and newq-crash.log are here: send them if NewQ misbehaves.",
        );
    });
}

fn media_page(ui: &mut Ui, show: &Show, st: &mut UiState, actions: &mut Vec<Action>) {
    let files = show
        .cues
        .iter()
        .filter_map(|c| c.kind.file_media())
        .filter(|m| !m.file.as_os_str().is_empty())
        .count();
    let missing = relink::missing_among(show, &st.missing).len();
    section(ui, "Files", |ui| {
        ui.horizontal(|ui| {
            if files == 0 {
                ui.label(RichText::new("No cue plays a file yet.").weak());
            } else if missing == 0 {
                ui.label(RichText::new(format!("All {files} files are in place.")).color(GO_GREEN));
            } else {
                ui.label(
                    RichText::new(format!("{missing} of {files} files can't be found."))
                        .color(STOP_RED),
                );
            }
        });
        ui.add_space(4.0);
        if ui
            .button("Open the media manager…")
            .on_hover_text("Find moved files, or gather every file into one folder.")
            .clicked()
        {
            actions.push(Action::ToggleMedia);
        }
        hint(
            ui,
            "Files are saved in the show relative to it, so a show and its media can be moved \
             together.",
        );
    });
    section(ui, "Programs (this computer)", |ui| {
        let mut on = st.prefs.allow_scripts;
        if ui
            .checkbox(&mut on, "Let send cues run programs on this computer")
            .changed()
        {
            st.prefs.allow_scripts = on;
            crate::output::set_scripts_allowed(on);
            if let Some(dir) = &st.layout_dir
                && let Err(e) = st.prefs.save(dir)
            {
                log::warn!("could not save the settings of this computer: {e}");
            }
        }
        warning(
            ui,
            "A show can come from anywhere and a program can do anything you can. Leave this off \
             unless you have read the send cues of the show.",
        );
    });
    section(ui, "Decoding (this computer)", |ui| {
        let mut on = st.prefs.hw_decode;
        if ui
            .checkbox(
                &mut on,
                "Decode video on the graphics hardware where possible",
            )
            .changed()
        {
            st.prefs.hw_decode = on;
            crate::media::video::set_hw_decode(on);
            if let Some(dir) = &st.layout_dir
                && let Err(e) = st.prefs.save(dir)
            {
                log::warn!("could not save the settings of this computer: {e}");
            }
        }
        hint(
            ui,
            "Takes decoding of 4K and many-layer shows off the processor, at the cost of \
             copying each picture back from the card. Takes effect for cues that start after \
             the change. Try it on this computer with the show you mean to run; a file or card \
             it can't do is decoded by the processor as before.",
        );
    });
}

fn sync_page(ui: &mut Ui, show: &mut Show, st: &mut UiState, actions: &mut Vec<Action>) {
    if let Some(rx) = &st.sync_clip {
        match rx.try_recv() {
            Ok(Ok(path)) => {
                st.sync_clip = None;
                actions.push(Action::AddSyncCue(path));
            }
            Ok(Err(e)) => {
                st.sync_clip = None;
                st.sync_note = format!("Couldn't make the test clip: {e}");
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => st.sync_clip = None,
        }
    }
    let delay = |ui: &mut Ui, ms: &mut u32| {
        ui.add(
            egui::DragValue::new(ms)
                .range(0..=MAX_DELAY_MS)
                .speed(1.0)
                .suffix(" ms"),
        )
        .changed()
    };
    section(ui, "Everything", |ui| {
        ui.label(
            "Hold back whichever comes first. If the picture is ahead of the sound, delay the \
             picture; if the sound is ahead, delay the sound.",
        );
        egui::Grid::new("sync_all").num_columns(2).show(ui, |ui| {
            ui.label("Sound delay");
            delay(ui, &mut show.sync.audio_ms);
            ui.end_row();
            ui.label("Picture delay");
            delay(ui, &mut show.sync.video_ms);
            ui.end_row();
        });
        hint(
            ui,
            "These apply to every output. A delay changes cues as they start.",
        );
    });
    if !show.audio_outputs.is_empty() || !show.outputs.is_empty() {
        section(ui, "One output at a time", |ui| {
            ui.label("On top of that, for one output (a slow projector, a Bluetooth speaker):");
            egui::Grid::new("sync_outputs")
                .num_columns(2)
                .show(ui, |ui| {
                    for o in &mut show.audio_outputs {
                        ui.label(format!("Sound: {}", o.name));
                        delay(ui, &mut o.delay_ms);
                        ui.end_row();
                    }
                    for o in &mut show.outputs {
                        ui.label(format!("Picture: {}", o.name));
                        delay(ui, &mut o.delay_ms);
                        ui.end_row();
                    }
                });
        });
    }
    section(ui, "Test it", |ui| {
        ui.horizontal(|ui| {
            let making = st.sync_clip.is_some();
            if ui
                .add_enabled(!making, egui::Button::new("Add a sync test cue"))
                .on_hover_text(
                    "A looping clip that flashes white and beeps together once a second. Play \
                     it and adjust until the flash and the beep land together.",
                )
                .clicked()
            {
                actions.push(Action::MakeSyncClip);
            }
            if making {
                ui.spinner();
            }
            if ui.button("Reset all delays").clicked() {
                show.sync = SyncSettings::default();
                for o in &mut show.audio_outputs {
                    o.delay_ms = 0;
                }
                for o in &mut show.outputs {
                    o.delay_ms = 0;
                }
            }
        });
        if !st.sync_note.is_empty() {
            ui.label(RichText::new(&st.sync_note).color(STOP_RED).small());
        }
    });
}

/// A line saying how a listener is doing: a problem, what it is doing, or
/// that it is starting.
fn listener_status(
    ui: &mut Ui,
    error: &Option<String>,
    working: Option<String>,
    starting: Option<&str>,
) {
    match (working, error) {
        (_, Some(e)) => {
            ui.label(RichText::new(format!("⚠ {e}")).color(STOP_RED));
        }
        (Some(text), None) => {
            ui.label(RichText::new(text).color(GO_GREEN));
        }
        (None, None) => {
            if let Some(text) = starting {
                ui.label(RichText::new(text).weak());
            }
        }
    }
}

fn osc_page(ui: &mut Ui, show: &mut Show, st: &UiState) {
    let status = &st.remote_status;
    let r = &mut show.remote;
    section(ui, "OSC over UDP", |ui| {
        ui.checkbox(&mut r.osc, "Listen for OSC");
        if r.osc {
            ui.horizontal(|ui| {
                ui.label("Port");
                ui.add(
                    egui::DragValue::new(&mut r.osc_port)
                        .range(1..=65535)
                        .speed(1.0),
                );
            });
            listener_status(
                ui,
                &status.osc_error,
                status
                    .osc_port
                    .map(|p| format!("Listening on UDP port {p}")),
                Some("Starting…"),
            );
            warning(
                ui,
                "Anyone who can reach this port on your network can control the show.",
            );
        }
    });
    section(ui, "Feedback", |ui| {
        ui.label(
            RichText::new(
                "Send the show's state to a controller (a TouchOSC page, a desk): the playhead, \
                 which cues are playing and how far on they are.",
            )
            .weak(),
        );
        ui.horizontal(|ui| {
            ui.label("Send to");
            ui.add(
                egui::TextEdit::singleline(&mut r.osc_feedback)
                    .hint_text("192.168.1.20:9000 (empty = off)")
                    .desired_width(220.0),
            );
        });
        if !r.osc_feedback.trim().is_empty() {
            if let Some(e) = &st.feedback_error {
                warning(ui, e);
            }
            ui.label(
                RichText::new(
                    "/newq/playhead <number> <name>   /newq/paused <0|1>   /newq/running <n>\n\
                     /newq/cue/<number>/status <idle|prewait|playing>\n\
                     /newq/cue/<number>/progress <0 to 1>",
                )
                .monospace()
                .small(),
            );
        }
    });
    if r.osc {
        section(ui, "What it understands", |ui| {
            ui.label(
                RichText::new(
                    "/go  /panic  /stop  /pause  /resume\n/playhead/next  /playhead/previous\n\
                     /cue/<number>/start  go  select  stop  fade",
                )
                .monospace()
                .small(),
            );
        });
    }
}

fn midi_page(ui: &mut Ui, show: &mut Show, st: &UiState, engine: &Engine) {
    let status = &st.remote_status;
    let r = &mut show.remote;
    section(ui, "MIDI input", |ui| {
        ui.checkbox(&mut r.midi, "Listen to a MIDI input");
        if r.midi {
            egui::Grid::new("settings_midi")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Input");
                    egui::ComboBox::from_id_salt("midi_port")
                        .selected_text(r.midi_port.clone().unwrap_or_else(|| "Choose…".into()))
                        .width(240.0)
                        .show_ui(ui, |ui| {
                            for p in &st.midi_ports {
                                ui.selectable_value(&mut r.midi_port, Some(p.clone()), p);
                            }
                        });
                    ui.end_row();
                    ui.label("Channel");
                    let text = r
                        .midi_channel
                        .map_or("Any".to_string(), |c| (c + 1).to_string());
                    egui::ComboBox::from_id_salt("midi_channel")
                        .selected_text(text)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut r.midi_channel, None, "Any");
                            for c in 0..16u8 {
                                ui.selectable_value(
                                    &mut r.midi_channel,
                                    Some(c),
                                    (c + 1).to_string(),
                                );
                            }
                        });
                    ui.end_row();
                    ui.label("Show control device");
                    let text = r.msc_device.map_or("Any".to_string(), |d| d.to_string());
                    egui::ComboBox::from_id_salt("msc_device")
                        .selected_text(text)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut r.msc_device, None, "Any");
                            for d in 0..=111u8 {
                                ui.selectable_value(&mut r.msc_device, Some(d), d.to_string());
                            }
                        })
                        .response
                        .on_hover_text(
                            "Act only on MIDI Show Control sent to this device id (and to \
                             everyone, 127), when several machines share a MIDI line.",
                        );
                    ui.end_row();
                });
            listener_status(
                ui,
                &status.midi_error,
                status
                    .midi_port
                    .as_ref()
                    .map(|p| format!("Listening to {p}")),
                None,
            );
            hint(
                ui,
                "Note on N: fire the cue given that note, else cue N · Program change N: select \
                 cue N\nMIDI Show Control: GO, STOP, RESUME, LOAD, ALL OFF, RESET (cue numbers work)",
            );
        }
    });
    if r.midi {
        section(ui, "MIDI Time Code", |ui| {
            ui.checkbox(&mut r.mtc, "Follow MIDI Time Code")
                .on_hover_text("Fire cues that have a timecode as the incoming time reaches it.");
            if r.mtc {
                match engine.timecode.position() {
                    Some((tc, rate)) if engine.timecode.running() => ui
                        .label(RichText::new(format!("{tc}  ·  {}", rate.label())).color(GO_GREEN)),
                    Some((tc, _)) => ui.label(RichText::new(format!("{tc}  ·  stopped")).weak()),
                    None => ui.label(RichText::new("Waiting for timecode…").weak()),
                };
            }
        });
    }
}

fn web_page(ui: &mut Ui, show: &mut Show, st: &UiState) {
    let status = &st.remote_status;
    let r = &mut show.remote;
    section(ui, "Web remote", |ui| {
        ui.checkbox(&mut r.web, "Serve the web remote")
            .on_hover_text("Serves a page that runs the show from a browser on this network.");
        if r.web {
            egui::Grid::new("settings_web")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Port");
                    ui.add(
                        egui::DragValue::new(&mut r.web_port)
                            .range(1..=65535)
                            .speed(1.0),
                    );
                    ui.end_row();
                    ui.label("PIN");
                    ui.add(
                        egui::TextEdit::singleline(&mut r.web_pin)
                            .password(true)
                            .desired_width(90.0)
                            .hint_text("none"),
                    );
                    ui.end_row();
                });
            // What the page can send: no spaces or accented letters.
            let clean = crate::web::clean_pin(&r.web_pin);
            if clean != r.web_pin {
                r.web_pin = clean;
            }
            let host = crate::web::lan_address()
                .map_or("<this computer>".to_string(), |ip| ip.to_string());
            listener_status(
                ui,
                &status.web_error,
                status
                    .web_port
                    .map(|p| format!("Open http://{host}:{p} on a phone")),
                Some("Starting…"),
            );
            warning(
                ui,
                "GO, Stop All, Pause, the playhead and a button for every cue. Anyone on your \
                 network who can reach the port (and has the PIN, if you set one) can run the \
                 show. The PIN is saved in the show file.",
            );
        }
    });
}
