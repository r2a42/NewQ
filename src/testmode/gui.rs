//! The Test Mode window: choose a test, watch the media being made ready,
//! watch the test run, and read what it found.

use super::analysis::{Severity, Verdict};
use super::assets::{self, State};
use super::plan::{DisplayChoice, SoundChoice};
use super::session::{End, Run, View, clock, open_folder};
use super::{Flavour, Level};
use crate::engine::Engine;
use crate::ui::UiState;
use egui::{Color32, RichText};

const RED: Color32 = Color32::from_rgb(220, 90, 80);
const AMBER: Color32 = Color32::from_rgb(220, 170, 60);
const GREEN: Color32 = Color32::from_rgb(90, 190, 110);

fn megabytes(b: u64) -> String {
    format!("{:.1} MB", b as f64 / 1_048_576.0)
}

/// Draw the window if it is open (called every frame).
pub fn window(ctx: &egui::Context, engine: &Engine, st: &mut UiState) {
    let opened = egui::Id::new("test_window_opened");
    if !st.test.open {
        ctx.data_mut(|d| d.remove::<bool>(opened));
        return;
    }
    let mut open = true;
    let mut test = std::mem::take(&mut st.test);
    let facts = Facts {
        refusal: st.test_refusal(),
        decode_refusal: st.decode_refusal(),
        across: super::session::across(st),
        net: net_facts(st),
    };
    let live = live_lines(&test, st);
    let mut apply: Option<bool> = None;
    let screens = screens_of(st);
    let mut selected = st.prefs.test_displays.clone();
    // Opened with none of the screens offered ticked: all of them are.
    if !ctx.data(|d| d.get_temp::<bool>(opened)).unwrap_or(false) {
        ctx.data_mut(|d| d.insert_temp(opened, true));
        let offered: Vec<&Screen> = screens.iter().filter(|s| !s.is_main).collect();
        if !test.busy() && !offered.iter().any(|s| selected.contains(&s.choice.key)) {
            selected.extend(offered.iter().map(|s| s.choice.key.clone()));
        }
        // The 3D stage part: ticked by itself for a show with a projector or
        // a display that shows the stage (owner's decision 8).
        if !test.busy() {
            test.stage3d = test.stage_wanted;
        }
    }
    let mut sound = (st.prefs.test_sound.clone(), st.prefs.test_muted);
    test.heavy = st.prefs.test_heavy;
    test.load = st.prefs.test_load;
    let devices = st.audio_devices.clone();
    egui::Window::new("Test Mode")
        .open(&mut open)
        .collapsible(false)
        .default_width(520.0)
        .show(ctx, |ui| {
            // Taller than the screen with every computer's screens listed:
            // it scrolls, so the Start button is always within reach.
            egui::ScrollArea::vertical()
                .max_height((ctx.content_rect().height() - 140.0).max(200.0))
                .min_scrolled_height((ctx.content_rect().height() - 140.0).clamp(200.0, 620.0))
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    body(
                        ui,
                        engine,
                        st,
                        &mut test,
                        &facts,
                        &live,
                        &mut apply,
                        &screens,
                        &mut selected,
                        &devices,
                        &mut sound,
                    );
                });
        });
    if !open {
        if test.busy() {
            // The cross is not a way to leave a test half done: it asks to stop.
            test.stop();
            open = true;
        } else {
            test.dismiss();
        }
    }
    test.open = open;
    let heavy = test.heavy;
    let load = test.load;
    let pick = test.pick();
    st.test = test;
    if selected != st.prefs.test_displays
        || sound != (st.prefs.test_sound.clone(), st.prefs.test_muted)
        || heavy != st.prefs.test_heavy
        || load != st.prefs.test_load
        || pick != st.prefs.test_pick
    {
        st.prefs.test_heavy = heavy;
        st.prefs.test_load = load;
        st.prefs.test_pick = pick;
        st.prefs.test_displays = selected;
        (st.prefs.test_sound, st.prefs.test_muted) = sound;
        if let Some(dir) = &st.layout_dir
            && let Err(e) = st.prefs.save(dir)
        {
            log::warn!("could not save the settings of this computer: {e}");
        }
    }
    if let Some(on) = apply {
        // The user asked for what the comparison advised.
        st.prefs.hw_decode = on;
        crate::media::video::set_hw_decode(on);
        if let Some(dir) = &st.layout_dir
            && let Err(e) = st.prefs.save(dir)
        {
            log::warn!("could not save the settings of this computer: {e}");
        }
    }
    if st.test.busy() {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// The statistics window (Tools ▸ Statistics): the figures Test Mode shows
/// while it runs, for an ordinary show. The app samples while it is open.
pub fn stats_window(ctx: &egui::Context, st: &mut UiState) {
    if !st.stats_open {
        return;
    }
    let mut open = true;
    let mut to_file = st.stats_to_file;
    egui::Window::new("Statistics")
        .open(&mut open)
        .default_width(420.0)
        .show(ctx, |ui| {
            let h = &st.live_stats;
            let tail = |f: &dyn Fn(&crate::stats::Sample) -> f32| -> Vec<f32> {
                h[h.len().saturating_sub(120)..].iter().map(f).collect()
            };
            match h.last() {
                None => {
                    ui.label(RichText::new("Measuring…").weak());
                }
                Some(s) => {
                    spark(
                        ui,
                        "Memory",
                        &tail(&|s| s.rss_mb as f32),
                        format!("{} MB", s.rss_mb),
                    );
                    spark(
                        ui,
                        "Processor",
                        &tail(&|s| s.cpu_pct),
                        format!("{:.0}%", s.cpu_pct),
                    );
                    spark(
                        ui,
                        "FFmpeg processes",
                        &tail(&|s| s.ffmpeg as f32),
                        format!("{}", s.ffmpeg),
                    );
                    spark(
                        ui,
                        "FFmpeg processor",
                        &tail(&|s| s.ffmpeg_cpu_pct),
                        format!("{:.0}%", s.ffmpeg_cpu_pct),
                    );
                    spark(
                        ui,
                        "Slowest frame",
                        &tail(&|s| s.stall_ms as f32),
                        format!("{} ms", s.stall_ms),
                    );
                    spark(
                        ui,
                        "Late frames",
                        &tail(&|s| s.inputs.late_permille as f32 / 10.0),
                        format!("{:.1}%", s.inputs.late_permille as f32 / 10.0),
                    );
                    spark(
                        ui,
                        "Audio load",
                        &tail(&|s| s.inputs.audio_permille as f32 / 10.0),
                        format!("{:.1}%", s.inputs.audio_permille as f32 / 10.0),
                    );
                    ui.label(format!(
                        "Sound dropouts since NewQ started: {}",
                        s.inputs.audio_errors
                    ));
                    ui.label(format!(
                        "{} cue{} playing, {} picture layer{}",
                        s.inputs.playing,
                        if s.inputs.playing == 1 { "" } else { "s" },
                        s.inputs.layers,
                        if s.inputs.layers == 1 { "" } else { "s" },
                    ));
                }
            }
            let mut rates: Vec<_> = st.output_fps.iter().collect();
            rates.sort_by_key(|(id, _)| id.0);
            for (id, f) in rates {
                let name = st
                    .previews
                    .iter()
                    .find(|p| p.id == *id)
                    .map_or("A display", |p| p.name.as_str());
                ui.label(format!("{name}: {} on its display", crate::fps::text(*f)));
            }
            ui.separator();
            ui.checkbox(&mut to_file, "Also write a log file")
                .on_hover_text("Appends a row every few seconds to newq-stats.csv in the log folder (the same file the NEWQ_STATS setting makes).");
            ui.label(
                RichText::new(
                    "Measured every 2 seconds while this window is open, and not at all when it is closed. Test Mode has its own, fuller log.",
                )
                .weak()
                .small(),
            );
        });
    st.stats_to_file = to_file;
    st.stats_open = open;
    if st.stats_open {
        ctx.request_repaint_after(std::time::Duration::from_secs(1));
    }
}

/// Each computer's line while a test runs: in step or not, each output's
/// frames a second, and anything wrong (the flag: something is).
fn live_lines(test: &super::TestMode, st: &UiState) -> Vec<(String, bool)> {
    let now = crate::net::time::local_now();
    let name = if st.net.me.on {
        st.net.name.clone()
    } else {
        "This computer".to_string()
    };
    let mut v = match test.view() {
        View::Network(r) => return r.live_lines(st, now),
        View::Running(r) => vec![super::network::own_line(&name, &st.drawing, &r.hitches)],
        View::Checking(r) => vec![super::network::own_line(&name, &st.drawing, &r.hitches)],
        _ => Vec::new(),
    };
    if let Some(f) = test.fleet() {
        v.extend(f.live_lines(st, now));
    }
    v
}

/// The live lines, each in its colour.
fn live_section(ui: &mut egui::Ui, live: &[(String, bool)]) {
    for (line, warn) in live {
        if *warn {
            ui.colored_label(AMBER, line);
        } else {
            ui.label(RichText::new(line).small());
        }
        #[cfg(test)]
        note_net(ui, line, egui::Rect::NOTHING);
    }
}

/// What the window knows of this computer and its show network.
struct Facts {
    /// Why no test can start now (unsaved show, not in step, a follower).
    refusal: Option<String>,
    decode_refusal: Option<String>,
    /// Tests started here play on every computer of the show network.
    across: bool,
    net: super::choice::NetFacts,
}

fn net_facts(st: &UiState) -> super::choice::NetFacts {
    let v = &st.net;
    super::choice::NetFacts {
        primary: v.me.on && v.is_primary(),
        computers: v
            .peers
            .iter()
            .filter(|p| p.ours && p.connected)
            .map(|p| (p.name.clone(), p.status.as_ref().is_some_and(|s| s.in_step)))
            .collect(),
    }
}

#[allow(clippy::too_many_arguments)]
fn body(
    ui: &mut egui::Ui,
    engine: &Engine,
    st: &mut UiState,
    test: &mut super::TestMode,
    facts: &Facts,
    live: &[(String, bool)],
    apply: &mut Option<bool>,
    screens: &[Screen],
    selected: &mut Vec<String>,
    devices: &[String],
    sound: &mut (Option<String>, bool),
) {
    let log_dir = crate::crash::log_dir();
    let mut dismiss = false;
    let mut failed = None;
    // A run of several parts: where it is, over whatever the part shows.
    let unchanged = match test.sequence() {
        Some((i, n, part)) if test.busy() => {
            net_label(
                ui,
                RichText::new(format!("Part {} of {n}: {}", i + 1, part.name()))
                    .strong()
                    .color(Color32::LIGHT_BLUE),
            );
            if i > 0 {
                "Your show comes back when the test ends."
            } else {
                "Nothing about your show has changed."
            }
        }
        _ => "Nothing about your show has changed.",
    };
    match test.view() {
        View::Idle => idle(
            ui, engine, test, facts, &log_dir, screens, selected, devices, sound,
        ),
        View::NetPreparing(p) => {
            ui.label(RichText::new("Getting the network test ready").strong());
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(super::network::prep_line(p));
            });
            if ui.button("Cancel").clicked() {
                test.stop();
            }
            ui.label(RichText::new(unchanged).weak().small());
        }
        View::Network(r) => {
            if network_running(ui, r, live) {
                test.stop();
            }
        }
        View::Spreading(s, f) => {
            net_label(
                ui,
                RichText::new(format!("{} test: sending it to the computers", s.name)).strong(),
            );
            ui.horizontal(|ui| {
                ui.spinner();
                net_label(
                    ui,
                    format!(
                        "Waiting until every computer has the test's media and is in step ({})",
                        clock(s.started.elapsed())
                    ),
                );
            });
            for line in f.media_lines() {
                net_label(ui, line);
            }
            let b = ui.button("Cancel");
            #[cfg(test)]
            note_net(ui, "Cancel", b.rect);
            if b.clicked() {
                test.stop();
            }
            net_label(
                ui,
                RichText::new("Cancelling puts your show back on every computer.")
                    .weak()
                    .small(),
            );
        }
        View::Closing => {
            ui.horizontal(|ui| {
                ui.spinner();
                net_label(ui, "Collecting the computers' last measurements and logs…");
            });
        }
        View::Preparing(p) => {
            let st = p.fetch.status.lock();
            ui.label(
                RichText::new(format!(
                    "Getting the {} test ready",
                    p.flavour.name(p.level)
                ))
                .strong(),
            );
            let total: u64 = st.items.iter().map(|i| i.bytes).sum();
            let done: u64 = st
                .items
                .iter()
                .map(|i| match &i.state {
                    State::Waiting => 0,
                    State::Downloading { done, .. } => *done,
                    _ => i.bytes,
                })
                .sum();
            ui.add(
                egui::ProgressBar::new((done as f32 / total.max(1) as f32).clamp(0.0, 1.0))
                    .text(format!("{} of {}", megabytes(done), megabytes(total)))
                    .animate(true),
            );
            if st.offline {
                ui.colored_label(
                    AMBER,
                    "Can't reach the internet: the media that can't be downloaded is being made on this computer instead.",
                );
            }
            ui.add_space(4.0);
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    for i in &st.items {
                        ui.horizontal(|ui| {
                            let (text, colour) = match &i.state {
                                State::Waiting => ("waiting".to_string(), Color32::GRAY),
                                State::Downloading { done, total } => (
                                    format!(
                                        "downloading {} of {}",
                                        megabytes(*done),
                                        megabytes(*total)
                                    ),
                                    Color32::LIGHT_BLUE,
                                ),
                                State::Checking => ("checking".to_string(), Color32::LIGHT_BLUE),
                                State::Ready(s) => (s.label().to_string(), GREEN),
                            };
                            ui.label(i.title);
                            ui.colored_label(colour, text);
                        });
                    }
                });
            if let Some(n) = st.notes.last() {
                ui.label(RichText::new(n).small().weak());
            }
            drop(st);
            if ui.button("Cancel").clicked() {
                test.stop();
            }
            ui.label(RichText::new(unchanged).weak().small());
        }
        View::Comparing(c) => {
            comparing(ui, c);
            ui.separator();
            if ui.button("Stop the comparison").clicked() {
                test.stop();
            }
        }
        View::Running(r) => {
            running(ui, r);
            if let Some(s) = &r.stage {
                ui.separator();
                stage_running(ui, st, s, r.elapsed().as_secs_f64());
            }
            if !live.is_empty() {
                ui.separator();
                live_section(ui, live);
            }
            ui.separator();
            if ui.button("Stop the test").clicked() {
                test.stop();
            }
        }
        View::PreparingFull(p) => {
            ui.label(RichText::new("Getting the full check ready").strong());
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!(
                    "Getting its film and pictures ready and cutting its clips ({} s)…",
                    p.since.elapsed().as_secs()
                ));
            });
            if ui.button("Cancel").clicked() {
                test.stop();
            }
            ui.label(RichText::new(unchanged).weak().small());
        }
        View::Checking(r) => {
            checking(ui, r);
            if !live.is_empty() {
                ui.separator();
                live_section(ui, live);
            }
            ui.separator();
            if ui.button("Stop the test").clicked() {
                test.stop();
            }
            ui.label(
                RichText::new("Stopping still makes the zip, with what was done so far.")
                    .weak()
                    .small(),
            );
        }
        View::Done(d) => {
            let (colour, word) = match d.verdict {
                Verdict::Passed => (GREEN, "Passed"),
                Verdict::PassedWithNotes => (AMBER, "Passed, with notes"),
                Verdict::ProblemsFound => (RED, "Problems found"),
            };
            let combined = !d.parts.is_empty();
            if combined {
                // The whole run first, then a section per part.
                ui.horizontal(|ui| {
                    ui.label(RichText::new(word).size(22.0).strong().color(colour));
                    ui.label(format!("{} parts, {}", d.parts.len(), clock(d.elapsed)));
                });
                net_label(ui, RichText::new(&d.headline).strong());
                // (The window itself scrolls: no list in a list.)
                for (i, p) in d.parts.iter().enumerate() {
                    part_section(ui, i, p);
                }
            } else if let Some(cmp) = &d.compare {
                use super::decode::Advice;
                let (c, w) = match cmp.advice {
                    Advice::TurnOn => (GREEN, "Turn it on"),
                    Advice::LeaveOff => (RED, "Leave it off"),
                    Advice::NoDifference => (AMBER, "Little difference"),
                    Advice::Inconclusive => (Color32::GRAY, "Inconclusive"),
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(w).size(22.0).strong().color(c));
                    ui.label(format!("Hardware decoding, {}", clock(d.elapsed)));
                });
                let change = match (cmp.advice, cmp.was_on) {
                    (Advice::TurnOn, false) => Some((true, "Turn hardware decoding on")),
                    (Advice::LeaveOff, true) => Some((false, "Turn hardware decoding off")),
                    _ => None,
                };
                if let Some((to, label)) = change
                    && ui.button(label).clicked()
                {
                    *apply = Some(to);
                }
                egui::Grid::new("decode-table")
                    .num_columns(5)
                    .show(ui, |ui| {
                        for h in ["", "Processor", "Late frames", "Longest stall", "Memory"] {
                            ui.label(RichText::new(h).weak().small());
                        }
                        ui.end_row();
                        for (name, s) in [("Off", &cmp.off), ("On", &cmp.on)] {
                            ui.label(name);
                            match s {
                                Some(s) => {
                                    ui.label(format!("{:.0}% of a core", s.total_cpu()));
                                    ui.label(format!("{:.1}%", s.late_permille / 10.0));
                                    ui.label(format!("{} ms", s.stall_ms));
                                    ui.label(format!("{} MB", s.rss_mb));
                                }
                                None => {
                                    ui.label("not measured");
                                }
                            }
                            ui.end_row();
                        }
                    });
                ui.label(
                    RichText::new(match &cmp.engaged {
                        Some(how) => format!("FFmpeg said: {how}"),
                        None => "FFmpeg did not say it used a hardware decoder.".to_string(),
                    })
                    .weak()
                    .small(),
                );
            } else if let Some(n) = &d.network {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(word).size(22.0).strong().color(colour));
                    ui.label(format!(
                        "{}, {}",
                        super::network::title(n.laps),
                        clock(d.elapsed)
                    ));
                });
                computers_results(ui, &n.computers);
            } else if let Some(kinds) = &d.full {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(word).size(22.0).strong().color(colour));
                    ui.label(format!("Full check, {}", clock(d.elapsed)));
                });
                kinds_table(ui, kinds);
            } else {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(word).size(22.0).strong().color(colour));
                    ui.label(format!(
                        "{} test, {}, {} lap{}",
                        d.flavour.name(d.level),
                        clock(d.elapsed),
                        d.laps,
                        if d.laps == 1 { "" } else { "s" }
                    ));
                });
            }
            // Across a show network: a part per computer.
            if !d.computers.is_empty() {
                computers_results(ui, &d.computers);
            }
            // What no test across computers can see, said once.
            let across = d.network.is_some()
                || !d.computers.is_empty()
                || d.parts
                    .iter()
                    .filter_map(|p| p.done.as_ref())
                    .any(|p| p.network.is_some() || !p.computers.is_empty());
            if across {
                ui.label(RichText::new(super::net_judge::CANNOT_SEE).weak().small());
            }
            if !combined {
                ui.label(&d.headline);
                if matches!(d.ended, End::Stopped) {
                    ui.label(RichText::new("You stopped it early.").weak());
                }
            }
            ui.colored_label(
                GREEN,
                if across {
                    "Your show is back exactly as it was, here and (through the show network) on every computer. Nothing was saved or changed."
                } else {
                    "Your show is back exactly as it was. Nothing was saved or changed."
                },
            );
            ui.separator();
            if !combined {
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| findings_list(ui, &d.findings, d.steady));
                ui.separator();
            }
            // The one file to send: everything above and NewQ's own log.
            match d.bundle.get() {
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Packing everything into one zip…");
                    });
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(200));
                }
                Some(Ok(zip)) => {
                    ui.label(RichText::new("One file to send").strong());
                    let mut path = zip.display().to_string();
                    ui.add(egui::TextEdit::singleline(&mut path).desired_width(f32::INFINITY));
                    ui.horizontal(|ui| {
                        if ui
                            .button(RichText::new("Show the zip in its folder").strong())
                            .clicked()
                            && let Err(e) = super::bundle::reveal(&zip)
                        {
                            failed = Some(e);
                        }
                        if ui.button("Copy its path").clicked() {
                            ui.ctx().copy_text(zip.display().to_string());
                        }
                    });
                    ui.label(
                        RichText::new(
                            "The report, the events, the measurements, this computer and its settings, NewQ's log and any crash reports. It stays on this computer: nothing is sent anywhere unless you send it.",
                        )
                        .weak()
                        .small(),
                    );
                    if across {
                        let text = logs_inside(d);
                        ui.label(RichText::new(&text).small());
                        #[cfg(test)]
                        note_net(ui, &text, egui::Rect::NOTHING);
                    }
                }
                Some(Err(why)) => {
                    ui.colored_label(AMBER, format!("The zip could not be made: {why}"));
                }
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                // The folder is only left when the zip couldn't hold it.
                if d.folder.exists() {
                    if ui.small_button("Open the run's folder").clicked()
                        && let Err(e) = open_folder(&d.folder)
                    {
                        failed = Some(e);
                    }
                    if ui.small_button("Copy its path").clicked() {
                        ui.ctx().copy_text(d.folder.display().to_string());
                    }
                }
                if ui.button("Run another test").clicked() {
                    dismiss = true;
                }
            });
            ui.collapsing("Full report", |ui| {
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| {
                        ui.add(egui::Label::new(
                            RichText::new(&d.report).monospace().small(),
                        ));
                    });
            });
        }
    }
    if dismiss {
        test.dismiss();
    }
    if failed.is_some() {
        test.notice = failed;
    }
    if let Some(n) = test.notice.clone() {
        ui.separator();
        ui.colored_label(AMBER, n);
    }
}

