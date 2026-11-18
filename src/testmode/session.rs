//! A test from start to finish: make the media ready, put the test show in
//! the place of the user's, run it while logging, and always put the user's
//! show back, with a report of what was found.
//!
//! Everything the user could lose is out of reach for the whole time: the
//! show is put aside in memory (`UiState::begin_test`), nothing may be saved
//! over its file, the autosave copy is left alone, and the test's files go to
//! a folder of their own in the log folder.

use super::analysis::{Finding, Severity, Verdict, analyse};
use super::assets::{self, Asset, Fetch, Resolved, Source};
use super::decode::{Compare, CompareResult};
use super::plan::{DisplayChoice, SoundChoice};
use super::{Flavour, Level, plan};
use crate::awake::{KeepAwake, SleepWatch};
use crate::engine::{CueStatus, Engine};
use crate::media::ffmpeg::Ffmpeg;
use crate::show::{CueId, Show};
use crate::stats::{Inputs, Sample, StatsLog};
use crate::ui::{TestStash, UiState};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Everything Test Mode keeps (a field of the interface state).
pub struct TestMode {
    /// The window is showing.
    pub open: bool,
    pub level: Level,
    /// Cue tests or timeline tests.
    pub flavour: Flavour,
    /// How long the Medium and Long tests run, in minutes.
    pub minutes: u32,
    /// Download the real media (otherwise it is made on this computer).
    pub download: bool,
    /// Seconds measured in each trial of the decoding comparison.
    pub decode_secs: u64,
    /// The heavier lap for the graphics card (remembered by the window).
    pub heavy: bool,
    /// How hard a test across the show network loads it (`net::load`;
    /// remembered by the window, read by `Fleet::new` from `Prefs`).
    pub load: crate::net::load::LoadLevel,
    /// What the window has ticked to test (`choice`).
    pub parts: super::choice::Parts,
    /// Why the last Start did nothing, for the window.
    pub notice: Option<String>,
    // ---- The network test (`network`) ----
    /// From the computers of the show network (the app puts them here).
    pub net_in: Vec<(crate::net::node::NodeId, crate::net::measure::NetTest)>,
    /// For the computers (the app sends them; None: to all).
    pub net_out: Vec<(
        Option<crate::net::node::NodeId>,
        crate::net::measure::NetTest,
    )>,
    // ----
    phase: Phase,
    /// The computers of a show network taking part (every test but the
    /// network test's own, which keeps its own), from the moment the test
    /// show is put in place until the test is done.
    fleet: Option<Box<super::fleet::Fleet>>,
    /// Several parts run one after another as one test (`sequence`).
    seq: Option<Box<super::sequence::Sequence>>,
    /// That test's id, for the computers of a show network (`Fleet::test`):
    /// each sends its log from the first part on. 0: a test of one part.
    test_id: u64,
}

impl Default for TestMode {
    fn default() -> TestMode {
        TestMode {
            open: false,
            level: Level::Short,
            flavour: Flavour::Cues,
            minutes: Level::Short.default_minutes(),
            download: true,
            decode_secs: super::decode::TRIAL_SECS,
            heavy: false,
            load: crate::net::load::LoadLevel::Off,
            parts: super::choice::Parts::default(),
            notice: None,
            net_in: Vec::new(),
            net_out: Vec::new(),
            phase: Phase::Idle,
            fleet: None,
            seq: None,
            test_id: 0,
        }
    }
}

enum Phase {
    Idle,
    Preparing(Box<Prep>),
    Running(Box<Run>),
    Comparing(Box<Compare>),
    /// The full check (see `full`): its media being made, and it running.
    FullPrep(Box<super::full::FullPrep>),
    Full(Box<super::full::FullRun>),
    /// The network test: its media being made ready here, and it running.
    NetPrep(Box<super::network::NetPrep>),
    Network(Box<super::network::NetRun>),
    /// Across a show network: the test show is in place, on its way to the
    /// computers with its media (`TestMode::fleet`).
    Spreading(Box<Spread>),
    /// Across a show network: it has ended and played its last; the
    /// computers' last reports are awaited (`fleet::ENDING_LIMIT`).
    Closing(Box<Closing>),
    Done(Box<Done>),
}

/// A test across a show network, its show in place, waiting for every
/// computer to have it and its media and to be in step.
pub struct Spread {
    then: Then,
    stash: Option<TestStash>,
    pub started: Instant,
    /// What the window calls it ("Short", "The debug test").
    pub name: String,
    stop: bool,
}

/// What a spreading test launches once every computer is ready.
enum Then {
    Level {
        prep: Box<Prep>,
        built: Box<plan::Built>,
    },
    Full {
        prep: Box<super::full::FullPrep>,
        media: Box<super::feature::Media>,
        plan: Box<super::feature::Plan>,
    },
}

/// A test across a show network that has ended, before its account.
pub struct Closing {
    what: Closed,
    end: End,
    since: f64,
}

enum Closed {
    Level(Box<Run>),
    Full(Box<super::full::FullRun>),
}

/// The media being made ready.
pub struct Prep {
    pub fetch: Fetch,
    pub compare: bool,
    /// The displays the test is to be shown on (none: the show's own).
    pub displays: Vec<DisplayChoice>,
    /// Where the sound goes, and whether it is off.
    pub sound: SoundChoice,
    /// A heavier lap for the graphics card (see `plan::build_with`).
    pub heavy: bool,
    pub level: Level,
    pub flavour: Flavour,
    pub minutes: u32,
    pub wanted: Vec<&'static Asset>,
    log_dir: PathBuf,
}

/// A test under way.
pub struct Run {
    pub level: Level,
    pub flavour: Flavour,
    /// The heavier lap: late frames are an answer, not a fault.
    pub heavy: bool,
    pub started: Instant,
    /// How long it is meant to last (the Short test ends when its lap does).
    pub planned: Option<Duration>,
    pub folder: PathBuf,
    pub history: Vec<Sample>,
    pub last: Option<Sample>,
    /// Seconds each finished lap took.
    pub laps: Vec<f64>,
    /// When each of them began, in seconds into the test.
    lap_starts: Vec<f64>,
    pub lap_secs: f64,
    pub findings: Vec<Finding>,
    pub media_notes: Vec<String>,
    first: CueId,
    repeats: bool,
    stash: Option<TestStash>,
    stats: StatsLog,
    events: Option<std::io::BufWriter<std::fs::File>>,
    lap_file: Option<std::io::BufWriter<std::fs::File>>,
    lap_started_at: Option<f64>,
    /// A lap that is running long has been written to the log (once a lap).
    overrun_logged: bool,
    first_playing: bool,
    idle_since: Option<Instant>,
    messages: Vec<String>,
    stop: bool,
    resolved: HashMap<&'static str, Resolved>,
    wanted: Vec<&'static Asset>,
    last_analysis: Instant,
    finding_log: FindingLog,
    /// Holds the computer awake while the test runs.
    awake: KeepAwake,
    sleep_watch: SleepWatch,
    /// Each time the computer slept: when (seconds into the test) and for how long.
    sleeps: Vec<(f64, f64)>,
    log_path: PathBuf,
    /// Samples of the frozen program copied into the folder (see `freeze`).
    freeze_files: Vec<String>,
    memory: MemoryWatch,
    /// The main loop's stalls and the sound's dropouts, as they happen.
    pub hitches: super::hitches::Hitches,
    /// The sound device the test plays on, if it is working.
    sound_device: Option<String>,
    /// A timeline test's driver and what it watches.
    pub timeline: Option<Box<TimelineRun>>,
    /// This computer's screens' drawing (`net_judge::note_drawing`), and
    /// when it was last looked at (seconds into the test).
    pub drawing: Vec<super::net_judge::MonitorNotes>,
    drawing_looked: Option<f64>,
    /// This computer drew in software (`gpu::drawing_in_software`).
    pub software: Option<crate::adapters::SoftwareDrawing>,
}

/// A timeline test's part of a run (see `testmode::timeline`).
pub struct TimelineRun {
    pub driver: super::timeline::Driver,
    pub watch: super::timeline::Watch,
    csv: Option<std::io::BufWriter<std::fs::File>>,
    /// How many of each of the watch's notes are in the file.
    written: (usize, usize, usize),
}

impl TimelineRun {
    fn new(script: super::timeline::Script, repeats: bool, folder: &Path) -> TimelineRun {
        let mut csv = std::fs::File::create(folder.join("timeline.csv"))
            .ok()
            .map(std::io::BufWriter::new);
        if let Some(f) = &mut csv {
            let _ = writeln!(f, "t_s,what,lap,value_s,detail");
        }
        TimelineRun {
            driver: super::timeline::Driver::new(script, repeats),
            watch: super::timeline::Watch::default(),
            csv,
            written: (0, 0, 0),
        }
    }

    /// The watch's new notes into `timeline.csv`.
    fn write(&mut self) {
        let Some(f) = &mut self.csv else {
            return;
        };
        let n = &self.watch.notes;
        let lap = self.driver.laps;
        for s in &n.stretches[self.written.0..] {
            let _ = writeln!(
                f,
                "{:.3},clock_drift,{lap},{:.4},over {:.1} s of running",
                s.at, s.drift, s.secs
            );
        }
        for a in &n.arrivals[self.written.1..] {
            let value = a.late.map_or("never".to_string(), |l| format!("{l:.4}"));
            // A late clip says what it waited for (`ClipUp::waiting`).
            let waited = match (a.late, a.waited) {
                (Some(l), Some(w)) if l > super::analysis::LATE_PROBLEM => {
                    format!(" (waited: {w})")
                }
                (None, Some(w)) => format!(" (waited: {w})"),
                _ => String::new(),
            };
            let _ = writeln!(
                f,
                "{:.3},clip_late,{lap},{value},\"{}{waited}\"",
                a.at,
                a.name.replace('"', "'")
            );
        }
        for c in &n.cueing[self.written.2..] {
            let _ = writeln!(
                f,
                "{:.3},cueing,{},{:.3},{}",
                c.at,
                c.lap,
                c.secs,
                if c.gave_up {
                    "ran out of time"
                } else {
                    "ready"
                }
            );
        }
        self.written = (n.stretches.len(), n.arrivals.len(), n.cueing.len());
        let _ = f.flush();
    }
}

/// What a test may use of the computer's memory before it is stopped: half of
/// it, and never less than 2 GB. A runaway that goes on to fill the memory and
/// swap makes the whole computer unusable, which no test is worth.
pub fn memory_limit_mb(ram_mb: u64) -> u64 {
    (ram_mb / 2).max(2048)
}

/// Watches the memory of NewQ and of the FFmpeg processes it started: NewQ's
/// own once a second, much more often than the log's rows (a runaway can fill
/// the memory between two of those), and the children every few seconds (finding
/// them means looking at every process, which costs more). The looking is done
/// on a thread of its own: on Windows, going through every process took up to
/// 3 s at times, and done by the main loop that was the show stopping.
pub(super) struct MemoryWatch {
    limit_mb: u64,
    pub(super) ram_mb: u64,
    seen: Arc<MemorySeen>,
    stop: Arc<AtomicBool>,
}

/// What the watching thread has seen, in MB.
#[derive(Default)]
struct MemorySeen {
    own: AtomicU64,
    children: AtomicU64,
    peak: AtomicU64,
    /// How many times it has looked at NewQ's own memory.
    looks: AtomicU64,
    /// How many times it has gone through every process for the children.
    child_looks: AtomicU64,
}

