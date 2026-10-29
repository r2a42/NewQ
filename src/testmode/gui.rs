//! The Test Mode window: choose a test, watch the media being made ready,
//! watch the test run, and read what it found.

use super::Level;
use super::analysis::{Severity, Verdict};
use super::assets::{self, State};
use super::session::{End, Run, View, clock, open_folder};
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
    if !st.test.open {
        return;
    }
    let mut open = true;
    let mut test = std::mem::take(&mut st.test);
    let refusal = st.test_refusal();
    let mut apply: Option<bool> = None;
    egui::Window::new("Test mode")
        .open(&mut open)
        .collapsible(false)
        .default_width(520.0)
        .show(ctx, |ui| {
            body(ui, engine, &mut test, refusal, &mut apply);
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
    st.test = test;
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

fn body(
    ui: &mut egui::Ui,
    engine: &Engine,
    test: &mut super::TestMode,
    refusal: Option<&'static str>,
    apply: &mut Option<bool>,
) {
    let log_dir = crate::crash::log_dir();
    let mut dismiss = false;
    let mut failed = None;
    match test.view() {
        View::Idle => {
            idle(ui, engine, test, refusal, &log_dir);
        }
        View::Preparing(p) => {
            let st = p.fetch.status.lock();
            ui.label(RichText::new(format!("Getting the {} test ready", p.level.label())).strong());
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
            ui.label(
                RichText::new("Nothing about your show has changed.")
                    .weak()
                    .small(),
            );
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
            ui.separator();
            if ui.button("Stop the test").clicked() {
                test.stop();
            }
        }
        View::Done(d) => {
            let (colour, word) = match d.verdict {
                Verdict::Passed => (GREEN, "Passed"),
                Verdict::PassedWithNotes => (AMBER, "Passed, with notes"),
                Verdict::ProblemsFound => (RED, "Problems found"),
            };
            if let Some(cmp) = &d.compare {
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
            } else {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(word).size(22.0).strong().color(colour));
                    ui.label(format!(
                        "{} test, {}, {} lap{}",
                        d.level.label(),
                        clock(d.elapsed),
                        d.laps,
                        if d.laps == 1 { "" } else { "s" }
                    ));
                });
            }
            ui.label(&d.headline);
            if matches!(d.ended, End::Stopped) {
                ui.label(RichText::new("You stopped it early.").weak());
            }
            ui.colored_label(
                GREEN,
                "Your show is back exactly as it was. Nothing was saved or changed.",
            );
            ui.separator();
            egui::ScrollArea::vertical()
                .max_height(260.0)
                .show(ui, |ui| {
                    if d.findings.is_empty() {
                        ui.label("Nothing to report.");
                    }
                    for f in &d.findings {
                        let (c, t) = match f.severity {
                            Severity::Problem => (RED, "Problem"),
                            Severity::Note => (Color32::GRAY, "Note"),
                        };
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(c, t);
                            ui.label(&f.text);
                        });
                    }
                });
            ui.separator();
            ui.label("The log files are in:");
            let mut path = d.folder.display().to_string();
            ui.add(egui::TextEdit::singleline(&mut path).desired_width(f32::INFINITY));
            ui.horizontal(|ui| {
                if ui.button("Open the folder").clicked()
                    && let Err(e) = open_folder(&d.folder)
                {
                    failed = Some(e);
                }
                if ui.button("Copy the path").clicked() {
                    ui.ctx().copy_text(d.folder.display().to_string());
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

fn idle(
    ui: &mut egui::Ui,
    engine: &Engine,
    test: &mut super::TestMode,
    refusal: Option<&'static str>,
    log_dir: &std::path::Path,
) {
    ui.label("Plays a show of its own to see how this computer copes, then puts yours back exactly as it was.");
    ui.add_space(6.0);
    choose(ui, test);
    ui.label(RichText::new(test.level.summary()).weak());
    let lengths = test.level.lengths();
    if !lengths.is_empty() {
        ui.horizontal(|ui| {
            ui.label("Run for");
            egui::ComboBox::from_id_salt("test-minutes")
                .selected_text(minutes_text(test.minutes))
                .show_ui(ui, |ui| {
                    for &m in lengths {
                        ui.selectable_value(&mut test.minutes, m, minutes_text(m));
                    }
                });
        });
    }
    let wanted = assets::wanted(test.level);
    let dir = assets::cache_dir(log_dir);
    let need = assets::download_size(&dir, &wanted);
    ui.add_space(4.0);
    ui.checkbox(
        &mut test.download,
        "Download real films and pictures (free to use)",
    );
    if test.download {
        if need == 0 {
            ui.label(
                RichText::new("Everything is already on this computer.")
                    .weak()
                    .small(),
            );
        } else {
            ui.label(
                RichText::new(format!(
                    "About {} will be downloaded once, from test-videos.co.uk, download.blender.org and images-assets.nasa.gov (Blender films under CC-BY, NASA pictures in the public domain). It is kept in {}.",
                    megabytes(need),
                    dir.display()
                ))
                .weak()
                .small(),
            );
        }
    } else {
        ui.label(
            RichText::new("Plain test pictures and tones will be made on this computer.")
                .weak()
                .small(),
        );
    }
    ui.separator();
    if let Some(why) = refusal {
        ui.colored_label(AMBER, why);
    }
    if engine.ff.is_none() {
        ui.colored_label(
            RED,
            "FFmpeg wasn't found, so there is nothing to play with. See Settings.",
        );
    }
    let can = refusal.is_none() && engine.ff.is_some();
    if ui
        .add_enabled(
            can,
            egui::Button::new(RichText::new("Start the test").strong()),
        )
        .clicked()
        && let Some(ff) = engine.ff.clone()
    {
        test.start(ff, log_dir);
    }
    ui.add_space(6.0);
    ui.separator();
    ui.label(RichText::new("Does hardware decoding help here?").strong());
    ui.label(
        RichText::new(
            "Plays a wall of 4K clips twice, with it off and on, about two minutes, and says which to use. It makes a 4K clip from the downloaded film (or, if this computer can't, plays the 1080p film several times).",
        )
        .weak()
        .small(),
    );
    if ui
        .add_enabled(can, egui::Button::new("Compare hardware decoding"))
        .clicked()
        && let Some(ff) = engine.ff.clone()
    {
        test.start_compare(ff, log_dir);
    }
    ui.label(
        RichText::new("Sound and pictures go to your show's outputs while it runs. Log files: ")
            .weak()
            .small(),
    );
    ui.label(
        RichText::new(log_dir.join("test-mode").join("runs").display().to_string())
            .weak()
            .small(),
    );
}

fn minutes_text(m: u32) -> String {
    if m >= 60 && m.is_multiple_of(60) {
        format!("{} hour{}", m / 60, if m == 60 { "" } else { "s" })
    } else {
        format!("{m} minutes")
    }
}

fn choose(ui: &mut egui::Ui, test: &mut super::TestMode) {
    ui.horizontal(|ui| {
        for level in Level::ALL {
            let selected = test.level == level;
            if ui.selectable_label(selected, level.label()).clicked() {
                test.set_level(level);
            }
        }
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

fn running(ui: &mut egui::Ui, r: &Run) {
    ui.label(RichText::new(format!("{} test running", r.level.label())).strong());
    match r.progress() {
        Some(p) => {
            ui.add(egui::ProgressBar::new(p).text(format!(
                    "{} of {}",
                    clock(r.elapsed()),
                    clock(
                        r.elapsed()
                            .mul_f32(1.0 / p.max(1e-4))
                            .min(std::time::Duration::from_secs(3600 * 24))
                    )
                )));
        }
        None => {
            ui.label(format!("{} elapsed", clock(r.elapsed())));
        }
    }
    ui.label(format!(
        "{} laps finished (a lap should take {:.0} s)",
        r.laps.len(),
        r.lap_secs
    ));
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
    ui.separator();
    if r.findings.is_empty() {
        ui.colored_label(GREEN, "Nothing wrong so far.");
    }
    for f in r.findings.iter().take(6) {
        let c = if f.severity == Severity::Problem {
            RED
        } else {
            Color32::GRAY
        };
        ui.colored_label(c, &f.text);
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
