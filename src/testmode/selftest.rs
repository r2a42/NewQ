//! `newq --self-test [--out <folder>]`: the full check with no window. The
//! show engine plays every part of `feature` against FFmpeg and a mixer that
//! throws its sound away, and the result is written as a report and the same
//! zip Test Mode's window makes. For a build server (the Windows and Mac
//! runners), and for a computer whose windows are what is in doubt.
//!
//! What needs the app's windows and graphics card (the recording, the stream
//! to a server) is listed as not run; the window's own "Run the full check"
//! does those.

use super::bundle;
use super::feature::{self, Media, Outcome, PartResult, Runner};
use crate::engine::{Engine, ShowEdit};
use crate::media::audio::AudioOut;
use crate::media::ffmpeg::Ffmpeg;
use crate::show::Show;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

/// Play the check through an engine of its own. Returns each part's result
/// and what the engine said, with times. `limit` ends it early (the parts
/// left are listed as not run).
pub fn play(
    ff: &Ffmpeg,
    original: &Show,
    media: &Media,
    limit: Option<Duration>,
) -> (Vec<PartResult>, Vec<String>) {
    let plan = feature::build(
        original,
        media,
        Err("not run without the app's windows".into()),
    );
    let mut show = plan.show;
    let mut engine = Engine::new();
    engine.ff = Some(ff.clone());
    // Sound is decoded, put through its effects and mixed, and goes nowhere.
    engine.audio = AudioOut::silent();
    engine.set_log_tap(true);
    let mut runner = Runner::new(plan.parts, plan.osc, false);
    let began = Instant::now();
    let mut last = began;
    let mut was_active = false;
    let mut said = Vec::new();
    loop {
        let now = Instant::now();
        // As the app does it: the time nothing was playing is not a cue's time.
        let dt = if was_active {
            now.duration_since(last).as_secs_f64()
        } else {
            0.0
        };
        last = now;
        was_active = engine.is_active();
        engine.tick(&show, dt);
        for edit in engine.take_show_edits() {
            match edit {
                ShowEdit::Arm(id, on) => {
                    if let Some(c) = show.cues.iter_mut().find(|c| c.id == id) {
                        c.armed = on;
                    }
                }
            }
        }
        for (at, text) in engine.take_log_tap() {
            said.push(format!(
                "[{:>9.3}] {text}",
                at.saturating_duration_since(began).as_secs_f64()
            ));
        }
        let problems = engine.take_problems();
        if runner.tick(&show, &mut engine, now, problems) {
            break;
        }
        if limit.is_some_and(|l| now.duration_since(began) > l) {
            runner.abandon(&show, &mut engine, "the time allowed ran out");
            break;
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    engine.stop_all(0.0);
    (runner.results, said)
}

/// The report of a headless run.
pub fn report(results: &[PartResult], media_notes: &[String], took: Duration) -> String {
    let count = |o: Outcome| results.iter().filter(|r| r.outcome == o).count();
    let (passed, failed, skipped) = (
        count(Outcome::Passed),
        count(Outcome::Failed),
        count(Outcome::Skipped),
    );
    let mut out = String::new();
    let mut line = |s: &str| {
        out.push_str(s);
        out.push('\n');
    };
    line("NewQ self-test report");
    line("=====================");
    line(&format!(
        "Result: {}",
        if failed > 0 {
            "Problems found"
        } else if passed == 0 {
            "Nothing could be run"
        } else {
            "Passed"
        }
    ));
    line(&format!(
        "{passed} parts passed, {failed} failed, {skipped} not run, in {} s.",
        took.as_secs()
    ));
    line(&format!(
        "NewQ {} on {} ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    line("Run with no window: the show engine, FFmpeg and a mixer that plays to nothing.");
    line("");
    for n in media_notes {
        line(&format!("note: {n}"));
    }
    out.push_str(&feature::account(results));
    out
}

/// This computer and the programs the run used.
fn system_text(ff: &Ffmpeg) -> String {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_all();
    let first_line = |program: &Path, arg: &str| -> String {
        let mut cmd = std::process::Command::new(program);
        cmd.arg(arg);
        crate::media::ffmpeg::no_console(&mut cmd);
        let never = AtomicBool::new(false);
        let limit = Instant::now() + Duration::from_secs(10);
        crate::media::ffmpeg::output_until(&mut cmd, &never, Some(limit))
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .next()
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "(it did not say)".into())
    };
    let mut out = format!(
        "NewQ {}\nSystem: {} {}, kernel {}\nArchitecture: {}\nProcessor: {} ({} threads)\nMemory: {} MB\nGraphics: not used (no window)\n",
        env!("CARGO_PKG_VERSION"),
        System::name().unwrap_or_default(),
        System::os_version().unwrap_or_default(),
        System::kernel_version().unwrap_or_default(),
        std::env::consts::ARCH,
        sys.cpus().first().map_or("unknown", |c| c.brand().trim()),
        sys.cpus().len(),
        sys.total_memory() >> 20,
    );
    out.push_str(&format!(
        "FFmpeg: {}\n  at {}\n  x264 encoder: {}, Hap encoder: {}\n",
        first_line(&ff.ffmpeg, "-version"),
        ff.ffmpeg.display(),
        ff.has_encoder("libx264"),
        ff.has_encoder("hap")
    ));
    match crate::mediamtx::locate() {
        Some(exe) => out.push_str(&format!(
            "MediaMTX: {}\n  at {}\n",
            first_line(&exe, "--version"),
            exe.display()
        )),
        None => out.push_str("MediaMTX: not with this copy of NewQ\n"),
    }
    out
}

/// What `--self-test` came to: 0 passed, 1 a part failed, 2 it could not run.
pub fn run(folder: &Path) -> i32 {
    let began = Instant::now();
    if let Err(e) = std::fs::create_dir_all(folder) {
        eprintln!("self-test: can't make {}: {e}", folder.display());
        return 2;
    }
    let log_dir = folder.to_path_buf();
    // Its own log, in the folder it writes everything else to.
    crate::logging::init(&log_dir);
    crate::crash::install_in_folder(log_dir.clone());
    // The whole story, cue by cue, unless NEWQ_LOG asks otherwise: this log
    // is read by someone looking for what went wrong.
    crate::logging::set_level(crate::logging::LogLevel::Debug);
    log::info!(
        "NewQ {} self-test on {} ({})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let fail = |why: String| -> i32 {
        log::error!("self-test: {why}");
        eprintln!("self-test: {why}");
        let _ = std::fs::write(
            folder.join("report.txt"),
            format!("NewQ self-test report\n=====================\nResult: Could not run\n{why}\n"),
        );
        let _ = bundle::Bundle::start(folder.to_path_buf(), log_dir.clone(), true, false);
        2
    };
    let Some(ff) = Ffmpeg::locate() else {
        return fail("FFmpeg was not found (set NEWQ_FFMPEG_DIR to its folder)".into());
    };
    let _ = std::fs::write(folder.join("system.txt"), system_text(&ff));
    let env: Vec<(String, String)> = std::env::vars().collect();
    let _ = std::fs::write(
        folder.join("settings.txt"),
        bundle::settings_text(&crate::prefs::Prefs::default(), &env),
    );
    let cancel = AtomicBool::new(false);
    let media = match feature::make_media(&ff, &folder.join("media"), &cancel) {
        Ok(m) => m,
        Err(e) => return fail(format!("the test's media could not be made: {e}")),
    };
    // (A new show: one display, no audio outputs of its own.)
    let show = Show::default();
    // A panic in the engine is the finding, not the end of the report.
    let played = crate::crash::survive("The self-test", || {
        play(&ff, &show, &media, Some(Duration::from_secs(600)))
    });
    let Some((results, said)) = played else {
        return fail("NewQ panicked while playing the check (see newq-crash.log)".into());
    };
    let _ = std::fs::write(folder.join("events.log"), said.join("\n") + "\n");
    let text = report(&results, &media.notes, began.elapsed());
    let _ = std::fs::write(folder.join("report.txt"), &text);
    println!("{text}");
    let failed = results.iter().any(|r| r.outcome == Outcome::Failed);
    let nothing = !results.iter().any(|r| r.outcome == Outcome::Passed);
    let bundle = bundle::Bundle::start(folder.to_path_buf(), log_dir, true, false);
    match bundle.get() {
        Some(Ok(zip)) => println!("The files to send: {}", zip.display()),
        Some(Err(e)) => eprintln!("self-test: the zip could not be made: {e}"),
        None => {}
    }
    crate::logging::flush();
    if failed || crate::crash::panic_count() > 0 {
        1
    } else if nothing {
        2
    } else {
        0
    }
}

/// `--self-test` on the command line: run it and say how the program should end.
pub fn run_if_asked(args: &[String]) -> Option<i32> {
    if !args.iter().any(|a| a == "--self-test") {
        return None;
    }
    let folder = args
        .iter()
        .position(|a| a == "--out")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            super::session::run_folder_named(
                &crate::crash::log_dir(),
                "selftest",
                super::session::unix_now(),
            )
        });
    Some(run(&folder))
}

#[cfg(test)]
mod tests {
    use super::*;
    use feature::Verdict;

    #[test]
    fn the_command_is_only_run_when_asked_for() {
        let args = |a: &[&str]| -> Vec<String> { a.iter().map(|s| s.to_string()).collect() };
        assert_eq!(run_if_asked(&args(&[])), None);
        assert_eq!(
            run_if_asked(&args(&["show.newq", "--main-window-only"])),
            None
        );
    }

    #[test]
    fn the_report_says_how_many_parts_passed_and_which_failed() {
        let part = |kind, outcome, check: Verdict| PartResult::for_test(kind, outcome, vec![check]);
        let results = vec![
            part("Video", Outcome::Passed, Verdict::Ok("it played".into())),
            part(
                "Audio",
                Outcome::Failed,
                Verdict::Failed("no sound was taken".into()),
            ),
        ];
        let text = report(
            &results,
            &["The Hap clip could not be made".into()],
            Duration::from_secs(75),
        );
        assert!(text.contains("Result: Problems found"), "{text}");
        assert!(
            text.contains("1 parts passed, 1 failed, 0 not run, in 75 s."),
            "{text}"
        );
        assert!(
            text.contains("note: The Hap clip could not be made"),
            "{text}"
        );
        assert!(text.contains("FAILED   no sound was taken"), "{text}");
        let fine = report(&results[..1], &[], Duration::from_secs(60));
        assert!(fine.contains("Result: Passed"), "{fine}");
        assert!(report(&[], &[], Duration::ZERO).contains("Nothing could be run"));
    }
}