/// The window before a test: a length, what to test, the screens, the
/// sound, Start, and the rarely needed things under More.
#[allow(clippy::too_many_arguments)]
fn idle(
    ui: &mut egui::Ui,
    engine: &Engine,
    test: &mut super::TestMode,
    facts: &Facts,
    log_dir: &std::path::Path,
    screens: &[Screen],
    selected: &mut Vec<String>,
    devices: &[String],
    sound: &mut (Option<String>, bool),
) {
    use super::choice;
    let net_why = choice::network_reason(&facts.net);
    let net_ok = net_why.is_none();
    ui.label("Plays a show of its own, then puts yours back exactly as it was.");
    ui.add_space(4.0);

    // 1. How long.
    let plan = choice::plan_with(test.level, test.parts, test.stage3d, net_ok);
    egui::Grid::new("test-choices")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label(RichText::new("Length").strong());
            let r = egui::ComboBox::from_id_salt("test-length")
                .selected_text(choice::level_item_with(
                    test.level,
                    test.parts,
                    test.stage3d,
                    net_ok,
                ))
                .width(220.0)
                .show_ui(ui, |ui| {
                    for level in Level::ALL {
                        let r = ui.selectable_label(
                            test.level == level,
                            choice::level_item_with(level, test.parts, test.stage3d, net_ok),
                        );
                        #[cfg(test)]
                        note_choice(ui, format!("level:{}", level.label()), &r);
                        if r.clicked() {
                            test.set_level(level);
                        }
                    }
                });
            #[cfg(test)]
            note_choice(ui, "Length".into(), &r.response);
            let _ = r;
            ui.end_row();

            // 2. What to test.
            ui.label(RichText::new("Test").strong());
            ui.horizontal(|ui| {
                let a = ui.checkbox(&mut test.parts.cue_list, "Cue list");
                let b = ui.checkbox(&mut test.parts.timeline, "Timeline");
                let d = ui.checkbox(&mut test.stage3d, "3D stage").on_hover_text(
                    "Whether the 3D stage makes the displays miss pictures on this computer: the same lap with previews off, with a turning 3D view, with its light on, and with a display showing the stage.",
                );
                let c = ui
                    .add_enabled_ui(net_ok, |ui| {
                        ui.checkbox(&mut test.parts.network, "Network")
                    })
                    .inner;
                let c = match &net_why {
                    Some(why) => c.on_disabled_hover_text(why.as_str()),
                    None => c.on_hover_text(
                        "How closely every computer keeps in time with this one, with a Sync flash to film.",
                    ),
                };
                #[cfg(test)]
                {
                    note_choice(ui, "Cue list".into(), &a);
                    note_choice(ui, "Timeline".into(), &b);
                    note_choice(ui, "3D stage".into(), &d);
                    note_choice(ui, "Network".into(), &c);
                }
                let _ = (a, b, c, d);
            });
            ui.end_row();
        });
    // Why Network can't be ticked (under the ticks), then one line: what the
    // run is, and how long.
    if let Some(why) = &net_why {
        net_label(ui, RichText::new(format!("Network: {why}")).weak().small());
    } else if facts.across {
        let names: Vec<&str> = facts
            .net
            .computers
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
        net_label(
            ui,
            RichText::new(format!(
                "The cue list and timeline play on {} too, and each measures itself.",
                choice::and_list(&names)
            ))
            .weak()
            .small(),
        );
    }
    if test.stage3d && test.stage_unscreened {
        net_label(
            ui,
            RichText::new(
                "Your show has no 3D stage: the 3D stage part uses a sample venue with projectors, on the test's copy of your show only.",
            )
            .weak()
            .small(),
        );
    }
    let summary = choice::summary(&plan);
    if !summary.is_empty() {
        net_label(ui, RichText::new(summary).weak());
        ui.collapsing("What does this do?", |ui| {
            for p in &plan {
                ui.label(RichText::new(choice::what_it_does(p)).weak().small());
            }
            if plan.iter().any(|p| p.part == choice::Part::Timeline) {
                ui.label(
                    RichText::new(
                        "Your hands aren't needed: where the timeline waits for GO, the test presses it.",
                    )
                    .weak()
                    .small(),
                );
            }
        });
    }
    ui.add_space(4.0);

    // 3. Where it shows.
    let offered = screens.iter().filter(|s| !s.is_main).count();
    let ticked = chosen(screens, selected).len();
    displays_section(ui, screens, selected);
    ui.add_space(4.0);

    // 4. Sound.
    sound_section(ui, devices, sound);
    ui.add_space(6.0);

    // 5. Start.
    let why = choice::start_refusal(&plan, offered, ticked)
        .map(str::to_string)
        .or_else(|| facts.refusal.clone())
        .or_else(|| {
            engine.ff.is_none().then(|| {
                "FFmpeg wasn't found, so there is nothing to play with. See Settings.".into()
            })
        });
    if let Some(why) = &why {
        net_label(ui, RichText::new(why.as_str()).color(AMBER));
    }
    let start = ui.add_enabled(
        why.is_none(),
        egui::Button::new(RichText::new("Start").strong()).min_size(egui::vec2(120.0, 28.0)),
    );
    #[cfg(test)]
    note_choice(ui, "Start".into(), &start);
    if start.clicked()
        && let Some(ff) = engine.ff.clone()
    {
        test.start_plan(
            ff,
            log_dir,
            plan,
            chosen(screens, selected),
            sound_of(devices, sound),
        );
    }
    ui.add_space(4.0);

    // 6. More: what is rarely needed.
    let more = egui::CollapsingHeader::new("More")
        .id_salt("test-more")
        .default_open(false)
        .show(ui, |ui| {
            more(
                ui, engine, test, facts, log_dir, screens, selected, devices, sound,
            )
        });
    #[cfg(test)]
    note_choice(ui, "More".into(), &more.header_response);
    let _ = more;
}

