//! The network test's measurements (stage 6 of `.claude/notes/network-plan.md`).
//!
//! While the primary runs Test Mode's network test (`testmode::network`) it
//! tells each computer `NetTest::Begin`; each follower then measures how it
//! keeps to the primary (`Measurer`) and sends what it saw once a second as a
//! `NetTest::Report`: its shared clock's error and round trip, how much lead
//! each command had left when it arrived (negative: it came after its time),
//! the corrections it made (`Engine::net_correct`), how far apart its cues
//! were from the primary's at each heartbeat (when they began, and what the
//! screens show: `Row::shown_apart_ms`), its main loop's stalls and late
//! frames. At `NetTest::End` it sends the last of them and its own log since
//! the test began (`LogMark`; `NetTest::Whole` names the test a part belongs
//! to, so a test of several parts gets one log). A NewQ without the network
//! test reads these as `Msg::Unknown`.
//!
//! Everything here is pure (times come in as arguments); the app feeds it
//! (`app/play.rs`) and the tests feed it from a simulated network.

use crate::net::play::PlayState;
use crate::net::proto::Link;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// What the primary and its computers say to each other about the test.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "nt", rename_all = "snake_case")]
pub enum NetTest {
    /// Primary to its computers: measure for test `run`, times counted from
    /// the shared time `start`.
    Begin { run: u64, start: f64 },
    /// Primary to its computers: the test is over; send the last report and
    /// the end of the log.
    End { run: u64 },
    /// A computer to its primary: what it measured since its last report.
    Report(Box<Report>),
    /// A computer to its primary: its own `newq.log` since the test began
    /// (`log_since`; at most `LOG_WHOLE`, the end kept).
    LogTail { run: u64, text: String },
    // ---- Every test across the network (`net::measure_local`) ----
    /// A computer to its primary: what it measured of itself (memory,
    /// processor, frame rates, dropouts) since its last such report.
    Local(Box<super::measure_local::LocalReport>),
    /// Primary to its computers: play the test's sound muted (or not).
    Mute { run: u64, muted: bool },
    /// Primary to its computers (after `Begin`): run `run` is a part of the
    /// test `test`, so the log sent at its end goes back to the start of that
    /// test's first part, not of this part alone (`LogMark`).
    Whole { run: u64, test: u64 },
}

/// What one computer measured over about a second.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Report {
    pub run: u64,
    /// Counts this computer's reports of the run (a gap is a lost one).
    pub seq: u32,
    pub link: Link,
    /// A refresh of its monitors (Hz): the slowest since 0.24.3, the fastest
    /// before. The primary counts frames at the slowest screen that drew
    /// (`monitors`), at this only when none did.
    pub refresh_hz: Option<f32>,
    pub rows: Vec<Row>,
    pub commands: Vec<CommandNote>,
    pub corrections: Vec<CorrectionNote>,
    pub stalls: Vec<StallNote>,
    /// Its outputs' late frames, in thousandths of the time (the worst output).
    pub late_frames_permille: Option<u32>,
    /// The last report of the run.
    pub last: bool,
    /// How fast its output windows drew since the last report, a monitor
    /// each (those drawing continuously). A NewQ before 0.24.2 sends none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub monitors: Vec<MonitorRate>,
}

/// How fast a computer's output window on one of its monitors drew.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorRate {
    /// Its place in the computer's monitors (0 = the first), and its name.
    pub index: usize,
    pub name: String,
    pub refresh_hz: Option<f32>,
    /// The slowest it drew (frames a second, over a second at a time).
    pub fps: f32,
    /// Its late frames, in thousandths (the most seen).
    pub late_permille: u32,
}

