//! The network test's judging and its files' text (pure; tested on made-up
//! numbers). What each computer sent (`net::measure::Report`s, gathered in a
//! `ComputerNotes`) becomes a verdict in plain words per computer, findings
//! for the report and the window, `network.csv` and the report's network part.

use super::analysis::{Finding, Severity, Verdict};
use crate::net::measure::{CommandNote, CorrectionNote, MonitorRate, Report, Row, StallNote};
use crate::net::proto::Link;
use std::fmt::Write as _;

/// Cues further apart than this many frames now and then: a note.
pub const APART_NOTE_FRAMES: f64 = 1.0;
/// Further apart than this: a problem (when it is more than a moment).
pub const APART_PROBLEM_FRAMES: f64 = 2.0;
/// Rows over `APART_PROBLEM_FRAMES` before it is a problem (half a second each).
pub const APART_PROBLEM_ROWS: usize = 3;
/// The shared clock's error bound over this (ms): a note.
pub const CLOCK_NOTE_MS: f64 = 2.0;
/// Over this (half a 60 Hz frame): a problem.
pub const CLOCK_PROBLEM_MS: f64 = 8.0;
/// A round trip over this (ms): a note (a show LAN pings in about 2 ms).
pub const RTT_NOTE_MS: f64 = 15.0;
/// A command that arrived with less than this to spare (ms): tight.
pub const TIGHT_LEAD_MS: f64 = 10.0;
/// More of the commands than this came late: a problem, not a note.
pub const LATE_PROBLEM_SHARE: f64 = 0.05;
/// And more than this many of them.
pub const LATE_PROBLEM_COUNT: usize = 2;
/// Corrections more often than this (a minute): a problem.
pub const CORRECTIONS_PROBLEM_PER_MIN: f64 = 1.0;
/// A stall of the main loop this long (ms): a problem; shorter ones are notes.
pub const STALL_PROBLEM_MS: u64 = 250;
/// Late frames over this, in thousandths: a note.
pub const LATE_FRAMES_NOTE: u32 = 20;
/// No report for this long (seconds): it dropped out.
pub const GAP_SECS: f64 = 3.0;
/// The clock may still be settling for this long after the test begins (s).
pub const SETTLING_SECS: f64 = 5.0;
/// A clock error bound of this or more (ms; `clock::REPORT_CAP_MS`) is no measure: the clock has lost track
/// of the primary's (0.24.2 sent 2,605,312 ms, still marked locked, just
/// before the lock went). Such a row counts as not locked.
pub const CLOCK_LOST_MS: f64 = 1000.0;
/// Not in step (or nothing compared while the primary played) for longer than
/// this at a stretch (s): a problem. A command makes a moment of it.
pub const NOT_IN_STEP_PROBLEM_SECS: f64 = 3.0;
/// Something played on the primary and not here for this long (s): a problem.
pub const MISSING_PROBLEM_SECS: f64 = 2.0;
/// A display drawing slowly (`proto::slow_below`) in this many reports (a
/// second each): a problem; fewer, a note.
pub const SLOW_DRAWING_PROBLEM_SECS: u32 = 3;
/// Sounds further apart than this (ms, by what each sound device was sent):
/// a note. (Each is kept within `audio::SOUND_SLACK` of the shared clock.)
pub const SOUND_NOTE_MS: f64 = 60.0;

/// How the network figures are made, said in every network report.
pub const HOW_MEASURED: &str = "How it is measured: at each of the primary's heartbeats (four a second) each computer compares every playing cue's time on its screens, by its own shared clock, with the primary's, as the heartbeat says it was when it was made, allowing half the shortest recent round trip for its way, and taking the least clock offset the heartbeats of the last 5 s showed: a heartbeat that waited on its way (behind other messages, or for a busy computer) shows the clock later, never earlier, so its wait isn't read as the screens apart (\"apart on the screens\": it holds the clock's own error). A difference counts only when two heartbeats in a row show it (a loop going round between the primary's heartbeat and the computer's look shows at one). It notes the largest twice a second. Frames are counted at the computer's slowest screen that drew: a difference shorter than one of its refreshes can't be seen there. \"Began\" compares when each cue began by the shared clock: a command carries the primary's time, so it can't see a wrong clock. Each command's lead is how long before its shared time it arrived. The clock error is the clock's own bound while locked, usually well above the real error; a bound of a second or more means the clock had lost track, and counts as not locked. network.csv has every row.";

/// What software can't see, said in every report.
pub const CANNOT_SEE: &str = "What this test can't see: each display draws at its own moment within its refresh, so two projectors without genlock can be up to one refresh apart even when NewQ chose the same frame on both (film the Sync flash to see it). Sound from two computers is not locked to the sample: it is meant for separate rooms or separate speakers, not one sound from two computers. The numbers come from the computers' own clocks; the network itself is measured by the round trips.";

/// A transfer of the test's media to one computer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Transfer {
    pub bytes: u64,
    pub secs: f64,
    /// It was all there at the first look: it came within `secs` of the
    /// primary's list of it, so the rate is at least this.
    pub within: bool,
}

impl Transfer {
    /// Bytes a second, if anything was transferred.
    pub fn rate(&self) -> Option<f64> {
        (self.bytes > 0 && self.secs > 0.0).then(|| self.bytes as f64 / self.secs)
    }
}

/// Everything one computer sent during the test.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComputerNotes {
    pub name: String,
    pub link: Link,
    pub refresh_hz: Option<f32>,
    pub rows: Vec<Row>,
    pub commands: Vec<CommandNote>,
    pub corrections: Vec<CorrectionNote>,
    pub stalls: Vec<StallNote>,
    pub late_frames_permille: u32,
    pub reports: u32,
    /// Reports that never came (gaps in their numbers).
    pub lost_reports: u32,
    last_seq: u32,
    /// Silences: when (seconds into the test) and how long.
    pub gaps: Vec<(f64, f64)>,
    pub transfer: Option<Transfer>,
    /// Its log's end arrived.
    pub log_tail: bool,
    /// Its last report arrived.
    pub ended: bool,
    /// Its output windows' drawing, a monitor each.
    pub monitors: Vec<MonitorNotes>,
    /// It drew in software at some time during the test (from its status).
    pub software: Option<crate::adapters::SoftwareDrawing>,
}

/// How one of a computer's output windows drew over the test.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MonitorNotes {
    pub index: usize,
    pub name: String,
    pub refresh_hz: Option<f32>,
    /// Reports (seconds) in which it drew, and the rates of those in which it
    /// drew slowly.
    pub drawing: u32,
    pub slow: Vec<f32>,
    pub slowest: Option<f32>,
    pub late_permille: u32,
    /// Run by Windows' basic display driver at some time in the test.
    pub basic_driver: bool,
}