/// Under More: the downloads, the heavier lap, the check of every feature,
/// the decoding comparison and where the files go.
#[allow(clippy::too_many_arguments)]
fn more(
    ui: &mut egui::Ui,
    engine: &Engine,
    test: &mut super::TestMode,
    facts: &Facts,
    log_dir: &std::path::Path,
    screens: &[Screen],
    selected: &[String],
    devices: &[String],
    sound: &(Option<String>, bool),
) {
    let dir = assets::cache_dir(log_dir);
    let mut wanted = assets::wanted_for(test.level, Flavour::Cues);
    for a in assets::wanted_for(test.level, Flavour::Timeline) {
        if !wanted.iter().any(|w| w.id == a.id) {
            wanted.push(a);
        }
    }
    let need = assets::download_size(&dir, &wanted);
    ui.checkbox(
        &mut test.download,
        "Download real films and pictures (free to use)",
    );
    ui.label(
        RichText::new(match (test.download, need) {
            (true, 0) => "They are already on this computer.".to_string(),
            (true, n) => format!("About {} once, kept for next time.", megabytes(n)),
            (false, _) => "Plain pictures and tones are made on this computer instead.".into(),
        })
        .weak()
        .small(),
    );
    ui.collapsing("Where from?", |ui| {
        ui.label(
            RichText::new(format!(
                "test-videos.co.uk, download.blender.org and images-assets.nasa.gov (Blender films under CC-BY, NASA pictures in the public domain). Kept in {}.",
                dir.display()
            ))
            .weak()
            .small(),
        );
    });
    ui.checkbox(&mut test.heavy, "Heavier load for the graphics card")
        .on_hover_text("The cue list part blends three more films the size of the picture, with effects, over the others: late frames then show where this computer's limit is.");
    load_choice(ui, test, &facts.net);
    ui.separator();
    let can = facts.refusal.is_none() && engine.ff.is_some();
    ui.horizontal(|ui| {
        let r = ui.add_enabled(can, egui::Button::new("Check every feature"));
        #[cfg(test)]
        note_choice(ui, "Check every feature".into(), &r);
        if r.clicked()
            && let Some(ff) = engine.ff.clone()
        {
            test.start_full(
                ff,
                log_dir,
                chosen(screens, selected),
                sound_of(devices, sound),
            );
        }
        ui.label(
            RichText::new("About 2 minutes: every kind of cue, each checked.")
                .weak()
                .small(),
        );
    });
    let can_decode = facts.decode_refusal.is_none() && engine.ff.is_some();
    ui.horizontal(|ui| {
        let r = ui
            .add_enabled(can_decode, egui::Button::new("Compare hardware decoding"))
            .on_disabled_hover_text(facts.decode_refusal.clone().unwrap_or_default());
        #[cfg(test)]
        note_choice(ui, "Compare hardware decoding".into(), &r);
        if r.clicked()
            && let Some(ff) = engine.ff.clone()
        {
            test.start_compare(ff, log_dir, chosen(screens, selected));
        }
        ui.label(
            RichText::new("About 2 minutes: says whether to turn it on.")
                .weak()
                .small(),
        );
    });
    if facts.refusal.is_none() && facts.decode_refusal.is_some() {
        ui.label(
            RichText::new("The comparison runs on a computer alone, with the show network off.")
                .weak()
                .small(),
        );
    }
    ui.label(
        RichText::new(format!(
            "Each test leaves one zip in {}",
            log_dir.join("test-mode").join("runs").display()
        ))
        .weak()
        .small(),
    );
}