/// A look at one moment (every `ROW_EVERY` seconds).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Row {
    /// Seconds into the test, by the shared clock.
    pub t: f64,
    /// The shared clock's error bound (ms).
    pub clock_error_ms: Option<f64>,
    /// The longest recent round trip to the primary (ms).
    pub rtt_ms: Option<f64>,
    pub locked: bool,
    /// The last heartbeat agreed with what plays here.
    pub in_step: bool,
    /// The largest difference between when a cue (or a timeline's clock)
    /// began here and on the primary, by the shared clock, at the heartbeats
    /// since the last row (ms); None: nothing to compare. A cue a command
    /// started carries the primary's time, so this reads near 0 even when the
    /// clock here is wrong: `shown_apart_ms` is what the screens show.
    /// (`network.csv` calls it `began_apart_ms`.) A difference seen at one
    /// heartbeat alone isn't counted (a loop going round between the
    /// primary's heartbeat and the look here).
    pub apart_ms: Option<f64>,
    /// Cues (and timelines) compared, and those playing there but not here.
    pub compared: u32,
    pub missing: u32,
    /// The largest difference between a sound's place here and on the
    /// primary, by what each sound device has been sent (ms); None: no sound
    /// playing on both. Apart from `apart_ms`, which is the cues' (and so the
    /// pictures') time by the shared clock.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sound_apart_ms: Option<f64>,
    /// The largest difference between a cue's time on the screens here and on
    /// the primary's, at the heartbeats since the last row (ms): each cue's
    /// time now by this computer's shared clock, against the primary's time
    /// as its heartbeat says it was, less half a round trip for the
    /// heartbeat's way here (`seen_offset`). So it holds the clock's own
    /// error, which `apart_ms` can't see. None: nothing to compare, or a NewQ
    /// before 0.24.3.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shown_apart_ms: Option<f64>,
}

/// A command as it arrived.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CommandNote {
    pub t: f64,
    pub seq: u64,
    /// How long before its shared time it arrived (ms); negative: after it.
    pub lead_ms: f64,
}

/// A correction this computer made to keep in step.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CorrectionNote {
    pub t: f64,
    /// In the engine's words ("Film moved +40 ms", "Title started 1.20 s in").
    pub what: String,
    /// How much, where the words say (ms).
    pub ms: Option<f64>,
}

/// A stall of the main loop.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StallNote {
    pub t: f64,
    pub ms: u64,
    pub part: String,
}

/// A row is noted this often (seconds).
pub const ROW_EVERY: f64 = 0.5;
/// A report is sent this often (seconds).
pub const SEND_EVERY: f64 = 1.0;
/// The most of a computer's log sent at the end of a part (bytes): its log
/// since the test began, the end kept if longer. A display computer's log for
/// an hour is some tens of KB; this goes as one frame, after the part.
pub const LOG_WHOLE: usize = 4 * 1024 * 1024;
/// At most this many notes of each kind in one report (a burst is cut).
const MAX_NOTES: usize = 200;

/// What a computer sees of itself at a row.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub clock_error_ms: Option<f64>,
    pub rtt_ms: Option<f64>,
    pub locked: bool,
    pub in_step: bool,
}

/// A follower's measuring, between `begin` and `end`.
#[derive(Default)]
pub struct Measurer {
    /// The run and its start (shared time).
    run: Option<(u64, f64)>,
    pending: Report,
    apart: Option<f64>,
    sound_apart: Option<f64>,
    compared: u32,
    missing: u32,
    next_row: f64,
    next_send: f64,
    seq: u32,
    /// The engine's count of corrections when last looked at.
    corrections_seen: Option<u64>,
    /// Its output windows' drawing since the last report.
    rates: Vec<MonitorRate>,
    /// The last heartbeat compared (its shared time) and how far apart each
    /// key was then (began, shown; ms), so that a difference counts only when
    /// seen twice in a row.
    last_beat: Option<(f64, Beat)>,
    shown_apart: Option<f64>,
}

/// How far apart each key was at a heartbeat (began, shown; ms).
type Beat = HashMap<Key, (f64, Option<f64>)>;

/// What is compared at a heartbeat: the `n`th instance (oldest first) of a
/// cue, or a timeline's clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    Cue(crate::show::CueId, usize),
    Run(crate::timeline::TimelineKey),
}

/// Heartbeats come four a second: one further than this (s) after the last
/// compared is too late to confirm what that one saw.
const BEATS_IN_A_ROW: f64 = 1.0;

impl Measurer {
    /// Start measuring for `run` (again: the same run is not started twice).
    pub fn begin(&mut self, run: u64, start: f64) {
        if self.run.is_some_and(|(r, _)| r == run) {
            return;
        }
        *self = Measurer {
            run: Some((run, start)),
            ..Measurer::default()
        };
        log::info!("show network: the network test began; measuring");
    }

    /// The run being measured.
    pub fn running(&self) -> Option<u64> {
        self.run.map(|(r, _)| r)
    }

