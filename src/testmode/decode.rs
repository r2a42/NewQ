//! Does hardware decoding help on this computer? Plays a wall of four 4K clips
//! twice, with hardware decoding off and then on, measures what each costs and
//! says which to use. The user's show is put aside and the setting put back,
//! exactly as for the other tests (see `session`).
//!
//! The 4K clip is made from the real film already downloaded for the other
//! tests (scaled up and encoded here), because the downloads have no 4K. Where
//! this FFmpeg has no H.264 encoder that works (the Mac download's had none
//! before 0.14.1), the 1080p H.264 film itself is played, eight at a time: the
//! same number of pixels a second, though not the same size of picture.

use super::analysis::{Finding, Severity};
use super::assets::{self, Kind, Resolved};
use super::session::{End, MemoryWatch, clock};
use crate::engine::Engine;
use crate::media::ffmpeg::{Ffmpeg, no_console, output_cancellable};
use crate::media::video;
use crate::show::Show;
use crate::stats::{Inputs, Sample, StatsLog};
use crate::ui::{TestStash, UiState};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

/// Seconds at the start of each trial that are not counted: the decoders are
/// starting and the first frames are being made.
pub const WARM_UP: u64 = 8;
/// Seconds measured in each trial, unless a test says otherwise.
pub const TRIAL_SECS: u64 = 40;
/// How many 4K clips play at once.
pub const TILES: usize = 4;
/// How many 1080p clips, when there is no 4K clip to make: the same pixels a second.
pub const TILES_1080: usize = 8;

/// What one trial cost, averaged over its measured seconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Summary {
    pub samples: usize,
    /// NewQ's own use of the processor, in percent of one core.
    pub newq_cpu: f32,
    /// FFmpeg's, the same way.
    pub ffmpeg_cpu: f32,
    /// Thousandths of frames that were late.
    pub late_permille: f32,
    /// The longest the main loop stopped, in milliseconds.
    pub stall_ms: u64,
    pub rss_mb: u64,
}

impl Summary {
    pub fn total_cpu(&self) -> f32 {
        self.newq_cpu + self.ffmpeg_cpu
    }
}