/// Why the network can't be loaded; None: it can.
pub fn load_reason(net: &super::choice::NetFacts) -> Option<&'static str> {
    (!net.primary || net.computers.is_empty())
        .then_some("Needs another computer in the show network.")
}

/// Under More: how hard to load the show network while a test runs across
/// it (`net::load`), greyed with the reason when there is no other computer.
fn load_choice(ui: &mut egui::Ui, test: &mut super::TestMode, net: &super::choice::NetFacts) {
    use crate::net::load::LoadLevel;
    let why = load_reason(net);
    ui.horizontal(|ui| {
        ui.label("Load the network while testing");
        let r = ui
            .add_enabled_ui(why.is_none(), |ui| {
                egui::ComboBox::from_id_salt("test-load")
                    .selected_text(test.load.label())
                    .width(190.0)
                    .show_ui(ui, |ui| {
                        for level in LoadLevel::ALL {
                            let r = ui.selectable_label(test.load == level, level.label());
                            #[cfg(test)]
                            note_choice(ui, format!("load:{}", level.short()), &r);
                            if r.clicked() {
                                test.load = level;
                            }
                        }
                    })
                    .response
            })
            .inner;
        let r = match why {
            Some(why) => r.on_disabled_hover_text(why),
            None => r.on_hover_text(
                "Made-up bytes go both ways between this computer and each other one, on a connection of their own, for as long as the test plays: compare a run with it and one without. Nothing is downloaded or saved.",
            ),
        };
        #[cfg(test)]
        note_choice(ui, "Load the network".into(), &r);
        let _ = r;
    });
    if let Some(why) = why {
        net_label(
            ui,
            RichText::new(format!("Network load: {why}")).weak().small(),
        );
    } else if !test.load.is_off() {
        net_label(
            ui,
            RichText::new(format!(
                "Each computer: {}. The report says how much went each way.",
                test.load.describe()
            ))
            .weak()
            .small(),
        );
    }
}

