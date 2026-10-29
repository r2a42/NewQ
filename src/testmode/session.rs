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
            notice: None,
            phase: Phase::Idle,
        }
    }
}

enum Phase {
    Idle,
    Preparing(Box<Prep>),
    Running(Box<Run>),
    Done(Box<Done>),
}

/// The media being made ready.
pub struct Prep {
    pub fetch: Fetch,
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
    log_path: PathBuf,
}

impl Run {
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
            Phase::Done(d) => View::Done(d),
        }
    }

    /// Whether a test is being prepared or run (the window can't be changed).
    pub fn busy(&self) -> bool {
        matches!(self.phase, Phase::Preparing(_) | Phase::Running(_))
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
    pub fn start(&mut self, ff: Ffmpeg, log_dir: &Path) {
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
            level: self.level,
            minutes: self.minutes,
            wanted,
            log_dir: log_dir.to_path_buf(),
        }));
    }

    /// Stop what is under way: cancel the preparation, or end the test.
    pub fn stop(&mut self) {
        match &mut self.phase {
            Phase::Preparing(p) => p.fetch.cancel(),
            Phase::Running(r) => r.stop = true,
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
                match launch(*p, show, engine, st) {
                    Ok(run) => self.phase = Phase::Running(Box::new(run)),
                    Err(e) => {
                        log::error!("test mode: {e}");
                        self.notice = Some(e);
                    }
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
    log_dir.join("test-mode").join("runs").join(format!(
        "{}_{}",
        utc_stamp(secs),
        level.label().to_lowercase()
    ))
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
    let built = plan::build(show, prep.level, &resolved);
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
        system_report(engine, st, show, &prep.wanted, &resolved),
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
        log_path,
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
        let _ = show;
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
        let version = std::process::Command::new(&ff.ffmpeg)
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
        test.start(ff, &dir);
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
        test.start(ff, &dir);
        let mut was_locked = false;
        let ok = wait_for(
            || {
                engine.tick(&show, 0.02);
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
        assert!(!d.report.contains("PROBLEM"), "{}", d.report);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