    fn at(&self, now: f64) -> f64 {
        self.run.map_or(0.0, |(_, s)| now - s)
    }

    /// A command arrived at shared time `now` (None: no locked clock yet), to
    /// be done at `at`.
    pub fn command(&mut self, now: Option<f64>, seq: u64, at: f64) {
        let (Some(_), Some(now)) = (self.run, now) else {
            return;
        };
        if self.pending.commands.len() < MAX_NOTES {
            self.pending.commands.push(CommandNote {
                t: self.at(now),
                seq,
                lead_ms: round2((at - now) * 1000.0),
            });
        }
    }

    /// The primary's heartbeat, and what plays here at the same shared time.
    /// `seen` is this computer's clock less the primary's, as this heartbeat
    /// shows it (`seen_offset`; None without a clock): with it, what the
    /// screens show is compared too. A difference counts only when the last
    /// heartbeat compared (within `BEATS_IN_A_ROW`) showed one for the same
    /// cue too, and then the smaller of the two: a loop going round between
    /// the primary's heartbeat and the look here, or a heartbeat that waited
    /// to be read, makes one at a single heartbeat.
    pub fn heartbeat(&mut self, theirs: &PlayState, ours: &PlayState, seen: Option<f64>) {
        if self.run.is_none() {
            return;
        }
        let (pairs, compared, missing) = pairs_between(theirs, ours);
        let now: Beat = pairs
            .into_iter()
            .map(|(k, d)| {
                (
                    k,
                    ((d * 1000.0).abs(), seen.map(|e| ((e - d) * 1000.0).abs())),
                )
            })
            .collect();
        let before = self
            .last_beat
            .take()
            .filter(|(t, _)| (theirs.time - t).abs() <= BEATS_IN_A_ROW)
            .map(|(_, m)| m)
            .unwrap_or_default();
        let most = |w: Option<f64>, a: f64| Some(w.map_or(a, |w: f64| w.max(a)));
        for (k, (began, shown)) in &now {
            let Some((b0, s0)) = before.get(k) else {
                continue;
            };
            self.apart = most(self.apart, began.min(*b0));
            if let (Some(s), Some(s0)) = (shown, s0) {
                self.shown_apart = most(self.shown_apart, s.min(*s0));
            }
        }
        self.last_beat = Some((theirs.time, now));
        self.compared = self.compared.max(compared);
        self.missing = self.missing.max(missing);
        if let Some(a) = sound_apart_between(theirs, ours) {
            self.sound_apart = Some(self.sound_apart.map_or(a, |w: f64| w.max(a)));
        }
    }

    /// The engine's corrections so far (`Follower::corrections`) and the words
    /// of the last ones: new ones are noted.
    pub fn corrections(&mut self, now: f64, total: u64, words: &[String]) {
        if self.run.is_none() {
            return;
        }
        let seen = *self.corrections_seen.get_or_insert(total);
        if total > seen {
            let new = (total - seen) as usize;
            let t = self.at(now);
            for w in words.iter().rev().take(new).rev() {
                if self.pending.corrections.len() < MAX_NOTES {
                    self.pending.corrections.push(CorrectionNote {
                        t,
                        what: w.clone(),
                        ms: correction_ms(w),
                    });
                }
            }
        }
        self.corrections_seen = Some(total);
    }

    /// A stall of this computer's main loop.
    pub fn stall(&mut self, now: f64, ms: u64, part: &str) {
        if self.run.is_some() && self.pending.stalls.len() < MAX_NOTES {
            self.pending.stalls.push(StallNote {
                t: round2(self.at(now)),
                ms,
                part: part.to_string(),
            });
        }
    }

    /// How fast its output windows are drawing now (those drawing): the
    /// slowest of each since the last report goes in it.
    pub fn drawing(&mut self, now: &[MonitorRate]) {
        if self.run.is_none() {
            return;
        }
        for r in now {
            match self.rates.iter_mut().find(|m| m.index == r.index) {
                Some(m) => {
                    m.fps = m.fps.min(r.fps);
                    m.late_permille = m.late_permille.max(r.late_permille);
                    m.refresh_hz = r.refresh_hz.or(m.refresh_hz);
                }
                None => self.rates.push(r.clone()),
            }
        }
    }

