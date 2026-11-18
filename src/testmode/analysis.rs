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

/// What a heavier lap makes of the findings: it is meant to push the graphics
/// card, so a card that fell behind is where its limit is, not a fault.
pub fn for_load(findings: Vec<Finding>, heavy: bool) -> Vec<Finding> {
    if !heavy {
        return findings;
    }
    findings
        .into_iter()
        .map(|f| {
            if f.severity == Severity::Problem && f.text.starts_with("The graphics card was behind")
            {
                note(format!(
                    "This is the heavier test, which is meant to push the graphics card until it falls behind. {} That is where this computer's limit is under this load; run the ordinary test to see whether it copes with a normal show.",
                    f.text.split(" Fewer or smaller").next().unwrap_or(&f.text).trim_end()
                ))
            } else {
                f
            }
        })
        .collect()
}

/// Whether a run was long enough to judge memory by (the growth checks in
/// `analyse` need it).
pub fn judged_memory(samples: usize, hours: f64) -> bool {
    hours >= 0.4 && samples >= 30
}

/// "Memory and cache use steady", when the run was long enough to say so and
/// nothing about memory, handles or threads was found or is being tracked.
pub fn steady_line(findings: &[Finding], samples: usize, hours: f64) -> Option<&'static str> {
    let memory = |f: &Finding| {
        let t = f.text.to_lowercase();
        t.contains("memory") || t.contains("open files") || t.contains("threads")
    };
    (judged_memory(samples, hours) && !findings.iter().any(memory))
        .then_some("Memory and cache use steady during this test.")
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
            "NewQ's mixing of the sound took up to {}% of the time it has: above about 90% it breaks up. Fewer sound effects or sounds at once would help.",
            audio / 10
        )));
    } else if audio > 400 {
        out.push(note(format!(
            "NewQ's mixing of the sound took up to {}% of its time.",
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

// ----- Stalls and sound dropouts ----------------------------------------------------

/// A stall of the main loop (`crumbs::Stall`), as a test noted it.
#[derive(Clone, Debug, PartialEq)]
pub struct StallNote {
    /// Seconds into the test when it ended.
    pub t: f64,
    pub ms: u64,
    /// The part of the loop that took longest.
    pub part: String,
    /// Seconds past the whole minute by the wall clock when it began.
    pub past_minute: f64,
}

/// A stall this long is a Problem by itself: a quarter of a second of every
/// display standing still is plain to see.
pub const STALL_PROBLEM_MS: u64 = 250;
/// More stalls of 100 ms or more than this an hour is a Problem: 100 ms is 10
/// frames at 100 Hz, a hitch an audience can see in moving pictures, and more
/// than one every five minutes is several in any act.
pub const STALLS_PER_HOUR: f64 = 12.0;
/// More sound dropouts than this an hour is a Problem: each is a click or gap
/// that can be heard, and one every quarter of an hour is several a show.
pub const DROPOUTS_PER_HOUR: f64 = 4.0;
/// ...and so is this many within `BURST_SECS` of each other.
pub const DROPOUT_BURST: usize = 3;
pub const BURST_SECS: f64 = 10.0;

/// A rate over `hours`, a run shorter than a quarter of an hour counted as that
/// long (two in a five-minute test is not 24 an hour).
fn per_hour(n: usize, hours: f64) -> f64 {
    n as f64 / hours.max(0.25)
}

/// What a part of the main loop is called in a report.
fn part_words(part: &str) -> String {
    match part {
        "test mode" => "the test's own measuring".into(),
        "the show engine" => "the show engine".into(),
        "making the pictures (textures and uploads)" => "making the pictures".into(),
        other => other.into(),
    }
}

fn mmss(t: f64) -> String {
    super::session::clock(std::time::Duration::from_secs_f64(t.max(0.0)))
}

/// Whether a stall began within a quarter of a second after a whole minute.
fn on_the_minute(s: &StallNote) -> bool {
    s.past_minute < 0.25 || s.past_minute > 59.95
}

/// The finding for the stalls of a run (100 ms or more each), `hours` long.
pub fn analyse_stalls(stalls: &[StallNote], hours: f64) -> Vec<Finding> {
    if stalls.is_empty() {
        return Vec::new();
    }
    let worst = stalls.iter().map(|s| s.ms).max().unwrap_or(0);
    let n = stalls.len();
    let mut parts: Vec<(String, usize)> = Vec::new();
    for s in stalls {
        let w = part_words(&s.part);
        match parts.iter_mut().find(|(p, _)| *p == w) {
            Some((_, c)) => *c += 1,
            None => parts.push((w, 1)),
        }
    }
    parts.sort_by_key(|p| std::cmp::Reverse(p.1));
    let by_part = parts
        .iter()
        .map(|(p, c)| format!("{p} {c}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut worst_first: Vec<&StallNote> = stalls.iter().collect();
    worst_first.sort_by_key(|s| std::cmp::Reverse(s.ms));
    let listed = worst_first
        .iter()
        .take(8)
        .map(|s| format!("{} ({}, {} ms)", mmss(s.t), part_words(&s.part), s.ms))
        .collect::<Vec<_>>()
        .join(", ");
    let mut text = format!(
        "The main loop was held up {n} time{} for 100 ms or more (the worst {worst} ms); meanwhile the pictures stand still on every display and cues wait. By what it was doing: {by_part}. {}: {listed}.",
        if n == 1 { "" } else { "s" },
        if n > 8 { "The longest" } else { "When" },
    );
    let ours = parts
        .iter()
        .find(|(p, _)| p == "the test's own measuring")
        .map_or(0, |(_, c)| *c);
    if ours > 0 {
        text.push_str(&format!(
            " {ours} of them were the test's own measuring, not the show."
        ));
    }
    let minute = stalls.iter().filter(|s| on_the_minute(s)).count();
    // By chance a quarter-second window in each minute holds about 1 in 240.
    if minute >= 3 && minute * 2 >= n {
        text.push_str(&format!(
            " {minute} of them began within a quarter of a second after a whole minute by the clock: something done once a minute (by the system, another program or NewQ) is behind those, not what the show was playing."
        ));
    }
    let bad = worst >= STALL_PROBLEM_MS || (n >= 3 && per_hour(n, hours) > STALLS_PER_HOUR);
    vec![if bad { problem(text) } else { watch(text) }]
}

/// One line for the report about the stalls.
pub fn stall_summary(stalls: &[StallNote]) -> String {
    match stalls.iter().map(|s| s.ms).max() {
        None => "Main loop held up for 100 ms or more: never.".into(),
        Some(worst) => format!(
            "Main loop held up for 100 ms or more: {} time{}, the worst {worst} ms ({} began just after a whole minute).",
            stalls.len(),
            if stalls.len() == 1 { "" } else { "s" },
            stalls.iter().filter(|s| on_the_minute(s)).count()
        ),
    }
}

/// What the dropouts are put down to, as a sentence, when `late` of `n` came
/// with the system calling for sound late while NewQ's mixing was quick
/// (`audio::cause`).
pub fn dropout_cause(n: usize, late: usize) -> String {
    const LATE: &str = "the system called for sound late, not NewQ's mixing: another program or the sound device's power saving; a larger buffer may help";
    match late.min(n) {
        0 => "newq.log says, for each, whether NewQ's mixing was slow or the system called for sound late".into(),
        l if l == n => format!("Each time {LATE}"),
        l => format!("{l} of them: {LATE}; newq.log says what the others were"),
    }
}

/// The finding for the sound device's dropouts (stream errors), at `times`
/// seconds into a run `hours` long, `late` of them with the system calling for
/// sound late, on `device` (None: no working device).
pub fn analyse_dropouts(
    times: &[f64],
    late: usize,
    hours: f64,
    device: Option<&str>,
) -> Vec<Finding> {
    if times.is_empty() {
        return Vec::new();
    }
    let n = times.len();
    let burst = times
        .windows(DROPOUT_BURST)
        .any(|w| w[DROPOUT_BURST - 1] - w[0] <= BURST_SECS);
    let at = times
        .iter()
        .take(10)
        .map(|t| mmss(*t))
        .collect::<Vec<_>>()
        .join(", ");
    let text = format!(
        "The sound device{} reported {n} dropout{} (a buffer underrun: the sound broke up for a moment), at {at}{}{}. {}.",
        device.map_or(String::new(), |d| format!(" ({d})")),
        if n == 1 { "" } else { "s" },
        if n > 10 { " and later" } else { "" },
        if burst {
            format!(", {DROPOUT_BURST} or more within {BURST_SECS:.0} s")
        } else {
            String::new()
        },
        dropout_cause(n, late),
    );
    let bad = burst || per_hour(n, hours) > DROPOUTS_PER_HOUR;
    vec![if bad { problem(text) } else { watch(text) }]
}

/// One line for the report about the sound.
pub fn dropout_summary(times: &[f64], device: Option<&str>) -> String {
    match (times.len(), device) {
        (0, Some(d)) => format!("No audio dropouts on {d}."),
        (0, None) => "No working sound device, so dropouts were not measured.".into(),
        (n, d) => format!(
            "Audio dropouts: {n} on {}.",
            d.unwrap_or("the sound device")
        ),
    }
}

// ----- Timelines -------------------------------------------------------------------

/// A timeline's clock, run against the real time it should keep: more than this
/// apart over one stretch of running is a problem (a little over two frames at
/// 25 a second, where sound and picture are seen to part).
pub const DRIFT_PROBLEM: f64 = 0.1;
/// ...and more than this is tracked (a frame).
pub const DRIFT_WATCH: f64 = 0.04;
/// A clip whose first frame (or sound) came more than this after its place on
/// the clock was late enough to be seen as a late cue.
pub const LATE_PROBLEM: f64 = 0.5;
/// ...and more than this is tracked (five frames).
pub const LATE_WATCH: f64 = 0.2;
/// Getting a timeline's clips ready after a seek or start taking longer than
/// this is tracked; reaching the engine's limit (`CUE_LIMIT`) is a problem.
pub const CUEING_SLOW: f64 = 1.0;
/// Laps of a timeline may differ from their median by this much, or this part
/// of it if more: they are timed by its clock, and only getting clips ready
/// (up to two seconds a seek) and GO's moment move them.
pub const LAP_SPREAD_SECS: f64 = 2.0;
pub const LAP_SPREAD_PART: f64 = 0.02;

/// A stretch of a timeline's clock running on its own (no pause, seek, loop or
/// sleep in it).
#[derive(Clone, Debug, PartialEq)]
pub struct Stretch {
    /// Seconds into the test it began.
    pub at: f64,
    /// Its length in real time.
    pub secs: f64,
    /// The clock less the real time over it: + ahead, - behind.
    pub drift: f64,
}

/// A clip the clock reached while running, and when it came up.
#[derive(Clone, Debug, PartialEq)]
pub struct Arrival {
    pub at: f64,
    pub name: String,
    /// Seconds on the clock after its place that its first frame or sound came;
    /// None: it never came up.
    pub late: Option<f64>,
    /// What it was first seen waiting for (`ClipUp::waiting`), if it waited.
    pub waited: Option<&'static str>,
}

/// A time a timeline stood still getting its clips ready (`RunState::Cueing`).
#[derive(Clone, Debug, PartialEq)]
pub struct CueSpell {
    pub at: f64,
    pub lap: usize,
    pub secs: f64,
    /// It ran out of time (the engine's limit) and went on without them all.
    pub gave_up: bool,
}

/// What a timeline test noted about its timelines (see `testmode::timeline`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TimelineNotes {
    pub stretches: Vec<Stretch>,
    pub arrivals: Vec<Arrival>,
    pub cueing: Vec<CueSpell>,
}

fn secs_text(s: f64) -> String {
    // (An empty sum is -0.)
    let s = s + 0.0;
    if s.abs() < 1.0 {
        format!("{:.0} ms", s * 1000.0)
    } else {
        format!("{s:.1} s")
    }
}

/// What the timeline notes and the laps say, on top of `analyse`.
pub fn analyse_timeline(n: &TimelineNotes, laps: &[f64]) -> Vec<Finding> {
    let mut out = Vec::new();
    let clock = |t: f64| super::session::clock(std::time::Duration::from_secs_f64(t.max(0.0)));

    // 1. The clock against the real time.
    if let Some(worst) = n
        .stretches
        .iter()
        .max_by(|a, b| a.drift.abs().total_cmp(&b.drift.abs()))
    {
        let side = if worst.drift > 0.0 {
            "ahead of"
        } else {
            "behind"
        };
        let words = format!(
            "the timeline's clock ran {} {side} the real time over {} of running ({} into the test)",
            secs_text(worst.drift.abs()),
            secs_text(worst.secs),
            clock(worst.at)
        );
        if worst.drift.abs() > DRIFT_PROBLEM {
            out.push(problem(format!(
                "At worst {words}. Its clips start by that clock, so they come out of step with sound and anything else timed by the real clock."
            )));
        } else if worst.drift.abs() > DRIFT_WATCH {
            out.push(watch(format!(
                "Tracking: at worst {words}. Under {} it isn't seen.",
                secs_text(DRIFT_PROBLEM)
            )));
        }
    }

    // 2. Clips that came up late, or never.
    let never: Vec<&Arrival> = n.arrivals.iter().filter(|a| a.late.is_none()).collect();
    if let Some(first) = never.first() {
        // (The words stay the same as the count grows: the window's log
        // follows a finding by its words without their numbers.)
        out.push(problem(format!(
            "Clips the timeline reached that never came up: {} (the first: {}, {} into the test).",
            never.len(),
            first.name,
            clock(first.at)
        )));
    }
    let came: Vec<(&Arrival, f64)> = n
        .arrivals
        .iter()
        .filter_map(|a| a.late.map(|l| (a, l)))
        .collect();
    if let Some((worst, late)) = came.iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
        let over = |bound: f64| came.iter().filter(|(_, l)| *l > bound).count();
        let words = format!(
            "of the {} clips the timeline reached came up more than {} after their place on its clock (the latest: {}, {} late, {} into the test)",
            came.len(),
            secs_text(if *late > LATE_PROBLEM {
                LATE_PROBLEM
            } else {
                LATE_WATCH
            }),
            worst.name,
            secs_text(*late),
            clock(worst.at)
        );
        if *late > LATE_PROBLEM {
            out.push(problem(format!(
                "{} {words}: the audience sees a late cue.",
                over(LATE_PROBLEM)
            )));
        } else if *late > LATE_WATCH {
            out.push(watch(format!("Tracking: {} {words}.", over(LATE_WATCH))));
        }
    }

    // 3. Standing still to get clips ready.
    let gave_up: Vec<&CueSpell> = n.cueing.iter().filter(|c| c.gave_up).collect();
    if let Some(first) = gave_up.first() {
        out.push(problem(format!(
            "{} time{} the timeline's clips were not all ready within the time it gives them after a start or seek, and it ran on without them (the first in lap {}, {} into the test).",
            gave_up.len(),
            if gave_up.len() == 1 { "" } else { "s" },
            first.lap.max(1),
            clock(first.at)
        )));
    } else if let Some(slow) = n
        .cueing
        .iter()
        .filter(|c| c.secs > CUEING_SLOW)
        .max_by(|a, b| a.secs.total_cmp(&b.secs))
    {
        out.push(watch(format!(
            "Tracking: getting the timeline's clips ready after a start or seek took up to {} (lap {}); the clock stands meanwhile.",
            secs_text(slow.secs),
            slow.lap.max(1)
        )));
    }

    // 4. Laps the same length.
    if laps.len() >= 2 {
        let mut sorted = laps.to_vec();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let median = sorted[sorted.len() / 2];
        let bound = LAP_SPREAD_SECS.max(median * LAP_SPREAD_PART);
        let odd: Vec<(usize, f64)> = laps
            .iter()
            .enumerate()
            .filter(|(_, l)| (**l - median).abs() > bound)
            .map(|(i, l)| (i + 1, *l))
            .collect();
        if let Some(&(i, l)) = odd.first() {
            out.push(problem(format!(
                "{} of {} laps differed from the usual {median:.1} s by more than {bound:.1} s (the first: lap {i}, {l:.1} s). The timeline's clock times its laps, so they should match.",
                odd.len(),
                laps.len()
            )));
        }
    }
    out
}

/// The timeline part of the report: what was measured, found or not.
pub fn timeline_summary(n: &TimelineNotes) -> Vec<String> {
    let mut out = Vec::new();
    match n
        .stretches
        .iter()
        .max_by(|a, b| a.drift.abs().total_cmp(&b.drift.abs()))
    {
        Some(w) => out.push(format!(
            "Clock against real time: {} stretches of running, {} in all; at worst {:+.0} ms over {}.",
            n.stretches.len(),
            secs_text(n.stretches.iter().map(|s| s.secs).sum()),
            (w.drift * 1000.0).round() + 0.0,
            secs_text(w.secs)
        )),
        None => out.push("Clock against real time: no stretch of running was measured.".into()),
    }
    let came: Vec<f64> = n.arrivals.iter().filter_map(|a| a.late).collect();
    let never = n.arrivals.len() - came.len();
    if came.is_empty() && never == 0 {
        out.push("Clips on time: none was measured.".into());
    } else {
        let worst = came.iter().copied().fold(0.0, f64::max);
        let mean = came.iter().sum::<f64>() / came.len().max(1) as f64;
        out.push(format!(
            "Clips on time: {} came up, on average {:.0} ms after their place, at worst {:.0} ms; {never} never did.",
            came.len(),
            mean * 1000.0,
            worst * 1000.0
        ));
    }
    let total: f64 = n.cueing.iter().map(|c| c.secs).sum();
    let longest = n.cueing.iter().map(|c| c.secs).fold(0.0, f64::max);
    out.push(format!(
        "Getting clips ready: {} times, {} in all, the longest {}; {} ran out of time.",
        n.cueing.len(),
        secs_text(total),
        secs_text(longest),
        n.cueing.iter().filter(|c| c.gave_up).count()
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::Inputs;

    fn stretch(secs: f64, drift: f64) -> Stretch {
        Stretch {
            at: 10.0,
            secs,
            drift,
        }
    }

    fn arrival(name: &str, late: Option<f64>) -> Arrival {
        Arrival {
            at: 20.0,
            name: name.into(),
            late,
            waited: None,
        }
    }

    #[test]
    fn a_clock_that_keeps_time_passes_and_one_that_drifts_is_tracked_then_a_problem() {
        let mut n = TimelineNotes {
            stretches: vec![stretch(30.0, 0.008), stretch(12.0, -0.015)],
            ..Default::default()
        };
        assert!(analyse_timeline(&n, &[]).is_empty());
        n.stretches.push(stretch(40.0, -0.06));
        let f = analyse_timeline(&n, &[]);
        assert_eq!(f.len(), 1, "{f:?}");
        assert_eq!(f[0].severity, Severity::Watch);
        assert!(f[0].text.contains("60 ms behind"), "{}", f[0].text);
        assert_eq!(Verdict::of(&f), Verdict::Passed, "tracking doesn't count");
        n.stretches.push(stretch(50.0, 0.25));
        let f = analyse_timeline(&n, &[]);
        assert_eq!(f[0].severity, Severity::Problem);
        assert!(
            f[0].text.contains("250 ms ahead of") && f[0].text.contains("50.0 s"),
            "{}",
            f[0].text
        );
    }

    #[test]
    fn late_clips_are_tracked_then_a_problem_and_one_that_never_came_is_named() {
        let mut n = TimelineNotes {
            arrivals: vec![arrival("Sintel", Some(0.03)), arrival("Title", Some(0.0))],
            ..Default::default()
        };
        assert!(analyse_timeline(&n, &[]).is_empty());
        n.arrivals.push(arrival("Jellyfish", Some(0.3)));
        let f = analyse_timeline(&n, &[]);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Watch);
        assert!(
            f[0].text.contains("1 of the 3") && f[0].text.contains("Jellyfish, 300 ms late"),
            "{}",
            f[0].text
        );
        n.arrivals.push(arrival("Big Buck Bunny", Some(1.4)));
        n.arrivals.push(arrival("Blue Marble", None));
        let f = analyse_timeline(&n, &[]);
        assert!(f.iter().all(|x| x.severity == Severity::Problem), "{f:?}");
        assert!(
            f.iter()
                .any(|x| x.text.contains("never came up: 1") && x.text.contains("Blue Marble")),
            "{f:?}"
        );
        assert!(
            f.iter()
                .any(|x| x.text.contains("1 of the 4") && x.text.contains("1.4 s late")),
            "{f:?}"
        );
    }

    #[test]
    fn cueing_that_runs_out_of_time_is_a_problem_and_slow_cueing_is_tracked() {
        let spell = |secs: f64, gave_up: bool| CueSpell {
            at: 5.0,
            lap: 2,
            secs,
            gave_up,
        };
        let mut n = TimelineNotes {
            cueing: vec![spell(0.2, false), spell(0.4, false)],
            ..Default::default()
        };
        assert!(analyse_timeline(&n, &[]).is_empty());
        n.cueing.push(spell(1.6, false));
        let f = analyse_timeline(&n, &[]);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Watch);
        assert!(
            f[0].text.contains("1.6 s") && f[0].text.contains("lap 2"),
            "{}",
            f[0].text
        );
        n.cueing.push(spell(2.0, true));
        let f = analyse_timeline(&n, &[]);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Problem);
        assert!(
            f[0].text.starts_with("1 time the timeline's clips"),
            "{}",
            f[0].text
        );
    }

    #[test]
    fn laps_of_a_timeline_should_match_and_one_that_does_not_is_a_problem() {
        let n = TimelineNotes::default();
        // One lap: nothing to compare.
        assert!(analyse_timeline(&n, &[150.0]).is_empty());
        // Within two seconds of each other: the same.
        assert!(analyse_timeline(&n, &[150.0, 151.2, 149.6, 150.4]).is_empty());
        // A long lap's bound is a fiftieth of it: 6 s at 300 s.
        assert!(analyse_timeline(&n, &[300.0, 305.0, 297.0]).is_empty());
        let f = analyse_timeline(&n, &[150.0, 150.3, 157.0, 150.1]);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Problem);
        assert!(
            f[0].text.contains("lap 3, 157.0 s") && f[0].text.contains("150.3 s"),
            "{}",
            f[0].text
        );
    }

    #[test]
    fn the_timeline_summary_says_what_was_measured() {
        let n = TimelineNotes {
            stretches: vec![stretch(30.0, 0.008), stretch(10.0, -0.02)],
            arrivals: vec![arrival("A", Some(0.02)), arrival("B", None)],
            cueing: vec![CueSpell {
                at: 1.0,
                lap: 1,
                secs: 0.5,
                gave_up: false,
            }],
        };
        let s = timeline_summary(&n).join("\n");
        assert!(s.contains("2 stretches") && s.contains("-20 ms"), "{s}");
        assert!(s.contains("1 came up") && s.contains("1 never"), "{s}");
        assert!(s.contains("1 times") && s.contains("0 ran out"), "{s}");
        assert!(timeline_summary(&TimelineNotes::default())[0].contains("no stretch"));
    }

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
    fn a_steady_run_passes_and_a_short_one_says_nothing_about_leaks() {
        let f = analyse(&steady(400), &[100.0; 20], 100.0, &[], 1.1);
        assert!(f.is_empty(), "{f:?}");
        assert_eq!(Verdict::of(&f), Verdict::Passed);
        // A short run has nothing to say about slow leaks, and says nothing.
        let f = analyse(&steady(8), &[50.0], 50.0, &[], 0.02);
        assert!(f.is_empty(), "{f:?}");
        assert_eq!(Verdict::of(&f), Verdict::Passed);
        assert_eq!(steady_line(&f, 8, 0.02), None);
        // A long one that found nothing says memory was steady...
        let f = analyse(&steady(400), &[100.0; 20], 100.0, &[], 1.1);
        assert!(steady_line(&f, 400, 1.1).is_some());
        // ...and not when memory is being tracked or has a problem.
        let leaking: Vec<Sample> = (0..400).map(|i| sample(i, 400 + i)).collect();
        let f = analyse(&leaking, &[], 0.0, &[], 1.1);
        assert_eq!(steady_line(&f, 400, 1.1), None);
    }

    #[test]
    fn a_graphics_card_falling_behind_is_only_a_note_in_the_heavy_test() {
        let mut s = steady(100);
        for x in &mut s {
            x.inputs.layers = 3;
            x.inputs.late_permille = 400;
        }
        let f = analyse(&s, &[], 0.0, &[], 1.0);
        assert!(f.iter().any(|f| f.severity == Severity::Problem));
        let heavy = for_load(f.clone(), true);
        assert!(
            !heavy.iter().any(|f| f.severity == Severity::Problem),
            "{heavy:?}"
        );
        assert!(heavy.iter().any(|f| f.text.contains("heavier test")));
        assert!(
            heavy
                .iter()
                .any(|f| f.text.contains("40% of frames were late"))
        );
        assert_eq!(for_load(f.clone(), false), f);
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

    fn stall(t: f64, ms: u64, part: &str, past_minute: f64) -> StallNote {
        StallNote {
            t,
            ms,
            part: part.into(),
            past_minute,
        }
    }

    #[test]
    fn a_few_short_stalls_are_tracked_with_their_parts_and_times() {
        let s = [
            stall(186.5, 149, "test mode", 59.97),
            stall(
                761.8,
                125,
                "making the pictures (textures and uploads)",
                35.27,
            ),
        ];
        let f = analyse_stalls(&s, 1.0);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::Watch);
        let t = &f[0].text;
        assert!(
            t.contains("held up 2 times") && t.contains("the worst 149 ms"),
            "{t}"
        );
        assert!(
            t.contains("the test's own measuring 1, making the pictures 1"),
            "{t}"
        );
        assert!(t.contains("3:06 (the test's own measuring, 149 ms)"), "{t}");
        assert!(t.contains("1 of them were the test's own measuring"), "{t}");
        assert!(!t.contains("whole minute"), "{t}");
        assert!(analyse_stalls(&[], 1.0).is_empty());
    }

    #[test]
    fn many_stalls_or_a_long_one_are_a_problem_and_the_minute_is_noticed() {
        // The Mac hour: 14 of 100-150 ms, nine just after a whole minute.
        let mut s: Vec<StallNote> = (0..9)
            .map(|i| {
                stall(
                    186.5 + 300.0 * i as f64,
                    110,
                    "making the pictures (textures and uploads)",
                    0.01,
                )
            })
            .collect();
        s.extend((0..5).map(|i| {
            stall(
                100.0 + 600.0 * i as f64,
                105,
                "drawing the control window",
                35.3,
            )
        }));
        let f = analyse_stalls(&s, 1.0);
        assert_eq!(f[0].severity, Severity::Problem, "14 an hour");
        assert!(
            f[0].text
                .contains("9 of them began within a quarter of a second after a whole minute"),
            "{}",
            f[0].text
        );
        // Twelve an hour is still only tracked...
        let f = analyse_stalls(&s[..12], 1.0);
        assert_eq!(f[0].severity, Severity::Watch);
        // ...but one of a quarter of a second is a problem.
        let f = analyse_stalls(&[stall(5.0, 260, "the show engine", 12.0)], 1.0);
        assert_eq!(f[0].severity, Severity::Problem);
        // A short run is judged as a quarter of an hour: 3 in 5 minutes is 12 an hour.
        let three: Vec<_> = (0..3)
            .map(|i| stall(i as f64 * 60.0, 120, "test mode", 20.0))
            .collect();
        assert_eq!(analyse_stalls(&three, 0.08)[0].severity, Severity::Watch);
        assert!(stall_summary(&s).contains("14 times, the worst 110 ms (9 began just after"));
        assert_eq!(
            stall_summary(&[]),
            "Main loop held up for 100 ms or more: never."
        );
    }

    #[test]
    fn dropouts_are_tracked_and_too_many_or_a_burst_are_a_problem() {
        let mac = [885.8, 917.2, 2250.7, 2492.9, 2714.4, 2854.4];
        let f = analyse_dropouts(&mac[..2], 0, 1.0, Some("MacBook Pro Speakers"));
        assert_eq!(f[0].severity, Severity::Watch);
        assert!(
            f[0].text
                .contains("(MacBook Pro Speakers) reported 2 dropouts"),
            "{}",
            f[0].text
        );
        assert!(f[0].text.contains("at 14:45, 15:17"), "{}", f[0].text);
        // Six in an hour is more than four.
        assert_eq!(
            analyse_dropouts(&mac, 0, 1.0, None)[0].severity,
            Severity::Problem
        );
        // Three within ten seconds, in a long run.
        let f = analyse_dropouts(&[100.0, 104.0, 109.0], 0, 8.0, None);
        assert_eq!(f[0].severity, Severity::Problem);
        assert!(f[0].text.contains("3 or more within 10 s"), "{}", f[0].text);
        assert!(analyse_dropouts(&[], 0, 1.0, None).is_empty());
        assert_eq!(
            dropout_summary(&[], Some("Speakers")),
            "No audio dropouts on Speakers."
        );
        assert_eq!(
            dropout_summary(&mac, Some("Speakers")),
            "Audio dropouts: 6 on Speakers."
        );
    }

    #[test]
    fn dropouts_with_the_system_calling_late_are_not_put_down_to_the_mixing() {
        let mac = [885.8, 917.2, 2250.7];
        // Every one with the system calling late (the 0.24.3 Mac's).
        let f = analyse_dropouts(&mac, 3, 1.0, Some("MacBook Pro Speakers"));
        let t = &f[0].text;
        assert!(
            t.contains("Each time the system called for sound late, not NewQ's mixing: another program or the sound device's power saving; a larger buffer may help."),
            "{t}"
        );
        assert!(!t.contains("newq.log says"), "{t}");
        // Some of them.
        let t = &analyse_dropouts(&mac, 2, 1.0, None)[0].text;
        assert!(
            t.contains("2 of them: the system called for sound late"),
            "{t}"
        );
        assert!(t.contains("newq.log says what the others were"), "{t}");
        // None known: the log tells each.
        let t = &analyse_dropouts(&mac, 0, 1.0, None)[0].text;
        assert!(
            t.contains("newq.log says, for each, whether NewQ's mixing was slow"),
            "{t}"
        );
        assert!(!t.contains("Each time"), "{t}");
        // More late than dropouts (counted a moment apart) is all of them.
        assert_eq!(dropout_cause(1, 5), dropout_cause(1, 1));
    }

    #[test]
    fn the_mixing_load_finding_names_newqs_mixing() {
        let mut s = steady(100);
        s[50].inputs.audio_permille = 700;
        let f = analyse(&s, &[], 0.0, &[], 1.0);
        assert!(
            f.iter().any(|f| f.severity == Severity::Problem
                && f.text
                    .starts_with("NewQ's mixing of the sound took up to 70% of the time it has")),
            "{f:?}"
        );
    }
}
