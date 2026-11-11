//! NewQ's log: how much is written (a level chosen in Settings, or by
//! `NEWQ_LOG`), where (`newq.log` in the log folder, and the console where
//! there is one), and in what shape (time, level, thread, where from, message).
//!
//! Writing is done by a thread of its own, so a slow disk never holds the
//! show up: a line is handed over through a queue of limited length and, if
//! that is ever full, dropped and counted. The file is limited too: when it
//! reaches `MAX_FILE_BYTES` it becomes `newq.earlier.log` and a new one is
//! begun (the last run's stays `newq.previous.log`), so a show that runs for a week can't fill the disk.

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

/// How much the log says, from least to most.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Error,
    Warn,
    #[default]
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub const ALL: [LogLevel; 5] = [
        LogLevel::Error,
        LogLevel::Warn,
        LogLevel::Info,
        LogLevel::Debug,
        LogLevel::Trace,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LogLevel::Error => "Errors only",
            LogLevel::Warn => "Warnings",
            LogLevel::Info => "Normal",
            LogLevel::Debug => "Detailed",
            LogLevel::Trace => "Everything",
        }
    }

    /// What it writes, for the settings page.
    pub fn summary(self) -> &'static str {
        match self {
            LogLevel::Error => "Only what went wrong.",
            LogLevel::Warn => "What went wrong, and what looked wrong (a stall, a late start).",
            LogLevel::Info => {
                "Also what the show did: each GO, windows, devices, streams and servers."
            }
            LogLevel::Debug => {
                "Also each cue starting and ending, each FFmpeg started and why it stopped. For finding a fault."
            }
            LogLevel::Trace => {
                "Also the small steps: every preload, seek and retry. The file fills quickly; for finding a fault that Detailed doesn't show."
            }
        }
    }

    /// The name it has in the settings file and in `NEWQ_LOG`.
    pub fn name(self) -> &'static str {
        match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        }
    }

    pub fn from_name(name: &str) -> Option<LogLevel> {
        match name.trim().to_ascii_lowercase().as_str() {
            "error" | "errors" => Some(LogLevel::Error),
            "warn" | "warning" | "warnings" => Some(LogLevel::Warn),
            "info" => Some(LogLevel::Info),
            "debug" => Some(LogLevel::Debug),
            "trace" => Some(LogLevel::Trace),
            _ => None,
        }
    }

    fn filter(self) -> log::LevelFilter {
        match self {
            LogLevel::Error => log::LevelFilter::Error,
            LogLevel::Warn => log::LevelFilter::Warn,
            LogLevel::Info => log::LevelFilter::Info,
            LogLevel::Debug => log::LevelFilter::Debug,
            LogLevel::Trace => log::LevelFilter::Trace,
        }
    }

    fn from_u8(n: u8) -> LogLevel {
        LogLevel::ALL[usize::from(n).min(LogLevel::ALL.len() - 1)]
    }

    /// What the libraries NewQ is built on (the graphics, the windows, the
    /// sound) may say at this level: a step less than NewQ itself, since at
    /// their own "info" and below they write a great deal about every frame.
    fn for_libraries(self) -> log::LevelFilter {
        match self {
            LogLevel::Error => log::LevelFilter::Error,
            LogLevel::Warn | LogLevel::Info => log::LevelFilter::Warn,
            LogLevel::Debug | LogLevel::Trace => log::LevelFilter::Info,
        }
    }
}

/// What `NEWQ_LOG` (or the older `RUST_LOG`) asks for: a level for everything,
/// and levels for parts by the start of their name, as in
/// `debug,wgpu=info,newq::engine=trace`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Spec {
    pub level: Option<LogLevel>,
    pub parts: Vec<(String, log::LevelFilter)>,
}

impl Spec {
    pub fn parse(text: &str) -> Spec {
        let mut spec = Spec::default();
        for item in text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            match item.split_once('=') {
                None => spec.level = LogLevel::from_name(item).or(spec.level),
                Some((part, level)) => {
                    let level = match level.trim().to_ascii_lowercase().as_str() {
                        "off" => Some(log::LevelFilter::Off),
                        other => LogLevel::from_name(other).map(LogLevel::filter),
                    };
                    if let Some(level) = level
                        && !part.trim().is_empty()
                    {
                        spec.parts.push((part.trim().to_string(), level));
                    }
                }
            }
        }
        spec
    }

    /// Read from the environment; `NEWQ_LOG` wins over `RUST_LOG`.
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Spec {
        get("NEWQ_LOG")
            .or_else(|| get("RUST_LOG"))
            .map(|t| Spec::parse(&t))
            .unwrap_or_default()
    }
}

