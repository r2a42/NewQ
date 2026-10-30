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
use super::plan::DisplayChoice;
use super::{Level, plan};
use crate::engine::{CueStatus, Engine};
use crate::media::ffmpeg::Ffmpeg;
use crate::show::{CueId, Show};
use crate::stats::{Inputs, Sample, StatsLog};
use crate::ui::{TestStash, UiState};
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Everything Test Mode keeps (a field of the interface state).
pub struct TestMode {
    /// The window is showing.
    pub open: bool,
    pub level: Level,
    /// How long the Medium and Long tests run, in minutes.
    pub minutes: u32,
    /// Download the real media (otherwise it is made on this computer).
    pub download: bool,
    /// Seconds measured in each trial of the decoding comparison.
    pub decode_secs: u64,
    /// Why the last Start did nothing, for the window.
    pub notice: Option<String>,
    phase: Phase,
}

impl Default for TestMode {
    fn default() -> TestMode {
        TestMode {
            open: false,
            level: Level::Short,
            minutes: Level::Short.default_minutes(),
            download: true,
            decode_secs: super::decode::TRIAL_SECS,
            notice: None,
            phase: Phase::Idle,
        }
    }
}

enum Phase {
    Idle,
    Preparing(Box<Prep>),
    Running(Box<Run>),
    Comparing(Box<Compare>),
    Done(Box<Done>),
}

/// The media being made ready.
pub struct Prep {
    pub fetch: Fetch,
    pub compare: bool,
    /// The displays the test is to be shown on (none: the show's own).
    pub displays: Vec<DisplayChoice>,
    pub level: Level,
    pub minutes: u32,
    pub wanted: Vec<&'static Asset>,
    log_dir: PathBuf,
}

/// A test under way.
pub struct Run {
    pub level: Level,
    pub started: Instant,
    /// How long it is meant to last (the Short test ends when its lap does).
    pub planned: Option<Duration>,
    pub folder: PathBuf,
    pub history: Vec<Sample>,
    pub last: Option<Sample>,
    /// Seconds each finished lap took.
    pub laps: Vec<f64>,
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
    first_playing: bool,
    idle_since: Option<Instant>,
    messages: Vec<String>,
    stop: bool,
    resolved: HashMap<&'static str, Resolved>,
    wanted: Vec<&'static Asset>,
    last_analysis: Instant,
    finding_log: FindingLog,
    log_path: PathBuf,
    memory: MemoryWatch,
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
/// them means looking at every process, which costs more).
pub(super) struct MemoryWatch {
    sys: sysinfo::System,
    me: sysinfo::Pid,
    limit_mb: u64,
    pub(super) ram_mb: u64,
    last: Instant,
    last_children: Instant,
    children_mb: u64,
    peak_mb: u64,
}

impl MemoryWatch {
    pub(super) fn new() -> MemoryWatch {
        let mut sys = sysinfo::System::new();
        sys.refresh_memory();
        let ram_mb = sys.total_memory() >> 20;
        MemoryWatch {
            sys,
            me: sysinfo::Pid::from_u32(std::process::id()),
            limit_mb: memory_limit_mb(ram_mb),
            ram_mb,
            last: Instant::now(),
            last_children: Instant::now(),
            children_mb: 0,
            peak_mb: 0,
        }
    }