pub fn summarise(samples: &[Sample]) -> Summary {
    if samples.is_empty() {
        return Summary::default();
    }
    let n = samples.len() as f32;
    Summary {
        samples: samples.len(),
        newq_cpu: samples.iter().map(|s| s.cpu_pct).sum::<f32>() / n,
        ffmpeg_cpu: samples.iter().map(|s| s.ffmpeg_cpu_pct).sum::<f32>() / n,
        late_permille: samples
            .iter()
            .map(|s| s.inputs.late_permille as f32)
            .sum::<f32>()
            / n,
        stall_ms: samples.iter().map(|s| s.stall_ms).max().unwrap_or(0),
        rss_mb: samples.iter().map(|s| s.rss_mb).max().unwrap_or(0),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Advice {
    TurnOn,
    LeaveOff,
    /// It makes no real difference here: leave the setting as it is.
    NoDifference,
    Inconclusive,
}

/// What the two trials say. `engaged` is what FFmpeg said about the hardware
/// decoder it picked, if it said.
pub fn advise(off: &Summary, on: &Summary, engaged: Option<&str>) -> (Advice, String) {
    if off.samples < 3 || on.samples < 3 {
        return (
            Advice::Inconclusive,
            "The trials were too short to measure anything. Run the comparison again and let it finish."
                .into(),
        );
    }
    let (a, b) = (off.total_cpu(), on.total_cpu());
    let saved = (a - b) / a.max(1.0);
    let worse_late = on.late_permille - off.late_permille > 50.0;
    let worse_stall = on.stall_ms > 250 && on.stall_ms as f64 > off.stall_ms as f64 * 2.0;
    let numbers = format!(
        "Processor use {a:.0}% of one core with it off, {b:.0}% with it on; late frames {:.1}% and {:.1}%.",
        off.late_permille / 10.0,
        on.late_permille / 10.0
    );
    if worse_late || worse_stall || saved <= -0.10 {
        return (
            Advice::LeaveOff,
            format!(
                "Hardware decoding made things worse on this computer. Leave it off. {numbers}"
            ),
        );
    }
    if saved >= 0.15 {
        return (
            Advice::TurnOn,
            format!(
                "Hardware decoding takes {:.0}% off the processor's work here, with no more late frames. Turn it on. {numbers}",
                saved * 100.0
            ),
        );
    }
    let why = match engaged {
        Some(how) => format!("FFmpeg chose a hardware decoder ({how}) but it changes little."),
        None => "FFmpeg did not say it used a hardware decoder, so this computer may not be able to decode this on the graphics card.".into(),
    };
    (
        Advice::NoDifference,
        format!(
            "Hardware decoding makes little difference on this computer, so leave the setting as it is. {why} {numbers}"
        ),
    )
}

/// The line in FFmpeg's log that says which hardware decoder it chose.
pub fn engaged_from(log: &str) -> Option<String> {
    log.lines()
        .map(str::trim)
        .find(|l| {
            let l = l.to_lowercase();
            l.contains("hwaccel type") || l.contains("using hardware") || l.contains("hwaccel:")
        })
        .map(|l| l.to_string())
}

/// The H.264 encoders to try for the 4K clip, best first, with their options.
/// The Windows and Mac downloads' FFmpeg has no x264, but has the system's.
pub fn encoders(has: impl Fn(&str) -> bool) -> Vec<(&'static str, Vec<String>)> {
    let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut out = Vec::new();
    let table: [(&str, &[&str]); 6] = [
        (
            "libx264",
            &[
                "-preset", "veryfast", "-b:v", "40M", "-maxrate", "60M", "-bufsize", "80M",
            ],
        ),
        ("h264_videotoolbox", &["-b:v", "40M"]),
        ("h264_nvenc", &["-b:v", "40M"]),
        ("h264_amf", &["-b:v", "40M"]),
        ("h264_qsv", &["-b:v", "40M"]),
        ("h264_mf", &["-b:v", "40M"]),
    ];
    for (name, opts) in table {
        if has(name) {
            let mut args = v(&["-c:v", name]);
            args.extend(v(opts));
            args.extend(v(&["-pix_fmt", "yuv420p"]));
            out.push((name, args));
        }
    }
    out
}

/// The 4K clip and how it was made.
struct Made {
    clip: Resolved,
    encoder: String,
    engaged: Option<String>,
    /// How many play at once, and how they are described in the report.
    tiles: usize,
    what: String,
}

fn make_clip(
    ff: &Ffmpeg,
    source: &Path,
    dest: &Path,
    cancel: &AtomicBool,
    candidates: Vec<(&'static str, Vec<String>)>,
) -> Result<Made, String> {
    let engaged = |clip: &Path| {
        let mut cmd = std::process::Command::new(&ff.ffmpeg);
        no_console(&mut cmd);
        let out = cmd
            .args([
                "-hide_banner",
                "-nostdin",
                "-loglevel",
                "verbose",
                "-hwaccel",
                "auto",
                "-i",
            ])
            .arg(clip)
            .args(["-t", "1", "-f", "null", "-"])
            .stdin(std::process::Stdio::null())
            .output()
            .ok()?;
        engaged_from(&String::from_utf8_lossy(&out.stderr))
    };
    let good = |r: &Resolved| r.width == 3840 && r.height == 2160 && r.codec == "h264";
    if let Ok(r) = assets::check(ff, dest, Kind::Video)
        && good(&r)
    {
        return Ok(Made {
            engaged: engaged(dest),
            clip: r,
            encoder: "kept from an earlier run".into(),
            tiles: TILES,
            what: four_k(),
        });
    }
    // A name of this attempt's own: a stopped comparison's encoder may still be
    // finishing, and must not share a file with the next one.
    let tmp = dest.with_extension(format!(
        "{}.part.mp4",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    ));
    let mut last = String::new();
    for (name, args) in candidates {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled.".into());
        }
        let _ = std::fs::remove_file(&tmp);
        let out = output_cancellable(
            ff.command()
                .args(["-y", "-i"])
                .arg(source)
                .args([
                    "-t",
                    "10",
                    "-an",
                    "-vf",
                    "scale=3840:2160:flags=bicubic,fps=30",
                ])
                .args(&args)
                .arg(&tmp),
            cancel,
        )
        .map_err(|e| e.to_string())?;
        if cancel.load(Ordering::Relaxed) {
            let _ = std::fs::remove_file(&tmp);
            return Err("Cancelled.".into());
        }
        if out.status.success()
            && let Ok(r) = assets::check(ff, &tmp, Kind::Video)
            && good(&r)
        {
            std::fs::rename(&tmp, dest).map_err(|e| e.to_string())?;
            let r = assets::check(ff, dest, Kind::Video)?;
            return Ok(Made {
                engaged: engaged(dest),
                clip: r,
                encoder: name.to_string(),
                tiles: TILES,
                what: four_k(),
            });
        }
        last = String::from_utf8_lossy(&out.stderr).trim().to_string();
    }
    let _ = std::fs::remove_file(&tmp);
    // No 4K clip could be made: the film itself will do, if it is H.264.
    match assets::check(ff, source, Kind::Video) {
        Ok(r) if r.codec == "h264" => Ok(Made {
            engaged: engaged(source),
            clip: r,
            encoder: if last.is_empty() {
                "none: this FFmpeg has no H.264 encoder".to_string()
            } else {
                format!("none worked (the last said: {last})")
            },
            tiles: TILES_1080,
            what: format!("{} 1080p H.264 clips at 60 fps (no 4K clip could be made here)", TILES_1080),
        }),
        _ => Err(
            "No 4K H.264 clip could be made here (this FFmpeg has no working H.264 encoder) and the film to test with isn't H.264 either, so there is nothing for the graphics card to decode."
                .into(),
        ),
    }
}

fn four_k() -> String {
    format!("{TILES} 4K H.264 clips at 30 fps")
}

pub enum Stage {
    /// The 4K clip is being made.
    Making,
    /// Playing the wall with hardware decoding `hw`.
    Trial { hw: bool, began: Instant },
}

/// A comparison under way.
pub struct Compare {
    pub stage: Stage,
    pub folder: PathBuf,
    pub secs: u64,
    pub live: Option<Sample>,
    pub encoder: Option<String>,
    pub engaged: Option<String>,
    pub results: [Option<Summary>; 2],
    orig_hw: bool,
    rx: Option<Receiver<Result<Made, String>>>,
    cancel: Arc<AtomicBool>,
    started: Instant,
    stash: Option<TestStash>,
    samples: Vec<Sample>,
    stats: Option<StatsLog>,
    memory: MemoryWatch,
    pub(super) stop: bool,
    log_path: PathBuf,
    clip: Option<Resolved>,
    displays: Vec<super::plan::DisplayChoice>,
    /// What plays, for the window and the report.
    pub what: String,
    tiles: usize,
}

/// What a finished comparison found.
pub struct CompareResult {
    pub off: Option<Summary>,
    pub on: Option<Summary>,
    pub advice: Advice,
    pub engaged: Option<String>,
    /// The setting the computer had before.
    pub was_on: bool,
}

impl Compare {
    pub fn begin(
        ff: Ffmpeg,
        log_dir: &Path,
        source: PathBuf,
        folder: PathBuf,
        secs: u64,
        orig_hw: bool,
        displays: Vec<super::plan::DisplayChoice>,
    ) -> Compare {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let dest = assets::cache_dir(log_dir).join("compare_4k_h264.mp4");
        let c2 = cancel.clone();
        let _ = std::thread::Builder::new()
            .name("decode-clip".into())
            .spawn(move || {
                let candidates = encoders(|n| ff.has_encoder(n));
                let _ = tx.send(make_clip(&ff, &source, &dest, &c2, candidates));
            });
        Compare {
            stage: Stage::Making,
            folder,
            secs,
            live: None,
            encoder: None,
            engaged: None,
            results: [None, None],
            orig_hw,
            rx: Some(rx),
            cancel,
            started: Instant::now(),
            stash: None,
            samples: Vec::new(),
            stats: None,
            memory: MemoryWatch::new(),
            stop: false,
            log_path: log_dir.join("newq.log"),
            clip: None,
            displays,
            what: four_k(),
            tiles: TILES,
        }
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn request_stop(&mut self) {
        self.stop = true;
        self.cancel.store(true, Ordering::Relaxed);
    }

    fn start_trial(&mut self, hw: bool, show: &Show, engine: &mut Engine, now: Instant) {
        engine.stop_all(0.0);
        video::set_hw_decode(hw);
        self.samples.clear();
        let name = if hw {
            "stats-hardware-on.csv"
        } else {
            "stats-hardware-off.csv"
        };
        self.stats = StatsLog::create(
            &self.folder.join(name),
            Duration::from_secs(2),
            self.log_path.clone(),
        )
        .ok();
        engine.playhead = show.cues.first().map(|c| c.id);
        engine.go(show);
        engine.log(format!(
            "Test mode: decoding comparison, hardware decoding {}",
            if hw { "on" } else { "off" }
        ));
        self.stage = Stage::Trial { hw, began: now };
    }

    /// One look. Returns why the comparison should end, if it should.
    pub(super) fn tick(
        &mut self,
        show: &mut Show,
        engine: &mut Engine,
        st: &mut UiState,
        now: Instant,
    ) -> Option<End> {
        if self.stop {
            return Some(End::Stopped);
        }
        if let Some(mb) = self.memory.over(now) {
            return Some(End::Failed(format!(
                "NewQ was using {:.1} GB of memory, so the comparison was stopped",
                mb as f64 / 1024.0
            )));
        }
        match self.stage {
            Stage::Making => {
                let made = match self.rx.as_ref()?.try_recv() {
                    Err(TryRecvError::Empty) => return None,
                    Err(TryRecvError::Disconnected) => {
                        return Some(End::Failed(
                            "making the 4K clip stopped unexpectedly".into(),
                        ));
                    }
                    Ok(Err(e)) => return Some(End::Failed(e)),
                    Ok(Ok(m)) => m,
                };
                self.rx = None;
                if let Some(why) = st.test_refusal() {
                    return Some(End::Failed(why.to_string()));
                }
                self.encoder = Some(made.encoder.clone());
                self.engaged = made.engaged.clone();
                self.what = made.what.clone();
                self.tiles = made.tiles;
                let built =
                    super::plan::build_decode(show, &made.clip.path, made.tiles, &self.displays);
                self.clip = Some(made.clip);
                self.stash = Some(st.begin_test(show, engine, built.show));
                engine.set_log_tap(true);
                self.start_trial(false, show, engine, now);
                None
            }
            Stage::Trial { hw, began } => {
                let elapsed = now.duration_since(began).as_secs();
                if let Some(sample) = self.stats.as_mut().and_then(|s| {
                    s.frame(now, || Inputs {
                        playing: engine.active.len(),
                        started_total: engine.started_total(),
                        layers: engine.picture_layers(),
                        late_permille: crate::render::late_frames_permille(),
                        audio_permille: 0,
                    })
                }) {
                    self.live = Some(sample);
                    if elapsed >= WARM_UP {
                        self.samples.push(sample);
                    }
                }
                if !engine.is_active() && elapsed > 3 {
                    return Some(End::Failed("the video wall stopped by itself".into()));
                }
                if elapsed >= WARM_UP + self.secs {
                    self.results[usize::from(hw)] = Some(summarise(&self.samples));
                    if hw {
                        return Some(End::Finished);
                    }
                    self.start_trial(true, show, engine, now);
                }
                None
            }
        }
    }

    /// End: stop, put the show and the setting back, write the files.
    pub(super) fn finish(
        mut self,
        end: End,
        show: &mut Show,
        engine: &mut Engine,
        st: &mut UiState,
    ) -> (CompareResult, Vec<Finding>, String, End) {
        engine.stop_all(0.0);
        if let Some(stash) = self.stash.take() {
            st.end_test(show, engine, stash);
        }
        engine.set_log_tap(false);
        video::set_hw_decode(self.orig_hw);
        let (advice, text) = match (&self.results[0], &self.results[1]) {
            (Some(off), Some(on)) => advise(off, on, self.engaged.as_deref()),
            _ => (
                Advice::Inconclusive,
                "Both trials have to finish to compare them.".into(),
            ),
        };
        let mut findings = Vec::new();
        match &end {
            End::Failed(why) => findings.push(Finding {
                severity: Severity::Problem,
                text: format!("The comparison could not go on: {why}."),
            }),
            End::Stopped | End::Quit => findings.push(Finding {
                severity: Severity::Note,
                text: "The comparison was stopped before both trials were done.".into(),
            }),
            End::Finished => {}
        }
        findings.push(Finding {
            severity: if advice == Advice::LeaveOff {
                Severity::Problem
            } else {
                Severity::Note
            },
            text: text.clone(),
        });
        if let Some(e) = &self.encoder {
            findings.push(Finding {
                severity: Severity::Note,
                text: format!("The test clip was made with: {e}."),
            });
        }
        let row = |name: &str, s: &Option<Summary>| match s {
            Some(s) => format!(
                "  {name:<20} processor {:>5.0}% (NewQ {:.0}%, FFmpeg {:.0}%), late frames {:.1}%, longest stall {} ms, memory {} MB",
                s.total_cpu(),
                s.newq_cpu,
                s.ffmpeg_cpu,
                s.late_permille / 10.0,
                s.stall_ms,
                s.rss_mb
            ),
            None => format!("  {name:<20} not measured"),
        };
        let report = format!(
            "NewQ Test Mode: hardware decoding comparison\n\
             ===========================================\n\
             NewQ {}\n\
             A wall of {}, {} s measured per trial after {} s of warming up.\n\
             Ended: {} after {}. Your show was put aside and is back, unchanged; the setting is as it was (hardware decoding {}).\n\n\
             Result\n------\n  {}\n\n\
             Measurements (processor use is in percent of one core)\n-----------------------------------------------------\n{}\n{}\n\n\
             FFmpeg's own account of the hardware decoder: {}\n\
             The clip was made with: {}\n",
            env!("CARGO_PKG_VERSION"),
            self.what,
            self.secs,
            WARM_UP,
            match &end {
                End::Finished => "it ran its course".to_string(),
                End::Stopped => "stopped by you".to_string(),
                End::Failed(w) => format!("could not go on ({w})"),
                End::Quit => "NewQ was closed".to_string(),
            },
            clock(self.started.elapsed()),
            if self.orig_hw { "on" } else { "off" },
            text,
            row("Hardware decoding off", &self.results[0]),
            row("Hardware decoding on", &self.results[1]),
            self.engaged.as_deref().unwrap_or("it did not say"),
            self.encoder.as_deref().unwrap_or("not made"),
        );
        let _ = std::fs::write(self.folder.join("report.txt"), &report);
        let result = CompareResult {
            off: self.results[0],
            on: self.results[1],
            advice,
            engaged: self.engaged.clone(),
            was_on: self.orig_hw,
        };
        (result, findings, report, end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(cpu: f32, ff: f32, late: u32, stall: u64) -> Sample {
        Sample {
            cpu_pct: cpu,
            ffmpeg_cpu_pct: ff,
            stall_ms: stall,
            inputs: Inputs {
                late_permille: late,
                ..Inputs::default()
            },
            ..Sample::default()
        }
    }

    fn summary(cpu: f32, ff: f32, late: f32, stall: u64) -> Summary {
        Summary {
            samples: 20,
            newq_cpu: cpu,
            ffmpeg_cpu: ff,
            late_permille: late,
            stall_ms: stall,
            rss_mb: 500,
        }
    }

    #[test]
    fn samples_are_averaged_and_the_worst_stall_kept() {
        let s = summarise(&[sample(10.0, 100.0, 0, 20), sample(30.0, 200.0, 40, 90)]);
        assert_eq!(s.samples, 2);
        assert!((s.newq_cpu - 20.0).abs() < 1e-3 && (s.ffmpeg_cpu - 150.0).abs() < 1e-3);
        assert!((s.late_permille - 20.0).abs() < 1e-3);
        assert_eq!(s.stall_ms, 90);
        assert_eq!(summarise(&[]), Summary::default());
    }

    #[test]
    fn the_advice_follows_the_numbers() {
        let off = summary(30.0, 300.0, 0.0, 40);
        // Much cheaper and no worse: turn it on.
        let (a, t) = advise(&off, &summary(40.0, 120.0, 0.0, 40), Some("videotoolbox"));
        assert_eq!(a, Advice::TurnOn, "{t}");
        // About the same: no difference, and it says whether FFmpeg used hardware.
        let (a, t) = advise(&off, &summary(30.0, 290.0, 0.0, 40), None);
        assert_eq!(a, Advice::NoDifference);
        assert!(t.contains("did not say"), "{t}");
        let (a, t) = advise(
            &off,
            &summary(30.0, 290.0, 0.0, 40),
            Some("using hwaccel type d3d11va"),
        );
        assert_eq!(a, Advice::NoDifference);
        assert!(t.contains("d3d11va") && t.contains("chose"), "{t}");
        // Cheaper but dropping frames: leave it off.
        let (a, _) = advise(&off, &summary(30.0, 100.0, 120.0, 40), Some("x"));
        assert_eq!(a, Advice::LeaveOff);
        // Costing more: leave it off.
        let (a, _) = advise(&off, &summary(60.0, 330.0, 0.0, 40), Some("x"));
        assert_eq!(a, Advice::LeaveOff);
        // A long stall that the other run didn't have: leave it off.
        let (a, _) = advise(&off, &summary(30.0, 100.0, 0.0, 900), Some("x"));
        assert_eq!(a, Advice::LeaveOff);
        // Nothing measured.
        let (a, _) = advise(&Summary::default(), &off, None);
        assert_eq!(a, Advice::Inconclusive);
    }

    #[test]
    fn ffmpegs_choice_of_hardware_is_found_in_its_log() {
        let log = "Input #0, mov\n[h264 @ 0x1] Using auto hwaccel type videotoolbox with new default device.\nStream #0";
        assert!(engaged_from(log).unwrap().contains("videotoolbox"));
        assert_eq!(engaged_from("nothing about it\nStream #0:0"), None);
    }

    #[test]
    fn without_an_encoder_the_h264_film_itself_is_played_eight_at_a_time() {
        let Some((ff, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let dest =
            std::env::temp_dir().join(format!("newq_decode_none_{}.mp4", std::process::id()));
        let _ = std::fs::remove_file(&dest);
        let cancel = AtomicBool::new(false);
        let made = make_clip(&ff, &clip, &dest, &cancel, Vec::new());
        let is_h264 = assets::check(&ff, &clip, Kind::Video).is_ok_and(|r| r.codec == "h264");
        if is_h264 {
            let Ok(m) = made else {
                panic!("the film itself should be used")
            };
            assert_eq!(m.tiles, TILES_1080);
            assert!(m.what.contains("1080p") && m.encoder.contains("no H.264 encoder"));
            assert!(!dest.exists());
        } else {
            // Nothing for hardware to decode: said plainly, not guessed around.
            assert!(made.err().is_some_and(|e| e.contains("isn't H.264")));
        }
    }

    #[test]
    fn the_clip_is_made_with_the_best_encoder_present_and_none_is_an_error_not_a_guess() {
        let e = encoders(|n| n == "h264_videotoolbox" || n == "libx264");
        assert_eq!(e[0].0, "libx264");
        assert_eq!(e[1].0, "h264_videotoolbox");
        assert!(e[0].1.contains(&"yuv420p".to_string()));
        assert!(encoders(|_| false).is_empty());
        assert_eq!(encoders(|n| n == "h264_mf")[0].0, "h264_mf");
    }
}