impl MemoryWatch {
    pub(super) fn new() -> MemoryWatch {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let ram_mb = sys.total_memory() >> 20;
        let seen = Arc::new(MemorySeen::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (seen2, stop2) = (seen.clone(), stop.clone());
        let me = sysinfo::Pid::from_u32(std::process::id());
        let _ = std::thread::Builder::new()
            .name("memory-watch".into())
            .spawn(move || {
                let mut n = 0u64;
                while !stop2.load(Ordering::Relaxed) {
                    if n.is_multiple_of(3) {
                        sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
                        let children = sys
                            .processes()
                            .values()
                            .filter(|p| p.thread_kind().is_none() && p.parent() == Some(me))
                            .map(|p| p.memory() >> 20)
                            .sum();
                        seen2.children.store(children, Ordering::Relaxed);
                        seen2.child_looks.fetch_add(1, Ordering::Release);
                    } else {
                        sys.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[me]), true);
                    }
                    if let Some(p) = sys.process(me) {
                        let mb = p.memory() >> 20;
                        seen2.own.store(mb, Ordering::Relaxed);
                        seen2.peak.fetch_max(mb, Ordering::Relaxed);
                        seen2.looks.fetch_add(1, Ordering::Release);
                    }
                    n += 1;
                    for _ in 0..20 {
                        if stop2.load(Ordering::Relaxed) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
            });
        MemoryWatch {
            limit_mb: memory_limit_mb(ram_mb),
            ram_mb,
            seen,
            stop,
        }
    }

    /// The memory of NewQ and its children in MB, if it is over the limit, as
    /// last seen. Never waits.
    pub(super) fn over(&mut self, _now: Instant) -> Option<u64> {
        if self.seen.looks.load(Ordering::Acquire) == 0 {
            return None;
        }
        let total =
            self.seen.own.load(Ordering::Relaxed) + self.seen.children.load(Ordering::Relaxed);
        (total > self.limit_mb).then_some(total)
    }

    /// The most NewQ's own memory has been, in MB.
    pub(super) fn peak_mb(&self) -> u64 {
        self.seen.peak.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn children_mb(&self) -> u64 {
        self.seen.children.load(Ordering::Relaxed)
    }
}

impl Drop for MemoryWatch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// A finding the window showed during the run, kept so that it is in the log
/// and the report even if it had gone by the end.
struct Seen {
    severity: Severity,
    /// The finding's words without their numbers, which change as it is
    /// recomputed: what tells one finding from another.
    identity: String,
    text: String,
    first: f64,
    ended: Option<f64>,
    /// The biggest number in the text when it was last written to the log, so
    /// that a finding that gets clearly worse (a longer freeze) is logged again.
    logged: f64,
}

/// The biggest number in some text.
fn biggest_number(text: &str) -> f64 {
    let mut best = 0.0_f64;
    let mut cur = String::new();
    for c in text.chars().chain(std::iter::once(' ')) {
        if c.is_ascii_digit() || (c == '.' && !cur.is_empty()) {
            cur.push(c);
        } else {
            if let Ok(v) = cur.trim_end_matches('.').parse::<f64>() {
                best = best.max(v);
            }
            cur.clear();
        }
    }
    best
}

fn identity_of(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_ascii_digit() && !matches!(c, '.' | '+' | '-' | '\u{2192}'))
        .collect()
}

/// What the window has shown during a run (see `Seen`).
#[derive(Default)]
pub(super) struct FindingLog {
    seen: Vec<Seen>,
}

impl FindingLog {
    /// Take note of what the window is showing at `t` seconds. Returns the
    /// lines for `events.log`: one for each finding that has appeared, and for
    /// each that has gone (whether or not it lasted).
    pub(super) fn update(&mut self, t: f64, current: &[Finding]) -> Vec<String> {
        let mut lines = Vec::new();
        for f in current {
            let id = identity_of(&f.text);
            match self
                .seen
                .iter_mut()
                .find(|s| s.ended.is_none() && s.identity == id)
            {
                Some(s) => {
                    s.text = f.text.clone();
                    // A problem that gets clearly worse is written again; the
                    // first line alone would hide the worst of a run.
                    let now = biggest_number(&f.text);
                    if f.severity == Severity::Problem && now > s.logged * 1.1 {
                        s.logged = now;
                        lines.push(format!("Problem in the window, now worse: {}", f.text));
                    }
                }
                None => {
                    let label = match f.severity {
                        Severity::Watch => "Showing in the window",
                        Severity::Note => "Note in the window",
                        Severity::Problem => "Problem in the window",
                    };
                    lines.push(format!("{label}: {}", f.text));
                    self.seen.push(Seen {
                        severity: f.severity,
                        identity: id,
                        text: f.text.clone(),
                        first: t,
                        ended: None,
                        logged: biggest_number(&f.text),
                    });
                }
            }
        }
        lines.extend(self.close(t, current));
        lines
    }

    /// Mark what is no longer among `current` as gone; returns the log lines.
    pub(super) fn close(&mut self, t: f64, current: &[Finding]) -> Vec<String> {
        let mut lines = Vec::new();
        for s in &mut self.seen {
            if s.ended.is_none() && !current.iter().any(|f| identity_of(&f.text) == s.identity) {
                s.ended = Some(t);
                lines.push(format!(
                    "No longer showing (it had been there {}): {}",
                    clock(Duration::from_secs_f64((t - s.first).max(0.0))),
                    s.text
                ));
            }
        }
        lines
    }

    /// Findings for the report from what was shown and has gone: those of
    /// `Watch` and `Problem` kind that the final account doesn't have in
    /// its own words.
    pub(super) fn gone(&self, final_findings: &[Finding]) -> Vec<Finding> {
        self.seen
            .iter()
            .filter(|s| s.severity != Severity::Note)
            .filter(|s| s.ended.is_some())
            .filter(|s| {
                !final_findings
                    .iter()
                    .any(|f| identity_of(&f.text) == s.identity)
            })
            .map(|s| Finding {
                severity: Severity::Watch,
                text: format!(
                    "From {} to {} the window was showing this, and it cleared by itself: {}",
                    clock(Duration::from_secs_f64(s.first)),
                    clock(Duration::from_secs_f64(s.ended.unwrap_or(s.first))),
                    s.text
                ),
            })
            .collect()
    }
}

impl Run {
    /// Write a line to `events.log`, at `t` seconds into the run.
    fn log_at(&mut self, t: f64, text: &str) {
        if let Some(f) = &mut self.events {
            let _ = writeln!(f, "[{t:>9.3}] {text}");
            let _ = f.flush();
        }
    }

    /// The laps that no sleep of the computer fell in: the others say nothing
    /// about how the show ran.
    fn clean_laps(&self) -> Vec<f64> {
        undisturbed(&self.lap_starts, &self.laps, &self.sleeps)
    }