/// Whether a record from `target` at `level` is written, with the level
/// chosen and what the environment asked for.
fn wanted(chosen: LogLevel, spec: &Spec, target: &str, level: log::Level) -> bool {
    // The longest part named that the target begins with decides.
    if let Some((_, filter)) = spec
        .parts
        .iter()
        .filter(|(p, _)| target.starts_with(p.as_str()))
        .max_by_key(|(p, _)| p.len())
    {
        return level <= *filter;
    }
    let ours = target == "newq" || target.starts_with("newq::");
    let filter = if ours {
        chosen.filter()
    } else {
        chosen.for_libraries()
    };
    level <= filter
}

/// One line of the log.
pub fn format_line(
    when: &str,
    level: log::Level,
    thread: &str,
    target: &str,
    text: &str,
) -> String {
    // The crate's own name says nothing; the module does.
    let from = target.strip_prefix("newq::").unwrap_or(target);
    format!("{when} {level:<5} [{thread}] {from}: {text}\n")
}

fn now_text() -> String {
    jiff::Zoned::now()
        .strftime("%Y-%m-%d %H:%M:%S%.3f")
        .to_string()
}

/// The file may grow to this before it is put aside as `newq.earlier.log`.
pub const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
/// Lines waiting to be written; more than this are dropped and counted.
const QUEUE: usize = 8192;

enum Msg {
    Line(String),
    /// Write everything so far out, then answer.
    Flush(std::sync::mpsc::SyncSender<()>),
}

/// The log file, put aside and begun again when it is full.
pub struct Rotating {
    dir: PathBuf,
    file: Option<std::fs::File>,
    written: u64,
    limit: u64,
}

impl Rotating {
    /// Begin this run's file in `dir`; the last run's is kept as
    /// `newq.previous.log`. With a folder that can't be written to there is
    /// no file, and writing does nothing.
    pub fn begin(dir: &Path, limit: u64) -> Rotating {
        Rotating {
            dir: dir.to_path_buf(),
            file: crate::crash::open_log(dir),
            written: 0,
            limit: limit.max(1024),
        }
    }

    pub fn has_file(&self) -> bool {
        self.file.is_some()
    }

    pub fn write(&mut self, line: &str) {
        if self.written + line.len() as u64 > self.limit && self.written > 0 {
            // Closed before it is renamed (Windows won't rename an open file).
            // Not `open_log`: that would put this run's log where the last
            // run's is kept and lose it.
            self.file = None;
            let log = self.dir.join("newq.log");
            let _ = std::fs::rename(&log, self.dir.join("newq.earlier.log"));
            self.file = std::fs::File::create(log).ok();
            self.written = 0;
            let note = format_line(
                &now_text(),
                log::Level::Info,
                "log",
                "newq::logging",
                "the log reached its size limit: the earlier part is in newq.earlier.log",
            );
            self.put(&note);
        }
        self.put(line);
    }

    fn put(&mut self, line: &str) {
        if let Some(f) = &mut self.file
            && f.write_all(line.as_bytes()).is_ok()
        {
            self.written += line.len() as u64;
        }
    }
}

struct Logger {
    chosen: AtomicU8,
    spec: Spec,
    tx: crossbeam_channel::Sender<Msg>,
    dropped: AtomicU64,
}

static LOGGER: std::sync::OnceLock<Logger> = std::sync::OnceLock::new();

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        wanted(
            LogLevel::from_u8(self.chosen.load(Ordering::Relaxed)),
            &self.spec,
            m.target(),
            m.level(),
        )
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let thread = std::thread::current();
        let line = format_line(
            &now_text(),
            record.level(),
            thread.name().unwrap_or("unnamed"),
            record.target(),
            &record.args().to_string(),
        );
        if self.tx.try_send(Msg::Line(line)).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn flush(&self) {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        if self.tx.try_send(Msg::Flush(tx)).is_ok() {
            // Not for ever: this is called from a panic, and at the end.
            let _ = rx.recv_timeout(Duration::from_secs(2));
        }
    }
}