/// For the tests: the network test's words and buttons as drawn.
#[cfg(test)]
fn note_net(ui: &egui::Ui, name: &str, r: egui::Rect) {
    ui.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(String, egui::Rect)>>(egui::Id::new("test_net"))
            .push((name.to_string(), r))
    });
}

fn net_label(ui: &mut egui::Ui, text: impl Into<RichText>) {
    let t: RichText = text.into();
    #[cfg(test)]
    let s = t.text().to_string();
    let _r = ui.label(t);
    #[cfg(test)]
    note_net(ui, &s, _r.rect);
}

/// The network test under way. True when the user asked to stop it.
fn network_running(ui: &mut egui::Ui, r: &super::network::NetRun, live: &[(String, bool)]) -> bool {
    use super::network::Stage;
    let now = crate::net::time::local_now();
    let title = super::network::title(r.laps);
    match r.stage {
        Stage::WaitingForMedia => {
            net_label(
                ui,
                RichText::new(format!(
                    "{title}: sending the test's media to the computers"
                ))
                .strong(),
            );
            ui.horizontal(|ui| {
                ui.spinner();
                net_label(
                    ui,
                    format!(
                        "Waiting until every computer has all of it ({})",
                        clock(r.started.elapsed())
                    ),
                );
            });
            for line in r.media_lines() {
                net_label(ui, line);
            }
            if !r.notes.is_empty() {
                for n in &r.notes {
                    net_label(ui, RichText::new(n).weak().small());
                }
            }
        }
        Stage::Playing => {
            let played = r.played(now);
            net_label(ui, RichText::new(format!("{title}: playing")).strong());
            ui.add(
                egui::ProgressBar::new((played / r.planned.max(1.0)).clamp(0.0, 1.0) as f32).text(
                    format!(
                        "{} of {}",
                        clock(std::time::Duration::from_secs_f64(played)),
                        clock(std::time::Duration::from_secs_f64(r.planned))
                    ),
                ),
            );
            let lap = played % super::network::LAP_SECS;
            if lap < 25.0 || (80.0..110.0).contains(&lap) {
                net_label(
                    ui,
                    RichText::new(
                        "The Sync flash is up: film two displays at once in slow motion now.",
                    )
                    .color(Color32::LIGHT_BLUE),
                );
            }
            live_section(ui, live);
        }
        Stage::Ending { .. } => {
            ui.horizontal(|ui| {
                ui.spinner();
                net_label(ui, "Collecting the computers' last measurements and logs…");
            });
        }
    }
    ui.separator();
    let label = match r.stage {
        Stage::WaitingForMedia => "Cancel",
        _ => "Stop the test",
    };
    let b = ui.button(label);
    #[cfg(test)]
    note_net(ui, label, b.rect);
    net_label(
        ui,
        RichText::new("Stopping puts your show back on every computer and still makes the zip.")
            .weak()
            .small(),
    );
    b.clicked()
}

/// What a test found, a line each (and the steady line, if any).
fn findings_list(ui: &mut egui::Ui, findings: &[super::analysis::Finding], steady: Option<&str>) {
    if findings.is_empty() {
        ui.label("Nothing to report.");
    }
    if let Some(t) = steady {
        ui.colored_label(GREEN, t);
    }
    for f in findings {
        let (c, t) = match f.severity {
            Severity::Problem => (RED, "Problem"),
            Severity::Note => (Color32::GRAY, "Note"),
            Severity::Watch => (Color32::LIGHT_BLUE, "Tracked"),
        };
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(c, t);
            ui.label(&f.text);
        });
    }
}

fn verdict_colour(v: Verdict) -> Color32 {
    match v {
        Verdict::Passed => GREEN,
        Verdict::PassedWithNotes => AMBER,
        Verdict::ProblemsFound => RED,
    }
}

/// One part of a run of several: its title and verdict, its headline, each
/// computer's line (details folded), and what it found (folded).
fn part_section(ui: &mut egui::Ui, i: usize, p: &super::sequence::PartDone) {
    use super::sequence::{part_title, part_word};
    let colour = p
        .done
        .as_ref()
        .map_or(Color32::GRAY, |d| verdict_colour(d.verdict));
    let title = part_title(&p.part);
    let word = part_word(p);
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(&title).strong());
        ui.colored_label(colour, &word);
    });
    #[cfg(test)]
    note_net(ui, &format!("part:{title}: {word}"), egui::Rect::NOTHING);
    let Some(d) = &p.done else {
        return;
    };
    ui.indent(("test-part", i), |ui| {
        ui.label(&d.headline);
        if let Some(n) = &d.network {
            computers_results(ui, &n.computers);
        } else if !d.computers.is_empty() {
            computers_results(ui, &d.computers);
        }
        let shown = d.findings.len();
        egui::CollapsingHeader::new(if shown == 0 {
            "Nothing else to report".to_string()
        } else {
            format!("What it found ({shown})")
        })
        .id_salt(("test-part-findings", i))
        .default_open(false)
        .show(ui, |ui| findings_list(ui, &d.findings, d.steady));
    });
}

/// The finished window's line on the other computers' logs: each is in the
/// zip (`fleet::log_path`), since a display computer makes no zip of its own.
pub fn logs_inside(d: &super::session::Done) -> String {
    // Each computer once, by name: its log's place if it came in any part.
    let mut seen: Vec<(&str, Option<&str>)> = Vec::new();
    let all = std::iter::once(d).chain(d.parts.iter().filter_map(|p| p.done.as_deref()));
    for done in all {
        let net = done.network.iter().flat_map(|n| n.computers.iter());
        for c in net.chain(done.computers.iter()) {
            match seen.iter_mut().find(|(n, _)| *n == c.name) {
                Some((_, log)) => *log = log.or(c.log.as_deref()),
                None => seen.push((&c.name, c.log.as_deref())),
            }
        }
    }
    let inside: Vec<&str> = seen.iter().filter_map(|(_, l)| *l).collect();
    let missing: Vec<&str> = seen
        .iter()
        .filter(|(_, l)| l.is_none())
        .map(|(n, _)| *n)
        .collect();
    let send =
        "the other computers make no zip of their own, so this one file is all there is to send.";
    let away = |names: &[&str]| match names {
        [one] => format!(
            "{one}'s own log did not come back: it is newq.log in NewQ's log folder on {one}."
        ),
        _ => format!(
            "The own logs of {} did not come back: each is newq.log in NewQ's log folder on that computer.",
            names.join(", ")
        ),
    };
    match (inside.is_empty(), missing.is_empty()) {
        (false, true) => format!(
            "Every computer's own log is inside the zip too ({}): {send}",
            inside.join(", ")
        ),
        (false, false) => format!(
            "The other computers' own logs are inside the zip too ({}): {send} {}",
            inside.join(", "),
            away(&missing)
        ),
        (true, _) => away(&missing),
    }
}

/// A line per computer: its verdict and headline, its numbers folded under it.
fn computers_results(ui: &mut egui::Ui, computers: &[super::net_judge::ComputerVerdict]) {
    for c in computers {
        let colour = verdict_colour(c.verdict);
        // The headline wraps; the figures are folded under it.
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&c.name).strong());
            ui.colored_label(colour, &c.headline);
        });
        #[cfg(test)]
        note_net(
            ui,
            &format!("{}: {}", c.name, c.headline),
            egui::Rect::NOTHING,
        );
        if c.lines.is_empty() {
            continue;
        }
        ui.indent(("net-lines-in", &c.name), |ui| {
            egui::CollapsingHeader::new(RichText::new("Its figures").small())
                .id_salt(("net-lines", &c.name, ui.id()))
                .default_open(false)
                .show(ui, |ui| {
                    for l in &c.lines {
                        ui.label(RichText::new(l).weak().small());
                    }
                });
        });
    }
}

/// For the tests: where each choice of test is, and whether it is the one chosen.
#[cfg(test)]
fn note_choice(ui: &egui::Ui, name: String, r: &egui::Response) {
    ui.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(String, egui::Rect)>>(egui::Id::new("test_choices"))
            .push((name, r.rect))
    });
}