    pub fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    pub fn progress(&self) -> Option<f32> {
        self.planned
            .map(|p| (self.elapsed().as_secs_f32() / p.as_secs_f32().max(1.0)).clamp(0.0, 1.0))
    }
}

/// A test that has ended and put the show back.
pub struct Done {
    pub compare: Option<CompareResult>,
    pub flavour: Flavour,
    /// It was the full check: how each kind of cue came out (passed, failed, not run).
    pub full: Option<Vec<(&'static str, usize, usize, usize)>>,
    /// It was the network test: each computer's verdict.
    pub network: Option<super::network::NetDone>,
    /// A run of several parts: each part's own account, in order, and
    /// those that didn't run (`sequence`). Empty for a test of one part.
    pub parts: Vec<super::sequence::PartDone>,
    /// Another test across a show network: each other computer's verdict.
    pub computers: Vec<super::net_judge::ComputerVerdict>,
    /// The one zip of everything, being made or made (see `bundle`).
    pub bundle: super::bundle::Bundle,
    pub level: Level,
    pub verdict: Verdict,
    pub headline: String,
    pub findings: Vec<Finding>,
    /// Said when the run was long enough to judge memory and found nothing.
    pub steady: Option<&'static str>,
    pub folder: PathBuf,
    pub report: String,
    pub laps: usize,
    pub elapsed: Duration,
    pub ended: End,
}

#[derive(Clone, Debug, PartialEq)]
pub enum End {
    /// It ran its course.
    Finished,
    /// The user stopped it.
    Stopped,
    /// The show stopped by itself, or the test could not go on.
    Failed(String),
    /// The program was closed.
    Quit,
}

pub enum View<'a> {
    Idle,
    Preparing(&'a Prep),
    Running(&'a Run),
    Comparing(&'a Compare),
    PreparingFull(&'a super::full::FullPrep),
    Checking(&'a super::full::FullRun),
    NetPreparing(&'a super::network::NetPrep),
    Network(&'a super::network::NetRun),
    Spreading(&'a Spread, &'a super::fleet::Fleet),
    /// A test across a show network playing: its computers.
    Closing,
    Done(&'a Done),
}

impl Level {
    pub fn default_minutes(self) -> u32 {
        match self {
            Level::Short => 0,
            Level::Medium => 60,
            Level::Long => 480,
        }
    }

    /// Seconds between rows of the log.
    pub fn stats_every(self) -> u64 {
        match self {
            Level::Short => 2,
            Level::Medium => 10,
            Level::Long => 15,
        }
    }

    pub fn summary(self) -> &'static str {
        match self {
            Level::Short => {
                "About a minute. One lap of everything: clips at three frame rates, large pictures, layers, a 5.1 trailer."
            }
            Level::Medium => {
                "About an hour. The same lap, with a second trailer through sound effects, round and round: the kind of run that finds a slow leak."
            }
            Level::Long => {
                "Overnight. A longer lap with H.265, a very high bitrate clip, six large pictures, more effects, and a burst of twenty quick cues, round and round."
            }
        }
    }
}

impl Flavour {
    /// What the window says of a test.
    pub fn summary(self, level: Level) -> &'static str {
        match (self, level) {
            (Flavour::Cues, l) => l.summary(),
            (Flavour::Timeline, Level::Short) => {
                "About a minute. One run of a timeline: films at three frame rates butted and crossfaded with a sound track under them, a still under a caption, a picture a fade cue brings up, a pause that GO ends (pressed for you), and a Stop that fades it all out."
            }
            (Flavour::Timeline, Level::Medium) => {
                "About an hour. A longer timeline and a timeline group, round and round: a third track with speed and opacity keyframes, a picture also on a second display, a pause that freezes a film, a loop left after three passes, a jump to a marker."
            }
            (Flavour::Timeline, Level::Long) => {
                "Overnight. The medium timeline with H.265 and a 25 Mbit/s film, a mosaic of five films with effects, very large pictures, and seeks to a marker and back while it runs."
            }
        }
    }
}

pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

impl TestMode {
    pub fn view(&self) -> View<'_> {
        match &self.phase {
            Phase::Idle => View::Idle,
            Phase::Preparing(p) => View::Preparing(p),
            Phase::Running(r) => View::Running(r),
            Phase::Comparing(c) => View::Comparing(c),
            Phase::FullPrep(p) => View::PreparingFull(p),
            Phase::Full(r) => View::Checking(r),
            Phase::NetPrep(p) => View::NetPreparing(p),
            Phase::Network(r) => View::Network(r),
            Phase::Spreading(s) => match &self.fleet {
                Some(f) => View::Spreading(s, f),
                None => View::Idle,
            },
            Phase::Closing(_) => View::Closing,
            Phase::Done(d) => View::Done(d),
        }
    }

    /// The computers of a test across a show network, while it runs.
    pub fn fleet(&self) -> Option<&super::fleet::Fleet> {
        self.fleet.as_deref()
    }

    /// Whether a test is being prepared or run (the window can't be changed).
    pub fn busy(&self) -> bool {
        matches!(
            self.phase,
            Phase::Preparing(_)
                | Phase::Running(_)
                | Phase::Comparing(_)
                | Phase::FullPrep(_)
                | Phase::Full(_)
                | Phase::NetPrep(_)
                | Phase::Network(_)
                | Phase::Spreading(_)
                | Phase::Closing(_)
        )
    }

    /// Back to choosing a test.
    pub fn dismiss(&mut self) {
        if matches!(self.phase, Phase::Done(_)) {
            self.phase = Phase::Idle;
            self.notice = None;
        }
    }

    pub fn set_level(&mut self, level: Level) {
        if level != self.level {
            self.level = level;
            self.minutes = level.default_minutes();
        }
    }

    /// Choose a test of one part: its level and flavour.
    #[cfg(test)]
    pub fn choose(&mut self, level: Level, flavour: Flavour) {
        self.flavour = flavour;
        self.parts = super::choice::Parts {
            cue_list: flavour == Flavour::Cues,
            timeline: flavour == Flavour::Timeline,
            network: false,
        };
        self.set_level(level);
    }

    /// The test chosen, as this computer keeps it (`Prefs::test_pick`).
    pub fn pick(&self) -> crate::prefs::TestPick {
        crate::prefs::TestPick {
            level: self.level,
            parts: self.parts,
        }
    }

    /// Choose `pick` again (the window opening), unless a test is under way.
    pub fn choose_pick(&mut self, pick: crate::prefs::TestPick) {
        if self.busy() || self.pick() == pick {
            return;
        }
        self.set_level(pick.level);
        self.parts = pick.parts;
    }

    /// Where a run of several parts is: the part playing (from 0), of how
    /// many, and which.
    pub fn sequence(&self) -> Option<(usize, usize, super::choice::Part)> {
        self.seq.as_ref().map(|s| s.at())
    }

    /// Start what the window chose: the parts of `plan` one after another,
    /// as one test (one part alone runs as it always has).
    pub fn start_plan(
        &mut self,
        ff: Ffmpeg,
        log_dir: &Path,
        plan: Vec<super::choice::PartPlan>,
        displays: Vec<DisplayChoice>,
        sound: SoundChoice,
    ) {
        if self.busy() || plan.is_empty() {
            return;
        }
        let first = plan[0];
        let go = super::sequence::Go {
            ff,
            log_dir: log_dir.to_path_buf(),
            displays,
            sound,
            heavy: self.heavy,
        };
        self.test_id = if plan.len() > 1 {
            crate::net::node::random_u64()
        } else {
            0
        };
        self.seq =
            (plan.len() > 1).then(|| Box::new(super::sequence::Sequence::new(plan, go.clone())));
        self.start_part(first, &go);
    }

    /// Begin one part.
    fn start_part(&mut self, p: super::choice::PartPlan, go: &super::sequence::Go) {
        self.level = p.level;
        match p.part.flavour() {
            Some(flavour) => {
                self.flavour = flavour;
                self.minutes = p.minutes;
                self.start(
                    go.ff.clone(),
                    &go.log_dir,
                    go.displays.clone(),
                    go.sound.clone(),
                    go.heavy,
                );
            }
            None => self.start_network(go.ff.clone(), &go.log_dir, p.laps, go.displays.clone()),
        }
    }

    /// A run of several parts, after each look: the user's show kept aside
    /// between parts, the next part begun once one has finished, and the
    /// whole run's account once the last is done or one ended it early.
    fn sequence_step(&mut self, show: &mut Show, engine: &mut Engine, st: &mut UiState) {
        let Some(seq) = &mut self.seq else {
            // (Never left aside with no run to bring it back.)
            if st.holds_test() && !self.busy() {
                st.release_test(show, engine);
            }
            st.test_carry.parts_run = false;
            return;
        };
        match &self.phase {
            Phase::Done(_) => {
                let Phase::Done(done) = std::mem::replace(&mut self.phase, Phase::Idle) else {
                    return;
                };
                let finished = done.ended == End::Finished;
                seq.record(*done);
                if finished
                    && !seq.quit
                    && let Some(next) = seq.next_part()
                {
                    let go = seq.go.clone();
                    self.notice = None;
                    #[cfg(test)]
                    if let Some(made) = seq.made.pop_front() {
                        self.launch_made(made, &go, show, engine, st);
                        return;
                    }
                    self.start_part(next, &go);
                    return;
                }
                self.end_sequence(show, engine, st, None);
            }
            Phase::Idle => {
                // The part ended before it began (cancelled, refused).
                let why = self
                    .notice
                    .clone()
                    .unwrap_or_else(|| "it did not start".into());
                self.end_sequence(show, engine, st, Some(why));
            }
            _ => {}
        }
    }

    /// The run of several parts is over: the user's show back (if a part
    /// left it aside) and the one account.
    fn end_sequence(
        &mut self,
        show: &mut Show,
        engine: &mut Engine,
        st: &mut UiState,
        unstarted: Option<String>,
    ) {
        st.release_test(show, engine);
        st.test_carry.parts_run = false;
        let Some(seq) = self.seq.take() else {
            return;
        };
        if let Some(done) = seq.finish(unstarted) {
            self.notice = None;
            self.phase = Phase::Done(Box::new(done));
        }
    }

    /// Begin: make the media ready (downloading what is missing).
    pub fn start(
        &mut self,
        ff: Ffmpeg,
        log_dir: &Path,
        displays: Vec<DisplayChoice>,
        sound: SoundChoice,
        heavy: bool,
    ) {
        if self.busy() {
            return;
        }
        let wanted = assets::wanted_for(self.level, self.flavour);
        let fetch = assets::start(
            ff,
            assets::cache_dir(log_dir),
            wanted.clone(),
            self.download,
        );
        self.notice = None;
        self.phase = Phase::Preparing(Box::new(Prep {
            fetch,
            compare: false,
            displays,
            sound,
            heavy: heavy && self.flavour == Flavour::Cues,
            level: self.level,
            flavour: self.flavour,
            minutes: self.minutes,
            wanted,
            log_dir: log_dir.to_path_buf(),
        }));
    }

    /// Begin the full check, with no more questions: every kind of cue, from
    /// media made on this computer, and one zip of everything at the end.
    pub fn start_full(
        &mut self,
        ff: Ffmpeg,
        log_dir: &Path,
        displays: Vec<DisplayChoice>,
        sound: SoundChoice,
    ) {
        if self.busy() {
            return;
        }
        self.notice = None;
        match super::full::FullPrep::start(ff, log_dir, displays, sound, self.download) {
            Ok(prep) => self.phase = Phase::FullPrep(Box::new(prep)),
            Err(e) => self.notice = Some(e),
        }
    }

    /// Begin the decoding comparison: the film it makes its 4K clip from is
    /// made ready first.
    pub fn start_compare(&mut self, ff: Ffmpeg, log_dir: &Path, displays: Vec<DisplayChoice>) {
        if self.busy() {
            return;
        }
        let wanted: Vec<&'static Asset> = assets::find("bbb_60").into_iter().collect();
        let fetch = assets::start(
            ff,
            assets::cache_dir(log_dir),
            wanted.clone(),
            self.download,
        );
        self.notice = None;
        self.phase = Phase::Preparing(Box::new(Prep {
            fetch,
            compare: true,
            displays,
            sound: SoundChoice::default(),
            heavy: false,
            level: self.level,
            flavour: Flavour::Cues,
            minutes: 0,
            wanted,
            log_dir: log_dir.to_path_buf(),
        }));
    }

    /// Stop what is under way: cancel the preparation, or end the test.
    pub fn stop(&mut self) {
        if let Some(s) = &mut self.seq {
            s.stopped = true;
        }
        match &mut self.phase {
            Phase::Preparing(p) => p.fetch.cancel(),
            Phase::Running(r) => r.stop = true,
            Phase::Comparing(c) => c.request_stop(),
            Phase::FullPrep(p) => p.cancel(),
            Phase::Full(r) => r.stop = true,
            Phase::NetPrep(p) => p.cancel(),
            Phase::Network(r) => r.request_stop(),
            Phase::Spreading(s) => s.stop = true,
            _ => {}
        }
    }

    /// Begin the network test (or soak): its media made ready here first.
    pub fn start_network(
        &mut self,
        ff: Ffmpeg,
        log_dir: &Path,
        laps: u32,
        displays: Vec<DisplayChoice>,
    ) {
        if self.busy() {
            return;
        }
        self.notice = None;
        let mut prep = super::network::NetPrep::start(ff, log_dir, laps, self.download);
        prep.displays = displays;
        self.phase = Phase::NetPrep(Box::new(prep));
    }

    /// Start a run of several parts with their shows already made (the
    /// tests of the test): the first is launched now, the others in turn.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn launch_plan(
        &mut self,
        ff: Ffmpeg,
        plan: Vec<super::choice::PartPlan>,
        mut made: Vec<super::sequence::Made>,
        log_dir: &Path,
        show: &mut Show,
        engine: &mut Engine,
        st: &mut UiState,
    ) {
        let go = super::sequence::Go {
            ff,
            log_dir: log_dir.to_path_buf(),
            displays: Vec::new(),
            sound: SoundChoice::default(),
            heavy: false,
        };
        let first = made.remove(0);
        self.test_id = crate::net::node::random_u64();
        let mut seq = super::sequence::Sequence::new(plan, go.clone());
        seq.made = made.into();
        self.seq = Some(Box::new(seq));
        st.test_carry.keep = true;
        st.test_carry.parts_run = true;
        self.launch_made(first, &go, show, engine, st);
    }

    #[cfg(test)]
    fn launch_made(
        &mut self,
        made: super::sequence::Made,
        go: &super::sequence::Go,
        show: &mut Show,
        engine: &mut Engine,
        st: &mut UiState,
    ) {
        use super::sequence::Made;
        match made {
            Made::Cues(built) => self.launch_across(
                go.ff.clone(),
                *built,
                &go.log_dir,
                Level::Short,
                show,
                engine,
                st,
            ),
            Made::Network(built, script, planned) => {
                match self.launch_network(*built, &go.log_dir, show, engine, st) {
                    Ok(()) => {
                        if let Some(r) = self.network_run_mut() {
                            r.shorten(script, planned);
                        }
                    }
                    Err(e) => self.notice = Some(e),
                }
            }
        }
    }

    /// Start the network test with a show already made (the tests of the test).
    #[cfg(test)]
    pub fn launch_network(
        &mut self,
        built: super::network::Built,
        log_dir: &Path,
        show: &mut Show,
        engine: &mut Engine,
        st: &mut UiState,
    ) -> Result<(), String> {
        let mut run =
            super::network::NetRun::launch(built, 1, log_dir, Vec::new(), show, engine, st)?;
        run.fleet.test = self.test_id;
        self.phase = Phase::Network(Box::new(run));
        Ok(())
    }

    /// The network test under way, if it is.
    #[cfg(test)]
    pub fn network_run(&self) -> Option<&super::network::NetRun> {
        match &self.phase {
            Phase::Network(r) => Some(r),
            _ => None,
        }
    }

    /// Show a finished test's result (the window's tests).
    #[cfg(test)]
    pub fn show_done(&mut self, done: Done) {
        self.phase = Phase::Done(Box::new(done));
    }

    #[cfg(test)]
    pub fn network_run_mut(&mut self) -> Option<&mut super::network::NetRun> {
        match &mut self.phase {
            Phase::Network(r) => Some(r),
            _ => None,
        }
    }

    /// Called every frame of the application.
    pub fn update(&mut self, show: &mut Show, engine: &mut Engine, st: &mut UiState) {
        self.update_at(show, engine, st, crate::net::time::local_now());
    }

    /// `update`, at the primary's shared time `now` (the tests set it).
    pub fn update_at(&mut self, show: &mut Show, engine: &mut Engine, st: &mut UiState, now: f64) {
        // Between the parts of a run the user's show stays aside: a part
        // that more follow keeps it when it ends, and makes no zip of its own.
        st.test_carry.keep = self.seq.as_ref().is_some_and(|s| s.more_follow());
        st.test_carry.parts_run = self.seq.is_some();
        let mut inbox = std::mem::take(&mut self.net_in);
        // A test across the show network: what the computers sent, and
        // telling one that came back to measure.
        if let Some(f) = &mut self.fleet {
            for (from, m) in std::mem::take(&mut inbox) {
                f.take(from, m, now, &st.net);
            }
            f.keep_up(st, now);
        }
        match &mut self.phase {
            // This computer's screens, once a second (every test judges a
            // screen that draws far below its refresh).
            Phase::Running(r) => {
                let t = r.started.elapsed().as_secs_f64();
                if r.drawing_looked.is_none_or(|l| t - l >= 1.0) {
                    r.drawing_looked = Some(t);
                    super::net_judge::note_drawing(&mut r.drawing, &st.drawing);
                    super::net_judge::note_software(
                        &mut r.software,
                        crate::gpu::drawing_in_software(),
                    );
                }
            }
            Phase::Full(r) => r.note_drawing(&st.drawing),
            _ => {}
        }
        self.update_phase(show, engine, st, now, inbox);
        self.sequence_step(show, engine, st);
        if let Some(f) = &mut self.fleet {
            self.net_out.append(&mut f.out);
            let said = std::mem::take(&mut f.said);
            match &mut self.phase {
                Phase::Running(r) => {
                    for (_, l) in said {
                        let t = r.started.elapsed().as_secs_f64();
                        r.log_at(t, &l);
                    }
                }
                Phase::Full(r) => r.said(said),
                _ => {}
            }
        }
        if !self.busy() && !matches!(self.phase, Phase::Done(_)) {
            self.fleet = None;
        }
    }

    fn update_phase(
        &mut self,
        show: &mut Show,
        engine: &mut Engine,
        st: &mut UiState,
        now: f64,
        inbox: Vec<(crate::net::node::NodeId, crate::net::measure::NetTest)>,
    ) {
        match &mut self.phase {
            Phase::Idle | Phase::Done(_) => {}
            Phase::Spreading(s) => {
                let Some(f) = &mut self.fleet else {
                    self.phase = Phase::Idle;
                    return;
                };
                let first = f.first_look(now);
                let why = if s.stop {
                    Some(End::Stopped)
                } else {
                    f.waited_too_long(st, s.started).map(End::Failed)
                };
                if let Some(end) = why {
                    // Nothing played: the user's show back, here and so everywhere.
                    if let Some(stash) = s.stash.take() {
                        st.end_test(show, engine, stash);
                    }
                    let name = s.name.clone();
                    self.fleet = None;
                    self.phase = Phase::Idle;
                    self.notice = Some(match end {
                        End::Failed(why) => format!(
                            "The {name} test did not start: {why}. Your show is back on every computer; nothing was changed."
                        ),
                        _ => "Cancelled. Your show is back on every computer; nothing was changed."
                            .into(),
                    });
                    return;
                }
                if !(f.media_ready(st) && now - first >= 2.0) {
                    return;
                }
                f.begin(now);
                let Phase::Spreading(s) = std::mem::replace(&mut self.phase, Phase::Idle) else {
                    return;
                };
                let Spread { then, stash, .. } = *s;
                let Some(stash) = stash else {
                    return;
                };
                match then {
                    Then::Level { prep, built } => {
                        match launch(*prep, Some((stash, *built)), show, engine, st) {
                            Ok(run) => self.phase = Phase::Running(Box::new(run)),
                            Err(e) => self.notice = Some(e),
                        }
                    }
                    Then::Full { prep, media, plan } => {
                        let (folder, log_dir) = (prep.folder.clone(), prep.log_dir().to_path_buf());
                        match super::full::launch(
                            *prep,
                            *media,
                            Some((stash, *plan)),
                            show,
                            engine,
                            st,
                        ) {
                            Ok(run) => self.phase = Phase::Full(Box::new(run)),
                            Err(e) => {
                                self.phase = Phase::Done(Box::new(super::full::could_not_start(
                                    &folder, &log_dir, &e,
                                )));
                            }
                        }
                    }
                }
            }
            Phase::Closing(c) => {
                let done = self.fleet.as_ref().is_none_or(|f| {
                    f.all_in(st, now) || now - c.since > super::fleet::ENDING_LIMIT
                });
                if !done {
                    return;
                }
                let Phase::Closing(c) = std::mem::replace(&mut self.phase, Phase::Idle) else {
                    return;
                };
                let fleet = self.fleet.take().map(|f| (f, now));
                let Closing { what, end, .. } = *c;
                self.phase = Phase::Done(Box::new(match what {
                    Closed::Level(r) => finish(*r, end, show, engine, st, fleet),
                    Closed::Full(r) => super::full::finish(*r, end, show, engine, st, fleet),
                }));
            }
            Phase::NetPrep(p) => {
                let Some((res, flash)) = p.ready() else {
                    return;
                };
                let Phase::NetPrep(p) = std::mem::replace(&mut self.phase, Phase::Idle) else {
                    return;
                };
                if p.cancelled() {
                    self.notice = Some("Cancelled. Nothing was changed.".into());
                    return;
                }
                if let Err(e) = &flash {
                    log::warn!("network test: the Sync flash couldn't be made: {e}");
                }
                let peers = st.net.peers.clone();
                let built =
                    super::network::build(show, &res, flash.as_deref().ok(), &peers, &p.displays);
                match super::network::NetRun::launch(
                    built,
                    p.laps,
                    &p.log_dir,
                    p.wanted.clone(),
                    show,
                    engine,
                    st,
                ) {
                    Ok(mut run) => {
                        run.fleet.test = self.test_id;
                        self.phase = Phase::Network(Box::new(run));
                    }
                    Err(e) => self.notice = Some(e),
                }
            }
            Phase::Network(r) => {
                let end = r.tick(show, engine, st, now, inbox);
                self.net_out.append(&mut r.fleet.out);
                if let Some(end) = end {
                    let Phase::Network(r) = std::mem::replace(&mut self.phase, Phase::Idle) else {
                        return;
                    };
                    self.phase = Phase::Done(Box::new(super::network::finish(
                        *r, end, show, engine, st, now,
                    )));
                }
            }
            Phase::Preparing(p) => {
                if !p.fetch.finished() {
                    return;
                }
                if p.fetch.status.lock().cancelled {
                    self.phase = Phase::Idle;
                    self.notice = Some("Cancelled. Nothing was changed.".into());
                    return;
                }
                let Phase::Preparing(p) = std::mem::replace(&mut self.phase, Phase::Idle) else {
                    return;
                };
                if p.compare {
                    let resolved = p.fetch.status.lock().resolved.clone();
                    let Some(source) = resolved.get("bbb_60").map(|r| r.path.clone()) else {
                        self.notice =
                            Some("The film the 4K clip is made from isn't available.".into());
                        return;
                    };
                    let Some(ff) = engine.ff.clone() else {
                        self.notice = Some("FFmpeg wasn't found.".into());
                        return;
                    };
                    let folder = run_folder_named(&p.log_dir, "decode", unix_now());
                    if let Err(e) = std::fs::create_dir_all(&folder) {
                        self.notice = Some(format!(
                            "Can't make the folder for the log files ({}): {e}. Nothing was changed.",
                            folder.display()
                        ));
                        return;
                    }
                    let orig = st.prefs.hw_decode;
                    let _ = std::fs::write(
                        folder.join("system.txt"),
                        system_report(
                            engine,
                            st,
                            show,
                            &p.wanted,
                            &resolved,
                            &p.displays,
                            &p.sound,
                            false,
                        ),
                    );
                    self.phase = Phase::Comparing(Box::new(Compare::begin(
                        ff,
                        &p.log_dir,
                        source,
                        folder,
                        self.decode_secs,
                        orig,
                        p.displays.clone(),
                    )));
                    return;
                }
                // Across a show network: the show is put in place and sent
                // first; the test begins once every computer is ready.
                if across(st) {
                    if let Some(why) = st.test_refusal() {
                        self.notice = Some(why);
                        return;
                    }
                    let built = build(&p, show, true);
                    let files = super::fleet::files_of(&built.show, None);
                    let name = p.flavour.name(p.level);
                    let muted = p.sound.muted;
                    let test_show = built.show.clone();
                    self.spread(
                        Then::Level {
                            prep: p,
                            built: Box::new(built),
                        },
                        test_show,
                        files,
                        name,
                        muted,
                        show,
                        engine,
                        st,
                    );
                    return;
                }
                match launch(*p, None, show, engine, st) {
                    Ok(run) => self.phase = Phase::Running(Box::new(run)),
                    Err(e) => {
                        log::error!("test mode: {e}");
                        self.notice = Some(e);
                    }
                }
            }
            Phase::Comparing(c) => {
                if let Some(end) = c.tick(show, engine, st, Instant::now()) {
                    let Phase::Comparing(c) = std::mem::replace(&mut self.phase, Phase::Idle)
                    else {
                        return;
                    };
                    let elapsed = c.elapsed();
                    let folder = c.folder.clone();
                    let (result, findings, report, end) = c.finish(end, show, engine, st);
                    self.phase = Phase::Done(Box::new(compare_done(
                        result, findings, report, end, folder, elapsed,
                    )));
                }
            }
            Phase::Running(r) => {
                if let Some(end) = r.tick(show, engine, Instant::now()) {
                    let Phase::Running(r) = std::mem::replace(&mut self.phase, Phase::Idle) else {
                        return;
                    };
                    match &mut self.fleet {
                        Some(f) => {
                            // The computers stop with this one, then send their last.
                            engine.stop_all(0.0);
                            f.end(now);
                            self.phase = Phase::Closing(Box::new(Closing {
                                what: Closed::Level(r),
                                end,
                                since: now,
                            }));
                        }
                        None => {
                            self.phase =
                                Phase::Done(Box::new(finish(*r, end, show, engine, st, None)))
                        }
                    }
                }
            }
            Phase::FullPrep(p) => {
                let Some(made) = p.ready() else {
                    return;
                };
                let Phase::FullPrep(p) = std::mem::replace(&mut self.phase, Phase::Idle) else {
                    return;
                };
                if p.cancelled() {
                    self.notice = Some("Cancelled. Nothing was changed.".into());
                    return;
                }
                // A show that has changed meanwhile is a reason to wait, not a failure.
                if let Some(why) = st.test_refusal() {
                    self.notice = Some(why);
                    return;
                }
                let (folder, log_dir) = (p.folder.clone(), p.log_dir().to_path_buf());
                if across(st)
                    && let Ok(media) = made.clone()
                {
                    let plan = super::full::plan_for(&p, &media, show, st, true);
                    let test_show = plan.show.clone();
                    let files = super::fleet::files_of(&plan.show, None);
                    let muted = super::full::muted(&p);
                    self.spread(
                        Then::Full {
                            prep: p,
                            media: Box::new(media),
                            plan: Box::new(plan),
                        },
                        test_show,
                        files,
                        "debug".into(),
                        muted,
                        show,
                        engine,
                        st,
                    );
                    return;
                }
                let launched =
                    made.and_then(|media| super::full::launch(*p, media, None, show, engine, st));
                match launched {
                    Ok(run) => self.phase = Phase::Full(Box::new(run)),
                    Err(e) => {
                        // It could not even begin: that is a finding too, and
                        // the zip (the reason, the log) is made all the same.
                        log::error!("test mode: the full check could not start: {e}");
                        self.phase = Phase::Done(Box::new(super::full::could_not_start(
                            &folder, &log_dir, &e,
                        )));
                    }
                }
            }
            Phase::Full(r) => {
                if let Some(end) = r.tick(show, engine, st, Instant::now()) {
                    let Phase::Full(r) = std::mem::replace(&mut self.phase, Phase::Idle) else {
                        return;
                    };
                    match &mut self.fleet {
                        Some(f) => {
                            engine.stop_all(0.0);
                            f.end(now);
                            self.phase = Phase::Closing(Box::new(Closing {
                                what: Closed::Full(r),
                                end,
                                since: now,
                            }));
                        }
                        None => {
                            self.phase = Phase::Done(Box::new(super::full::finish(
                                *r, end, show, engine, st, None,
                            )))
                        }
                    }
                }
            }
        }
    }

    /// The program is closing: end a test properly so the show is back and
    /// the files are complete.
    pub fn shutdown(&mut self, show: &mut Show, engine: &mut Engine, st: &mut UiState) {
        // Nothing is kept aside for a next part: the show comes back now.
        st.test_carry.keep = false;
        if let Some(s) = &mut self.seq {
            s.quit = true;
        }
        self.shutdown_part(show, engine, st);
        if self.seq.is_some() {
            self.sequence_step(show, engine, st);
        }
        if self.seq.is_some() || st.holds_test() {
            self.end_sequence(show, engine, st, Some("NewQ was closed".into()));
        }
    }

    fn shutdown_part(&mut self, show: &mut Show, engine: &mut Engine, st: &mut UiState) {
        // The computers are told it is over (their show comes back with ours).
        let now = crate::net::time::local_now();
        let mut fleet = self.fleet.take();
        if let Some(f) = &mut fleet {
            f.end(now);
            self.net_out.append(&mut f.out);
        }
        let fleet = fleet.map(|f| (f, now));
        match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Running(r) => {
                self.phase = Phase::Done(Box::new(finish(*r, End::Quit, show, engine, st, fleet)));
            }
            Phase::Spreading(mut s) => {
                if let Some(stash) = s.stash.take() {
                    st.end_test(show, engine, stash);
                }
            }
            Phase::Closing(c) => {
                let Closing { what, end, .. } = *c;
                self.phase = Phase::Done(Box::new(match what {
                    Closed::Level(r) => finish(*r, end, show, engine, st, fleet),
                    Closed::Full(r) => super::full::finish(*r, end, show, engine, st, fleet),
                }));
            }
            Phase::Comparing(c) => {
                let (elapsed, folder) = (c.elapsed(), c.folder.clone());
                let (result, findings, report, end) = c.finish(End::Quit, show, engine, st);
                self.phase = Phase::Done(Box::new(compare_done(
                    result, findings, report, end, folder, elapsed,
                )));
            }
            Phase::Preparing(p) => p.fetch.cancel(),
            Phase::FullPrep(p) => p.cancel(),
            Phase::NetPrep(p) => p.cancel(),
            Phase::Network(mut r) => {
                // The computers are told it is over; the show comes back here.
                r.fleet.end(now);
                self.net_out.append(&mut r.fleet.out);
                self.phase = Phase::Done(Box::new(super::network::finish(
                    *r,
                    End::Quit,
                    show,
                    engine,
                    st,
                    crate::net::time::local_now(),
                )));
            }
            Phase::Full(r) => {
                self.phase = Phase::Done(Box::new(super::full::finish(
                    *r,
                    End::Quit,
                    show,
                    engine,
                    st,
                    fleet,
                )));
            }
            other => self.phase = other,
        }
    }

    /// Put a test's show in place and send it, with its media, to the
    /// computers of the show network; it begins once they are all ready.
    #[allow(clippy::too_many_arguments)]
    fn spread(
        &mut self,
        then: Then,
        test_show: Show,
        files: Vec<(String, u64)>,
        name: String,
        muted: bool,
        show: &mut Show,
        engine: &mut Engine,
        st: &mut UiState,
    ) {
        let mut fleet = super::fleet::Fleet::new(st, files, muted);
        fleet.test = self.test_id;
        log::info!(
            "test mode: the {name} test goes to {} first ({} files)",
            fleet.names(),
            fleet.files.len()
        );
        let stash = st.begin_test(show, engine, test_show);
        self.fleet = Some(Box::new(fleet));
        self.phase = Phase::Spreading(Box::new(Spread {
            then,
            stash: Some(stash),
            started: Instant::now(),
            name,
            stop: false,
        }));
    }

    /// Start a test across the network with a show already made (the tests
    /// of the test): it is put in place and waits for the computers.
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn launch_across(
        &mut self,
        ff: Ffmpeg,
        built: plan::Built,
        log_dir: &Path,
        level: Level,
        show: &mut Show,
        engine: &mut Engine,
        st: &mut UiState,
    ) {
        let prep = Prep {
            fetch: assets::start(ff, assets::cache_dir(log_dir), Vec::new(), false),
            compare: false,
            displays: Vec::new(),
            sound: SoundChoice::default(),
            heavy: false,
            level,
            flavour: Flavour::Cues,
            minutes: 0,
            wanted: Vec::new(),
            log_dir: log_dir.to_path_buf(),
        };
        let files = super::fleet::files_of(&built.show, None);
        let test_show = built.show.clone();
        self.spread(
            Then::Level {
                prep: Box::new(prep),
                built: Box::new(built),
            },
            test_show,
            files,
            level.label().into(),
            false,
            show,
            engine,
            st,
        );
    }
}

/// Whether a test started here runs across a show network: this computer
/// runs one, with computers connected to it.
pub fn across(st: &UiState) -> bool {
    st.net.is_primary() && st.net.peers.iter().any(|p| p.ours && p.connected)
}

/// The test show for `prep`, from the user's `show`; `spread` across a show
/// network (every chosen display shows each picture, `plan::spread_pictures`).
fn build(prep: &Prep, show: &Show, spread: bool) -> plan::Built {
    let resolved = prep.fetch.status.lock().resolved.clone();
    let mut built = match prep.flavour {
        Flavour::Cues => plan::build_with_as(
            show,
            prep.level,
            &resolved,
            &prep.displays,
            &prep.sound,
            prep.heavy,
            !spread,
        ),
        Flavour::Timeline => {
            plan::build_timeline(show, prep.level, &resolved, &prep.displays, &prep.sound)
        }
    };
    if spread {
        plan::spread_pictures(&mut built.show);
    }
    built
}

// ----- Starting -------------------------------------------------------------------

fn utc_stamp(secs: i64) -> String {
    let (y, mo, d, h, mi) = crate::export::civil_from_unix(secs);
    format!(
        "{y:04}-{mo:02}-{d:02}_{h:02}-{mi:02}-{:02}",
        secs.rem_euclid(60)
    )
}

pub(super) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The folder a run's files go in.
pub fn run_folder(log_dir: &Path, level: Level, flavour: Flavour, secs: i64) -> PathBuf {
    run_folder_named(
        log_dir,
        &flavour.name(level).to_lowercase().replace(' ', "-"),
        secs,
    )
}

pub(super) fn run_folder_named(log_dir: &Path, name: &str, secs: i64) -> PathBuf {
    log_dir
        .join("test-mode")
        .join("runs")
        .join(format!("{}_{name}", utc_stamp(secs)))
}

/// A test's zip: made now, or, for a part of a longer run, with the whole
/// run's (`sequence`).
pub(super) fn part_bundle(
    st: &UiState,
    folder: PathBuf,
    log_dir: PathBuf,
    wait: bool,
) -> super::bundle::Bundle {
    if st.test_carry.parts_run {
        super::bundle::Bundle::with_the_run()
    } else {
        super::bundle::Bundle::start(folder, log_dir, wait, super::bundle::TIDY)
    }
}

fn compare_done(
    result: CompareResult,
    findings: Vec<Finding>,
    report: String,
    end: End,
    folder: PathBuf,
    elapsed: Duration,
) -> Done {
    use super::decode::Advice;
    let verdict = Verdict::of(&findings);
    Done {
        parts: Vec::new(),
        full: None,
        network: None,
        computers: Vec::new(),
        flavour: Flavour::Cues,
        bundle: super::bundle::Bundle::start(
            folder.clone(),
            crate::crash::log_dir(),
            matches!(end, End::Quit),
            super::bundle::TIDY,
        ),
        level: Level::Short,
        verdict,
        headline: match &end {
            End::Failed(why) => format!("The comparison could not go on: {why}"),
            _ => match result.advice {
                Advice::TurnOn => "Hardware decoding helps here.".into(),
                Advice::LeaveOff => "Leave hardware decoding off.".into(),
                Advice::NoDifference => "It makes little difference here.".into(),
                Advice::Inconclusive => "Nothing could be concluded.".into(),
            },
        },
        compare: Some(result),
        steady: None,
        findings,
        folder,
        report,
        laps: 0,
        elapsed,
        ended: end,
    }
}

/// Start a test; `placed`, across a show network, is the user's show put
/// aside and the test show already in place.
fn launch(
    prep: Prep,
    placed: Option<(TestStash, plan::Built)>,
    show: &mut Show,
    engine: &mut Engine,
    st: &mut UiState,
) -> Result<Run, String> {
    let resolved = prep.fetch.status.lock().resolved.clone();
    let (stash, built) = match placed {
        Some((stash, built)) => (Some(stash), built),
        None => {
            if let Some(why) = st.test_refusal() {
                return Err(why);
            }
            (None, build(&prep, show, false))
        }
    };
    let name = prep.flavour.name(prep.level);
    let folder = run_folder(&prep.log_dir, prep.level, prep.flavour, unix_now());
    std::fs::create_dir_all(&folder).map_err(|e| {
        format!(
            "Can't make the folder for the test's log files ({}): {e}. Nothing was changed.",
            folder.display()
        )
    })?;
    let log_path = prep.log_dir.join("newq.log");
    let stats = StatsLog::create(
        &folder.join("stats.csv"),
        Duration::from_secs(prep.level.stats_every()),
        log_path.clone(),
    )
    .map_err(|e| {
        format!(
            "Can't write the test's log in {}: {e}. Nothing was changed.",
            folder.display()
        )
    })?;
    let open = |name: &str| {
        std::fs::File::create(folder.join(name))
            .ok()
            .map(std::io::BufWriter::new)
    };
    let mut lap_file = open("laps.csv");
    if let Some(f) = &mut lap_file {
        let _ = writeln!(f, "lap,started_s,took_s,should_take_s");
    }
    let events = open("events.log");
    let planned = (built.repeats && prep.minutes > 0)
        .then(|| Duration::from_secs(u64::from(prep.minutes) * 60));
    let _ = std::fs::write(
        folder.join("system.txt"),
        system_report(
            engine,
            st,
            show,
            &prep.wanted,
            &resolved,
            &prep.displays,
            &prep.sound,
            prep.heavy,
        ),
    );
    let media_notes = built.notes.clone();
    let awake = KeepAwake::new("NewQ is running a test");
    // The point of no return: the user's show is put aside.
    let stash = match stash {
        Some(s) => s,
        None => st.begin_test(show, engine, built.show),
    };
    crate::media::audio::set_muted(prep.sound.muted);
    engine.set_log_tap(true);
    engine.log(format!("Test mode: the sound {}", prep.sound.describe()));
    engine.log(format!(
        "Test mode: {} test begins ({} cues from {} media files, a lap should take {:.0} s)",
        name,
        built.cues,
        built.used.len(),
        built.lap_secs
    ));
    engine.log(format!(
        "Test mode: the computer is {} ({})",
        if awake.held() {
            "being kept awake"
        } else {
            "NOT being kept awake: it may sleep and spoil the test"
        },
        awake.how()
    ));
    let timeline = built
        .timeline
        .clone()
        .map(|script| Box::new(TimelineRun::new(script, built.repeats, &folder)));
    // A timeline test is started by its driver, at its first look.
    if timeline.is_none() {
        engine.go(show);
    }
    let started = Instant::now();
    Ok(Run {
        level: prep.level,
        flavour: prep.flavour,
        timeline,
        heavy: prep.heavy,
        started,
        planned,
        folder,
        history: Vec::new(),
        last: None,
        laps: Vec::new(),
        lap_starts: Vec::new(),
        lap_secs: built.lap_secs,
        findings: Vec::new(),
        media_notes,
        first: built.first,
        repeats: built.repeats,
        stash: Some(stash),
        stats,
        events,
        lap_file,
        lap_started_at: None,
        overrun_logged: false,
        first_playing: false,
        idle_since: None,
        messages: Vec::new(),
        stop: false,
        resolved,
        wanted: prep.wanted,
        last_analysis: Instant::now(),
        finding_log: FindingLog::default(),
        awake,
        sleep_watch: SleepWatch::new(),
        sleeps: Vec::new(),
        log_path,
        freeze_files: Vec::new(),
        memory: MemoryWatch::new(),
        hitches: super::hitches::Hitches::new(started),
        sound_device: super::hitches::working_device(engine),
        drawing: Vec::new(),
        drawing_looked: None,
        software: None,
    })
}

impl Run {
    /// What has been found so far (`hours` into the run).
    fn judge(&self, hours: f64) -> Vec<Finding> {
        let clean = self.clean_laps();
        let mut f = super::analysis::for_load(
            analyse(&self.history, &clean, self.lap_secs, &self.messages, hours),
            self.heavy,
        );
        if let Some(tl) = &self.timeline {
            f.extend(super::analysis::analyse_timeline(&tl.watch.notes, &clean));
        }
        f.extend(self.hitches.findings(hours, self.sound_device.as_deref()));
        f.extend(super::net_judge::software_finding(
            "This computer",
            self.software.as_ref(),
        ));
        f.extend(super::net_judge::drawing_findings("This computer", &self.drawing).1);
        f
    }

    /// A lap has begun, `t` seconds into the test.
    fn lap_began(&mut self, t: f64) {
        self.lap_started_at = Some(t);
        self.lap_starts.push(t);
        self.overrun_logged = false;
    }

    /// The lap under way has ended, `t` seconds into the test.
    fn lap_ended(&mut self, t: f64) {
        let Some(prev) = self.lap_started_at.take() else {
            return;
        };
        let took = t - prev;
        self.laps.push(took);
        if let Some(f) = &mut self.lap_file {
            let _ = writeln!(
                f,
                "{},{prev:.1},{took:.1},{:.1}",
                self.laps.len(),
                self.lap_secs
            );
            let _ = f.flush();
        }
        if self.lap_secs > 0.0 && took > self.lap_secs * 1.25 + 3.0 {
            let line = format!(
                "Lap {} took {took:.0} s; it should take {:.0} s",
                self.laps.len(),
                self.lap_secs
            );
            self.log_at(t, &line);
        }
    }

    /// One look at the test. Returns why it should end, if it should.
    fn tick(&mut self, show: &Show, engine: &mut Engine, now: Instant) -> Option<End> {
        let elapsed = now.duration_since(self.started);
        // Did the computer sleep since the last look? The test's clock stops
        // while it sleeps, so only the wall clock shows it.
        if let Some(gap) = self.sleep_watch.check(now, std::time::SystemTime::now()) {
            let t = elapsed.as_secs_f64();
            self.sleeps.push((t, gap.as_secs_f64()));
            let line = format!(
                "The computer was asleep or suspended for about {} (the test's own clock does not count it; the show was stopped for that time)",
                clock(gap)
            );
            self.log_at(t, &line);
            if let Some(tl) = &mut self.timeline {
                tl.watch.slept();
            }
        }
        let mut stuck = None;
        let mut lap_over = false;
        if let Some(mut tl) = self.timeline.take() {
            // A timeline test: its driver plays it and says when laps begin and end.
            let t = elapsed.as_secs_f64();
            for h in tl.driver.tick(show, engine, now) {
                match h {
                    super::timeline::Happened::LapBegan => self.lap_began(t),
                    super::timeline::Happened::LapEnded => {
                        self.lap_ended(t);
                        lap_over = true;
                    }
                    super::timeline::Happened::Said(line) => self.log_at(t, &line),
                    super::timeline::Happened::Stuck(why) => stuck = Some(why),
                }
            }
            let keys = tl.driver.script.legs.clone();
            let lap = tl.driver.laps;
            tl.watch.tick(show, engine, &keys, now, t, lap);
            tl.write();
            self.timeline = Some(tl);
        } else {
            // A lap begins whenever the first cue starts.
            let playing = matches!(engine.status(self.first), CueStatus::Playing { .. });
            if playing && !self.first_playing {
                let t = elapsed.as_secs_f64();
                if self.lap_started_at.is_some() {
                    self.lap_ended(t);
                }
                self.lap_began(t);
            }
            self.first_playing = playing;
        }
        // A lap that is running long: what is playing now is what holds it up.
        if let Some(began) = self.lap_started_at
            && self.lap_secs > 0.0
            && !self.overrun_logged
            && elapsed.as_secs_f64() - began > self.lap_secs * 1.25 + 3.0
        {
            self.overrun_logged = true;
            let names: Vec<String> = engine
                .active
                .iter()
                .filter_map(|a| show.cue(a.cue_id))
                .map(|c| format!("{} {}", c.number, c.name))
                .collect();
            let line = format!(
                "Lap {} has been running {:.0} s of the {:.0} s it should take. Playing now: {}",
                self.laps.len() + 1,
                elapsed.as_secs_f64() - began,
                self.lap_secs,
                if names.is_empty() {
                    "nothing".to_string()
                } else {
                    names.join("; ")
                }
            );
            let t = elapsed.as_secs_f64();
            self.log_at(t, &line);
        }

        // The engine's own account of what happens, into the events file.
        for (at, text) in engine.take_log_tap() {
            let rel = at.saturating_duration_since(self.started);
            if let Some(f) = &mut self.events {
                let _ = writeln!(f, "[{:>9.3}] {text}", rel.as_secs_f64());
                let _ = f.flush();
            }
            self.messages.push(text);
        }

        if let Some(sample) = self.stats.frame(now, || Inputs {
            playing: engine.active.len(),
            started_total: engine.started_total(),
            layers: engine.picture_layers(),
            late_permille: crate::render::late_frames_permille(),
            audio_permille: engine
                .audio
                .device_status(None)
                .map_or(0, |d| d.load_permille),
            audio_errors: crate::media::audio::stream_errors(),
        }) {
            self.last = Some(sample);
            self.history.push(sample);
        }
        for (t, line) in self.hitches.tick(now) {
            self.log_at(t, &line);
        }
        // What has been found so far, for the window.
        if now.duration_since(self.last_analysis) > Duration::from_secs(5) {
            self.last_analysis = now;
            if self.sound_device.is_none() {
                self.sound_device = super::hitches::working_device(engine);
            }
            self.findings = self.judge(elapsed.as_secs_f64() / 3600.0);
            let t = elapsed.as_secs_f64();
            for line in self.finding_log.update(t, &self.findings) {
                self.log_at(t, &line);
            }
        }

        if let Some(mb) = self.memory.over(now) {
            // What was playing, for working out which cue runs away.
            let names: Vec<String> = engine
                .active
                .iter()
                .filter_map(|a| show.cue(a.cue_id))
                .map(|c| format!("{} {}", c.number, c.name))
                .collect();
            engine.log(format!(
                "Test mode: memory at {mb} MB, over the limit of {} MB. Playing: {}",
                self.memory.limit_mb,
                names.join("; ")
            ));
            return Some(End::Failed(format!(
                "NewQ and its FFmpeg processes were using {:.1} GB of this computer's {:.0} GB of memory, so the test was stopped before it made the computer unusable",
                mb as f64 / 1024.0,
                self.memory.ram_mb as f64 / 1024.0
            )));
        }
        if self.stop {
            return Some(End::Stopped);
        }
        if let Some(why) = stuck {
            return Some(End::Failed(why));
        }
        if self.timeline.is_some() {
            if !self.repeats && lap_over {
                return Some(End::Finished);
            }
            if self.planned.is_some_and(|p| elapsed >= p) {
                return Some(End::Finished);
            }
            // A lap that has gone on far too long.
            if let Some(began) = self.lap_started_at
                && elapsed.as_secs_f64() - began > self.lap_secs * 3.0 + 60.0
            {
                return Some(End::Failed(
                    "a lap took more than three times as long as it should".into(),
                ));
            }
            return None;
        }
        let active = engine.is_active();
        if active {
            self.idle_since = None;
        } else {
            self.idle_since.get_or_insert(now);
        }
        let idle_for = self
            .idle_since
            .map_or(Duration::ZERO, |t| now.duration_since(t));
        if self.repeats {
            if self.planned.is_some_and(|p| elapsed >= p) {
                return Some(End::Finished);
            }
            if elapsed > Duration::from_secs(3) && idle_for > Duration::from_secs(8) {
                return Some(End::Failed(format!(
                    "the show stopped by itself after {} laps",
                    self.laps.len()
                )));
            }
        } else {
            if elapsed > Duration::from_secs(5) && idle_for > Duration::from_secs(2) {
                return Some(End::Finished);
            }
            // A single lap that has gone on far too long.
            if elapsed.as_secs_f64() > self.lap_secs * 3.0 + 60.0 {
                return Some(End::Failed(
                    "the lap took more than three times as long as it should".into(),
                ));
            }
        }
        None
    }
}

// ----- Ending -------------------------------------------------------------------------

fn finish(
    mut run: Run,
    end: End,
    show: &mut Show,
    engine: &mut Engine,
    st: &mut UiState,
    fleet: Option<(Box<super::fleet::Fleet>, f64)>,
) -> Done {
    let elapsed = run.elapsed();
    engine.stop_all(0.0);
    // The user's show comes back before anything else is done.
    if let Some(stash) = run.stash.take() {
        st.end_test(show, engine, stash);
    }
    crate::media::audio::set_muted(false);
    for (at, text) in engine.take_log_tap() {
        let rel = at.saturating_duration_since(run.started);
        if let Some(f) = &mut run.events {
            let _ = writeln!(f, "[{:>9.3}] {text}", rel.as_secs_f64());
        }
        run.messages.push(text);
    }
    engine.set_log_tap(false);
    if let Some(f) = &mut run.events {
        let _ = writeln!(
            f,
            "[{:>9.3}] Test mode: ended ({end:?})",
            elapsed.as_secs_f64()
        );
        let _ = f.flush();
    }

    // The lap that was under way when the test ended counts if it was the last.
    if !run.repeats
        && matches!(end, End::Finished)
        && let Some(began) = run.lap_started_at.take()
    {
        let took = elapsed.as_secs_f64() - began;
        run.laps.push(took);
        if let Some(f) = &mut run.lap_file {
            let _ = writeln!(f, "1,{began:.1},{took:.1},{:.1}", run.lap_secs);
            let _ = f.flush();
        }
    }
    for (t, line) in run.hitches.tick(Instant::now()) {
        run.log_at(t, &line);
    }
    let hours = elapsed.as_secs_f64() / 3600.0;
    let clean = run.clean_laps();
    let mut findings = run.judge(hours);
    if !run.sleeps.is_empty() {
        let total: f64 = run.sleeps.iter().map(|(_, g)| g).sum();
        findings.insert(
            0,
            Finding {
                severity: Severity::Problem,
                text: format!(
                    "The computer went to sleep {} time{} during the test, for about {} in all (the first {} into the test){}. The show stops while it sleeps, so the {} lap{} it fell in {} left out of the lap checks, and the figures around those times are not a fair measure. Run the test again with the computer set not to sleep.",
                    run.sleeps.len(),
                    if run.sleeps.len() == 1 { "" } else { "s" },
                    clock(Duration::from_secs_f64(total)),
                    clock(Duration::from_secs_f64(run.sleeps[0].0)),
                    if run.awake.held() {
                        format!(", although NewQ asked the system to stay awake ({})", run.awake.how())
                    } else {
                        ", and NewQ could not keep it awake".to_string()
                    },
                    run.laps.len() - clean.len(),
                    if run.laps.len() - clean.len() == 1 { "" } else { "s" },
                    if run.laps.len() - clean.len() == 1 { "is" } else { "are" },
                ),
            },
        );
    }
    match &end {
        End::Failed(why) => findings.insert(
            0,
            Finding {
                severity: Severity::Problem,
                text: format!("The test could not go on: {why}."),
            },
        ),
        End::Stopped | End::Quit if run.repeats => findings.push(Finding {
            severity: Severity::Note,
            text: match &run.planned {
                Some(p) => format!(
                    "The test was stopped after {} of the {} planned.",
                    clock(elapsed),
                    clock(*p)
                ),
                None => "The test was stopped early.".into(),
            },
        }),
        End::Stopped | End::Quit => findings.push(Finding {
            severity: Severity::Note,
            text: "The test was stopped before its lap was done.".into(),
        }),
        End::Finished => {}
    }
    let peak = run
        .history
        .iter()
        .map(|x| x.rss_mb)
        .max()
        .unwrap_or(0)
        .max(run.memory.peak_mb());
    if run.memory.ram_mb > 0 && peak * 4 > run.memory.ram_mb && !matches!(end, End::Failed(_)) {
        findings.push(Finding {
            severity: Severity::Problem,
            text: format!(
                "NewQ's memory reached {:.1} GB ({:.0}% of this computer's {:.0} GB). A show should stay far below that.",
                peak as f64 / 1024.0,
                peak as f64 * 100.0 / run.memory.ram_mb as f64,
                run.memory.ram_mb as f64 / 1024.0
            ),
        });
    }
    let generated: Vec<String> = run
        .wanted
        .iter()
        .filter_map(|a| match run.resolved.get(a.id).map(|r| &r.source) {
            Some(Source::Generated(why)) => Some(format!("{} ({why})", a.title)),
            _ => None,
        })
        .collect();
    if !generated.is_empty() {
        findings.push(Finding {
            severity: Severity::Note,
            text: format!(
                "{} of the media files were made on this computer instead of downloaded, so pictures and sound are plainer than the real ones. {}",
                generated.len(),
                generated.join("; ")
            ),
        });
    }
    let mut seen = std::collections::HashSet::new();
    for n in run.media_notes.iter().filter(|n| seen.insert(n.as_str())) {
        findings.push(Finding {
            severity: Severity::Note,
            text: n.clone(),
        });
    }
    // What the window showed along the way and is not in the final account:
    // kept in the log and the report, since someone watching saw it.
    let t_end = elapsed.as_secs_f64();
    for line in run.finding_log.close(t_end, &findings) {
        run.log_at(t_end, &line);
    }
    // What the program was doing while it froze, if it was looked at (Mac).
    let since = std::time::SystemTime::now()
        .checked_sub(elapsed + Duration::from_secs(5))
        .unwrap_or(std::time::UNIX_EPOCH);
    run.freeze_files = crate::freeze::copy_samples(&crate::crash::log_dir(), &run.folder, since);
    if !run.freeze_files.is_empty() {
        findings.push(Finding {
            severity: Severity::Note,
            text: format!(
                "While the program was frozen it was looked at to see what it was waiting for: {} in the test's folder.",
                run.freeze_files.join(", ")
            ),
        });
    }
    let gone = run.finding_log.gone(&findings);
    findings.extend(gone);
    // Across a show network: each computer's part.
    let (computers, part) = super::fleet::fold_in(fleet, &mut findings, &run.folder, &st.net);
    let verdict = Verdict::of(&findings);
    let mut report = compose(&run, &end, elapsed, verdict, &findings, show);
    report.push_str(&part);
    let _ = std::fs::write(run.folder.join("report.txt"), &report);
    let _ = std::fs::write(
        run.folder.join("media-credits.txt"),
        assets::credits(&run.wanted),
    );
    excerpt_of_log(&run.log_path, &run.folder.join("newq-log-excerpt.txt"));
    let headline = match (&end, verdict) {
        (End::Failed(why), _) => format!("The test could not go on: {why}"),
        (_, Verdict::Passed) => "Everything held up.".into(),
        (_, Verdict::PassedWithNotes) => "Nothing went wrong; there are notes.".into(),
        (_, Verdict::ProblemsFound) => "Problems were found.".into(),
    };
    Done {
        parts: Vec::new(),
        compare: None,
        full: None,
        network: None,
        computers,
        flavour: run.flavour,
        bundle: part_bundle(
            st,
            run.folder.clone(),
            run.log_path
                .parent()
                .map_or_else(crate::crash::log_dir, Path::to_path_buf),
            matches!(end, End::Quit),
        ),
        level: run.level,
        verdict,
        headline,
        steady: super::analysis::steady_line(&findings, run.history.len(), hours),
        findings,
        folder: run.folder.clone(),
        report,
        laps: run.laps.len(),
        elapsed,
        ended: end,
    }
}

/// The last part of the program's own log, beside the test's files.
fn excerpt_of_log(log: &Path, to: &Path) {
    let Ok(text) = std::fs::read(log) else {
        return;
    };
    let keep = 2 * 1024 * 1024;
    let from = text.len().saturating_sub(keep);
    let _ = std::fs::write(to, &text[from..]);
}

/// Those of `laps` (each beginning at the same index of `starts`) that no
/// sleep, noted as (when, how long), happened in.
fn undisturbed(starts: &[f64], laps: &[f64], sleeps: &[(f64, f64)]) -> Vec<f64> {
    laps.iter()
        .enumerate()
        .filter(|(i, took)| {
            let from = starts.get(*i).copied().unwrap_or(0.0);
            !sleeps
                .iter()
                .any(|(t, _)| *t >= from && *t <= from + **took)
        })
        .map(|(_, took)| *took)
        .collect()
}

fn lap_summary(laps: &[f64], nominal: f64) -> String {
    if laps.is_empty() {
        return "No lap was finished.".into();
    }
    let min = laps.iter().copied().fold(f64::MAX, f64::min);
    let max = laps.iter().copied().fold(0.0, f64::max);
    let mean = laps.iter().sum::<f64>() / laps.len() as f64;
    format!(
        "{} laps finished; each should take {nominal:.0} s. Fastest {min:.1} s, average {mean:.1} s, slowest {max:.1} s.",
        laps.len()
    )
}

fn compose(
    run: &Run,
    end: &End,
    elapsed: Duration,
    verdict: Verdict,
    findings: &[Finding],
    show: &Show,
) -> String {
    let mut out = String::new();
    let line = |out: &mut String, s: &str| {
        out.push_str(s);
        out.push('\n');
    };
    line(&mut out, "NewQ Test Mode report");
    line(&mut out, "=====================");
    line(&mut out, &format!("Result: {}", verdict.label()));
    line(
        &mut out,
        &format!(
            "Test: {} ({}), ran {}. Ended: {}.",
            run.flavour.name(run.level),
            match &run.planned {
                Some(p) => format!("{} planned", clock(*p)),
                None => "one lap".into(),
            },
            clock(elapsed),
            match end {
                End::Finished => "it ran its course".to_string(),
                End::Stopped => "stopped by you".to_string(),
                End::Failed(w) => format!("could not go on ({w})"),
                End::Quit => "NewQ was closed".to_string(),
            }
        ),
    );
    line(&mut out, &format!("NewQ {}", env!("CARGO_PKG_VERSION")));
    line(
        &mut out,
        "Your show was put aside while it ran and is back, unchanged.",
    );
    line(&mut out, "");
    line(&mut out, "What was found");
    line(&mut out, "--------------");
    if findings.is_empty() {
        line(&mut out, "  Nothing to report.");
    }
    for f in findings {
        let tag = match f.severity {
            Severity::Problem => "PROBLEM",
            Severity::Note => "note   ",
            Severity::Watch => "tracked",
        };
        line(&mut out, &format!("  {tag}  {}", f.text));
    }
    if let Some(t) = super::analysis::steady_line(
        findings,
        run.history.len(),
        run.elapsed().as_secs_f64() / 3600.0,
    ) {
        line(&mut out, &format!("  ok       {t}"));
    }
    line(&mut out, "");
    line(&mut out, "Laps");
    line(&mut out, "----");
    line(
        &mut out,
        &format!("  {}", lap_summary(&run.laps, run.lap_secs)),
    );
    if let Some(tl) = &run.timeline {
        line(&mut out, "");
        line(&mut out, "Timelines");
        line(&mut out, "---------");
        line(
            &mut out,
            &format!(
                "  Played by pressing GO {} time{} at pauses (after {:.0} s each), leaving {} loop{} and seeking {} time{}.",
                tl.driver.presses,
                if tl.driver.presses == 1 { "" } else { "s" },
                tl.driver.script.hold,
                tl.driver.leaves,
                if tl.driver.leaves == 1 { "" } else { "s" },
                tl.driver.seeks,
                if tl.driver.seeks == 1 { "" } else { "s" },
            ),
        );
        for l in super::analysis::timeline_summary(&tl.watch.notes) {
            line(&mut out, &format!("  {l}"));
        }
    }
    line(&mut out, "");
    line(&mut out, "Measurements");
    line(&mut out, "------------");
    match std::fs::read_to_string(run.folder.join("stats.csv"))
        .map_err(|e| e.to_string())
        .and_then(|t| crate::stats::report(&t))
    {
        Ok(r) => {
            for l in r.lines().filter(|l| {
                !l.starts_with("Look at")
                    && !l.starts_with("  - ")
                    && !l.contains("Nothing looks wrong")
            }) {
                line(&mut out, &format!("  {l}"));
            }
        }
        Err(e) => line(&mut out, &format!("  (none: {e})")),
    }
    for l in run.hitches.summary(run.sound_device.as_deref()) {
        line(&mut out, &format!("  {l}"));
    }
    line(&mut out, "");
    line(&mut out, "Media");
    line(&mut out, "-----");
    for a in &run.wanted {
        let how = match run.resolved.get(a.id).map(|r| &r.source) {
            Some(Source::Cached) => "kept from an earlier run".to_string(),
            Some(Source::Downloaded) => "downloaded".to_string(),
            Some(Source::Generated(why)) => format!("made on this computer ({why})"),
            None => "not available".to_string(),
        };
        line(&mut out, &format!("  {}: {how}", a.title));
    }
    line(&mut out, "");
    line(
        &mut out,
        "Credits (the downloaded media is free to use under these terms)",
    );
    for l in assets::credits(&run.wanted).lines() {
        line(&mut out, &format!("  {l}"));
    }
    line(&mut out, "");
    line(&mut out, "Files in this folder");
    line(&mut out, "--------------------");
    for (f, what) in [
        ("report.txt", "this report"),
        ("stats.csv", "one row of measurements every few seconds"),
        ("laps.csv", "how long each lap took"),
        ("events.log", "what the show engine said, with times"),
        (
            "system.txt",
            "this computer, its displays, sound, FFmpeg and MediaMTX",
        ),
        ("newq-log-excerpt.txt", "the end of NewQ's own log"),
        ("media-credits.txt", "where the media came from"),
    ] {
        line(&mut out, &format!("  {f:22} {what}"));
    }
    if run.timeline.is_some() {
        line(
            &mut out,
            &format!(
                "  {:22} the timelines' clock, clips and cueing, as measured",
                "timeline.csv"
            ),
        );
    }
    for f in &run.freeze_files {
        line(
            &mut out,
            &format!("  {f:22} what the program was doing while it was frozen"),
        );
    }
    let _ = show;
    out
}

// ----- About this computer -------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub(super) fn system_report(
    engine: &Engine,
    st: &UiState,
    show: &Show,
    wanted: &[&'static Asset],
    resolved: &HashMap<&'static str, Resolved>,
    displays: &[DisplayChoice],
    sound: &SoundChoice,
    heavy: bool,
) -> String {
    use sysinfo::System;
    let mut sys = System::new();
    sys.refresh_memory();
    sys.refresh_cpu_all();
    let mut out = String::new();
    let mut p = |s: String| {
        out.push_str(&s);
        out.push('\n');
    };
    p(format!("NewQ {}", env!("CARGO_PKG_VERSION")));
    p(format!(
        "System: {} {}, kernel {}",
        System::name().unwrap_or_default(),
        System::os_version().unwrap_or_default(),
        System::kernel_version().unwrap_or_default()
    ));
    p(format!("Architecture: {}", std::env::consts::ARCH));
    p(format!(
        "Processor: {} ({} threads)",
        sys.cpus().first().map_or("unknown", |c| c.brand().trim()),
        sys.cpus().len()
    ));
    p(format!("Memory: {} MB", sys.total_memory() >> 20));
    p(format!(
        "Graphics: {}",
        crate::gpu::gpu_name().unwrap_or_else(|| "unknown".into())
    ));
    for m in &st.monitors {
        p(format!(
            "Display: {} {}x{} {}{}",
            m.title(),
            m.size.0,
            m.size.1,
            m.refresh_hz.map_or(String::new(), |h| format!("{h:.2} Hz")),
            if m.primary { " (primary)" } else { "" }
        ));
    }
    p(format!(
        "Outputs in your show: {} display(s)/sub display(s), {} audio output(s)",
        show.outputs.len(),
        show.audio_outputs.len()
    ));
    p(if displays.is_empty() {
        "Test shown on: the displays of your show (the main window's preview if it has none)"
            .to_string()
    } else {
        format!(
            "Test shown on: {}",
            displays
                .iter()
                .map(|d| d.label.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )
    });
    p(format!("Test sound: {}", sound.describe()));
    p(format!(
        "Test load: {}",
        if heavy {
            "heavier (three more full-screen blended layers with effects)"
        } else {
            "ordinary"
        }
    ));
    match engine.audio.device_status(None) {
        Some(d) => p(format!(
            "Sound: {} {} Hz, {} channels{}{}",
            d.name,
            d.sample_rate,
            d.channels,
            // (Known once it has played: the system's choice, NewQ asks for none.)
            if d.buffer_frames > 0 {
                format!(", {} frames a buffer", d.buffer_frames)
            } else {
                String::new()
            },
            d.error
                .map_or(String::new(), |e| format!(" (problem: {e})"))
        )),
        None => p("Sound: no output device".to_string()),
    }
    if let Some(ff) = &engine.ff {
        let mut version_cmd = std::process::Command::new(&ff.ffmpeg);
        crate::media::ffmpeg::no_console(&mut version_cmd);
        let version = version_cmd
            .arg("-version")
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .next()
                    .map(str::to_string)
            })
            .unwrap_or_default();
        p(format!("FFmpeg: {version}"));
        p(format!("  at {}", ff.ffmpeg.display()));
        p(format!(
            "  x264 encoder: {}, Hap encoder: {}",
            ff.has_encoder("libx264"),
            ff.has_encoder("hap")
        ));
    }
    match crate::mediamtx::locate() {
        Some(exe) => {
            let mut cmd = std::process::Command::new(&exe);
            cmd.arg("--version");
            crate::media::ffmpeg::no_console(&mut cmd);
            // (Not waited for without a limit: it is asked on the main loop.)
            let never = std::sync::atomic::AtomicBool::new(false);
            let limit = Instant::now() + Duration::from_secs(3);
            let version = crate::media::ffmpeg::output_until(&mut cmd, &never, Some(limit))
                .ok()
                .and_then(|o| {
                    String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .next()
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "(it did not say)".into());
            p(format!("MediaMTX: {version}"));
            p(format!("  at {}", exe.display()));
        }
        None => p("MediaMTX: not with this copy of NewQ".to_string()),
    }
    p(format!(
        "Streaming server allowed on this computer: {}",
        if st.prefs.allow_hosting { "yes" } else { "no" }
    ));
    p(format!("Log detail: {}", crate::logging::level().name()));
    p(format!(
        "Hardware video decoding: {}",
        if st.prefs.hw_decode { "on" } else { "off" }
    ));
    p(String::new());
    p("Media used".to_string());
    for a in wanted {
        match resolved.get(a.id) {
            Some(r) => p(format!(
                "  {} -> {} [{}, {}x{}, {:.1} s, {:.2} fps, {} ch audio] {}",
                a.id,
                r.path.display(),
                r.codec,
                r.width,
                r.height,
                r.duration,
                r.fps,
                r.audio_channels,
                r.source.label()
            )),
            None => p(format!("  {}: not available", a.id)),
        }
    }
    out
}

/// Show a folder in the system's file manager.
pub fn open_folder(path: &Path) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut cmd = std::process::Command::new("explorer");
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Can't open {}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(mut f: impl FnMut() -> bool, secs: u64) -> bool {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if f() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn watching(text: &str) -> Finding {
        Finding {
            severity: Severity::Watch,
            text: text.into(),
        }
    }

    #[test]
    fn a_finding_is_the_same_one_when_only_its_numbers_change() {
        assert_eq!(
            identity_of("Tracking: memory is higher (903 \u{2192} 1128)."),
            identity_of("Tracking: memory is higher (937 \u{2192} 1229).")
        );
        assert_ne!(
            identity_of("Tracking: memory 1"),
            identity_of("Tracking: handles 1")
        );
    }

    #[test]
    fn what_the_window_shows_is_logged_when_it_appears_and_when_it_goes_even_if_it_does() {
        let mut log = FindingLog::default();
        assert!(log.update(10.0, &[]).is_empty());
        let a = watching("Tracking: FFmpeg's memory is higher (903 \u{2192} 1128).");
        let lines = log.update(1800.0, std::slice::from_ref(&a));
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].starts_with("Showing in the window: Tracking: FFmpeg"),
            "{lines:?}"
        );
        // Recomputed with other numbers: still the same finding, nothing new said.
        let b = watching("Tracking: FFmpeg's memory is higher (937 \u{2192} 1229).");
        assert!(log.update(1805.0, std::slice::from_ref(&b)).is_empty());
        // It goes: said, with how long it was there.
        let lines = log.update(3000.0, &[]);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].starts_with("No longer showing (it had been there 20:00)"),
            "{lines:?}"
        );
        // And it is in the report though the final account is clean.
        let gone = log.gone(&[]);
        assert_eq!(gone.len(), 1);
        assert_eq!(gone[0].severity, Severity::Watch);
        assert!(gone[0].text.contains("30:00 to 50:00"), "{}", gone[0].text);
        assert!(gone[0].text.contains("cleared by itself"));
        // One the final account has in its own words is not repeated.
        assert!(log.gone(&[b]).is_empty());
        // Notes are not kept as history.
        let mut log = FindingLog::default();
        let note = Finding {
            severity: Severity::Note,
            text: "This test was too short".into(),
        };
        log.update(1.0, std::slice::from_ref(&note));
        log.update(2.0, &[]);
        assert!(log.gone(&[]).is_empty());
    }

    #[test]
    fn a_problem_that_gets_worse_is_logged_again_but_not_every_time() {
        let mut log = FindingLog::default();
        let p = |ms: u32| Finding {
            severity: Severity::Problem,
            text: format!("The program froze for {ms} ms once."),
        };
        assert_eq!(log.update(680.0, &[p(4810)]).len(), 1);
        // The same, or a hair more: nothing.
        assert!(log.update(685.0, &[p(4810)]).is_empty());
        assert!(log.update(690.0, &[p(4900)]).is_empty());
        // 5535 ms is clearly worse than 4810.
        let lines = log.update(2241.0, &[p(5535)]);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("now worse") && lines[0].contains("5535"));
        assert!(log.update(2250.0, &[p(5535)]).is_empty());
    }

    #[test]
    fn laps_a_sleep_fell_in_are_left_out() {
        // Nine ordinary laps and the ninth held up by sleeps: its start 1242 s,
        // 306 s long; sleeps noted at 1350 and 1400 s into the test.
        let starts = [
            0.0, 155.3, 310.5, 465.8, 621.0, 776.2, 931.5, 1086.8, 1242.0, 1548.6,
        ];
        let laps = [
            155.3, 155.2, 155.2, 155.2, 155.3, 155.3, 155.2, 155.2, 306.6, 155.3,
        ];
        let sleeps = [(1350.0, 300.0), (1400.0, 140.0)];
        let clean = undisturbed(&starts, &laps, &sleeps);
        assert_eq!(clean.len(), 9);
        assert!(clean.iter().all(|l| *l < 156.0), "{clean:?}");
        // No sleeps: all of them.
        assert_eq!(undisturbed(&starts, &laps, &[]).len(), 10);
    }

    #[test]
    fn the_memory_limit_is_half_the_memory_but_never_under_two_gigabytes() {
        assert_eq!(memory_limit_mb(24 * 1024), 12 * 1024);
        assert_eq!(memory_limit_mb(2048), 2048);
        assert_eq!(memory_limit_mb(0), 2048);
    }

    #[test]
    fn the_memory_watch_counts_the_processes_it_started() {
        let Some(ff) = Ffmpeg::locate() else { return };
        // A process that is busy and of a size worth counting.
        let mut child = ff
            .command()
            .args([
                "-f",
                "lavfi",
                "-i",
                "testsrc2=size=3840x2160:rate=30",
                "-t",
                "600",
                "-f",
                "null",
                "-",
            ])
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        let mut w = MemoryWatch::new();
        w.limit_mb = u64::MAX;
        // Going through every process takes seconds on a busy Windows machine
        // (it is why the watch has a thread of its own), so this waits for
        // looks that were finished, not for a time: three of them at most.
        let t = Instant::now();
        while w.children_mb() <= 5
            && w.seen.child_looks.load(Ordering::Acquire) < 3
            && t.elapsed() < Duration::from_secs(180)
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(w.over(Instant::now()), None);
        let counted = w.children_mb();
        let looks = w.seen.child_looks.load(Ordering::Acquire);
        let waited = t.elapsed();
        let _ = child.kill();
        let _ = child.wait();
        assert!(
            counted > 5,
            "the child's memory was not counted: {counted} MB after {looks} looks in {waited:?}"
        );
    }

    #[test]
    fn a_memory_watch_looks_on_its_own_thread_and_sees_its_own_process() {
        let mut w = MemoryWatch::new();
        let t = Instant::now();
        // Its first look goes through every process: seconds on a busy Windows machine.
        while w.peak_mb() == 0 && t.elapsed() < Duration::from_secs(120) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(w.peak_mb() > 0, "the process was found");
        assert!(w.over(Instant::now()).is_none() || w.limit_mb < 2048);
        w.limit_mb = 1;
        let began = Instant::now();
        assert!(w.over(Instant::now()).is_some());
        assert!(began.elapsed() < Duration::from_millis(20), "over() waited");
    }

    /// The comparison, offline: makes its 4K clip, plays both trials briefly,
    /// writes its report, and gives back the show and the setting.
    #[test]
    fn a_decoding_comparison_runs_reports_and_gives_everything_back() {
        let Some(ff) = Ffmpeg::locate() else {
            assert!(std::env::var("NEWQ_REQUIRE_MEDIA").is_err(), "no ffmpeg");
            return;
        };
        if !ff.has_encoder("libx264") {
            return;
        }
        let dir = std::env::temp_dir().join(format!("newq_tm_decode_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut show = Show::default();
        let c = show.make_cue(crate::show::CueType::Wait, None, 0);
        show.cues.push(c);
        let before = serde_json::to_string(&show).unwrap();
        let mut engine = Engine::for_tests();
        engine.ff = Some(ff.clone());
        let mut st = UiState::default();
        st.prefs.hw_decode = false;
        let mut test = TestMode {
            download: false,
            decode_secs: 3,
            ..Default::default()
        };
        test.start_compare(ff, &dir, Vec::new());
        let mut was_locked = false;
        let mut last = Instant::now();
        let ok = wait_for(
            || {
                let now = Instant::now();
                engine.tick(&show, now.duration_since(last).as_secs_f64());
                last = now;
                test.update(&mut show, &mut engine, &mut st);
                was_locked |= st.test_locked;
                matches!(test.view(), View::Done(_))
            },
            400,
        );
        let View::Done(d) = test.view() else {
            panic!("the comparison did not finish (ok={ok})");
        };
        let cmp = d.compare.as_ref().expect("a comparison's result");
        assert!(was_locked && !st.test_locked);
        assert_eq!(serde_json::to_string(&show).unwrap(), before);
        assert!(!st.prefs.hw_decode, "the setting is as it was");
        assert!(cmp.off.is_some() && cmp.on.is_some(), "{}", d.report);
        assert!(d.report.contains("Hardware decoding off"), "{}", d.report);
        for f in [
            "report.txt",
            "stats-hardware-off.csv",
            "stats-hardware-on.csv",
            "system.txt",
        ] {
            assert!(d.folder.join(f).exists(), "{f} missing");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn folders_are_named_by_time_and_level() {
        let p = run_folder(
            Path::new("/logs"),
            Level::Medium,
            Flavour::Cues,
            86_400 + 3_661,
        );
        assert_eq!(
            p,
            PathBuf::from("/logs/test-mode/runs/1970-01-02_01-01-01_medium")
        );
        assert_eq!(
            run_folder(Path::new("/l"), Level::Long, Flavour::Timeline, 0),
            PathBuf::from("/l/test-mode/runs/1970-01-01_00-00-00_timeline-long")
        );
        assert_eq!(clock(Duration::from_secs(3725)), "1:02:05");
        assert_eq!(clock(Duration::from_secs(65)), "1:05");
    }

    #[test]
    fn stopping_while_preparing_changes_nothing() {
        let Some(ff) = Ffmpeg::locate() else { return };
        let dir = std::env::temp_dir().join(format!("newq_tm_cancel_{}", std::process::id()));
        let mut test = TestMode {
            download: false,
            ..Default::default()
        };
        let (mut show, mut engine, mut st) =
            (Show::default(), Engine::for_tests(), UiState::default());
        let before = serde_json::to_string(&show).unwrap();
        test.start(ff, &dir, Vec::new(), SoundChoice::default(), false);
        test.stop();
        assert!(wait_for(
            || {
                test.update(&mut show, &mut engine, &mut st);
                !test.busy()
            },
            60
        ));
        assert!(!st.test_locked);
        assert_eq!(serde_json::to_string(&show).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The whole thing, offline: media is made here, the Short test runs one
    /// lap, files are written and the show comes back as it was.
    #[test]
    fn a_short_test_runs_logs_and_gives_the_show_back() {
        let _heavy = crate::testmode::heavy();
        let Some(ff) = Ffmpeg::locate() else {
            assert!(std::env::var("NEWQ_REQUIRE_MEDIA").is_err(), "no ffmpeg");
            return;
        };
        let dir = std::env::temp_dir().join(format!("newq_tm_short_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut show = Show::default();
        let c = show.make_cue(crate::show::CueType::Wait, None, 0);
        show.cues.push(c);
        let before = serde_json::to_string(&show).unwrap();
        let mut engine = Engine::for_tests();
        engine.ff = Some(ff.clone());
        let mut st = UiState::default();
        let mut test = TestMode {
            download: false,
            ..Default::default()
        };
        // Run muted, on a device the show names: the report says so, and the
        // mute is put back when the test is over.
        let sound = SoundChoice {
            device: Some("Some Interface".into()),
            muted: true,
        };
        test.start(ff, &dir, Vec::new(), sound, true);
        let mut was_locked = false;
        let mut last = Instant::now();
        let ok = wait_for(
            || {
                // The engine's clock is the real one: a loaded runner is slow, not wrong.
                let now = Instant::now();
                engine.tick(&show, now.duration_since(last).as_secs_f64());
                last = now;
                test.update(&mut show, &mut engine, &mut st);
                was_locked |= st.test_locked;
                matches!(test.view(), View::Done(_))
            },
            240,
        );
        let View::Done(d) = test.view() else {
            panic!("the test did not finish (ok={ok})");
        };
        assert!(was_locked && !st.test_locked);
        assert_eq!(serde_json::to_string(&show).unwrap(), before);
        for f in [
            "report.txt",
            "stats.csv",
            "laps.csv",
            "events.log",
            "system.txt",
        ] {
            assert!(d.folder.join(f).exists(), "{f} missing");
        }
        assert!(d.report.contains("back, unchanged"));
        assert!(!crate::media::audio::is_muted(), "the mute was left on");
        let system = std::fs::read_to_string(d.folder.join("system.txt")).unwrap();
        assert!(
            system.contains("Test sound: muted on purpose") && system.contains("Some Interface"),
            "{system}"
        );
        // This test shares its process with all the others, so memory, open
        // files and a stall on a loaded machine mean nothing here: what it
        // checks is that the show ran without the engine reporting a failure.
        assert!(
            !d.findings.iter().any(|f| f.severity == Severity::Problem
                && (f.text.contains("engine reported") || f.text.contains("could not go on"))),
            "{}",
            d.report
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stopping_a_timeline_test_while_preparing_changes_nothing() {
        let Some(ff) = Ffmpeg::locate() else { return };
        let dir = std::env::temp_dir().join(format!("newq_tm_tlcancel_{}", std::process::id()));
        let mut test = TestMode {
            download: false,
            ..Default::default()
        };
        test.choose(Level::Medium, Flavour::Timeline);
        let (mut show, mut engine, mut st) =
            (Show::default(), Engine::for_tests(), UiState::default());
        let before = serde_json::to_string(&show).unwrap();
        test.start(ff, &dir, Vec::new(), SoundChoice::default(), false);
        let View::Preparing(p) = test.view() else {
            panic!("not preparing")
        };
        assert_eq!(p.flavour, Flavour::Timeline);
        assert!(p.wanted.iter().all(|a| a.id != "bbb_trailer"));
        test.stop();
        assert!(wait_for(
            || {
                test.update(&mut show, &mut engine, &mut st);
                !test.busy()
            },
            60
        ));
        assert!(!st.test_locked);
        assert_eq!(serde_json::to_string(&show).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A timeline test stopped part way through its lap: the show comes back as
    /// it was, the run's files are there with the timeline's own, and the
    /// report says it was stopped.
    #[test]
    fn a_timeline_test_stopped_part_way_gives_the_show_back() {
        let _heavy = crate::testmode::heavy();
        let Some(ff) = Ffmpeg::locate() else {
            assert!(std::env::var("NEWQ_REQUIRE_MEDIA").is_err(), "no ffmpeg");
            return;
        };
        let dir = std::env::temp_dir().join(format!("newq_tm_tlstop_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut show = Show::default();
        let c = show.make_cue(crate::show::CueType::Wait, None, 0);
        show.cues.push(c);
        let l = show.add_list("Mine");
        show.lists.last_mut().unwrap().timeline = Some(Default::default());
        let before = serde_json::to_string(&show).unwrap();
        let mut engine = Engine::for_tests();
        engine.ff = Some(ff.clone());
        let mut st = UiState::default();
        let mut test = TestMode {
            download: false,
            ..Default::default()
        };
        test.choose(Level::Short, Flavour::Timeline);
        let sound = SoundChoice {
            device: None,
            muted: true,
        };
        test.start(ff, &dir, Vec::new(), sound, false);
        let mut last = Instant::now();
        let mut running_since = None;
        let mut saw_timeline_list = false;
        let ok = wait_for(
            || {
                let now = Instant::now();
                engine.tick(&show, now.duration_since(last).as_secs_f64());
                last = now;
                test.update(&mut show, &mut engine, &mut st);
                if let View::Running(r) = test.view() {
                    let since = *running_since.get_or_insert(now);
                    saw_timeline_list |= engine.active_list != 0
                        && show
                            .lists
                            .iter()
                            .any(|x| x.id == engine.active_list && x.timeline.is_some());
                    assert!(st.test_locked);
                    assert!(r.timeline.is_some());
                    if now.duration_since(since) > Duration::from_secs(8) {
                        test.stop();
                    }
                }
                matches!(test.view(), View::Done(_))
            },
            300,
        );
        let View::Done(d) = test.view() else {
            panic!("the test did not end (ok={ok})");
        };
        assert!(saw_timeline_list, "the timeline list was worked");
        assert_eq!(d.ended, End::Stopped);
        assert_eq!(d.flavour, Flavour::Timeline);
        assert!(!st.test_locked);
        assert_eq!(serde_json::to_string(&show).unwrap(), before);
        assert_eq!(engine.active_list, 0, "the lists are as they were");
        assert!(!crate::media::audio::is_muted());
        assert!(
            d.report.contains("Test: Timeline short") && d.report.contains("stopped by you"),
            "{}",
            d.report
        );
        assert!(
            d.report.contains("Timelines") && d.report.contains("Clips on time"),
            "{}",
            d.report
        );
        for f in ["report.txt", "timeline.csv", "events.log", "laps.csv"] {
            assert!(d.folder.join(f).exists(), "{f} missing");
        }
        let events = std::fs::read_to_string(d.folder.join("events.log")).unwrap();
        assert!(events.contains("Test mode: lap 1, GO"), "{events}");
        let _ = l;
        let _ = std::fs::remove_dir_all(&dir);
    }
}