/// The writer thread: lines to the file and the console, in order.
fn write_loop(
    rx: &crossbeam_channel::Receiver<Msg>,
    mut file: Rotating,
    console: bool,
    dropped: &AtomicU64,
) {
    let mut unflushed = false;
    loop {
        // A moment after the last line, what the system holds is written out,
        // so the file is never far behind what happened.
        let msg = if unflushed {
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(m) => Some(m),
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => None,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            }
        } else {
            match rx.recv() {
                Ok(m) => Some(m),
                Err(_) => return,
            }
        };
        let lost = dropped.swap(0, Ordering::Relaxed);
        if lost > 0 {
            let note = format_line(
                &now_text(),
                log::Level::Warn,
                "log",
                "newq::logging",
                &format!("{lost} lines were not written: they came faster than the disk took them"),
            );
            file.write(&note);
        }
        match msg {
            Some(Msg::Line(line)) => {
                file.write(&line);
                if console {
                    let _ = std::io::stderr().write_all(line.as_bytes());
                }
                unflushed = true;
            }
            Some(Msg::Flush(done)) => {
                if let Some(f) = &mut file.file {
                    let _ = f.flush();
                }
                unflushed = false;
                let _ = done.send(());
            }
            None => {
                if let Some(f) = &mut file.file {
                    let _ = f.flush();
                }
                unflushed = false;
            }
        }
    }
}

/// Whether there is a console to write the log to as well: a Windows release
/// has none (and an app opened from the Finder shows none, but writing to it
/// does no harm).
pub fn has_console(os: crate::crash::Os, debug_build: bool) -> bool {
    !(matches!(os, crate::crash::Os::Windows) && !debug_build)
}

/// Start logging: to `newq.log` in `dir` (the last run's kept beside it) and
/// to the console. Returns whether the file could be made.
pub fn init(dir: &Path) -> bool {
    let spec = Spec::from_env(|v| std::env::var(v).ok());
    let file = Rotating::begin(dir, MAX_FILE_BYTES);
    let has_file = file.has_file();
    let console = has_console(crate::crash::Os::current(), cfg!(debug_assertions));
    let (tx, rx) = crossbeam_channel::bounded(QUEUE);
    let chosen = spec.level.unwrap_or_default();
    let logger = LOGGER.get_or_init(|| Logger {
        chosen: AtomicU8::new(chosen as u8),
        spec,
        tx,
        dropped: AtomicU64::new(0),
    });
    let spawned = std::thread::Builder::new()
        .name("log-writer".into())
        .spawn(move || write_loop(&rx, file, console, &logger.dropped));
    if spawned.is_err() || log::set_logger(logger).is_err() {
        return false;
    }
    log::set_max_level(max_filter(chosen, &logger.spec));
    has_file
}

/// The most detailed level anything is written at (what the `log` macros are
/// told, so that what is not wanted costs nothing).
fn max_filter(chosen: LogLevel, spec: &Spec) -> log::LevelFilter {
    spec.parts
        .iter()
        .map(|(_, f)| *f)
        .fold(chosen.filter(), std::cmp::max)
}

/// The level `NEWQ_LOG` fixes for this run, if it does: the setting is then
/// only what is used the next time.
pub fn forced() -> Option<LogLevel> {
    LOGGER.get().and_then(|l| l.spec.level)
}

/// The level in use.
pub fn level() -> LogLevel {
    LOGGER.get().map_or(LogLevel::Info, |l| {
        LogLevel::from_u8(l.chosen.load(Ordering::Relaxed))
    })
}

/// Use the level chosen in Settings from now on (unless `NEWQ_LOG` fixes one).
pub fn set_level(level: LogLevel) {
    let Some(l) = LOGGER.get() else { return };
    if l.spec.level.is_some() {
        return;
    }
    if l.chosen.swap(level as u8, Ordering::Relaxed) != level as u8 {
        log::set_max_level(max_filter(level, &l.spec));
        // (At info, so that it is in the file whichever way the level went.)
        log::info!("log detail: {} ({})", level.label(), level.name());
    }
}