impl MonitorNotes {
    pub fn take(&mut self, r: &MonitorRate) {
        self.name.clone_from(&r.name);
        self.refresh_hz = r.refresh_hz.or(self.refresh_hz);
        self.drawing += 1;
        let slow_below = crate::net::proto::slow_below(self.refresh_hz);
        if r.fps < slow_below && self.slow.len() < 100_000 {
            self.slow.push(r.fps);
        }
        self.slowest = Some(self.slowest.map_or(r.fps, |s| s.min(r.fps)));
        self.late_permille = self.late_permille.max(r.late_permille);
        self.basic_driver |= r.basic_driver;
    }

    /// The rate it should draw at: its display's refresh (60 if unknown).
    pub fn should(&self) -> f32 {
        self.refresh_hz
            .filter(|r| r.is_finite() && *r >= 20.0)
            .unwrap_or(60.0)
    }

    /// The middle of the slow rates (what it mostly drew at while slow).
    fn typical_slow(&self) -> Option<f32> {
        let mut v = self.slow.clone();
        v.sort_by(f32::total_cmp);
        v.get(v.len() / 2).copied()
    }
}

impl ComputerNotes {
    pub fn new(name: &str) -> ComputerNotes {
        ComputerNotes {
            name: name.into(),
            ..Default::default()
        }
    }

    /// Take a report in.
    pub fn take(&mut self, r: Report) {
        if self.reports > 0 && r.seq > self.last_seq + 1 {
            self.lost_reports += r.seq - self.last_seq - 1;
        }
        self.last_seq = self.last_seq.max(r.seq);
        self.reports += 1;
        self.link = r.link;
        if r.refresh_hz.is_some() {
            self.refresh_hz = r.refresh_hz;
        }
        self.rows.extend(r.rows);
        self.commands.extend(r.commands);
        self.corrections.extend(r.corrections);
        self.stalls.extend(r.stalls);
        if let Some(l) = r.late_frames_permille {
            self.late_frames_permille = self.late_frames_permille.max(l);
        }
        note_drawing(&mut self.monitors, &r.monitors);
        self.ended |= r.last;
    }

    /// The refresh frames are counted at (Hz), and whether it is a screen's
    /// that drew: the slowest of its screens that drew for the test, else the
    /// refresh it reported, else 60. A difference shorter than one refresh of
    /// the slowest screen can't be seen there; counted at the fastest (165 Hz
    /// in 0.24.2's test), 24 ms read as 4 frames on a 60 Hz projector.
    pub fn frame_hz(&self) -> (f64, bool) {
        let ok = |r: &f32| r.is_finite() && *r >= 20.0;
        let slowest = self
            .monitors
            .iter()
            .filter(|m| m.drawing > 0)
            .filter_map(|m| m.refresh_hz.filter(ok))
            .reduce(f32::min);
        match (slowest, self.refresh_hz.filter(ok)) {
            (Some(r), _) => (f64::from(r), true),
            (None, Some(r)) => (f64::from(r), false),
            (None, None) => (60.0, false),
        }
    }

    /// Milliseconds of a frame at `frame_hz`.
    pub fn frame_ms(&self) -> f64 {
        1000.0 / self.frame_hz().0
    }

    /// "60 Hz, its slowest screen" (what frames are counted at).
    pub fn frame_words(&self) -> String {
        match self.frame_hz() {
            (r, true) => format!("{r:.0} Hz, its slowest screen"),
            (r, false) if self.refresh_hz.is_some() => format!("{r:.0} Hz"),
            _ => "60 Hz (its screens' refresh unknown)".into(),
        }
    }

    /// Whether what the screens show was measured (0.24.3 on): the rows'
    /// `shown_apart_ms`, which hold the clock's error; else when the cues
    /// began (`apart_ms`), which can't see it.
    pub fn shown_measured(&self) -> bool {
        self.rows.iter().any(|r| r.shown_apart_ms.is_some())
    }

    /// A row's "apart": what the screens show where measured, else when the
    /// cues began.
    pub fn apart_of(&self, r: &Row) -> Option<f64> {
        if self.shown_measured() {
            r.shown_apart_ms
        } else {
            r.apart_ms
        }
    }

    fn worst_apart(&self) -> Option<f64> {
        self.rows
            .iter()
            .filter_map(|r| self.apart_of(r))
            .reduce(f64::max)
    }

    fn worst_began(&self) -> Option<f64> {
        self.rows.iter().filter_map(|r| r.apart_ms).reduce(f64::max)
    }

    /// A row in which the clock kept to the primary's: locked, with a bound
    /// that is a measure (`CLOCK_LOST_MS`).
    pub fn clock_kept(r: &Row) -> bool {
        r.locked
            && r.clock_error_ms
                .is_none_or(|e| e.is_finite() && e < CLOCK_LOST_MS)
    }

    fn worst_clock(&self) -> Option<f64> {
        self.rows
            .iter()
            .filter(|r| Self::clock_kept(r) && r.t >= SETTLING_SECS)
            .filter_map(|r| r.clock_error_ms)
            .reduce(f64::max)
    }

    fn worst_sound(&self) -> Option<f64> {
        self.rows
            .iter()
            .filter_map(|r| r.sound_apart_ms)
            .reduce(f64::max)
    }

    fn worst_rtt(&self) -> Option<f64> {
        self.rows.iter().filter_map(|r| r.rtt_ms).reduce(f64::max)
    }
}

/// One computer's verdict.
#[derive(Clone, Debug, PartialEq)]
pub struct ComputerVerdict {
    pub name: String,
    pub verdict: Verdict,
    /// One line: "In step: worst 0.4 frames apart over 2 min".
    pub headline: String,
    /// The numbers, a line each.
    pub lines: Vec<String>,
    pub findings: Vec<Finding>,
    /// Where its own log is in the zip (None: it did not come back).
    pub log: Option<String>,
}

/// Everything judged.
#[derive(Clone, Debug, PartialEq)]
pub struct Judged {
    pub verdict: Verdict,
    pub headline: String,
    pub computers: Vec<ComputerVerdict>,
    /// Every computer's findings, named, with the test's own.
    pub findings: Vec<Finding>,
}

fn finding(severity: Severity, text: String) -> Finding {
    Finding { severity, text }
}

/// The most rows in a row for which `f` holds.
fn longest_run(rows: &[Row], f: impl Fn(&Row) -> bool) -> usize {
    let (mut most, mut now) = (0, 0);
    for r in rows {
        now = if f(r) { now + 1 } else { 0 };
        most = most.max(now);
    }
    most
}

/// "2 min", "45 s", "1 h 05 min".
pub fn span(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s < 90 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min", (s + 30) / 60)
    } else {
        format!("{} h {:02} min", s / 3600, s / 60 % 60)
    }
}