    /// Note a row if one is due.
    pub fn sample(&mut self, now: f64, s: Sample) {
        if self.run.is_none() || now < self.next_row {
            return;
        }
        self.next_row = now + ROW_EVERY;
        let row = Row {
            t: round2(self.at(now)),
            clock_error_ms: s.clock_error_ms.map(round2),
            rtt_ms: s.rtt_ms.map(round2),
            locked: s.locked,
            in_step: s.in_step,
            apart_ms: self.apart.take().map(round2),
            compared: std::mem::take(&mut self.compared),
            missing: std::mem::take(&mut self.missing),
            sound_apart_ms: self.sound_apart.take().map(round2),
            shown_apart_ms: self.shown_apart.take().map(round2),
        };
        if self.pending.rows.len() < MAX_NOTES {
            self.pending.rows.push(row);
        }
    }

    /// The report to send now, if one is due (or `last`).
    pub fn report(
        &mut self,
        now: f64,
        link: Link,
        refresh_hz: Option<f32>,
        late_frames_permille: Option<u32>,
        last: bool,
    ) -> Option<Report> {
        let (run, _) = self.run?;
        if !last && now < self.next_send {
            return None;
        }
        self.next_send = now + SEND_EVERY;
        self.seq += 1;
        let mut r = std::mem::take(&mut self.pending);
        r.run = run;
        r.seq = self.seq;
        r.link = link;
        r.refresh_hz = refresh_hz;
        r.late_frames_permille = late_frames_permille;
        r.last = last;
        r.monitors = std::mem::take(&mut self.rates);
        r.monitors.sort_by_key(|m| m.index);
        Some(r)
    }