fn spark(ui: &mut egui::Ui, label: &str, values: &[f32], now: String) {
    ui.horizontal(|ui| {
        ui.add_sized([110.0, 18.0], egui::Label::new(label));
        let (rect, _) = ui.allocate_exact_size(egui::vec2(220.0, 22.0), egui::Sense::hover());
        let p = ui.painter_at(rect);
        p.rect_filled(rect, 2.0, ui.visuals().extreme_bg_color);
        if values.len() > 1 {
            let max = values.iter().copied().fold(f32::MIN, f32::max);
            let min = values.iter().copied().fold(f32::MAX, f32::min);
            let span = (max - min).max(1e-3);
            let pts: Vec<egui::Pos2> = values
                .iter()
                .enumerate()
                .map(|(i, v)| {
                    egui::pos2(
                        rect.left() + rect.width() * i as f32 / (values.len() - 1) as f32,
                        rect.bottom() - 2.0 - (rect.height() - 4.0) * (v - min) / span,
                    )
                })
                .collect();
            p.add(egui::Shape::line(
                pts,
                egui::Stroke::new(1.5, Color32::LIGHT_BLUE),
            ));
        }
        ui.label(now);
    });
}

/// "12:34 of 1:00:00": the planned length as it is, and the time so far never
/// past it.
fn progress_text(elapsed: std::time::Duration, planned: std::time::Duration) -> String {
    format!("{} of {}", clock(elapsed.min(planned)), clock(planned))
}

fn running(ui: &mut egui::Ui, r: &Run) {
    ui.label(RichText::new(format!("{} test running", r.flavour.name(r.level))).strong());
    match (r.progress(), r.planned) {
        (Some(p), Some(planned)) => {
            // The planned length itself, not worked back from the progress:
            // that rounds, and the total flickered between 1:00:00 and 59:59.
            ui.add(egui::ProgressBar::new(p).text(progress_text(r.elapsed(), planned)));
        }
        _ => {
            ui.label(format!("{} elapsed", clock(r.elapsed())));
        }
    }
    ui.label(format!(
        "{} laps finished (a lap should take {:.0} s)",
        r.laps.len(),
        r.lap_secs
    ));
    if let Some(tl) = &r.timeline {
        let d = &tl.driver;
        ui.label(format!(
            "GO pressed for you {} time{} at pauses, {} loop{} left, {} seek{}",
            d.presses,
            if d.presses == 1 { "" } else { "s" },
            d.leaves,
            if d.leaves == 1 { "" } else { "s" },
            d.seeks,
            if d.seeks == 1 { "" } else { "s" },
        ));
        for l in super::analysis::timeline_summary(&tl.watch.notes) {
            ui.label(RichText::new(l).weak().small());
        }
    }
    let tail = |f: &dyn Fn(&crate::stats::Sample) -> f32| -> Vec<f32> {
        let n = r.history.len();
        r.history[n.saturating_sub(120)..].iter().map(f).collect()
    };
    if let Some(s) = &r.last {
        spark(
            ui,
            "Memory",
            &tail(&|s| s.rss_mb as f32),
            format!("{} MB", s.rss_mb),
        );
        spark(
            ui,
            "CPU",
            &tail(&|s| s.cpu_pct),
            format!("{:.0}%", s.cpu_pct),
        );
        spark(
            ui,
            "FFmpeg processes",
            &tail(&|s| s.ffmpeg as f32),
            format!("{}", s.ffmpeg),
        );
        spark(
            ui,
            "Slowest frame",
            &tail(&|s| s.stall_ms as f32),
            format!("{} ms", s.stall_ms),
        );
    } else {
        ui.label(RichText::new("Measuring…").weak());
    }
    ui.label(hitches_line(&r.hitches));
    ui.separator();
    if r.findings.is_empty() {
        ui.colored_label(GREEN, "Nothing wrong so far.");
    }
    for f in r.findings.iter().take(6) {
        let c = match f.severity {
            Severity::Problem => RED,
            Severity::Watch => Color32::LIGHT_BLUE,
            Severity::Note => Color32::GRAY,
        };
        ui.colored_label(c, &f.text);
    }
    if let Some(t) = super::analysis::steady_line(
        &r.findings,
        r.history.len(),
        r.elapsed().as_secs_f64() / 3600.0,
    ) {
        ui.colored_label(GREEN, t);
    }
    ui.separator();
    ui.label(RichText::new("Log files").small().weak());
    ui.label(RichText::new(r.folder.display().to_string()).small());
    ui.horizontal(|ui| {
        if ui.small_button("Open the folder").clicked() {
            let _ = open_folder(&r.folder);
        }
        if ui.small_button("Copy the path").clicked() {
            ui.ctx().copy_text(r.folder.display().to_string());
        }
    });
}

/// The 3D stage part under way: which leg, and the view it turns.
fn stage_running(ui: &mut egui::Ui, st: &mut UiState, s: &super::stage::StageRun, t: f64) {
    use super::stage::Leg;
    let Some(now) = s.now(t) else {
        ui.label(RichText::new("3D stage: done; the report is being written.").weak());
        return;
    };
    let line = format!(
        "3D stage, leg {} of {}{}: {} · {:.0} s left",
        now.number,
        now.of,
        if now.round > 1 {
            format!(", round {}", now.round)
        } else {
            String::new()
        },
        now.leg.name(),
        now.left
    );
    net_label(ui, RichText::new(line).strong());
    ui.label(
        RichText::new(match now.leg {
            Leg::Baseline => {
                "Every preview is held off for this leg, the 3D view's too: what the displays do without them is the baseline."
            }
            Leg::View => "The 3D view turns; the previews give way to the displays as they always do.",
            Leg::Light => "The projectors' light is on in the 3D view.",
            Leg::Display => match &s.setup.stage_display {
                Some(d) if d.windowed => {
                    "A display shows the stage in place of the test's pictures."
                }
                _ => "A display with no window shows the stage, drawn for the previews only, at their pace.",
            },
            Leg::Mapped => {
                "The last monitor shows the screens through a mapped projector, the lap on the canvas they show."
            }
        })
        .weak()
        .small(),
    );
    // The view itself: drawn as any 3D preview is, through the one governor.
    let mut view = s.view_state(t);
    let size = egui::vec2(ui.available_width().clamp(200.0, 480.0), 0.0);
    let size = egui::vec2(size.x, size.x * 9.0 / 16.0);
    ui.allocate_ui(size, |ui| {
        ui.set_min_size(size);
        crate::ui::stage_view::draw(ui, &s.show, st, &mut view, super::stage::VIEW);
    });
    // The view turns: the window comes round for it (the governor says
    // whether it is drawn); held off, it needn't.
    if now.leg != Leg::Baseline {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(16));
    }
}

