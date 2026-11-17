//! The network test's measurements (stage 6 of `.claude/notes/network-plan.md`).
//!
//! While the primary runs Test Mode's network test (`testmode::network`) it
//! tells each computer `NetTest::Begin`; each follower then measures how it
//! keeps to the primary (`Measurer`) and sends what it saw once a second as a
//! `NetTest::Report`: its shared clock's error and round trip, how much lead
//! each command had left when it arrived (negative: it came after its time),
//! the corrections it made (`Engine::net_correct`), how far apart its cues'
//! times were from the primary's at each heartbeat, its main loop's stalls and
//! late frames. At `NetTest::End` it sends the last of them and the end of its
//! own log. A NewQ without the network test reads these as `Msg::Unknown`.
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
    /// A computer to its primary: the end of its own `newq.log`.
    LogTail { run: u64, text: String },
    // ---- Every test across the network (`net::measure_local`) ----
    /// A computer to its primary: what it measured of itself (memory,
    /// processor, frame rates, dropouts) since its last such report.
    Local(Box<super::measure_local::LocalReport>),
    /// Primary to its computers: play the test's sound muted (or not).
    Mute { run: u64, muted: bool },
}

/// What one computer measured over about a second.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Report {
    pub run: u64,
    /// Counts this computer's reports of the run (a gap is a lost one).
    pub seq: u32,
    pub link: Link,
    /// The fastest refresh of its monitors (Hz), for "frames apart".
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
    /// The largest difference between a cue's time here and on the primary
    /// at the heartbeats since the last row (ms); None: nothing to compare.
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
/// The most of a computer's log sent at the end (bytes).
pub const LOG_TAIL: usize = 256 * 1024;
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
}

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
    pub fn heartbeat(&mut self, theirs: &PlayState, ours: &PlayState) {
        if self.run.is_none() {
            return;
        }
        let (apart, compared, missing) = apart_between(theirs, ours);
        if let Some(a) = apart {
            self.apart = Some(self.apart.map_or(a, |w: f64| w.max(a)));
        }
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
/// cue, oldest first; one that is stopping is left out.
pub fn apart_between(theirs: &PlayState, ours: &PlayState) -> (Option<f64>, u32, u32) {
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
    let (mut worst, mut compared, mut missing) = (None::<f64>, 0u32, 0u32);
    for (cue, mut list) in there {
        list.sort_by(f64::total_cmp);
        let mine = here.get(&cue);
        for (n, a) in list.iter().enumerate() {
            match mine.and_then(|m| m.get(n)) {
                Some(b) => {
                    compared += 1;
                    let d = ((b - a) * 1000.0).abs();
                    worst = Some(worst.map_or(d, |w| w.max(d)));
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
        let d = ((b - a) * 1000.0).abs();
        worst = Some(worst.map_or(d, |w| w.max(d)));
    }
    (worst, compared, missing)
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

/// The last `max` bytes of the file at `path`, from the start of a line.
pub fn log_tail(path: &Path, max: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = len.saturating_sub(max as u64);
    if f.seek(SeekFrom::Start(from)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    let _ = f.take(max as u64).read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf).into_owned();
    if from > 0 {
        match text.find('\n') {
            Some(i) => text[i + 1..].to_string(),
            None => text,
        }
    } else {
        text
    }
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
        m.heartbeat(&state(&[(1, 100.0)], None), &state(&[(1, 100.003)], None));
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

    #[test]
    fn a_log_tail_starts_at_a_line_and_is_bounded() {
        let dir = std::env::temp_dir().join(format!("newq_tail_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("newq.log");
        let text: String = (0..1000).map(|i| format!("line {i}\n")).collect();
        std::fs::write(&p, &text).unwrap();
        let t = log_tail(&p, 100);
        assert!(
            t.len() <= 100 && t.starts_with("line ") && t.ends_with("line 999\n"),
            "{t}"
        );
        assert_eq!(log_tail(&p, 1 << 20), text);
        assert_eq!(log_tail(&dir.join("none"), 100), "");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