/// Have everything said so far written to the file before going on.
pub fn flush() {
    log::logger().flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("newq_logging_{name}"));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn the_level_chosen_decides_what_newq_and_its_libraries_write() {
        let none = Spec::default();
        let ours = "newq::engine";
        use log::Level::*;
        assert!(wanted(LogLevel::Info, &none, ours, Info));
        assert!(!wanted(LogLevel::Info, &none, ours, Debug));
        assert!(wanted(LogLevel::Debug, &none, ours, Debug));
        assert!(!wanted(LogLevel::Debug, &none, ours, Trace));
        assert!(wanted(LogLevel::Trace, &none, ours, Trace));
        assert!(wanted(LogLevel::Error, &none, ours, Error));
        assert!(!wanted(LogLevel::Error, &none, ours, Warn));
        assert!(wanted(LogLevel::Warn, &none, "newq", Warn));
        // The graphics library says a lot at its own "info": a step less.
        assert!(!wanted(LogLevel::Info, &none, "wgpu_core::device", Info));
        assert!(wanted(LogLevel::Info, &none, "wgpu_core::device", Warn));
        assert!(wanted(LogLevel::Debug, &none, "wgpu_hal", Info));
        assert!(!wanted(LogLevel::Trace, &none, "wgpu_hal", Debug));
        // (A crate that only begins like ours is a library.)
        assert!(!wanted(LogLevel::Info, &none, "newqueue", Info));
    }

    #[test]
    fn the_environment_names_a_level_and_levels_for_parts() {
        let s = Spec::parse("debug, wgpu=info ,newq::engine=trace,naga=off");
        assert_eq!(s.level, Some(LogLevel::Debug));
        assert_eq!(s.parts.len(), 3);
        use log::Level::*;
        assert!(wanted(LogLevel::Debug, &s, "newq::engine", Trace));
        assert!(!wanted(LogLevel::Debug, &s, "newq::app", Trace));
        assert!(wanted(LogLevel::Debug, &s, "wgpu_core", Info));
        assert!(!wanted(LogLevel::Debug, &s, "naga::front", Error));
        assert_eq!(max_filter(LogLevel::Debug, &s), log::LevelFilter::Trace);
        assert_eq!(
            max_filter(LogLevel::Info, &Spec::default()),
            log::LevelFilter::Info
        );
        // Nonsense is left out, not a failure to start.
        let odd = Spec::parse("loud,=debug,x=,wgpu=shout,,");
        assert_eq!(odd, Spec::default());
        assert_eq!(Spec::parse("TRACE").level, Some(LogLevel::Trace));
        // NEWQ_LOG wins over the older RUST_LOG; neither is no wish at all.
        let env = |pairs: &'static [(&str, &str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(
            Spec::from_env(env(&[("NEWQ_LOG", "warn"), ("RUST_LOG", "trace")])).level,
            Some(LogLevel::Warn)
        );
        assert_eq!(
            Spec::from_env(env(&[("RUST_LOG", "trace")])).level,
            Some(LogLevel::Trace)
        );
        assert_eq!(Spec::from_env(env(&[])), Spec::default());
    }

    #[test]
    fn every_level_has_a_name_that_reads_back_and_a_line_for_the_settings() {
        for l in LogLevel::ALL {
            assert_eq!(LogLevel::from_name(l.name()), Some(l));
            assert_eq!(LogLevel::from_u8(l as u8), l);
            assert!(!l.label().is_empty() && !l.summary().is_empty());
            let json = serde_json::to_string(&l).unwrap();
            assert_eq!(json, format!("\"{}\"", l.name()));
        }
        assert_eq!(LogLevel::default(), LogLevel::Info);
        assert_eq!(LogLevel::from_u8(200), LogLevel::Trace);
        assert!(LogLevel::Error < LogLevel::Trace);
    }

    #[test]
    fn a_line_has_the_time_the_level_the_thread_and_where_it_came_from() {
        let line = format_line(
            "2026-11-11 19:14:03.250",
            log::Level::Warn,
            "output-render",
            "newq::render",
            "waiting for a frame took 140 ms",
        );
        assert_eq!(
            line,
            "2026-11-11 19:14:03.250 WARN  [output-render] render: waiting for a frame took 140 ms\n"
        );
        // The time is a real one, to the millisecond.
        let now = now_text();
        assert_eq!(now.len(), 23, "{now}");
        assert!(now.starts_with("20") && now.as_bytes()[19] == b'.', "{now}");
    }

    #[test]
    fn a_full_log_is_put_aside_and_a_new_one_begun() {
        let d = dir("rotate");
        // The run before this one.
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("newq.log"), "last run\n").unwrap();
        let mut log = Rotating::begin(&d, 2048);
        assert!(log.has_file());
        let line = format!("{}\n", "x".repeat(99));
        for _ in 0..30 {
            log.write(&line);
        }
        drop(log);
        let now = std::fs::read_to_string(d.join("newq.log")).unwrap();
        let earlier = std::fs::read_to_string(d.join("newq.earlier.log")).unwrap();
        // Never more than the limit in either, and nothing lost at the join.
        assert!(
            now.len() <= 2048 && earlier.len() <= 2048,
            "{} {}",
            now.len(),
            earlier.len()
        );
        assert!(now.contains("the earlier part is in newq.earlier.log"));
        assert!(earlier.lines().all(|l| l == "x".repeat(99)));
        assert!(now.lines().filter(|l| *l == "x".repeat(99)).count() >= 1);
        // Putting this run's log aside for size kept the last run's.
        assert_eq!(
            std::fs::read_to_string(d.join("newq.previous.log")).unwrap(),
            "last run\n"
        );
        // The next run keeps this one's file, and its earlier part, beside its own.
        let mut next = Rotating::begin(&d, 2048);
        next.write("second run\n");
        drop(next);
        assert_eq!(
            std::fs::read_to_string(d.join("newq.log")).unwrap(),
            "second run\n"
        );
        assert_eq!(
            std::fs::read_to_string(d.join("newq.previous.log")).unwrap(),
            now
        );
        assert_eq!(
            std::fs::read_to_string(d.join("newq.previous.earlier.log")).unwrap(),
            earlier
        );
        assert!(!d.join("newq.earlier.log").exists());
        // A run after one that never filled its log has no earlier part of it.
        drop(Rotating::begin(&d, 2048));
        assert!(!d.join("newq.previous.earlier.log").exists());
        let _ = std::fs::remove_dir_all(&d);
        // A folder that can't be made: no file, and writing is not a panic.
        let blocked = dir("blocked");
        std::fs::create_dir_all(&blocked).unwrap();
        std::fs::write(blocked.join("file"), "x").unwrap();
        let mut none = Rotating::begin(&blocked.join("file/inside"), 2048);
        assert!(!none.has_file());
        none.write("nowhere\n");
        let _ = std::fs::remove_dir_all(&blocked);
    }

    #[test]
    fn lines_are_written_in_order_by_a_thread_and_a_flush_waits_for_them() {
        let d = dir("writer");
        let (tx, rx) = crossbeam_channel::bounded(QUEUE);
        let file = Rotating::begin(&d, MAX_FILE_BYTES);
        static DROPPED: AtomicU64 = AtomicU64::new(3);
        let writer = std::thread::spawn(move || write_loop(&rx, file, false, &DROPPED));
        for n in 0..500 {
            tx.send(Msg::Line(format!("line {n}\n"))).unwrap();
        }
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
        tx.send(Msg::Flush(done_tx)).unwrap();
        done_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("flushed");
        let text = std::fs::read_to_string(d.join("newq.log")).unwrap();
        let lines: Vec<&str> = text.lines().filter(|l| l.starts_with("line ")).collect();
        assert_eq!(lines.len(), 500);
        assert!(
            lines
                .iter()
                .enumerate()
                .all(|(n, l)| *l == format!("line {n}"))
        );
        // What could not be queued is owned up to.
        assert!(text.contains("3 lines were not written"), "{text}");
        drop(tx);
        writer.join().unwrap();
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn only_a_windows_release_has_no_console() {
        use crate::crash::Os;
        assert!(!has_console(Os::Windows, false));
        assert!(has_console(Os::Windows, true));
        assert!(has_console(Os::Mac, false) && has_console(Os::Other, false));
    }
}
