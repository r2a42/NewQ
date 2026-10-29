//! What a test found, from its numbers. Pure functions of the samples, the
//! lap times and the engine's messages, so they are tested with made-up runs.

use crate::stats::Sample;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
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
        if findings.iter().any(|f| f.severity == Severity::Problem) {
            Verdict::ProblemsFound
        } else if findings.is_empty() {
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

/// Least-squares slope of `y` against `x`.
pub fn slope(x: &[f64], y: &[f64]) -> f64 {
    let n = x.len() as f64;
    if x.len() < 2 {
        return 0.0;
    }
    let (mx, my) = (x.iter().sum::<f64>() / n, y.iter().sum::<f64>() / n);
    let num: f64 = x.iter().zip(y).map(|(a, b)| (a - mx) * (b - my)).sum();
    let den: f64 = x.iter().map(|a| (a - mx).powi(2)).sum();
    if den == 0.0 { 0.0 } else { num / den }
}

/// How the high points of a number moved over the run, per hour, and whether
/// the later half of the run is really higher than the earlier. A show's memory
/// goes up and down with each lap (clips start and stop), so single samples
/// compared at different points of a lap say nothing: the peak of each window
/// of a couple of laps is compared instead, ignoring the first fifth (caches
/// filling). `None` if the run was too short for four windows.
fn growth(
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
    // The last window is usually not finished.
    peaks.pop();
    let peaks: Vec<f64> = peaks.into_iter().filter(|p| *p > f64::MIN).collect();
    let skip = peaks.len() / 5;
    let peaks = &peaks[skip..];
    if peaks.len() < 4 {
        return None;
    }
    let x: Vec<f64> = (0..peaks.len())
        .map(|i| i as f64 * window_s / 3600.0)
        .collect();
    let half = peaks.len() / 2;
    let mean = |p: &[f64]| p.iter().sum::<f64>() / p.len() as f64;
    let (early, late) = (mean(&peaks[..half]), mean(&peaks[half..]));
    Some((slope(&x, peaks), late / early.max(1.0)))
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
        let checks: [(&str, f64, Measure); 4] = [
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
            if let Some((per_hour, ratio)) = growth(samples, (nominal * 2.0).max(240.0), value)
                && per_hour > limit
                && ratio > 1.10
            {
                out.push(problem(format!(
                    "{what} kept growing ({per_hour:+.1} an hour): something is not being let go."
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
            out.push(note(format!(
                "{} of {} laps ran more than a quarter longer than the {nominal:.0} s they should take (the first was lap {i}, {l:.0} s): cues started late.",
                slow.len(),
                laps.len()
            )));
        }
        if laps.len() >= 6 {
            let k = (laps.len() / 3).clamp(2, 5);
            let first = laps[..k].iter().sum::<f64>() / k as f64;
            let last = laps[laps.len() - k..].iter().sum::<f64>() / k as f64;
            if last > first * 1.05 + 1.0 {
                out.push(problem(format!(
                    "Laps are taking longer as the test goes on ({first:.1} s at first, {last:.1} s lately): the program is slowing down."
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
        let f = analyse(&laps_of_ffmpeg(60.0), &[155.0; 11], 155.0, &[], 0.5);
        assert!(
            f.iter().any(|x| x.text.contains("FFmpeg's memory")),
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
                .any(|f| f.severity == Severity::Problem && f.text.contains("memory kept growing")),
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
