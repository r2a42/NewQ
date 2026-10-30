//! What a test found, from its numbers. Pure functions of the samples, the
//! lap times and the engine's messages, so they are tested with made-up runs.

use crate::stats::Sample;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Being watched: something moved that is usually harmless (a rise while
    /// the show warms up), and is only a problem if it keeps on. It does not
    /// count against the result.
    Watch,
    /// Worth knowing; nothing is wrong.
    Note,
    /// Something went wrong, or is going to.
    Problem,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub severity: Severity,
    pub text: String,
}

fn problem(text: impl Into<String>) -> Finding {
    Finding {
        severity: Severity::Problem,
        text: text.into(),
    }
}

fn watch(text: impl Into<String>) -> Finding {
    Finding {
        severity: Severity::Watch,
        text: text.into(),
    }
}

fn note(text: impl Into<String>) -> Finding {
    Finding {
        severity: Severity::Note,
        text: text.into(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Passed,
    PassedWithNotes,
    ProblemsFound,
}

impl Verdict {
    pub fn of(findings: &[Finding]) -> Verdict {
        let counted = findings.iter().filter(|f| f.severity != Severity::Watch);
        if counted.clone().any(|f| f.severity == Severity::Problem) {
            Verdict::ProblemsFound
        } else if counted.count() == 0 {
            Verdict::Passed
        } else {
            Verdict::PassedWithNotes
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Verdict::Passed => "Passed",
            Verdict::PassedWithNotes => "Passed, with notes",
            Verdict::ProblemsFound => "Problems found",
        }
    }
}

/// Whether the high points of a number are still climbing late in the run.
///
/// A show's memory goes up and down with each lap (clips start and stop), and
/// its high points are not even from lap to lap (how many FFmpeg processes
/// overlap depends on timing), so single samples say nothing: the peak of each
/// window of a couple of laps is taken, the first fifth of the run is ignored
/// (caches filling), and the middle third of what is left is compared with the
/// last, by their medians. A step up early in the run that then stays level
/// (the usual warm-up) is therefore not growth: the middle and the last are
/// alike. Returns the two medians and the hours between them, or `None` if the
/// run was too short to tell (nine windows).
fn growth(
    samples: &[Sample],
    window_s: f64,
    value: impl Fn(&Sample) -> Option<f64>,
) -> Option<(f64, f64, f64)> {
    let mut peaks: Vec<f64> = Vec::new();
    for s in samples {
        let Some(v) = value(s) else { continue };
        let w = (s.uptime_s as f64 / window_s) as usize;
        if peaks.len() <= w {
            peaks.resize(w + 1, f64::MIN);
        }
        peaks[w] = peaks[w].max(v);
    }
    // The last window is usually not finished.
    peaks.pop();
    let peaks: Vec<f64> = peaks.into_iter().filter(|p| *p > f64::MIN).collect();
    let skip = peaks.len() / 5;
    let q = &peaks[skip..];
    if q.len() < 9 {
        return None;
    }
    let third = q.len() / 3;
    let median = |part: &[f64]| {
        let mut v = part.to_vec();
        v.sort_by(|a, b| a.total_cmp(b));
        v[v.len() / 2]
    };
    let middle = median(&q[third..q.len() - third]);
    let last = median(&q[q.len() - third..]);
    let centre = |from: usize, to: usize| (from + to - 1) as f64 / 2.0;
    let span =
        (centre(q.len() - third, q.len()) - centre(third, q.len() - third)) * window_s / 3600.0;
    Some((middle, last, span))
}

/// Whether `growth`'s answer is a leak: the late high points are higher than the
/// middle ones by more than `limit` an hour would make them and by more than a
/// twentieth, which is more than the jitter of the peaks.
fn is_leak(g: Option<(f64, f64, f64)>, limit: f64) -> Option<f64> {
    let (middle, last, span) = g?;
    let rise = last - middle;
    (span > 0.0 && rise > (limit * span).max(0.05 * middle.max(1.0))).then(|| rise / span)
}

/// A rise worth keeping an eye on, short of what `is_leak` needs: the later
/// half of the high points is higher than the earlier by more than their
/// jitter. Returns the two halves' averages. Fewer windows are enough than
/// for `is_leak`, so this speaks earlier and is only ever a "tracking".
fn rising(
    samples: &[Sample],
    window_s: f64,
    value: impl Fn(&Sample) -> Option<f64>,
) -> Option<(f64, f64)> {
    let mut peaks: Vec<f64> = Vec::new();
    for s in samples {
        let Some(v) = value(s) else { continue };
        let w = (s.uptime_s as f64 / window_s) as usize;
        if peaks.len() <= w {
            peaks.resize(w + 1, f64::MIN);
        }
        peaks[w] = peaks[w].max(v);
    }
    peaks.pop();
    let peaks: Vec<f64> = peaks.into_iter().filter(|p| *p > f64::MIN).collect();
    let q = &peaks[peaks.len() / 5..];
    if q.len() < 4 {
        return None;
    }
    let mean = |p: &[f64]| p.iter().sum::<f64>() / p.len() as f64;
    let (early, late) = (mean(&q[..q.len() / 2]), mean(&q[q.len() / 2..]));
    (late > early * 1.10 && late - early > 20.0).then_some((early, late))
}

/// Messages from the engine that mean something failed.
pub fn is_failure(message: &str) -> bool {
    let m = message.to_lowercase();
    [
        "failed",
        "error",
        "could not",
        "couldn't",
        "can't",
        "not prepared",
        "took too long",
        "was lost",
        "unavailable",
        "not available",
        "no audio output",
    ]
    .iter()
    .any(|w| m.contains(w))
}

/// What a run found.
///
/// `samples` are the log's rows; `laps` the seconds each finished lap took;
/// `nominal` how long a lap should take; `messages` what the engine logged;
/// `hours` how long the test ran.
pub fn analyse(
    samples: &[Sample],
    laps: &[f64],
    nominal: f64,
    messages: &[String],
    hours: f64,
) -> Vec<Finding> {
    let mut out = Vec::new();

    // Growth over time needs a run long enough to tell it from filling caches.
    if hours >= 0.4 && samples.len() >= 30 {
        type Measure = fn(&Sample) -> Option<f64>;
        let checks: [(&str, f64, Measure); 6] = [
            ("The graphics card memory NewQ has asked for", 20.0, |s| {
                s.gpu_mb.map(|v| v as f64)
            }),
            ("NewQ's committed memory", 20.0, |s| {
                s.commit_mb.map(|v| v as f64)
            }),
            ("NewQ's memory", 5.0, |s| Some(s.rss_mb as f64)),
            ("FFmpeg's memory", 5.0, |s| Some(s.ffmpeg_rss_mb as f64)),
            ("The number of open files or handles", 20.0, |s| {
                s.fds.map(|v| v as f64)
            }),
            ("The number of threads", 10.0, |s| {
                s.threads.map(|v| v as f64)
            }),
        ];
        for (what, limit, value) in checks {
            let window = (nominal * 2.0).max(240.0);
            if let Some(per_hour) = is_leak(growth(samples, window, value), limit) {
                out.push(problem(format!(
                    "{what} has kept rising through the last part of the run and hasn't levelled off (about {per_hour:+.0} an hour): something may not be being released."
                )));
            } else if let Some((early, late)) = rising(samples, window, value) {
                out.push(watch(format!(
                    "Tracking: {what} is higher than earlier in the run ({early:.0} \u{2192} {late:.0}). A rise while the show warms up is normal and usually levels off; it only matters if it keeps rising."
                )));
            }
        }
    } else if hours < 0.4 {
        out.push(note(
            "This test was too short to tell a slow leak from caches filling: run the Medium or Long test for that.",
        ));
    }

    // The main loop stopping, even for a moment, is a dropped frame on every output.
    let stall = samples.iter().map(|s| s.stall_ms).max().unwrap_or(0);
    // The first sample's stall is the program starting its work.
    let stall = samples
        .iter()
        .skip(1)
        .map(|s| s.stall_ms)
        .max()
        .unwrap_or(stall);
    if stall > 1000 {
        out.push(problem(format!(
            "The program froze for {stall} ms once: every output stopped for that long."
        )));
    } else if stall > 250 {
        out.push(note(format!(
            "The longest pause of the main loop was {stall} ms."
        )));
    }

    // Late frames count only while a picture is being drawn.
    let drawn: Vec<u32> = samples
        .iter()
        .filter(|s| s.inputs.layers > 0)
        .map(|s| s.inputs.late_permille)
        .collect();
    if !drawn.is_empty() {
        let mean = drawn.iter().map(|v| f64::from(*v)).sum::<f64>() / drawn.len() as f64;
        let worst = drawn.iter().max().copied().unwrap_or(0);
        if mean > 200.0 {
            out.push(problem(format!(
                "The graphics card was behind: {:.0}% of frames were late on average while pictures were drawn. Fewer or smaller pictures, or a faster card, would help.",
                mean / 10.0
            )));
        } else if worst > 500 {
            out.push(note(format!(
                "At worst {}% of frames were late for a moment.",
                worst / 10
            )));
        }
    }

    let audio = samples
        .iter()
        .map(|s| s.inputs.audio_permille)
        .max()
        .unwrap_or(0);
    if audio > 600 {
        out.push(problem(format!(
            "Mixing the sound took up to {}% of the time it has: above about 90% it breaks up.",
            audio / 10
        )));
    } else if audio > 400 {
        out.push(note(format!(
            "Mixing the sound took up to {}% of its time.",
            audio / 10
        )));
    }

    // Laps: whether they take as long as they should, and whether that creeps up.
    if nominal > 0.0 && !laps.is_empty() {
        let slow: Vec<(usize, f64)> = laps
            .iter()
            .enumerate()
            .filter(|(_, l)| **l > nominal * 1.25 + 3.0)
            .map(|(i, l)| (i + 1, *l))
            .collect();
        if !slow.is_empty() {
            let (i, l) = slow[0];
            let twice = if l > nominal * 1.8 && l < nominal * 2.2 {
                " That is about two laps' worth: a lap may have been held up for most of its length, or its start missed."
            } else {
                ""
            };
            out.push(note(format!(
                "{} of {} laps ran more than a quarter longer than the {nominal:.0} s they should take (the first was lap {i}, {l:.0} s).{twice} What held it up isn't in the numbers; events.log says what was playing.",
                slow.len(),
                laps.len()
            )));
        }
        if laps.len() >= 6 {
            let k = (laps.len() / 3).clamp(2, 5);
            // Medians, not averages: one long lap among the last few (a lap held
            // up once) is not laps taking longer as the test goes on.
            let median = |part: &[f64]| {
                let mut v = part.to_vec();
                v.sort_by(|a, b| a.total_cmp(b));
                v[v.len() / 2]
            };
            let first = median(&laps[..k]);
            let last = median(&laps[laps.len() - k..]);
            if last > first * 1.05 + 1.0 {
                out.push(problem(format!(
                    "Laps have been taking longer as the test goes on ({first:.1} s at first, {last:.1} s lately): the program may be slowing down."
                )));
            }
        }
        let early = laps.iter().filter(|l| **l < nominal * 0.8 - 3.0).count();
        if early > 0 {
            out.push(note(format!(
                "{early} laps ended early: some cues did not play for their full length."
            )));
        }
    }

    // What the engine said.
    let mut failures: Vec<&String> = Vec::new();
    for m in messages {
        if is_failure(m) && !failures.contains(&m) {
            failures.push(m);
        }
    }
    let no_audio = failures.iter().any(|m| {
        let l = m.to_lowercase();
        l.contains("audio") && (l.contains("device") || l.contains("output"))
    });
    for m in failures.iter().take(8) {
        let l = m.to_lowercase();
        if l.contains("audio") && (l.contains("device") || l.contains("output")) {
            continue;
        }
        out.push(problem(format!("The engine reported: {m}")));
    }
    if failures.len() > 8 {
        out.push(problem(format!(
            "...and {} other kinds of message (see events.log).",
            failures.len() - 8
        )));
    }
    if no_audio {
        out.push(note(
            "There was no working audio output, so the sound was not tested.",
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::Inputs;

    fn sample(i: u64, rss: u64) -> Sample {
        Sample {
            uptime_s: i * 10,
            rss_mb: rss,
            ffmpeg_rss_mb: 200,
            threads: Some(30),
            fds: Some(100),
            stall_ms: 20,
            inputs: Inputs {
                layers: 2,
                ..Inputs::default()
            },
            ..Sample::default()
        }
    }

    fn steady(n: u64) -> Vec<Sample> {
        (0..n).map(|i| sample(i, 400)).collect()
    }

    /// Clips start and stop every lap, so FFmpeg's memory is a sawtooth: the
    /// phase of the lap at the first and last sample must not look like growth.
    fn laps_of_ffmpeg(growth_per_lap_mb: f64) -> Vec<Sample> {
        (0..180u64)
            .map(|i| {
                let t = i * 10;
                let lap = (t / 155) as f64;
                let phase = t % 155;
                let base = if (50..80).contains(&phase) {
                    900.0
                } else if phase < 50 {
                    400.0
                } else {
                    100.0
                };
                let mut s = sample(i, 250);
                s.ffmpeg_rss_mb = (base + lap * growth_per_lap_mb) as u64;
                s
            })
            .collect()
    }

    #[test]
    fn the_sawtooth_of_clips_starting_and_stopping_is_not_a_leak_but_real_growth_is() {
        let f = analyse(&laps_of_ffmpeg(0.0), &[155.0; 11], 155.0, &[], 0.5);
        assert!(f.is_empty(), "{f:?}");
        // Half an hour of growth is only tracked; it takes longer to call it.
        let f = analyse(&laps_of_ffmpeg(60.0), &[155.0; 11], 155.0, &[], 0.5);
        assert!(f.iter().all(|x| x.severity != Severity::Problem), "{f:?}");
        assert!(
            f.iter()
                .any(|x| x.severity == Severity::Watch && x.text.contains("FFmpeg's memory")),
            "{f:?}"
        );
    }

    /// FFmpeg's memory, one sample in a window of two laps carrying that
    /// window's peak (and a low reading in all the others).
    fn windows_of_ffmpeg(peaks: &[f64]) -> Vec<Sample> {
        let mut out = Vec::new();
        for (w, p) in peaks.iter().enumerate() {
            for k in 0..31u64 {
                let t = w as u64 * 310 + k * 10;
                let mut s = sample(t / 10, 250);
                s.ffmpeg_rss_mb = if k == 10 { *p as u64 } else { 100 };
                out.push(s);
            }
        }
        out
    }

    #[test]
    fn a_step_up_in_the_first_half_hour_that_then_stays_level_is_tracked_never_called_a_leak() {
        // The shape of an hour on Windows: the peaks of the first windows are
        // lower, then settle at about 1.2 GB. At every point of the run, the
        // worst it may say is that it is tracking it, in gentle words.
        let peaks = [
            627.0, 1021.0, 903.0, 937.0, 1128.0, 1229.0, 1218.0, 1217.0, 1227.0, 1232.0, 1213.0,
            1208.0, 1214.0,
        ];
        for upto in 6..=peaks.len() {
            let samples = windows_of_ffmpeg(&peaks[..upto]);
            let hours = samples.len() as f64 * 10.0 / 3600.0;
            let f = analyse(&samples, &[155.0; 5], 155.0, &[], hours);
            assert!(
                f.iter().all(|x| x.severity != Severity::Problem),
                "{upto} windows: {f:?}"
            );
            for x in &f {
                assert!(!x.text.contains("not being let go"), "{}", x.text);
            }
            // Tracking alone does not change the result.
            assert_eq!(Verdict::of(&f), Verdict::of(&[]), "{upto} windows: {f:?}");
        }
    }

    #[test]
    fn growth_that_goes_on_for_hours_is_a_problem_in_measured_words() {
        let peaks: Vec<f64> = (0..26).map(|w| 1000.0 + 80.0 * w as f64).collect();
        let samples = windows_of_ffmpeg(&peaks);
        let hours = samples.len() as f64 * 10.0 / 3600.0;
        let f = analyse(&samples, &[155.0; 5], 155.0, &[], hours);
        let p = f
            .iter()
            .find(|x| x.severity == Severity::Problem)
            .unwrap_or_else(|| panic!("{f:?}"));
        assert!(
            p.text.contains("hasn't levelled off") && p.text.contains("may not"),
            "{}",
            p.text
        );
    }

    #[test]
    fn one_long_lap_is_not_laps_taking_longer_as_the_test_goes_on() {
        // Ten laps, the ninth held up for a whole extra lap: the average of the
        // last three is 205.7 s, which once read as "the program is slowing down".
        let mut laps = vec![155.3; 10];
        laps[8] = 307.0;
        let f = analyse(&steady(400), &laps, 155.0, &[], 1.1);
        assert!(f.iter().all(|x| x.severity != Severity::Problem), "{f:?}");
        let long = f
            .iter()
            .find(|x| x.text.contains("quarter longer"))
            .expect("noted");
        assert!(
            long.text.contains("lap 9") && long.text.contains("about two laps"),
            "{}",
            long.text
        );
        // A real creep is still found.
        let creeping: Vec<f64> = (0..12).map(|i| 155.0 + i as f64 * 6.0).collect();
        let f = analyse(&steady(400), &creeping, 155.0, &[], 1.1);
        assert!(
            f.iter()
                .any(|x| x.severity == Severity::Problem && x.text.contains("taking longer")),
            "{f:?}"
        );
    }

    #[test]
    fn a_steady_run_passes_and_a_short_one_says_what_it_cannot_tell() {
        let f = analyse(&steady(400), &[100.0; 20], 100.0, &[], 1.1);
        assert!(f.is_empty(), "{f:?}");
        assert_eq!(Verdict::of(&f), Verdict::Passed);
        let f = analyse(&steady(8), &[50.0], 50.0, &[], 0.02);
        assert_eq!(Verdict::of(&f), Verdict::PassedWithNotes);
        assert!(f[0].text.contains("too short"));
    }

    #[test]
    fn growth_is_found_only_when_it_keeps_on() {
        // 1 MB a sample, 10 s apart: 360 MB an hour.
        let leaking: Vec<Sample> = (0..400).map(|i| sample(i, 400 + i)).collect();
        let f = analyse(&leaking, &[], 0.0, &[], 1.1);
        assert!(
            f.iter()
                .any(|f| f.severity == Severity::Problem && f.text.contains("kept rising")),
            "{f:?}"
        );
        // Filling up at the start and then level: not a leak.
        let warmup: Vec<Sample> = (0..400).map(|i| sample(i, 400 + 100.min(i * 5))).collect();
        assert!(analyse(&warmup, &[], 0.0, &[], 1.1).is_empty());
        // Threads and handles too.
        let mut handles = steady(400);
        for (i, s) in handles.iter_mut().enumerate() {
            s.fds = Some(100 + i);
        }
        let f = analyse(&handles, &[], 0.0, &[], 1.1);
        assert!(f.iter().any(|f| f.text.contains("open files")), "{f:?}");
    }

    #[test]
    fn freezes_late_frames_and_audio_load_are_judged() {
        let mut s = steady(100);
        s[50].stall_ms = 1500;
        let f = analyse(&s, &[], 0.0, &[], 1.0);
        assert!(
            f.iter()
                .any(|f| f.severity == Severity::Problem && f.text.contains("froze for 1500"))
        );
        // The first sample's stall is the start-up and not counted.
        let mut s = steady(100);
        s[0].stall_ms = 900;
        assert!(analyse(&s, &[], 0.0, &[], 1.0).is_empty());
        // Late frames count only while drawing.
        let mut s = steady(100);
        for x in &mut s {
            x.inputs.late_permille = 1000;
            x.inputs.layers = 0;
        }
        assert!(analyse(&s, &[], 0.0, &[], 1.0).is_empty());
        for x in &mut s {
            x.inputs.layers = 1;
        }
        let f = analyse(&s, &[], 0.0, &[], 1.0);
        assert!(
            f.iter()
                .any(|f| f.text.contains("graphics card was behind")),
            "{f:?}"
        );
        let mut s = steady(100);
        s[3].inputs.audio_permille = 700;
        assert!(
            analyse(&s, &[], 0.0, &[], 1.0)
                .iter()
                .any(|f| f.text.contains("sound took up to 70%"))
        );
    }

    #[test]
    fn laps_that_drift_or_run_late_or_short_are_noticed() {
        let drifting: Vec<f64> = (0..12).map(|i| 100.0 + i as f64 * 2.0).collect();
        let f = analyse(&steady(100), &drifting, 100.0, &[], 1.0);
        assert!(
            f.iter().any(|f| f.severity == Severity::Problem
                && f.text.contains("longer as the test goes on")),
            "{f:?}"
        );
        let mut laps = vec![100.0; 10];
        laps[4] = 160.0;
        let f = analyse(&steady(100), &laps, 100.0, &[], 1.0);
        assert!(f.iter().any(|f| f.text.contains("lap 5")), "{f:?}");
        assert!(!f.iter().any(|f| f.severity == Severity::Problem));
        let f = analyse(&steady(100), &[40.0, 100.0], 100.0, &[], 1.0);
        assert!(f.iter().any(|f| f.text.contains("ended early")));
    }

    #[test]
    fn engine_messages_are_sorted_into_problems_and_no_sound() {
        let msgs: Vec<String> = [
            "GO 1 (Title)",
            "Cue 6 (Music): The requested audio device is not available.",
            "Cue 6 (Music): The requested audio device is not available.",
            "Cue 4 (Clip): reading its file took too long",
            "Fade cue 8: target 4 is not playing",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let f = analyse(&steady(100), &[], 0.0, &msgs, 1.0);
        assert!(
            f.iter()
                .any(|f| f.severity == Severity::Problem && f.text.contains("took too long"))
        );
        let notes: Vec<_> = f.iter().filter(|f| f.severity == Severity::Note).collect();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].text.contains("no working audio output"));
        // The audio message is not a problem of its own.
        assert!(
            !f.iter()
                .any(|f| f.severity == Severity::Problem && f.text.contains("audio device"))
        );
    }
}