/// How each kind of cue came out, as a small table.
fn kinds_table(ui: &mut egui::Ui, kinds: &[(&'static str, usize, usize, usize)]) {
    egui::Grid::new("full-kinds")
        .num_columns(2)
        .spacing([10.0, 2.0])
        .show(ui, |ui| {
            for (kind, passed, failed, skipped) in kinds {
                let (colour, word) = if *failed > 0 {
                    (RED, format!("{failed} failed"))
                } else if *passed > 0 {
                    (GREEN, "ok".to_string())
                } else {
                    (Color32::GRAY, "not run".to_string())
                };
                ui.label(*kind);
                ui.horizontal(|ui| {
                    ui.colored_label(colour, word);
                    if *failed == 0 && *passed > 0 && *skipped > 0 {
                        ui.label(RichText::new(format!("({skipped} not run)")).weak().small());
                    }
                });
                ui.end_row();
            }
        });
}

/// The full check while it runs: where it is, and how the parts so far came out.
fn checking(ui: &mut egui::Ui, r: &super::full::FullRun) {
    use super::feature::Outcome;
    ui.label(RichText::new("The full check is running").strong());
    let (n, of, kind, title) = r.status();
    ui.add(
        egui::ProgressBar::new(r.progress())
            .text(format!("part {n} of {of}, {}", clock(r.elapsed()))),
    );
    ui.label(format!("{kind}: {title}"));
    let count = |o: Outcome| r.runner.results.iter().filter(|x| x.outcome == o).count();
    ui.horizontal(|ui| {
        ui.colored_label(GREEN, format!("{} ok", count(Outcome::Passed)));
        let failed = count(Outcome::Failed);
        if failed > 0 {
            ui.colored_label(RED, format!("{failed} failed"));
        }
        let skipped = count(Outcome::Skipped);
        if skipped > 0 {
            ui.label(RichText::new(format!("{skipped} not run")).weak());
        }
    });
    for x in r
        .runner
        .results
        .iter()
        .filter(|x| x.outcome == Outcome::Failed)
        .take(4)
    {
        ui.colored_label(RED, format!("{}: {}", x.kind, x.title));
    }
    if let Some(s) = &r.last {
        ui.label(
            RichText::new(format!(
                "Memory {} MB, processor {:.0}%, {} FFmpeg, slowest frame {} ms",
                s.rss_mb, s.cpu_pct, s.ffmpeg, s.stall_ms
            ))
            .weak()
            .small(),
        );
    }
    ui.label(RichText::new(hitches_line(&r.hitches)).weak().small());
    ui.separator();
    ui.label(RichText::new("Its files").small().weak());
    ui.label(RichText::new(r.folder.display().to_string()).small());
}

fn comparing(ui: &mut egui::Ui, c: &super::decode::Compare) {
    use super::decode::Stage;
    ui.label(RichText::new("Comparing hardware decoding").strong());
    match &c.stage {
        Stage::Making => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Making a 4K clip to test with (a few seconds to a minute)…");
            });
            ui.label(
                RichText::new("Nothing about your show has changed.")
                    .weak()
                    .small(),
            );
        }
        Stage::Trial { hw, began } => {
            let (n, name) = if *hw { (2, "on") } else { (1, "off") };
            let total = (super::decode::WARM_UP + c.secs) as f32;
            let done = began.elapsed().as_secs_f32().min(total);
            ui.label(format!("Trial {n} of 2: hardware decoding {name}"));
            ui.label(RichText::new(&c.what).weak().small());
            ui.add(
                egui::ProgressBar::new(done / total)
                    .text(format!("{:.0} s of {:.0} s", done, total)),
            );
            if let Some(s) = &c.live {
                ui.label(format!(
                    "Processor now: NewQ {:.0}%, FFmpeg {:.0}% (of one core)",
                    s.cpu_pct, s.ffmpeg_cpu_pct
                ));
            }
            if let Some(off) = &c.results[0] {
                ui.label(
                    RichText::new(format!(
                        "With it off: {:.0}% of one core, {:.1}% late frames",
                        off.total_cpu(),
                        off.late_permille / 10.0
                    ))
                    .weak(),
                );
            }
        }
    }
    ui.label(RichText::new(c.folder.display().to_string()).small().weak());
}

/// A connected monitor, as the Test Mode window offers it.
pub struct Screen {
    pub choice: DisplayChoice,
    /// Its tick's words: "Left · screen 1 · 1920×1080".
    pub short: String,
    /// The computer it is on, by name ("This computer" alone).
    pub computer: String,
    /// The main window is on it: a test can't be shown there (outputs never
    /// cover the main window's display), only in the main window's preview.
    pub is_main: bool,
}

/// What a monitor of another computer is remembered by (`Prefs::test_displays`):
/// the computer's id, its place in that computer's list and what it is.
pub fn remote_key(id: crate::show::NodeId, index: usize, what: &str) -> String {
    format!("{id}/{}/{what}", index + 1)
}

/// The monitors offered: this computer's, then, on a primary with computers,
/// each connected computer's (from its status report), grouped by computer.
fn screens_of(st: &UiState) -> Vec<Screen> {
    let mut v = local_screens(st);
    if st.net.is_primary() {
        for p in st.net.peers.iter().filter(|p| p.ours && p.connected) {
            let Some(s) = &p.status else { continue };
            let keys: Vec<crate::show::MonitorKey> = s.monitors.iter().map(|m| m.key()).collect();
            for (i, m) in s.monitors.iter().enumerate() {
                let what = m.hardware.clone().unwrap_or_else(|| m.identity.clone());
                v.push(Screen {
                    choice: DisplayChoice {
                        key: remote_key(p.id, i, &what),
                        reference: keys[i].reference(i),
                        twins: crate::show::twins_of(&keys, i),
                        label: format!(
                            "{} screen {}: {} ({}×{}{})",
                            p.name,
                            i + 1,
                            m.identity,
                            m.size.0,
                            m.size.1,
                            m.refresh_hz
                                .map(|r| format!(" @ {r:.0} Hz"))
                                .unwrap_or_default()
                        ),
                        computer: Some(p.id),
                    },
                    short: format!("{} · screen {} · {}×{}", p.name, i + 1, m.size.0, m.size.1),
                    computer: p.name.clone(),
                    is_main: false,
                });
            }
        }
    }
    v
}

fn local_screens(st: &UiState) -> Vec<Screen> {
    let keys = crate::ui::monitor_keys(&st.monitors);
    let name = if st.net.me.on && !st.net.name.is_empty() {
        st.net.name.clone()
    } else {
        "This computer".to_string()
    };
    (0..keys.len())
        .map(|i| Screen {
            short: {
                let m = &st.monitors[i];
                if st.net.me.on {
                    format!("{name} · screen {} · {}×{}", i + 1, m.size.0, m.size.1)
                } else {
                    format!(
                        "Screen {} · {} · {}×{}",
                        i + 1,
                        m.title(),
                        m.size.0,
                        m.size.1
                    )
                }
            },
            computer: name.clone(),
            choice: DisplayChoice {
                computer: None,
                key: keys[i]
                    .hardware
                    .clone()
                    .unwrap_or_else(|| keys[i].identity.clone()),
                reference: keys[i].reference(i),
                twins: crate::show::twins_of(&keys, i),
                label: crate::ui::monitor_label(st, i),
            },
            is_main: st.control_monitor == Some(i),
        })
        .collect()
}

/// The displays the test is to be shown on: the ticked ones that are connected.
fn chosen(screens: &[Screen], selected: &[String]) -> Vec<DisplayChoice> {
    screens
        .iter()
        .filter(|s| !s.is_main && selected.contains(&s.choice.key))
        .map(|s| s.choice.clone())
        .collect()
}

/// The sound the test is to use: the chosen device if it is still there.
fn sound_of(devices: &[String], sound: &(Option<String>, bool)) -> SoundChoice {
    SoundChoice {
        device: sound.0.clone().filter(|d| devices.iter().any(|x| x == d)),
        muted: sound.1,
    }
}

/// The sound, on one line: the device and a Muted tick.
fn sound_section(ui: &mut egui::Ui, devices: &[String], sound: &mut (Option<String>, bool)) {
    const SHOWS: &str = "Your show's audio outputs";
    let shown = match &sound.0 {
        Some(d) if devices.iter().any(|x| x == d) => d.clone(),
        Some(d) => format!("{d} (not connected: the default)"),
        None => SHOWS.to_string(),
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new("Sound").strong());
        egui::ComboBox::from_id_salt("test-sound")
            .selected_text(shown)
            .width(260.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut sound.0, None, SHOWS);
                for d in devices {
                    ui.selectable_value(&mut sound.0, Some(d.clone()), d);
                }
            });
        ui.checkbox(&mut sound.1, "Muted").on_hover_text(
            "Nothing is heard, but the sound is still decoded and mixed, so the test costs what it would with sound.",
        );
    });
}

/// How many rows of screens show before the list scrolls in its own space.
const SCREEN_ROWS: usize = 8;

/// A tick per monitor, one row each; with other computers, grouped under each
/// computer's name (this one first), with All and None where a computer has
/// more than one.
fn displays_section(ui: &mut egui::Ui, screens: &[Screen], selected: &mut Vec<String>) {
    ui.label(RichText::new("Show on").strong());
    let offered: Vec<&Screen> = screens.iter().filter(|s| !s.is_main).collect();
    if offered.is_empty() {
        net_label(
            ui,
            RichText::new("Only the main window's monitor: the test shows in its Preview pane.")
                .weak()
                .small(),
        );
        return;
    }
    let mut groups: Vec<(Option<crate::show::NodeId>, Vec<&Screen>)> = Vec::new();
    for s in &offered {
        match groups.iter_mut().find(|(c, _)| *c == s.choice.computer) {
            Some((_, v)) => v.push(s),
            None => groups.push((s.choice.computer, vec![s])),
        }
    }
    let titled = groups.len() > 1 || groups.iter().any(|(c, _)| c.is_some());
    let rows = offered.len()
        + if titled {
            groups.iter().filter(|(_, l)| l.len() > 1).count()
        } else {
            0
        };
    let mut draw = |ui: &mut egui::Ui| {
        for (_, list) in &groups {
            // (A computer with one screen needs no heading: its tick names it.)
            if titled && list.len() > 1 {
                let title = &list[0].computer;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title).small().strong());
                    if list.len() > 1 {
                        let all = ui.small_button("All");
                        let none = ui.small_button("None");
                        #[cfg(test)]
                        {
                            note_choice(ui, format!("All:{title}"), &all);
                            note_choice(ui, format!("None:{title}"), &none);
                        }
                        if all.clicked() {
                            for s in list {
                                if !selected.contains(&s.choice.key) {
                                    selected.push(s.choice.key.clone());
                                }
                            }
                        }
                        if none.clicked() {
                            selected.retain(|k| !list.iter().any(|s| &s.choice.key == k));
                        }
                    }
                });
            }
            for s in list {
                let mut on = selected.contains(&s.choice.key);
                let r = ui
                    .checkbox(&mut on, &s.short)
                    .on_hover_text(&s.choice.label);
                #[cfg(test)]
                note_choice(ui, format!("screen:{}", s.short), &r);
                if r.changed() {
                    selected.retain(|k| k != &s.choice.key);
                    if on {
                        selected.push(s.choice.key.clone());
                    }
                }
            }
        }
    };
    if rows > SCREEN_ROWS {
        egui::ScrollArea::vertical()
            .id_salt("test-screens")
            .max_height(SCREEN_ROWS as f32 * 22.0)
            .auto_shrink([false, true])
            .show(ui, |ui| draw(ui));
    } else {
        ui.indent("test-screens", |ui| draw(ui));
    }
}