    /// The memory of NewQ and its children in MB, if it is over the limit.
    pub(super) fn over(&mut self, now: Instant) -> Option<u64> {
        if now.duration_since(self.last) < Duration::from_secs(1) {
            return None;
        }
        self.last = now;
        if now.duration_since(self.last_children) >= Duration::from_secs(3) {
            self.last_children = now;
            self.sys
                .refresh_processes(sysinfo::ProcessesToUpdate::All, true);
            self.children_mb = self
                .sys
                .processes()
                .values()
                .filter(|p| p.thread_kind().is_none() && p.parent() == Some(self.me))
                .map(|p| p.memory() >> 20)
                .sum();
        } else {
            self.sys
                .refresh_processes(sysinfo::ProcessesToUpdate::Some(&[self.me]), true);
        }
        let mb = self.sys.process(self.me)?.memory() >> 20;
        self.peak_mb = self.peak_mb.max(mb);
        let total = mb + self.children_mb;
        (total > self.limit_mb).then_some(total)
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
}

fn identity_of(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_ascii_digit() && !matches!(c, '.' | '+' | '-' | '\u{2192}'))
        .collect()
}

/// What the window has shown during a run (see `Seen`).
#[derive(Default)]
struct FindingLog {
    seen: Vec<Seen>,
}

impl FindingLog {
    /// Take note of what the window is showing at `t` seconds. Returns the
    /// lines for `events.log`: one for each finding that has appeared, and for
    /// each that has gone (whether or not it lasted).
    fn update(&mut self, t: f64, current: &[Finding]) -> Vec<String> {
        let mut lines = Vec::new();
        for f in current {
            let id = identity_of(&f.text);
            match self
                .seen
                .iter_mut()
                .find(|s| s.ended.is_none() && s.identity == id)
            {
                Some(s) => s.text = f.text.clone(),
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
                    });
                }
            }
        }
        lines.extend(self.close(t, current));
        lines
    }

    /// Mark what is no longer among `current` as gone; returns the log lines.
    fn close(&mut self, t: f64, current: &[Finding]) -> Vec<String> {
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
    fn gone(&self, final_findings: &[Finding]) -> Vec<Finding> {
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
    pub level: Level,
    pub verdict: Verdict,
    pub headline: String,
    pub findings: Vec<Finding>,
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

    /// The lengths the window offers, in minutes (none for the Short test).
    pub fn lengths(self) -> &'static [u32] {
        match self {
            Level::Short => &[],
            Level::Medium => &[30, 60, 120],
            Level::Long => &[240, 480, 720],
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
            Phase::Done(d) => View::Done(d),
        }
    }

    /// Whether a test is being prepared or run (the window can't be changed).
    pub fn busy(&self) -> bool {
        matches!(
            self.phase,
            Phase::Preparing(_) | Phase::Running(_) | Phase::Comparing(_)
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

    /// Begin: make the media ready (downloading what is missing).
    pub fn start(&mut self, ff: Ffmpeg, log_dir: &Path, displays: Vec<DisplayChoice>) {
        if self.busy() {
            return;
        }
        let wanted = assets::wanted(self.level);
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
            level: self.level,
            minutes: self.minutes,
            wanted,
            log_dir: log_dir.to_path_buf(),
        }));
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
            level: self.level,
            minutes: 0,
            wanted,
            log_dir: log_dir.to_path_buf(),
        }));
    }

    /// Stop what is under way: cancel the preparation, or end the test.
    pub fn stop(&mut self) {
        match &mut self.phase {
            Phase::Preparing(p) => p.fetch.cancel(),
            Phase::Running(r) => r.stop = true,
            Phase::Comparing(c) => c.request_stop(),
            _ => {}
        }
    }

    /// Called every frame of the application.
    pub fn update(&mut self, show: &mut Show, engine: &mut Engine, st: &mut UiState) {
        match &mut self.phase {
            Phase::Idle | Phase::Done(_) => {}
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
                        system_report(engine, st, show, &p.wanted, &resolved, &p.displays),
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
                match launch(*p, show, engine, st) {
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
                    self.phase = Phase::Done(Box::new(finish(*r, end, show, engine, st)));
                }
            }
        }
    }

    /// The program is closing: end a test properly so the show is back and
    /// the files are complete.
    pub fn shutdown(&mut self, show: &mut Show, engine: &mut Engine, st: &mut UiState) {
        match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Running(r) => {
                self.phase = Phase::Done(Box::new(finish(*r, End::Quit, show, engine, st)));
            }
            Phase::Comparing(c) => {
                let (elapsed, folder) = (c.elapsed(), c.folder.clone());
                let (result, findings, report, end) = c.finish(End::Quit, show, engine, st);
                self.phase = Phase::Done(Box::new(compare_done(
                    result, findings, report, end, folder, elapsed,
                )));
            }
            Phase::Preparing(p) => p.fetch.cancel(),
            other => self.phase = other,
        }
    }
}

// ----- Starting -------------------------------------------------------------------

fn utc_stamp(secs: i64) -> String {
    let (y, mo, d, h, mi) = crate::export::civil_from_unix(secs);
    format!(
        "{y:04}-{mo:02}-{d:02}_{h:02}-{mi:02}-{:02}",
        secs.rem_euclid(60)
    )
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The folder a run's files go in.
pub fn run_folder(log_dir: &Path, level: Level, secs: i64) -> PathBuf {
    run_folder_named(log_dir, &level.label().to_lowercase(), secs)
}

fn run_folder_named(log_dir: &Path, name: &str, secs: i64) -> PathBuf {
    log_dir
        .join("test-mode")
        .join("runs")
        .join(format!("{}_{name}", utc_stamp(secs)))
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
        findings,
        folder,
        report,
        laps: 0,
        elapsed,
        ended: end,
    }
}