    /// The test is over: the last report (if `run` is the one measured).
    pub fn end(
        &mut self,
        run: u64,
        now: f64,
        link: Link,
        refresh_hz: Option<f32>,
    ) -> Option<Report> {
        if self.running() != Some(run) {
            return None;
        }
        let r = self.report(now, link, refresh_hz, None, true);
        *self = Measurer::default();
        log::info!("show network: the network test ended");
        r
    }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// How far apart what the primary plays and what plays here are: the largest
/// difference of a cue's (or a running timeline's) zero time, in ms, how many
/// were compared, and how many play there and not here. Cues are matched by
/// cue, oldest first; one that is stopping is left out. (The tests'; the
/// measurer uses `pairs_between`.)
#[cfg(test)]
pub fn apart_between(theirs: &PlayState, ours: &PlayState) -> (Option<f64>, u32, u32) {
    let (pairs, compared, missing) = pairs_between(theirs, ours);
    let worst = pairs
        .iter()
        .map(|(_, d)| (d * 1000.0).abs())
        .reduce(f64::max);
    (worst, compared, missing)
}

/// This computer's shared clock less the primary's, as a heartbeat shows it:
/// made at the primary's shared time `theirs`, taken in here at `here` by
/// this computer's shared clock, after a way of about half the round trip
/// `rtt` (seconds). The heartbeat's own waits (to be sent, to be read here)
/// count as clock error, so it errs towards "apart".
pub fn seen_offset(here: f64, theirs: f64, rtt: Option<f64>) -> f64 {
    here - theirs - rtt.unwrap_or(0.0).max(0.0) / 2.0
}

/// What `apart_between` compares, each with its signed difference (s): when
/// it began here less when it began there (later here: positive). What the
/// screens show differs by `seen_offset` less that.
pub fn pairs_between(theirs: &PlayState, ours: &PlayState) -> (Vec<(Key, f64)>, u32, u32) {
    let mut here: HashMap<crate::show::CueId, Vec<f64>> = HashMap::new();
    for c in ours.cues.iter().filter(|c| !c.stopping) {
        here.entry(c.cue).or_default().push(c.anchor);
    }
    for v in here.values_mut() {
        v.sort_by(f64::total_cmp);
    }
    let mut there: HashMap<crate::show::CueId, Vec<f64>> = HashMap::new();
    for c in theirs.cues.iter().filter(|c| !c.stopping) {
        there.entry(c.cue).or_default().push(c.anchor);
    }
    let (mut pairs, mut compared, mut missing) = (Vec::new(), 0u32, 0u32);
    for (cue, mut list) in there {
        list.sort_by(f64::total_cmp);
        let mine = here.get(&cue);
        for (n, a) in list.iter().enumerate() {
            match mine.and_then(|m| m.get(n)) {
                Some(b) => {
                    compared += 1;
                    pairs.push((Key::Cue(cue, n), b - a));
                }
                None => missing += 1,
            }
        }
    }
    for r in &theirs.runs {
        let (Some(a), Some(b)) = (
            r.anchor,
            ours.runs
                .iter()
                .find(|o| o.key == r.key)
                .and_then(|o| o.anchor),
        ) else {
            continue;
        };
        compared += 1;
        pairs.push((Key::Run(r.key), b - a));
    }
    (pairs, compared, missing)
}

/// How far apart the sounds playing on both are, by what each computer's
/// sound device has been sent (ms): cues matched as `apart_between` matches
/// them, each with its sound's zero time (`CueNow::sound`). None when no
/// sound plays on both.
pub fn sound_apart_between(theirs: &PlayState, ours: &PlayState) -> Option<f64> {
    let by_cue = |s: &PlayState| {
        let mut m: HashMap<crate::show::CueId, Vec<(f64, Option<f64>)>> = HashMap::new();
        for c in s.cues.iter().filter(|c| !c.stopping) {
            m.entry(c.cue).or_default().push((c.anchor, c.sound));
        }
        for v in m.values_mut() {
            v.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        m
    };
    let (here, there) = (by_cue(ours), by_cue(theirs));
    let mut worst: Option<f64> = None;
    for (cue, list) in there {
        let Some(mine) = here.get(&cue) else { continue };
        for ((_, a), (_, b)) in list.iter().zip(mine) {
            if let (Some(a), Some(b)) = (a, b) {
                let d = ((b - a) * 1000.0).abs();
                worst = Some(worst.map_or(d, |w| w.max(d)));
            }
        }
    }
    worst
}

/// The milliseconds in a correction's words: "moved +40 ms" is 40, "started
/// 1.20 s in" 1200; None where they say no amount.
pub fn correction_ms(words: &str) -> Option<f64> {
    let parts: Vec<&str> = words.split_whitespace().collect();
    for w in parts.windows(2) {
        let n = w[0].trim_start_matches('+').parse::<f64>().ok();
        match (n, w[1]) {
            (Some(n), "ms") => return Some(n.abs()),
            (Some(n), "s") => return Some(n.abs() * 1000.0),
            _ => {}
        }
    }
    None
}

/// Where a computer's log for a test begins: the size of `newq.log` when the
/// test's first part began here, kept for the parts that follow (the
/// primary's `NetTest::Whole`). A test of one part: where it began.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LogMark {
    /// The run that began last, and the log's size then.
    began: Option<(u64, u64)>,
    /// The test of several parts, and the log's size when its first began.
    whole: Option<(u64, u64)>,
    /// The run that is a part of `whole`.
    part: Option<u64>,
}

impl LogMark {
    /// Run `run` began, with the log `len` bytes long.
    pub fn begin(&mut self, run: u64, len: u64) {
        if self.began.is_none_or(|(r, _)| r != run) {
            self.began = Some((run, len));
        }
    }

    /// Run `run` (which began) is a part of test `test`.
    pub fn whole(&mut self, run: u64, test: u64) {
        let Some((r, len)) = self.began.filter(|(r, _)| *r == run) else {
            return;
        };
        if self.whole.is_none_or(|(t, _)| t != test) {
            self.whole = Some((test, len));
        }
        self.part = Some(r);
    }