/// The running count of the main loop's stalls and the sound's dropouts.
fn hitches_line(h: &super::hitches::Hitches) -> String {
    let worst = h.stalls.iter().map(|s| s.ms).max();
    format!(
        "Main loop held up 100 ms or more: {}{} · Sound dropouts: {}",
        h.stalls.len(),
        worst.map_or(String::new(), |w| format!(" (worst {w} ms)")),
        h.dropouts.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_test_chosen_in_the_window_is_kept_for_this_computer() {
        let dir = std::env::temp_dir().join(format!("newq_test_pick_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let engine = Engine::for_tests();
        let mut st = UiState::default();
        st.layout_dir = Some(dir.clone());
        st.test.open = true;
        let mut t = 1.0;
        let mut frame = |events: Vec<egui::Event>, st: &mut UiState| {
            ctx.data_mut(|d| d.remove::<Vec<(String, egui::Rect)>>(egui::Id::new("test_choices")));
            t += 0.5;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 760.0),
                )),
                time: Some(t),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| window(ui.ctx(), &engine, st));
            out.textures_delta.clear();
            ctx.data(|d| d.get_temp::<Vec<(String, egui::Rect)>>(egui::Id::new("test_choices")))
                .unwrap_or_default()
        };
        frame(vec![], &mut st);
        let mut click = |name: &str, st: &mut UiState| {
            let at = frame(vec![], st)
                .into_iter()
                .find(|c| c.0 == name)
                .unwrap_or_else(|| panic!("no {name}"))
                .1
                .center();
            let button = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            frame(vec![egui::Event::PointerMoved(at)], st);
            frame(vec![button(true)], st);
            frame(vec![button(false)], st);
        };
        use crate::testmode::choice::Parts;
        click("Timeline", &mut st);
        click("Length", &mut st);
        click("level:Medium", &mut st);
        let both = Parts {
            cue_list: true,
            timeline: true,
            network: false,
        };
        assert_eq!((st.test.level, st.test.parts), (Level::Medium, both));
        let saved = crate::prefs::Prefs::load(&dir).test_pick;
        assert_eq!(saved, st.test.pick(), "saved at once");
        assert_eq!((saved.level, saved.parts), (Level::Medium, both));
        // Alone, the Network tick can't be ticked.
        click("Network", &mut st);
        assert!(!st.test.parts.network);
        // Back to the first: the default, and not written.
        click("Timeline", &mut st);
        click("Length", &mut st);
        click("level:Short", &mut st);
        assert_eq!(
            crate::prefs::Prefs::load(&dir).test_pick,
            Default::default()
        );
        let text = std::fs::read_to_string(dir.join("prefs.json")).unwrap();
        assert!(!text.contains("test_pick"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The 3D stage tick: ticked by itself on opening for a show that wants
    /// it, untickable by the mouse, never kept by this computer, and its
    /// part in the plan's line.
    #[test]
    fn the_3d_stage_tick_follows_the_show_and_the_mouse() {
        let dir = std::env::temp_dir().join(format!("newq_test_stage_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let ctx = egui::Context::default();
        let engine = Engine::for_tests();
        let mut st = UiState::default();
        st.layout_dir = Some(dir.clone());
        st.test.stage_wanted = true;
        st.test.stage_unscreened = true;
        st.test.open = true;
        let mut t = 1.0;
        let mut frame = |events: Vec<egui::Event>, st: &mut UiState| {
            ctx.data_mut(|d| d.remove::<Vec<(String, egui::Rect)>>(egui::Id::new("test_choices")));
            t += 0.5;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 760.0),
                )),
                time: Some(t),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| window(ui.ctx(), &engine, st));
            out.textures_delta.clear();
            ctx.data(|d| d.get_temp::<Vec<(String, egui::Rect)>>(egui::Id::new("test_choices")))
                .unwrap_or_default()
        };
        let shown = frame(vec![], &mut st);
        assert!(st.test.stage3d, "ticked by itself");
        let at = shown
            .iter()
            .find(|c| c.0 == "3D stage")
            .expect("the tick is drawn")
            .1;
        assert!(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 760.0))
                .contains_rect(at)
        );
        let at = at.center();
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(vec![egui::Event::PointerMoved(at)], &mut st);
        frame(vec![button(true)], &mut st);
        frame(vec![button(false)], &mut st);
        assert!(!st.test.stage3d, "unticked by the mouse");
        // Not kept by this computer.
        assert!(
            !std::fs::read_to_string(dir.join("prefs.json"))
                .unwrap_or_default()
                .contains("stage")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_planned_length_in_the_progress_text_never_changes() {
        // It flickered between 1:00:00 and 59:59 when worked back from the progress.
        let planned = Duration::from_secs(3600);
        for ms in (0..3_700_000u64).step_by(997) {
            let t = progress_text(Duration::from_millis(ms), planned);
            assert!(t.ends_with(" of 1:00:00"), "{t}");
        }
        assert_eq!(
            progress_text(Duration::from_secs(65), planned),
            "1:05 of 1:00:00"
        );
        assert_eq!(
            progress_text(Duration::from_secs(4000), planned),
            "1:00:00 of 1:00:00"
        );
    }

    fn screen(key: &str, is_main: bool) -> Screen {
        Screen {
            choice: DisplayChoice {
                computer: None,
                key: key.into(),
                reference: crate::show::DisplayRef {
                    name: key.into(),
                    index: 0,
                    hardware: None,
                },
                twins: 1,
                label: key.into(),
            },
            short: key.into(),
            computer: "This computer".into(),
            is_main,
        }
    }

    #[test]
    fn the_ticked_screens_of_other_computers_are_used_and_a_gone_ones_ignored() {
        let left = crate::show::NodeId(0x20);
        let mut far = screen(&remote_key(left, 0, "Projector"), false);
        far.choice.computer = Some(left);
        let screens = [screen("A", true), screen("B", false), far];
        let sel = vec![
            "B".to_string(),
            remote_key(left, 0, "Projector"),
            // A computer that has gone, and a monitor of Left's unplugged.
            remote_key(crate::show::NodeId(0x99), 0, "Wall"),
            remote_key(left, 1, "Side screen"),
        ];
        let got = chosen(&screens, &sel);
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].computer, Some(left));
        assert_eq!(
            remote_key(left, 0, "Projector"),
            "0000000000000020/1/Projector"
        );
    }

    #[test]
    fn only_ticked_connected_displays_other_than_the_main_windows_are_used() {
        let screens = [screen("A", true), screen("B", false), screen("C", false)];
        let names = |v: Vec<DisplayChoice>| v.into_iter().map(|d| d.key).collect::<Vec<_>>();
        assert!(
            names(chosen(&screens, &[])).is_empty(),
            "none ticked: the show's own"
        );
        let sel = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(names(chosen(&screens, &sel(&["C", "B"]))), ["B", "C"]);
        // The main window's own display is never used; one that isn't connected is ignored.
        assert_eq!(names(chosen(&screens, &sel(&["A", "Z", "B"]))), ["B"]);
    }
}