fn launch(
    prep: Prep,
    show: &mut Show,
    engine: &mut Engine,
    st: &mut UiState,
) -> Result<Run, String> {
    if let Some(why) = st.test_refusal() {
        return Err(why.to_string());
    }
    let resolved = prep.fetch.status.lock().resolved.clone();
    let built = plan::build(show, prep.level, &resolved, &prep.displays);
    let folder = run_folder(&prep.log_dir, prep.level, unix_now());
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
        system_report(engine, st, show, &prep.wanted, &resolved, &prep.displays),
    );
    let media_notes = built.notes.clone();
    // The point of no return: the user's show is put aside.
    let stash = st.begin_test(show, engine, built.show);
    engine.set_log_tap(true);
    engine.log(format!(
        "Test mode: {} test begins ({} cues from {} media files, a lap should take {:.0} s)",
        prep.level.label(),
        built.cues,
        built.used.len(),
        built.lap_secs
    ));
    engine.go(show);
    Ok(Run {
        level: prep.level,
        started: Instant::now(),
        planned,
        folder,
        history: Vec::new(),
        last: None,
        laps: Vec::new(),
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
        first_playing: false,
        idle_since: None,
        messages: Vec::new(),
        stop: false,
        resolved,
        wanted: prep.wanted,
        last_analysis: Instant::now(),
        finding_log: FindingLog::default(),
        log_path,
        memory: MemoryWatch::new(),
    })
}

impl Run {
    /// One look at the test. Returns why it should end, if it should.
    fn tick(&mut self, show: &Show, engine: &mut Engine, now: Instant) -> Option<End> {
        let elapsed = now.duration_since(self.started);
        // A lap begins whenever the first cue starts.
        let playing = matches!(engine.status(self.first), CueStatus::Playing { .. });
        if playing && !self.first_playing {
            let t = elapsed.as_secs_f64();
            if let Some(prev) = self.lap_started_at {
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
            }
            self.lap_started_at = Some(t);
        }
        self.first_playing = playing;

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
        }) {
            self.last = Some(sample);
            self.history.push(sample);
        }
        // What has been found so far, for the window.
        if now.duration_since(self.last_analysis) > Duration::from_secs(5) {
            self.last_analysis = now;
            self.findings = analyse(
                &self.history,
                &self.laps,
                self.lap_secs,
                &self.messages,
                elapsed.as_secs_f64() / 3600.0,
            );
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

fn finish(mut run: Run, end: End, show: &mut Show, engine: &mut Engine, st: &mut UiState) -> Done {
    let elapsed = run.elapsed();
    engine.stop_all(0.0);
    // The user's show comes back before anything else is done.
    if let Some(stash) = run.stash.take() {
        st.end_test(show, engine, stash);
    }
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
    let hours = elapsed.as_secs_f64() / 3600.0;
    let mut findings = analyse(&run.history, &run.laps, run.lap_secs, &run.messages, hours);
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
        .max(run.memory.peak_mb);
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
    let gone = run.finding_log.gone(&findings);
    findings.extend(gone);
    let verdict = Verdict::of(&findings);
    let report = compose(&run, &end, elapsed, verdict, &findings, show);
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
        compare: None,
        level: run.level,
        verdict,
        headline,
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
            run.level.label(),
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
    line(&mut out, "");
    line(&mut out, "Laps");
    line(&mut out, "----");
    line(
        &mut out,
        &format!("  {}", lap_summary(&run.laps, run.lap_secs)),
    );
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
        ("system.txt", "this computer, its displays and sound"),
        ("newq-log-excerpt.txt", "the end of NewQ's own log"),
        ("media-credits.txt", "where the media came from"),
    ] {
        line(&mut out, &format!("  {f:22} {what}"));
    }
    let _ = show;
    out
}

// ----- About this computer -------------------------------------------------------------------

fn system_report(
    engine: &Engine,
    st: &UiState,
    show: &Show,
    wanted: &[&'static Asset],
    resolved: &HashMap<&'static str, Resolved>,
    displays: &[DisplayChoice],
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
        crate::gpu::GPU_NAME.get().map_or("unknown", String::as_str)
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
    match engine.audio.device_status(None) {
        Some(d) => p(format!(
            "Sound: {} {} Hz, {} channels{}",
            d.name,
            d.sample_rate,
            d.channels,
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
                "30",
                "-f",
                "null",
                "-",
            ])
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(1500));
        let mut w = MemoryWatch::new();
        let t = Instant::now();
        w.limit_mb = u64::MAX;
        let _ = w.over(t + Duration::from_secs(4));
        let counted = w.children_mb;
        let _ = child.kill();
        let _ = child.wait();
        assert!(
            counted > 5,
            "the child's memory was not counted: {counted} MB"
        );
    }

    #[test]
    fn a_memory_watch_looks_once_a_second_and_sees_its_own_process() {
        let mut w = MemoryWatch::new();
        let t = Instant::now();
        assert_eq!(w.over(t), None, "too soon to look again");
        assert!(w.over(t + Duration::from_secs(2)).is_none() || w.limit_mb < 2048);
        assert!(w.peak_mb > 0, "the process was found");
        w.limit_mb = 1;
        assert!(w.over(t + Duration::from_secs(4)).is_some());
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
        let p = run_folder(Path::new("/logs"), Level::Medium, 86_400 + 3_661);
        assert_eq!(
            p,
            PathBuf::from("/logs/test-mode/runs/1970-01-02_01-01-01_medium")
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
        test.start(ff, &dir, Vec::new());
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
        test.start(ff, &dir, Vec::new());
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
}