    /// Where the log for run `run` begins (bytes into `newq.log`).
    pub fn from(&self, run: u64) -> u64 {
        match (self.part, self.whole, self.began) {
            (Some(p), Some((_, len)), _) if p == run => len,
            (_, _, Some((r, len))) if r == run => len,
            _ => 0,
        }
    }
}

/// The first line of a log `log_since` cut at its start.
pub const LEFT_OUT: &str =
    "(The start of this computer's log for the test is left out: there was more than is sent.)";

/// `newq.log` in `dir` from `from` bytes on (after the end of
/// `newq.earlier.log` from there, if the log was put aside since), at most
/// `max` bytes, the end kept and then from the start of a line.
pub fn log_since(dir: &Path, from: u64, max: usize) -> String {
    let now = dir.join("newq.log");
    let len = std::fs::metadata(&now).map_or(0, |m| m.len());
    let (mut text, mut cut) = if len < from {
        // Put aside since: the earlier part from the mark, then all of this.
        let (a, cut) = read_from(&dir.join("newq.earlier.log"), from, max);
        let (b, cut_too) = read_from(&now, 0, max);
        (a + &b, cut || cut_too)
    } else {
        read_from(&now, from, max)
    };
    if text.len() > max {
        let mut at = text.len() - max;
        while !text.is_char_boundary(at) {
            at += 1;
        }
        text = text[at..].to_string();
        cut = true;
    }
    if cut {
        if let Some(i) = text.find('\n') {
            text = text[i + 1..].to_string();
        }
        text.insert_str(0, &format!("{LEFT_OUT}\n"));
    }
    text
}

/// The file at `path` from `from` bytes on, at most its last `max` bytes,
/// and whether that left some out.
fn read_from(path: &Path, from: u64, max: usize) -> (String, bool) {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return (String::new(), false);
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = from.max(len.saturating_sub(max as u64));
    if start >= len || f.seek(SeekFrom::Start(start)).is_err() {
        return (String::new(), false);
    }
    let mut buf = Vec::new();
    let _ = f.take(max as u64).read_to_end(&mut buf);
    (String::from_utf8_lossy(&buf).into_owned(), start > from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::play::{CueNow, RunNow, RunWord};
    use crate::net::proto::Msg;
    use crate::show::CueId;
    use crate::timeline::TimelineKey;

    fn state(cues: &[(u64, f64)], run: Option<f64>) -> PlayState {
        PlayState {
            cues: cues
                .iter()
                .map(|&(c, a)| CueNow {
                    cue: CueId(c),
                    anchor: a,
                    stopping: false,
                    sound: None,
                })
                .collect(),
            runs: run
                .map(|a| RunNow {
                    key: TimelineKey::Group(CueId(9)),
                    state: RunWord::Running,
                    pos: 1.0,
                    anchor: Some(a),
                })
                .into_iter()
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn the_messages_go_and_come_back_and_an_older_newq_lets_them_pass() {
        let msgs = vec![
            NetTest::Begin {
                run: 3,
                start: 12.5,
            },
            NetTest::End { run: 3 },
            NetTest::LogTail {
                run: 3,
                text: "a\nb".into(),
            },
            NetTest::Report(Box::new(Report {
                run: 3,
                seq: 2,
                link: Link::WiFi,
                refresh_hz: Some(60.0),
                rows: vec![Row {
                    t: 1.5,
                    clock_error_ms: Some(0.4),
                    apart_ms: Some(2.0),
                    ..Default::default()
                }],
                commands: vec![CommandNote {
                    t: 1.0,
                    seq: 4,
                    lead_ms: -3.5,
                }],
                ..Default::default()
            })),
        ];
        for m in msgs {
            let msg = Msg::NetTest(Box::new(m));
            let json = serde_json::to_string(&msg).unwrap();
            assert!(json.contains("\"t\":\"net_test\""), "{json}");
            assert_eq!(serde_json::from_str::<Msg>(&json).unwrap(), msg);
            // A NewQ that has no network test: an unknown message, passed by.
            let older = json.replace("\"net_test\"", "\"some_later_thing\"");
            assert_eq!(serde_json::from_str::<Msg>(&older).unwrap(), Msg::Unknown);
        }
    }

    #[test]
    fn cues_are_compared_by_cue_oldest_first_and_timelines_by_their_clock() {
        let theirs = state(&[(1, 10.0), (1, 20.0), (2, 5.0)], Some(30.0));
        let ours = state(&[(1, 20.004), (1, 10.0), (3, 1.0)], Some(29.99));
        let (apart, compared, missing) = apart_between(&theirs, &ours);
        assert_eq!(compared, 3);
        assert_eq!(missing, 1, "cue 2 plays there, not here");
        assert!((apart.unwrap() - 10.0).abs() < 1e-6, "{apart:?}");
        assert_eq!(apart_between(&state(&[], None), &ours), (None, 0, 0));
    }

    #[test]
    fn a_follower_notes_rows_commands_corrections_and_stalls_and_reports_each_second() {
        let mut m = Measurer::default();
        // Nothing before the test begins.
        m.command(Some(1.0), 1, 1.1);
        m.sample(1.0, Sample::default());
        assert!(m.report(1.0, Link::Wired, None, None, false).is_none());
        m.begin(7, 100.0);
        m.corrections(100.0, 4, &[]);
        m.command(Some(100.2), 1, 100.25);
        m.command(Some(100.4), 2, 100.39);
        // A difference counts once seen at two heartbeats in a row.
        m.heartbeat(
            &state(&[(1, 100.0)], None),
            &state(&[(1, 100.003)], None),
            None,
        );
        m.heartbeat(
            &state(&[(1, 100.0)], None),
            &state(&[(1, 100.003)], None),
            None,
        );
        m.corrections(
            100.5,
            6,
            &[
                "old".into(),
                "Film moved +40 ms".into(),
                "Title started 1.20 s in".into(),
            ],
        );
        m.stall(100.6, 140, "the show engine");
        m.sample(
            100.6,
            Sample {
                clock_error_ms: Some(0.123),
                rtt_ms: Some(1.0),
                locked: true,
                in_step: true,
            },
        );
        // Not again within the half second.
        m.sample(100.8, Sample::default());
        let r = m
            .report(100.9, Link::Wired, Some(60.0), Some(3), false)
            .unwrap();
        assert_eq!(r.run, 7);
        assert_eq!(r.seq, 1);
        assert_eq!(r.rows.len(), 1);
        assert_eq!(r.rows[0].t, 0.6);
        assert_eq!(r.rows[0].clock_error_ms, Some(0.12));
        assert_eq!(r.rows[0].apart_ms, Some(3.0));
        assert_eq!(r.rows[0].compared, 1);
        assert_eq!(
            r.commands.iter().map(|c| c.lead_ms).collect::<Vec<_>>(),
            [50.0, -10.0]
        );
        assert_eq!(r.corrections.len(), 2, "the two new ones only");
        assert_eq!(r.corrections[0].ms, Some(40.0));
        assert_eq!(r.corrections[1].ms, Some(1200.0));
        assert_eq!(r.stalls[0].ms, 140);
        // The next waits a second; the last goes at once, and ends it.
        assert!(m.report(101.0, Link::Wired, None, None, false).is_none());
        let last = m.end(7, 101.2, Link::Wired, None).unwrap();
        assert!(last.last && last.seq == 2 && last.rows.is_empty());
        assert_eq!(m.running(), None);
        assert!(m.end(7, 101.3, Link::Wired, None).is_none());
    }

    #[test]
    fn the_slowest_drawing_of_each_monitor_goes_in_the_next_report() {
        let mut m = Measurer::default();
        let rate = |index, fps, late| MonitorRate {
            index,
            name: format!("Screen {index}"),
            refresh_hz: Some(64.0),
            fps,
            late_permille: late,
        };
        // Not measuring: nothing kept.
        m.drawing(&[rate(0, 4.0, 1000)]);
        m.begin(1, 0.0);
        m.drawing(&[rate(0, 4.2, 900), rate(1, 60.0, 0)]);
        m.drawing(&[rate(0, 3.6, 1000)]);
        let r = m.report(1.0, Link::Wired, None, None, false).unwrap();
        assert_eq!(r.monitors, vec![rate(0, 3.6, 1000), rate(1, 60.0, 0)]);
        // The next report starts afresh.
        let r = m.report(2.0, Link::Wired, None, None, false).unwrap();
        assert!(r.monitors.is_empty());
    }

    fn at(mut s: PlayState, time: f64) -> PlayState {
        s.time = time;
        s
    }

    fn row(m: &mut Measurer, now: f64) -> Row {
        m.sample(now, Sample::default());
        m.report(now, Link::Wired, None, None, true)
            .unwrap()
            .rows
            .pop()
            .unwrap()
    }

    #[test]
    fn a_wrong_clock_shows_in_what_the_screens_show_not_in_when_cues_began() {
        // 0.24.2's test: the cue a command started carries the primary's time
        // here too, so "began" read 0 while this computer's clock was 40 ms
        // ahead of the primary's: its screens were 40 ms ahead.
        let mut m = Measurer::default();
        m.begin(1, 0.0);
        let theirs = state(&[(1, 10.0)], Some(5.0));
        let ours = state(&[(1, 10.0)], Some(5.0));
        for t in [20.0, 20.25] {
            // Taken in 41 ms after it was made by the primary's clock, with a
            // round trip of 2 ms: 40 ms off.
            let seen = seen_offset(t + 0.041, t, Some(0.002));
            m.heartbeat(&at(theirs.clone(), t), &at(ours.clone(), t), Some(seen));
        }
        let r = row(&mut m, 20.3);
        assert_eq!(r.apart_ms, Some(0.0));
        assert_eq!(r.shown_apart_ms, Some(40.0));
        // A cue that began 40 ms late here, with the clock 40 ms ahead, shows
        // the same frame as the primary's.
        let mut m = Measurer::default();
        m.begin(1, 0.0);
        let ours = state(&[(1, 10.04)], None);
        for t in [20.0, 20.25] {
            let seen = seen_offset(t + 0.041, t, Some(0.002));
            m.heartbeat(
                &at(state(&[(1, 10.0)], None), t),
                &at(ours.clone(), t),
                Some(seen),
            );
        }
        let r = row(&mut m, 20.3);
        assert_eq!(r.apart_ms, Some(40.0));
        assert_eq!(r.shown_apart_ms, Some(0.0));
    }

    #[test]
    fn a_difference_at_one_heartbeat_alone_is_not_counted() {
        // 0.24.2's timeline test: a loop going round (6 s) between the
        // primary's heartbeat and the look here read as 990 frames apart.
        let mut m = Measurer::default();
        m.begin(1, 0.0);
        let beat = |m: &mut Measurer, t: f64, ours: f64| {
            m.heartbeat(
                &at(state(&[], Some(100.0)), t),
                &at(state(&[], Some(ours)), t),
                Some(0.0),
            );
        };
        beat(&mut m, 10.0, 100.0);
        beat(&mut m, 10.25, 106.0);
        beat(&mut m, 10.5, 100.002);
        let r = row(&mut m, 10.6);
        assert_eq!(r.apart_ms, Some(2.0), "the smaller of two in a row");
        assert_eq!(r.shown_apart_ms, Some(2.0));
        // One that stays is counted from its second heartbeat.
        let mut m = Measurer::default();
        m.begin(1, 0.0);
        beat(&mut m, 10.0, 106.0);
        assert_eq!(row(&mut m, 10.1).apart_ms, None);
        m.begin(2, 0.0);
        beat(&mut m, 11.0, 106.0);
        beat(&mut m, 11.25, 106.0);
        assert_eq!(row(&mut m, 11.3).apart_ms, Some(6000.0));
        // Heartbeats too far apart don't confirm each other.
        let mut m = Measurer::default();
        m.begin(1, 0.0);
        beat(&mut m, 10.0, 106.0);
        beat(&mut m, 12.0, 106.0);
        assert_eq!(row(&mut m, 12.1).apart_ms, None);
    }

    #[test]
    fn a_tests_log_goes_back_to_its_first_part_and_the_log_put_aside_is_read() {
        let mut k = LogMark::default();
        assert_eq!(k.from(1), 0);
        // A test of one part: from where it began.
        k.begin(1, 100);
        k.begin(1, 150);
        assert_eq!(k.from(1), 100);
        // Parts of one test: each from where the first began.
        k.begin(2, 500);
        k.whole(2, 77);
        assert_eq!(k.from(2), 500);
        k.begin(3, 900);
        k.whole(3, 77);
        assert_eq!(k.from(3), 500);
        // Another test starts afresh.
        k.begin(4, 1200);
        k.whole(4, 78);
        assert_eq!(k.from(4), 1200);
        k.begin(5, 1300);
        assert_eq!(k.from(5), 1300, "alone, no Whole");

        let dir = std::env::temp_dir().join(format!("newq_since_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("newq.log"), "a\nb\nc\n").unwrap();
        assert_eq!(log_since(&dir, 2, 1000), "b\nc\n");
        // Put aside since the mark: the earlier part from it, then the new.
        std::fs::write(dir.join("newq.earlier.log"), "0123456789\nx\n").unwrap();
        std::fs::write(dir.join("newq.log"), "new\n").unwrap();
        assert_eq!(log_since(&dir, 11, 1000), "x\nnew\n");
        // Too long: the end, from a line, said.
        let long: String = (0..100).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.join("newq.log"), &long).unwrap();
        let t = log_since(&dir, 0, 60);
        assert!(t.starts_with("(The start of this computer's log"), "{t}");
        assert!(t.ends_with("line 99\n") && t.contains("\nline "), "{t}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