/// "1:05" into the test.
pub fn at(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn megabytes(b: u64) -> String {
    format!("{:.0} MB", b as f64 / 1_000_000.0)
}

/// The words for a transfer: "42 MB in 6 s (7.0 MB/s)".
pub fn transfer_words(t: &Transfer) -> String {
    let secs = if t.secs < 10.0 {
        format!("{:.1} s", t.secs)
    } else {
        span(t.secs)
    };
    match (t.rate(), t.within) {
        (Some(r), false) => format!(
            "{} in {secs} ({:.1} MB/s)",
            megabytes(t.bytes),
            r / 1_000_000.0
        ),
        (Some(r), true) => format!(
            "{} here within {secs} (at least {:.1} MB/s)",
            megabytes(t.bytes),
            r / 1_000_000.0
        ),
        (None, _) => "nothing had to be sent".into(),
    }
}

/// A computer's output windows, judged by how fast they drew (R21): a line
/// each, and a problem for one that drew under half its display's rate for
/// `SLOW_DRAWING_PROBLEM_SECS` or more (a note for less). `who` names the
/// computer ("Assistant", "This computer (the primary)").
pub fn drawing_findings(who: &str, monitors: &[MonitorNotes]) -> (Vec<String>, Vec<Finding>) {
    let (mut lines, mut f) = (Vec::new(), Vec::new());
    for m in monitors {
        let what = format!("{who}'s monitor {} ({})", m.index + 1, m.name);
        let should = m.should();
        if m.basic_driver {
            f.push(finding(
                Severity::Problem,
                crate::adapters::basic_driver_report(who, m.index, &m.name),
            ));
        }
        if let Some(s) = m.slowest {
            lines.push(format!(
                "Monitor {} ({}): drew at least {s:.0} frames a second (its display {})",
                m.index + 1,
                m.name,
                m.refresh_hz
                    .map_or("refresh unknown".into(), |r| format!("runs at {r:.0} Hz"))
            ));
        }
        let Some(typical) = m.typical_slow() else {
            continue;
        };
        let slow = m.slow.len() as u32;
        if slow >= SLOW_DRAWING_PROBLEM_SECS {
            f.push(finding(
                Severity::Problem,
                format!(
                    "{what} drew about {typical:.0} frames a second for about {}: it should draw {should:.0}. Its pictures stuttered, whatever the timing says",
                    span(f64::from(slow))
                ),
            ));
        } else {
            f.push(finding(
                Severity::Note,
                format!("{what} drew only {typical:.0} frames a second for a moment"),
            ));
        }
    }
    (lines, f)
}

/// A computer that drew in software during the test (`who` names it): a
/// problem, whatever else its drawing says.
pub fn software_finding(who: &str, s: Option<&crate::adapters::SoftwareDrawing>) -> Vec<Finding> {
    s.map(|s| finding(Severity::Problem, s.report_sentence(who)))
        .into_iter()
        .collect()
}

/// Keep the first sight of drawing in software (`now`) in `kept`.
pub fn note_software(
    kept: &mut Option<crate::adapters::SoftwareDrawing>,
    now: Option<crate::adapters::SoftwareDrawing>,
) {
    if kept.is_none() {
        *kept = now;
    }
}

/// Add what one look at a computer's drawing says to `notes`.
pub fn note_drawing(notes: &mut Vec<MonitorNotes>, rates: &[MonitorRate]) {
    for m in rates {
        if !notes.iter().any(|n| n.index == m.index) {
            notes.push(MonitorNotes {
                index: m.index,
                ..Default::default()
            });
            notes.sort_by_key(|n| n.index);
        }
        if let Some(n) = notes.iter_mut().find(|n| n.index == m.index) {
            n.take(m);
        }
    }
}

/// Judge one computer over a test `secs` long.
pub fn judge_computer(c: &ComputerNotes, secs: f64) -> ComputerVerdict {
    let mut f: Vec<Finding> = Vec::new();
    let mut lines = Vec::new();
    let frame = c.frame_ms();
    let wifi = c.link == Link::WiFi;
    let link = match c.link {
        Link::Wired => "wired",
        Link::WiFi => "on Wi-Fi",
        Link::Unknown => "link unknown",
    };
    lines.push(format!("{link}; frames counted at {}", c.frame_words()));
    if let Some(t) = &c.transfer {
        lines.push(format!("Media: {}", transfer_words(t)));
    }
    if c.reports == 0 {
        let text = format!(
            "No measurements came from {}: is it running this version of NewQ?",
            c.name
        );
        f.push(finding(Severity::Problem, text.clone()));
        return ComputerVerdict {
            name: c.name.clone(),
            verdict: Verdict::ProblemsFound,
            headline: text,
            lines,
            findings: f,
            log: None,
        };
    }
    // How far apart: what the screens show (by this computer's clock against
    // the primary's heartbeats), where measured.
    let shown = c.shown_measured();
    let worst = c.worst_apart();
    let worst_frames = worst.map(|w| w / frame);
    if let (Some(w), Some(wf)) = (worst, worst_frames) {
        lines.push(format!(
            "Worst {w:.1} ms apart {}({wf:.2} frames), over {} rows",
            if shown { "on the screens " } else { "" },
            c.rows.iter().filter(|r| c.apart_of(r).is_some()).count()
        ));
        if shown && let Some(b) = c.worst_began() {
            lines.push(format!(
                "Cues began within {b:.1} ms of the primary's (by the shared clock, which this doesn't check)"
            ));
        }
        let over = c
            .rows
            .iter()
            .filter(|r| {
                c.apart_of(r)
                    .is_some_and(|a| a / frame > APART_PROBLEM_FRAMES)
            })
            .count();
        if over >= APART_PROBLEM_ROWS {
            f.push(finding(
                Severity::Problem,
                format!(
                    "Out of step: {} was more than {APART_PROBLEM_FRAMES:.0} frames apart from the primary for about {}, worst {wf:.1} frames",
                    c.name,
                    span(over as f64 * crate::net::measure::ROW_EVERY)
                ),
            ));
        } else if wf > APART_NOTE_FRAMES {
            f.push(finding(
                Severity::Note,
                format!(
                    "{} was {wf:.1} frames apart from the primary for a moment",
                    c.name
                ),
            ));
        }
    } else {
        lines.push("Nothing played to compare".into());
    }
    // Its displays: a picture that keeps time but is drawn a few frames a
    // second is no show (R21). Judged whatever the times say.
    let (more, found) = drawing_findings(&c.name, &c.monitors);
    lines.extend(more);
    f.extend(software_finding(&c.name, c.software.as_ref()));
    f.extend(found);
    if let Some(w) = c.worst_sound() {
        lines.push(format!("Sound up to {w:.0} ms from the primary's"));
        if w > SOUND_NOTE_MS {
            f.push(finding(
                Severity::Note,
                format!(
                    "The sound on {} was up to {w:.0} ms from the primary's (by what each sound device was sent; a device adds its own fixed delay after that)",
                    c.name
                ),
            ));
        }
    }
    // The clock: its bound while it kept to the primary's; the rest is time
    // not locked.
    if let Some(e) = c.worst_clock() {
        lines.push(format!("Clock error up to {e:.1} ms while locked"));
        let why = if wifi {
            "Wi-Fi"
        } else {
            "a busy network or computer"
        };
        if e > CLOCK_PROBLEM_MS {
            f.push(finding(
                Severity::Problem,
                format!(
                    "Clock error up to {e:.0} ms on {}: {why}. Half a frame or more{}",
                    c.name,
                    if c.link == Link::Wired {
                        ""
                    } else {
                        "; use a wire"
                    }
                ),
            ));
        } else if e > CLOCK_NOTE_MS {
            f.push(finding(
                Severity::Note,
                format!("Clock error up to {e:.0} ms on {}: {why}", c.name),
            ));
        }
    }
    let unlocked = c
        .rows
        .iter()
        .filter(|r| r.t >= SETTLING_SECS && !ComputerNotes::clock_kept(r))
        .count();
    if unlocked > 0 {
        f.push(finding(
            Severity::Problem,
            format!(
                "{}'s clock was not locked to the primary's for about {}",
                c.name,
                span(unlocked as f64 * crate::net::measure::ROW_EVERY)
            ),
        ));
    }
    if let Some(r) = c.worst_rtt() {
        lines.push(format!("Round trip up to {r:.1} ms"));
        if r > RTT_NOTE_MS {
            f.push(finding(
                Severity::Note,
                format!(
                    "Round trips to {} up to {r:.0} ms (a show network pings in about 2 ms){}",
                    c.name,
                    if wifi { ": Wi-Fi" } else { "" }
                ),
            ));
        }
    }
    // Commands.
    let n = c.commands.len();
    let late: Vec<&CommandNote> = c.commands.iter().filter(|k| k.lead_ms < 0.0).collect();
    let tight = c
        .commands
        .iter()
        .filter(|k| k.lead_ms >= 0.0 && k.lead_ms < TIGHT_LEAD_MS)
        .count();
    if n > 0 {
        let least = c.commands.iter().map(|k| k.lead_ms).reduce(f64::min);
        lines.push(format!(
            "{n} commands, {} late, the least time to spare {:.0} ms",
            late.len(),
            least.unwrap_or(0.0)
        ));
    }
    if !late.is_empty() {
        let worst = late
            .iter()
            .map(|k| -k.lead_ms)
            .reduce(f64::max)
            .unwrap_or(0.0);
        let text = format!(
            "Late commands on {}: {} of {n} arrived after their time (up to {worst:.0} ms); they were put right, but a picture may have jumped. {}",
            c.name,
            late.len(),
            if c.link == Link::Wired {
                "A quieter network, or a less busy computer, would help"
            } else {
                "Use a wire, or a quieter network"
            }
        );
        let problem = late.len() > LATE_PROBLEM_COUNT
            || (late.len() as f64 / n.max(1) as f64) > LATE_PROBLEM_SHARE;
        f.push(finding(
            if problem {
                Severity::Problem
            } else {
                Severity::Note
            },
            text,
        ));
    } else if tight > 0 {
        f.push(finding(
            Severity::Note,
            format!(
                "{tight} of {n} commands reached {} with less than {TIGHT_LEAD_MS:.0} ms to spare",
                c.name
            ),
        ));
    }
    // Corrections.
    if !c.corrections.is_empty() {
        let k = c.corrections.len();
        let some: Vec<&str> = c
            .corrections
            .iter()
            .take(3)
            .map(|x| x.what.as_str())
            .collect();
        lines.push(format!("{k} corrections"));
        let per_min = k as f64 / (secs / 60.0).max(1.0);
        f.push(finding(
            if per_min > CORRECTIONS_PROBLEM_PER_MIN {
                Severity::Problem
            } else {
                Severity::Note
            },
            format!(
                "{} put itself right {k} time{} ({})",
                c.name,
                if k == 1 { "" } else { "s" },
                some.join("; ")
            ),
        ));
    }
    // Played less than the primary.
    let missing_secs =
        c.rows.iter().filter(|r| r.missing > 0).count() as f64 * crate::net::measure::ROW_EVERY;
    if missing_secs >= MISSING_PROBLEM_SECS {
        f.push(finding(
            Severity::Problem,
            format!(
                "{} played less than the primary for about {} (a file not there yet, or a camera it hasn't)",
                c.name,
                span(missing_secs)
            ),
        ));
    }
    // Not in step, or nothing could be compared while the primary played (a
    // lost command: nothing reads apart, as nothing is compared), for longer
    // than a moment. (Rows not locked are counted above.)
    let stuck = longest_run(&c.rows, |r| {
        r.t >= SETTLING_SECS
            && ComputerNotes::clock_kept(r)
            && (!r.in_step || (r.compared == 0 && r.uncompared > 0))
    }) as f64
        * crate::net::measure::ROW_EVERY;
    if stuck > NOT_IN_STEP_PROBLEM_SECS {
        f.push(finding(
            Severity::Problem,
            format!(
                "{} was not in step with the primary for about {} at a stretch, so what it played could not be compared with what the primary played",
                c.name,
                span(stuck)
            ),
        ));
    }
    // Its own computer.
    if let Some(s) = c.stalls.iter().max_by_key(|s| s.ms) {
        lines.push(format!(
            "{} stall{} of the main loop, the longest {} ms",
            c.stalls.len(),
            if c.stalls.len() == 1 { "" } else { "s" },
            s.ms
        ));
        f.push(finding(
            if s.ms >= STALL_PROBLEM_MS {
                Severity::Problem
            } else {
                Severity::Note
            },
            format!(
                "{} stalled for {} ms at {} ({})",
                c.name,
                s.ms,
                at(s.t),
                s.part
            ),
        ));
    }
    if c.late_frames_permille > LATE_FRAMES_NOTE {
        f.push(finding(
            Severity::Note,
            format!(
                "{} drew {:.1} % of its frames late",
                c.name,
                c.late_frames_permille as f64 / 10.0
            ),
        ));
    }
    for (t, len) in &c.gaps {
        f.push(finding(
            Severity::Problem,
            format!(
                "{} dropped out of the network for {} at {}",
                c.name,
                span(*len),
                at(*t)
            ),
        ));
    }
    if c.lost_reports > 0 {
        lines.push(format!("{} reports lost", c.lost_reports));
    }
    let verdict = Verdict::of(&f);
    let headline = match (verdict, worst_frames) {
        (Verdict::ProblemsFound, _) => f
            .iter()
            .find(|x| x.severity == Severity::Problem)
            .map(|x| x.text.clone())
            .unwrap_or_default(),
        (_, Some(wf)) => format!("In step: worst {wf:.1} frames apart over {}", span(secs)),
        (_, None) => format!("In touch for {}, but nothing played to compare", span(secs)),
    };
    ComputerVerdict {
        name: c.name.clone(),
        verdict,
        headline,
        lines,
        findings: f,
        log: None,
    }
}

/// Judge every computer; `own` are the test's own findings (the primary's
/// stalls, media that never arrived).
pub fn judge(computers: &[ComputerNotes], secs: f64, own: Vec<Finding>) -> Judged {
    let per: Vec<ComputerVerdict> = computers.iter().map(|c| judge_computer(c, secs)).collect();
    let mut findings = own;
    for c in &per {
        findings.extend(c.findings.iter().cloned());
    }
    let verdict = Verdict::of(&findings);
    let bad = per
        .iter()
        .filter(|c| c.verdict == Verdict::ProblemsFound)
        .count();
    let n = per.len();
    let headline = if n == 0 {
        "No computer took part.".to_string()
    } else if bad == 0 && verdict != Verdict::ProblemsFound {
        format!(
            "{} kept in step with this one.",
            if n == 1 {
                "The other computer".to_string()
            } else {
                format!("All {n} computers")
            }
        )
    } else if bad == 0 {
        "The computers kept in step, but see the problems below.".into()
    } else {
        format!(
            "{bad} of {n} computer{} had problems.",
            if n == 1 { "" } else { "s" }
        )
    };
    Judged {
        verdict,
        headline,
        computers: per,
        findings,
    }
}

/// `network.csv`: a row per look of each computer.
pub fn csv(computers: &[ComputerNotes]) -> String {
    let mut s = String::from(
        "computer,t_s,clock_error_ms,rtt_ms,locked,in_step,shown_apart_ms,shown_apart_frames,began_apart_ms,began_apart_frames,compared,missing,sound_apart_ms,uncompared\n",
    );
    let num = |v: Option<f64>| v.map_or(String::new(), |x| format!("{x:.2}"));
    for c in computers {
        let frame = c.frame_ms();
        for r in &c.rows {
            // A bound past `CLOCK_LOST_MS` is no measure: left empty, and the
            // row not locked.
            let kept = ComputerNotes::clock_kept(r);
            let _ = writeln!(
                s,
                "{},{:.2},{},{},{},{},{},{},{},{},{},{},{},{}",
                csv_field(&c.name),
                r.t,
                num(r.clock_error_ms.filter(|e| *e < CLOCK_LOST_MS)),
                num(r.rtt_ms),
                u8::from(kept),
                u8::from(r.in_step),
                num(r.shown_apart_ms),
                num(r.shown_apart_ms.map(|a| a / frame)),
                num(r.apart_ms),
                num(r.apart_ms.map(|a| a / frame)),
                r.compared,
                r.missing,
                num(r.sound_apart_ms),
                r.uncompared
            );
        }
    }
    s
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// The report's network part.
pub fn report(j: &Judged, computers: &[ComputerNotes], secs: f64, about: &str) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "NewQ network test");
    let _ = writeln!(s, "=================");
    let _ = writeln!(s, "{about}");
    let _ = writeln!(s, "Ran for {}.", span(secs));
    let _ = writeln!(s);
    let _ = writeln!(s, "Verdict: {}. {}", j.verdict.label(), j.headline);
    let _ = writeln!(s);
    for (c, notes) in j.computers.iter().zip(computers) {
        let _ = writeln!(s, "{} — {}", c.name, c.verdict.label());
        // With problems the headline is the first of them, listed below.
        if c.verdict != Verdict::ProblemsFound {
            let _ = writeln!(s, "  {}", c.headline);
        }
        for l in &c.lines {
            let _ = writeln!(s, "  {l}");
        }
        if !notes.corrections.is_empty() {
            let _ = writeln!(s, "  Corrections:");
            for k in &notes.corrections {
                let _ = writeln!(s, "    {} {}", at(k.t), k.what);
            }
        }
        let late: Vec<&CommandNote> = notes.commands.iter().filter(|k| k.lead_ms < 0.0).collect();
        if !late.is_empty() {
            let _ = writeln!(s, "  Late commands:");
            for k in late {
                let _ = writeln!(
                    s,
                    "    {} command {} {:.1} ms late",
                    at(k.t),
                    k.seq,
                    -k.lead_ms
                );
            }
        }
        let _ = writeln!(s);
    }
    if !j.findings.is_empty() {
        let _ = writeln!(s, "Findings");
        for f in &j.findings {
            let word = match f.severity {
                Severity::Problem => "Problem",
                Severity::Note => "Note",
                Severity::Watch => "Tracked",
            };
            let _ = writeln!(s, "  {word}: {}", f.text);
        }
        let _ = writeln!(s);
    }
    let _ = writeln!(s, "{CANNOT_SEE}");
    let _ = writeln!(s);
    let _ = writeln!(s, "{HOW_MEASURED}");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(n: usize, apart: f64, clock: f64) -> Vec<Row> {
        (0..n)
            .map(|i| Row {
                t: i as f64 * 0.5,
                clock_error_ms: Some(clock),
                rtt_ms: Some(1.2),
                locked: true,
                in_step: true,
                apart_ms: Some(apart),
                compared: 2,
                missing: 0,
                sound_apart_ms: None,
                shown_apart_ms: None,
                uncompared: 0,
            })
            .collect()
    }

    #[test]
    fn a_computer_that_compared_nothing_while_the_primary_played_is_a_problem() {
        // 0.24.4's review (C5): after a lost command every heartbeat found a
        // command done here that the primary hadn't had yet (or the other
        // way), nothing was compared, nothing read apart: it passed.
        let mut c = good("Left");
        for r in &mut c.rows[100..200] {
            r.in_step = false;
            r.compared = 0;
            r.apart_ms = None;
            r.uncompared = 2;
        }
        let v = judge_computer(&c, 120.0);
        assert_eq!(v.verdict, Verdict::ProblemsFound, "{:?}", v.findings);
        assert!(
            v.findings.iter().any(|f| f
                .text
                .starts_with("Left was not in step with the primary for about 50 s at a stretch")),
            "{:?}",
            v.findings
        );
        // Compared nothing though it says it is in step (the follower's own
        // look agreed): the same.
        let mut c = good("Left");
        for r in &mut c.rows[100..120] {
            r.compared = 0;
            r.apart_ms = None;
            r.uncompared = 2;
        }
        assert_eq!(judge_computer(&c, 120.0).verdict, Verdict::ProblemsFound);
        // A moment out of step at each command, and nothing playing: fine.
        let mut c = good("Left");
        for i in (20..230).step_by(9) {
            c.rows[i].in_step = false;
            c.rows[i + 1].in_step = false;
        }
        for r in &mut c.rows[60..120] {
            r.compared = 0;
            r.apart_ms = None;
        }
        let v = judge_computer(&c, 120.0);
        assert!(
            !v.findings.iter().any(|f| f.text.contains("not in step")),
            "{:?}",
            v.findings
        );
    }

    fn commands(n: usize, late: usize) -> Vec<CommandNote> {
        (0..n)
            .map(|i| CommandNote {
                t: i as f64,
                seq: i as u64 + 1,
                lead_ms: if i < late { -12.0 } else { 45.0 },
            })
            .collect()
    }

    fn good(name: &str) -> ComputerNotes {
        ComputerNotes {
            name: name.into(),
            link: Link::Wired,
            refresh_hz: Some(60.0),
            rows: rows(240, 6.7, 0.3),
            commands: commands(40, 0),
            reports: 120,
            transfer: Some(Transfer {
                bytes: 42_000_000,
                secs: 12.0,
                within: false,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn a_computer_in_step_says_so_with_its_worst_in_frames() {
        let v = judge_computer(&good("Left"), 120.0);
        assert_eq!(v.verdict, Verdict::Passed, "{:?}", v.findings);
        assert_eq!(v.headline, "In step: worst 0.4 frames apart over 2 min");
        assert!(
            v.lines
                .contains(&"Media: 42 MB in 12 s (3.5 MB/s)".to_string()),
            "{:?}",
            v.lines
        );
        assert!(
            v.lines
                .iter()
                .any(|l| l == "40 commands, 0 late, the least time to spare 45 ms")
        );
        // At 50 Hz the same 6.7 ms is a third of a frame.
        let mut c = good("Left");
        c.refresh_hz = Some(50.0);
        assert!(judge_computer(&c, 120.0).headline.contains("0.3 frames"));
    }

    /// WNO-2 in the 0.24.3 run: in step by the clock, a picture every 10 s
    /// on Microsoft Basic Render Driver. The report names it as a problem.
    #[test]
    fn a_computer_that_drew_in_software_is_a_problem() {
        let mut c = good("WNO-2");
        let soft = crate::adapters::SoftwareDrawing {
            adapter: "Microsoft Basic Render Driver".into(),
            lost: true,
        };
        note_software(&mut c.software, None);
        assert_eq!(c.software, None);
        note_software(&mut c.software, Some(soft.clone()));
        // The first sight is kept; a later "back on the card" doesn't clear it.
        note_software(&mut c.software, None);
        assert_eq!(c.software.as_ref(), Some(&soft));
        let v = judge_computer(&c, 120.0);
        assert_eq!(v.verdict, Verdict::ProblemsFound, "{:?}", v.findings);
        assert!(
            v.findings.iter().any(|f| f.severity == Severity::Problem
                && f.text.starts_with(
                    "WNO-2 lost its graphics card and drew in software (Microsoft Basic Render Driver): its pictures were very slow"
                )),
            "{:?}",
            v.findings
        );
        assert!(software_finding("This computer", None).is_empty());
    }

    #[test]
    fn late_commands_are_a_note_when_few_and_a_problem_when_many() {
        let mut c = good("Left");
        c.commands = commands(40, 1);
        let v = judge_computer(&c, 120.0);
        assert_eq!(v.verdict, Verdict::PassedWithNotes);
        assert!(
            v.findings[0]
                .text
                .starts_with("Late commands on Left: 1 of 40 arrived after their time")
        );
        c.commands = commands(40, 3);
        let v = judge_computer(&c, 120.0);
        assert_eq!(v.verdict, Verdict::ProblemsFound);
        assert!(
            v.headline.contains("3 of 40 arrived after their time"),
            "{}",
            v.headline
        );
        assert!(v.headline.contains("A quieter network"));
    }

    #[test]
    fn clock_error_is_named_with_its_likely_cause() {
        let mut c = good("Prop");
        c.link = Link::WiFi;
        c.rows = rows(240, 6.7, 6.0);
        let v = judge_computer(&c, 120.0);
        assert_eq!(v.verdict, Verdict::PassedWithNotes);
        assert_eq!(v.findings[0].text, "Clock error up to 6 ms on Prop: Wi-Fi");
        c.rows = rows(240, 6.7, 12.0);
        let v = judge_computer(&c, 120.0);
        assert_eq!(v.verdict, Verdict::ProblemsFound);
        assert!(v.headline.contains("Half a frame or more; use a wire"));
        // While it is still settling, the first seconds don't count.
        let mut c = good("Left");
        c.rows[0].clock_error_ms = Some(50.0);
        c.rows[0].locked = false;
        assert_eq!(judge_computer(&c, 120.0).verdict, Verdict::Passed);
    }

    #[test]
    fn frames_apart_a_moment_is_a_note_and_for_longer_a_problem() {
        let mut c = good("Left");
        c.rows[100].apart_ms = Some(25.0);
        let v = judge_computer(&c, 120.0);
        assert_eq!(v.verdict, Verdict::PassedWithNotes);
        assert!(
            v.findings[0].text.contains("1.5 frames apart"),
            "{:?}",
            v.findings
        );
        for r in &mut c.rows[100..110] {
            r.apart_ms = Some(60.0);
        }
        let v = judge_computer(&c, 120.0);
        assert_eq!(v.verdict, Verdict::ProblemsFound);
        assert!(
            v.headline
                .starts_with("Out of step: Left was more than 2 frames apart")
        );
    }

    #[test]
    fn silence_corrections_stalls_and_no_reports_are_said() {
        let mut c = good("Left");
        c.gaps.push((65.0, 12.0));
        c.corrections.push(CorrectionNote {
            t: 70.0,
            what: "Film moved +40 ms".into(),
            ms: Some(40.0),
        });
        c.stalls.push(StallNote {
            t: 30.0,
            ms: 140,
            part: "the show engine".into(),
        });
        let v = judge_computer(&c, 120.0);
        let texts: Vec<&str> = v.findings.iter().map(|f| f.text.as_str()).collect();
        assert!(
            texts.contains(&"Left dropped out of the network for 12 s at 1:05"),
            "{texts:?}"
        );
        assert!(texts.contains(&"Left put itself right 1 time (Film moved +40 ms)"));
        assert!(texts.contains(&"Left stalled for 140 ms at 0:30 (the show engine)"));
        assert_eq!(v.verdict, Verdict::ProblemsFound);
        let v = judge_computer(&ComputerNotes::new("Right"), 120.0);
        assert_eq!(v.verdict, Verdict::ProblemsFound);
        assert!(v.headline.starts_with("No measurements came from Right"));
    }

    #[test]
    fn a_clock_bound_at_the_cap_is_a_lost_clock_not_a_second_of_error() {
        // 0.24.4's review (C7): a follower caps its bound at
        // `REPORT_CAP_MS` (1000 ms) when it has lost track, and 1000 was
        // taken as a measure: "Clock error up to 1000 ms".
        assert_eq!(crate::net::clock::REPORT_CAP_MS, CLOCK_LOST_MS);
        let mut c = good("Left");
        for r in &mut c.rows[100..140] {
            r.clock_error_ms = Some(crate::net::clock::REPORT_CAP_MS);
        }
        assert!(!c.rows[100..140].iter().any(ComputerNotes::clock_kept));
        let v = judge_computer(&c, 120.0);
        let texts: Vec<&str> = v.findings.iter().map(|f| f.text.as_str()).collect();
        assert!(
            texts.contains(&"Left's clock was not locked to the primary's for about 20 s"),
            "{texts:?}"
        );
        assert!(!v.lines.iter().any(|l| l.contains("1000")), "{:?}", v.lines);
        assert!(!csv(std::slice::from_ref(&c)).contains(",1000.00,"));
        // Just under it is still a measure (a bad one).
        c.rows[100].clock_error_ms = Some(999.0);
        assert!(ComputerNotes::clock_kept(&c.rows[100]));
    }

    #[test]
    fn a_clock_bound_that_is_no_measure_is_time_not_locked_and_wired_is_not_told_to_use_a_wire() {
        // 0.24.2: rows still marked locked carried 2,605,312 ms just before
        // the lock went; the report said "Clock error up to 2605312 ms ...
        // use a wire" on a wired computer.
        let mut c = good("Stage-PC");
        for r in &mut c.rows[100..140] {
            r.clock_error_ms = Some(2_605_312.28);
        }
        for r in &mut c.rows[140..160] {
            r.locked = false;
            r.clock_error_ms = Some(2_605_300.0);
        }
        let v = judge_computer(&c, 120.0);
        let texts: Vec<&str> = v.findings.iter().map(|f| f.text.as_str()).collect();
        assert!(
            texts.contains(&"Stage-PC's clock was not locked to the primary's for about 30 s"),
            "{texts:?}"
        );
        assert!(texts.iter().all(|t| !t.contains("2605")), "{texts:?}");
        assert!(
            v.lines
                .contains(&"Clock error up to 0.3 ms while locked".to_string()),
            "{:?}",
            v.lines
        );
        let text = csv(std::slice::from_ref(&c));
        assert!(!text.contains("2605"));
        assert!(
            text.lines().nth(101).unwrap().contains(",1.20,0,1,"),
            "{text}"
        );
        // A bound of half a frame on a wire: no "use a wire"; on Wi-Fi, yes.
        let mut c = good("Left");
        c.rows = rows(240, 6.7, 12.0);
        let v = judge_computer(&c, 120.0);
        assert_eq!(
            v.headline,
            "Clock error up to 12 ms on Left: a busy network or computer. Half a frame or more"
        );
        c.commands = commands(40, 5);
        let v = judge_computer(&c, 120.0);
        assert!(
            v.findings
                .iter()
                .all(|f| !f.text.contains("use a wire") && !f.text.contains("Use a wire")),
            "{:?}",
            v.findings
        );
        c.link = Link::WiFi;
        let v = judge_computer(&c, 120.0);
        assert!(v.findings.iter().any(|f| f.text.ends_with("; use a wire")));
        assert!(
            v.findings
                .iter()
                .any(|f| f.text.ends_with("Use a wire, or a quieter network"))
        );
    }

    #[test]
    fn frames_are_counted_at_the_slowest_screen_that_drew() {
        // 0.24.2: 24 ms read as 4 frames at a 165 Hz screen, beside a 60 Hz one.
        let mut c = good("Stage-PC");
        c.refresh_hz = Some(165.0);
        for r in &mut c.rows {
            r.apart_ms = Some(24.0);
        }
        let rate = |index, refresh| MonitorRate {
            index,
            name: format!("M{index}"),
            refresh_hz: Some(refresh),
            fps: refresh,
            late_permille: 0,
            basic_driver: false,
        };
        // Reported alone (no screen drew): at its reported refresh.
        let has = |v: &ComputerVerdict, t: &str| v.lines.iter().any(|l| l.contains(t));
        assert!(has(&judge_computer(&c, 120.0), "(3.96 frames)"));
        c.take(Report {
            seq: 1,
            link: Link::Wired,
            monitors: vec![rate(0, 165.0), rate(1, 60.0)],
            ..Default::default()
        });
        assert_eq!(c.frame_hz(), (60.0, true));
        let v = judge_computer(&c, 120.0);
        assert_eq!(
            v.lines[0],
            "wired; frames counted at 60 Hz, its slowest screen"
        );
        assert!(has(&v, "(1.44 frames)"), "{:?}", v.lines);
        assert_eq!(v.verdict, Verdict::PassedWithNotes, "{:?}", v.findings);
    }

    #[test]
    fn what_the_screens_show_is_judged_where_measured_and_began_kept_as_a_figure() {
        // 0.24.2's network part: "1.8 ms apart" by when cues began, while the
        // clock was wrong. Now the screens' difference is judged.
        let mut c = good("Stage-PC");
        for r in &mut c.rows {
            r.apart_ms = Some(1.8);
            r.shown_apart_ms = Some(42.0);
        }
        let v = judge_computer(&c, 120.0);
        assert_eq!(v.verdict, Verdict::ProblemsFound);
        assert!(
            v.headline
                .starts_with("Out of step: Stage-PC was more than 2 frames apart")
        );
        assert!(v.headline.ends_with("worst 2.5 frames"), "{}", v.headline);
        let has = |t: &str| v.lines.iter().any(|l| l.starts_with(t));
        assert!(
            has("Worst 42.0 ms apart on the screens (2.52 frames)"),
            "{:?}",
            v.lines
        );
        assert!(has("Cues began within 1.8 ms of the primary's"));
        // Without the screens' measure (a NewQ before 0.24.3): when they began.
        for r in &mut c.rows {
            r.shown_apart_ms = None;
        }
        assert_eq!(judge_computer(&c, 120.0).verdict, Verdict::Passed);
    }

    #[test]
    fn a_computer_with_problems_says_each_once_in_the_report() {
        let mut c = good("Stage-PC");
        for r in &mut c.rows {
            r.apart_ms = Some(60.0);
        }
        let cs = [c];
        let j = judge(&cs, 120.0, vec![]);
        let r = report(&j, &cs, 120.0, "Network test");
        assert_eq!(r.matches("Out of step: Stage-PC").count(), 1, "{r}");
    }

    #[test]
    fn the_overall_verdict_counts_the_computers_with_problems() {
        let j = judge(&[good("Left"), good("Right")], 120.0, vec![]);
        assert_eq!(j.verdict, Verdict::Passed);
        assert_eq!(j.headline, "All 2 computers kept in step with this one.");
        let j = judge(&[good("Left")], 120.0, vec![]);
        assert_eq!(j.headline, "The other computer kept in step with this one.");
        let j = judge(&[good("Left"), ComputerNotes::new("Right")], 120.0, vec![]);
        assert_eq!(j.verdict, Verdict::ProblemsFound);
        assert_eq!(j.headline, "1 of 2 computers had problems.");
        let own = vec![Finding {
            severity: Severity::Problem,
            text: "x".into(),
        }];
        let j = judge(&[good("Left")], 120.0, own);
        assert_eq!(
            j.headline,
            "The computers kept in step, but see the problems below."
        );
    }

    #[test]
    fn a_screen_on_windows_basic_display_driver_is_a_problem_that_says_what_to_do() {
        // 0.24.4's test: the laptop's panel was on Windows' basic display
        // driver (its AMD driver had failed) and drew about 4 frames a second.
        let mut c = good("WNO-2");
        let rate = |index, name: &str, fps, basic_driver| MonitorRate {
            index,
            name: name.into(),
            refresh_hz: Some(64.0),
            fps,
            late_permille: 0,
            basic_driver,
        };
        for i in 0..80 {
            c.take(Report {
                seq: i + 1,
                link: Link::Wired,
                monitors: vec![
                    rate(0, "B173HAN04.9", 4.0, true),
                    rate(1, "DISPLAY2", 60.0, false),
                ],
                ..Default::default()
            });
        }
        let v = judge_computer(&c, 120.0);
        assert_eq!(v.verdict, Verdict::ProblemsFound);
        assert_eq!(
            v.headline,
            "WNO-2's screen 1 (B173HAN04.9) was run by Windows' basic display driver: install the graphics driver for that computer. Its pictures were very slow, whatever the timing says"
        );
        // Its slow drawing is said too; the other screen says nothing.
        assert_eq!(v.findings.len(), 2, "{:?}", v.findings);
        // This computer's own screens, as Test Mode alone judges them.
        let mut own = Vec::new();
        note_drawing(&mut own, &[rate(0, "B173HAN04.9", 60.0, true)]);
        let (_, f) = drawing_findings("This computer", &own);
        assert_eq!(f.len(), 1);
        assert!(
            f[0].text.starts_with(
                "This computer's screen 1 (B173HAN04.9) was run by Windows' basic display driver"
            ),
            "{:?}",
            f[0].text
        );
    }

    #[test]
    fn a_display_drawing_four_frames_a_second_is_a_problem_in_plain_words() {
        // 0.24.1's first test on two computers: in step by the times, while
        // the display computer's built-in screen drew about 4 frames a second.
        let mut c = good("Assistant");
        let rate = |index, name: &str, refresh, fps| MonitorRate {
            index,
            name: name.into(),
            refresh_hz: Some(refresh),
            fps,
            late_permille: if fps < 30.0 { 1000 } else { 0 },
            basic_driver: false,
        };
        for i in 0..80 {
            c.take(Report {
                seq: i + 1,
                link: Link::Wired,
                monitors: vec![
                    rate(0, "B173HAN04.9", 64.0, 3.5 + (i % 3) as f32 * 0.3),
                    rate(1, "DISPLAY2", 60.0, 59.8),
                ],
                ..Default::default()
            });
        }
        let v = judge_computer(&c, 120.0);
        assert_eq!(v.verdict, Verdict::ProblemsFound);
        assert_eq!(
            v.headline,
            "Assistant's monitor 1 (B173HAN04.9) drew about 4 frames a second for about 80 s: it should draw 64. Its pictures stuttered, whatever the timing says"
        );
        assert!(
            v.lines.iter().any(|l| l
                == "Monitor 2 (DISPLAY2): drew at least 60 frames a second (its display runs at 60 Hz)"),
            "{:?}",
            v.lines
        );
        // The other monitor, drawing at its rate, says nothing more; a moment
        // of slowness is a note.
        assert_eq!(v.findings.len(), 1, "{:?}", v.findings);
        let mut c = good("Assistant");
        c.take(Report {
            seq: 1,
            monitors: vec![rate(0, "B173HAN04.9", 64.0, 12.0)],
            ..Default::default()
        });
        let v = judge_computer(&c, 120.0);
        assert_eq!(v.verdict, Verdict::PassedWithNotes);
        assert!(
            v.findings[0]
                .text
                .ends_with("drew only 12 frames a second for a moment")
        );
        // A 144 Hz display drawn at 60 (all a window is asked) is fine.
        let mut c = good("Fast");
        c.take(Report {
            seq: 1,
            monitors: vec![rate(0, "Gaming", 144.0, 59.0)],
            ..Default::default()
        });
        assert_eq!(judge_computer(&c, 120.0).verdict, Verdict::Passed);
    }

    #[test]
    fn sounds_far_apart_are_a_note() {
        let mut c = good("Left");
        c.rows[10].sound_apart_ms = Some(20.0);
        assert_eq!(judge_computer(&c, 120.0).verdict, Verdict::Passed);
        c.rows[11].sound_apart_ms = Some(118.0);
        let v = judge_computer(&c, 120.0);
        assert_eq!(v.verdict, Verdict::PassedWithNotes);
        assert!(
            v.findings[0]
                .text
                .starts_with("The sound on Left was up to 118 ms")
        );
    }

    #[test]
    fn reports_are_taken_in_and_a_lost_one_is_counted() {
        let mut c = ComputerNotes::new("Left");
        for seq in [1, 2, 4] {
            c.take(Report {
                seq,
                link: Link::Wired,
                refresh_hz: Some(59.94),
                rows: rows(2, 1.0, 0.1),
                late_frames_permille: Some(seq),
                last: seq == 4,
                ..Default::default()
            });
        }
        assert_eq!((c.reports, c.lost_reports, c.rows.len()), (3, 1, 6));
        assert!(c.ended && c.late_frames_permille == 4);
        assert!((c.frame_ms() - 16.683).abs() < 0.01);
    }

    #[test]
    fn the_csv_and_the_report_hold_every_row_and_say_what_cant_be_seen() {
        let mut a = good("Left, stage");
        a.rows.truncate(2);
        let mut b = good("Right");
        b.rows.truncate(1);
        b.rows[0].apart_ms = None;
        let text = csv(&[a.clone(), b.clone()]);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(
            lines[0],
            "computer,t_s,clock_error_ms,rtt_ms,locked,in_step,shown_apart_ms,shown_apart_frames,began_apart_ms,began_apart_frames,compared,missing,sound_apart_ms,uncompared"
        );
        assert_eq!(
            lines[1],
            "\"Left, stage\",0.00,0.30,1.20,1,1,,,6.70,0.40,2,0,,0"
        );
        assert_eq!(lines[3], "Right,0.00,0.30,1.20,1,1,,,,,2,0,,0");
        b.commands = commands(40, 1);
        let cs = [a, b];
        let j = judge(&cs, 120.0, vec![]);
        let r = report(&j, &cs, 120.0, "Network test, from Booth");
        assert!(r.contains("Verdict: Passed, with notes."), "{r}");
        assert!(r.contains("Right — Passed, with notes"));
        assert!(r.contains("0:00 command 1 12.0 ms late"));
        assert!(r.contains("genlock") && r.contains("not locked to the sample"));
    }
}
