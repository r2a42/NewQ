//! Finding out why NewQ stopped: a log file and a crash report.
//!
//! The Windows release has no console, so a panic's message and the log go
//! nowhere anyone can see. Here the log goes to a file there, and a panic
//! writes a report (what, where, which thread, a backtrace) beside it and, on
//! Windows, says where it is. The file layout is plain functions on values
//! so it is tested without crashing anything.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    Mac,
    Other,
}

impl Os {
    pub const fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Mac
        } else {
            Os::Other
        }
    }
}

/// Where the log and crash reports go: `%LOCALAPPDATA%\NewQ` on Windows,
/// `~/Library/Logs/NewQ` on a Mac, `~/.local/state/newq` elsewhere. `get`
/// reads an environment variable; with no home to be found, the temporary folder.
pub fn log_dir_for(os: Os, get: impl Fn(&str) -> Option<String>) -> PathBuf {
    let under = |var: &str, rest: &[&str]| -> Option<PathBuf> {
        let base = get(var).filter(|v| !v.is_empty())?;
        Some(
            rest.iter()
                .fold(PathBuf::from(base), |p, part| p.join(part)),
        )
    };
    let found = match os {
        Os::Windows => under("LOCALAPPDATA", &["NewQ"]),
        Os::Mac => under("HOME", &["Library", "Logs", "NewQ"]),
        Os::Other => under("XDG_STATE_HOME", &["newq"])
            .or_else(|| under("HOME", &[".local", "state", "newq"])),
    };
    found.unwrap_or_else(|| std::env::temp_dir().join("newq"))
}

pub fn log_dir() -> PathBuf {
    log_dir_for(Os::current(), |v| std::env::var(v).ok())
}

/// The log file for this run, made ready: an earlier one is kept as
/// `newq.previous.log`. None if the folder can't be written to.
pub fn open_log(dir: &Path) -> Option<std::fs::File> {
    std::fs::create_dir_all(dir).ok()?;
    let log = dir.join("newq.log");
    if log.exists() {
        let _ = std::fs::rename(&log, dir.join("newq.previous.log"));
    }
    std::fs::File::create(log).ok()
}

/// Whether the log should go to a file: always when asked (`NEWQ_LOG_FILE`),
/// and by default where there is no console to see it on.
pub fn log_to_file(os: Os, debug_build: bool, asked: bool) -> bool {
    asked || (matches!(os, Os::Windows | Os::Mac) && !debug_build)
}

/// The text of a crash report.
pub fn report(
    version: &str,
    os: &str,
    when_utc: &str,
    thread: &str,
    what: &str,
    backtrace: &str,
) -> String {
    format!(
        "NewQ {version} on {os} crashed at {when_utc}\nthread '{thread}' panicked at {what}\n\nbacktrace:\n{backtrace}\n"
    )
}

/// Add a report to `newq-crash.log` in `dir`. Returns the file's path.
pub fn write_report(dir: &Path, text: &str) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    let path = dir.join("newq-crash.log");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    writeln!(file, "{}\n{text}", "=".repeat(60)).ok()?;
    Some(path)
}

fn utc_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let (y, mo, d, h, mi) = crate::export::civil_from_unix(secs);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02} UTC")
}

thread_local! {
    /// Set while this thread runs code whose panics are caught and dealt with.
    static EXPECTED_PANIC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// The part of the main loop this thread is in, while a panic in it is
    /// caught and the loop goes on (see `survive`).
    static SURVIVING: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

/// Run `f`, and turn a panic in it into `Err`. For code that handles its own
/// failure (a PDF page that can't be drawn is an error message): the panic is
/// only noted in the log, with no crash report and no "NewQ has stopped"
/// message, which on Windows would tell an operator the show was over.
pub fn catch_expected<R>(f: impl FnOnce() -> R) -> Result<R, ()> {
    EXPECTED_PANIC.with(|c| c.set(true));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    EXPECTED_PANIC.with(|c| c.set(false));
    result.map_err(|_| ())
}

/// Run a part of a loop that must go on whatever happens (`what` names it):
/// a panic in it leaves its report like any other, the operator is told (see
/// `take_notices`), and `None` comes back in place of the result. A show
/// does not end because one part of the program met something it didn't
/// expect; what that part was doing is left undone for this pass.
pub fn survive<R>(what: &'static str, f: impl FnOnce() -> R) -> Option<R> {
    let outer = SURVIVING.with(|c| c.replace(Some(what)));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    SURVIVING.with(|c| c.set(outer));
    result.ok()
}

/// Runs its closure if the thread is unwinding from a panic when it goes: for
/// a worker thread to say that it stopped (set its error, mark its end), so
/// what it worked for is not left waiting for it.
pub struct OnPanic<F: FnMut()>(pub F);

impl<F: FnMut()> Drop for OnPanic<F> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            (self.0)();
        }
    }
}

/// For trying the above on a running program: `NEWQ_TEST_PANIC=<part>@<seconds>`
/// panics once in that part of the program, that long after it started. The
/// parts are `engine`, `ui`, `outputs`, `render` (an output window's thread)
/// and `audio` (the mixer). Costs one atomic read where it is called.
pub fn test_panic(part: &str) {
    use std::sync::OnceLock;
    static FIRED: AtomicBool = AtomicBool::new(true);
    static WANT: OnceLock<Option<(String, std::time::Instant)>> = OnceLock::new();
    let want = WANT.get_or_init(|| {
        let asked = test_panic_asked(&std::env::var("NEWQ_TEST_PANIC").ok()?)?;
        FIRED.store(false, Ordering::Relaxed);
        Some((
            asked.0,
            std::time::Instant::now() + std::time::Duration::from_secs_f64(asked.1),
        ))
    });
    if FIRED.load(Ordering::Relaxed) {
        return;
    }
    if let Some((p, at)) = want
        && p == part
        && std::time::Instant::now() >= *at
        && !FIRED.swap(true, Ordering::SeqCst)
    {
        panic!("a panic asked for with NEWQ_TEST_PANIC in '{part}'");
    }
}

/// `engine@5` as the part and the seconds; None for anything else.
fn test_panic_asked(text: &str) -> Option<(String, f64)> {
    let (part, secs) = text.split_once('@')?;
    let secs: f64 = secs.trim().parse().ok()?;
    (!part.trim().is_empty() && secs.is_finite() && (0.0..=86_400.0).contains(&secs))
        .then(|| (part.trim().to_string(), secs))
}

/// How many times the same place may leave a full report in one run, and how
/// many reports a run may leave in all. A fault that happens on every frame
/// would otherwise fill the disk and spend the show writing backtraces.
const REPORTS_PER_PLACE: u32 = 2;
const REPORTS_PER_RUN: u32 = 40;
/// The most notices kept for the operator at once.
const MAX_NOTICES: usize = 20;

/// What the hook has seen in this run.
#[derive(Default)]
struct Seen {
    places: std::collections::HashMap<String, u32>,
    reports: u32,
}

/// Whether the `n`th panic at one place, with `reports` written so far in the
/// run, gets a full report (the rest are only counted in the log).
fn full_report(n: u32, reports: u32) -> bool {
    n <= REPORTS_PER_PLACE && reports < REPORTS_PER_RUN
}

/// Whether the `n`th repeat is worth a line in the log: ever more rarely.
fn repeat_logged(n: u32) -> bool {
    n.is_power_of_two()
}

/// Panics the program lived through, in words for the operator.
static NOTICES: parking_lot::Mutex<Vec<String>> = parking_lot::Mutex::new(Vec::new());
/// How many panics there have been in this run (expected ones not counted).
static PANICS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// What went wrong since this was last asked, to show the operator.
pub fn take_notices() -> Vec<String> {
    std::mem::take(&mut *NOTICES.lock())
}

/// How many panics this run has had (Test Mode counts the ones in a run).
#[cfg_attr(not(test), allow(dead_code))]
pub fn panic_count() -> u32 {
    PANICS.load(Ordering::Relaxed)
}

/// What to tell the operator about a panic the program lived through.
fn notice(thread: &str, surviving: Option<&str>, message: &str, saved: Option<&Path>) -> String {
    let part = match surviving {
        Some(what) => format!("{what} met an error and carried on"),
        None => format!("A part of NewQ stopped ({thread})"),
    };
    let first = message.lines().next().unwrap_or("").trim();
    let report = saved.map_or_else(String::new, |p| format!(" Report: {}", p.display()));
    format!("{part}: {first}.{report}")
}

/// Make a panic leave a report: in the log folder, in the log, and (on
/// Windows) in a message that says where it is. The usual message still
/// goes to stderr after it.
pub fn install() {
    install_in(log_dir(), cfg!(windows));
}

/// `install`, with the report folder and whether to show the message given.
fn install_in(dir: PathBuf, dialog: bool) {
    let previous = std::panic::take_hook();
    // One message box is enough if several threads fall over together.
    let told = AtomicBool::new(false);
    let seen = parking_lot::Mutex::new(Seen::default());
    std::panic::set_hook(Box::new(move |info| {
        if EXPECTED_PANIC.with(std::cell::Cell::get) {
            log::warn!("a panic that was caught and handled: {info}");
            return;
        }
        PANICS.fetch_add(1, Ordering::Relaxed);
        let thread = std::thread::current();
        let name = thread.name().unwrap_or("unnamed");
        let surviving = SURVIVING.with(std::cell::Cell::get);
        let place = info
            .location()
            .map_or("an unknown place".to_string(), ToString::to_string);
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(ToString::to_string)
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "(no message)".into());
        // The same fault again and again is counted, not written out each time.
        let (n, full) = {
            let mut seen = seen.lock();
            let reports = seen.reports;
            let n = seen.places.entry(place.clone()).or_insert(0);
            *n += 1;
            let (n, full) = (*n, full_report(*n, reports));
            if full {
                seen.reports += 1;
            }
            (n, full)
        };
        if !full {
            if repeat_logged(n) {
                log::error!("thread '{name}' panicked at {place} again ({n} times): {message}");
            }
            return;
        }
        let what = format!("{place}:\n{message}");
        let text = report(
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            &utc_now(),
            name,
            &what,
            &std::backtrace::Backtrace::force_capture().to_string(),
        );
        let saved = write_report(&dir, &text);
        log::error!("{text}");
        // The main thread outside its guarded loop (starting up) is the one
        // panic that ends the program; any other is lived through, and a
        // message box that says NewQ has to close would be wrong, and on the
        // main thread would hold the show up until someone clicked it.
        let fatal = name == "main" && surviving.is_none();
        if !fatal {
            let mut notices = NOTICES.lock();
            if notices.len() < MAX_NOTICES {
                notices.push(notice(name, surviving, &message, saved.as_deref()));
            }
        } else if dialog && !told.swap(true, Ordering::SeqCst) {
            let place = saved.map_or_else(
                || "(it could not be saved)".to_string(),
                |p| p.display().to_string(),
            );
            rfd::MessageDialog::new()
                .set_level(rfd::MessageLevel::Error)
                .set_title("NewQ has stopped")
                .set_description(format!(
                    "NewQ hit an error and has to close.\n\n{what}\n\nA report was saved to:\n{place}\n\nPlease send that file when you report it."
                ))
                .show();
        }
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn logs_go_where_each_system_keeps_them() {
        let win = log_dir_for(
            Os::Windows,
            env(&[("LOCALAPPDATA", "C:\\Users\\a\\AppData\\Local")]),
        );
        assert_eq!(win, Path::new("C:\\Users\\a\\AppData\\Local").join("NewQ"));
        let mac = log_dir_for(Os::Mac, env(&[("HOME", "/Users/a")]));
        assert_eq!(mac, Path::new("/Users/a/Library/Logs/NewQ"));
        let xdg = log_dir_for(Os::Other, env(&[("XDG_STATE_HOME", "/s"), ("HOME", "/h")]));
        assert_eq!(xdg, Path::new("/s/newq"));
        let home = log_dir_for(Os::Other, env(&[("HOME", "/h")]));
        assert_eq!(home, Path::new("/h/.local/state/newq"));
        // No home at all, or an empty one: somewhere that exists.
        for e in [env(&[]), env(&[("HOME", "")])] {
            let d = log_dir_for(Os::Other, e);
            assert!(d.starts_with(std::env::temp_dir()), "{d:?}");
        }
    }

    #[test]
    fn the_log_goes_to_a_file_where_there_is_no_console() {
        assert!(
            log_to_file(Os::Windows, false, false),
            "a Windows release has no console"
        );
        assert!(!log_to_file(Os::Windows, true, false), "a debug build does");
        assert!(!log_to_file(Os::Other, false, false));
        assert!(
            log_to_file(Os::Mac, false, false),
            "so does an app opened from the Finder"
        );
        assert!(!log_to_file(Os::Mac, true, false));
        assert!(
            log_to_file(Os::Other, true, true),
            "NEWQ_LOG_FILE asks for it anywhere"
        );
    }

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("newq_crash_{name}"));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn the_last_log_is_kept_when_a_new_one_starts() {
        let dir = temp("rotate").join("nested/logs");
        let mut first = open_log(&dir).expect("a log file");
        writeln!(first, "first run").unwrap();
        drop(first);
        let mut second = open_log(&dir).unwrap();
        writeln!(second, "second run").unwrap();
        drop(second);
        assert_eq!(
            std::fs::read_to_string(dir.join("newq.log")).unwrap(),
            "second run\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("newq.previous.log")).unwrap(),
            "first run\n"
        );
        let _ = std::fs::remove_dir_all(temp("rotate"));
        // A place that can't be made is None, not a panic.
        let file = temp("file");
        std::fs::create_dir_all(&file).unwrap();
        std::fs::write(file.join("blocker"), "x").unwrap();
        assert!(open_log(&file.join("blocker/inside")).is_none());
        let _ = std::fs::remove_dir_all(&file);
    }

    #[test]
    fn reports_say_what_where_and_which_thread_and_pile_up_in_one_file() {
        let text = report(
            "0.4.0",
            "windows",
            "2026-10-24 19:14 UTC",
            "output-render",
            "src/gpu.rs:75:\nindex out of bounds",
            "  0: frame",
        );
        for needle in [
            "NewQ 0.4.0 on windows crashed at 2026-10-24 19:14 UTC",
            "thread 'output-render' panicked at src/gpu.rs:75:",
            "index out of bounds",
            "backtrace:\n  0: frame",
        ] {
            assert!(text.contains(needle), "{needle}\n{text}");
        }
        let dir = temp("report");
        let path = write_report(&dir, &text).expect("written");
        assert_eq!(path, dir.join("newq-crash.log"));
        write_report(&dir, "second crash").unwrap();
        let all = std::fs::read_to_string(&path).unwrap();
        assert!(all.contains("index out of bounds") && all.contains("second crash"));
        assert_eq!(
            all.matches(&"=".repeat(60)).count(),
            2,
            "each report is set apart"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_panic_on_any_thread_leaves_a_report() {
        let dir = temp("hook");
        install_in(dir.clone(), false);
        let crashed = std::thread::Builder::new()
            .name("output-render".into())
            .spawn(|| {
                let screens: Vec<u32> = Vec::new();
                let i = std::hint::black_box(0);
                screens[i] // index out of bounds, as a missing display might
            })
            .unwrap()
            .join();
        assert!(crashed.is_err());
        // A panic the code catches and handles is no crash: no report for it.
        assert!(
            catch_expected(|| -> u8 { panic!("handled-quietly-marker") }).is_err(),
            "the panic comes back as an error"
        );
        assert_eq!(catch_expected(|| 7), Ok(7));
        let text = std::fs::read_to_string(dir.join("newq-crash.log")).expect("a report");
        assert!(!text.contains("handled-quietly-marker"), "{text}");
        for needle in [
            concat!("NewQ ", env!("CARGO_PKG_VERSION")),
            // The path has `\` on Windows, so only its parts are checked.
            "thread 'output-render' panicked at src",
            "crash.rs",
            "index out of bounds",
            "backtrace:",
        ] {
            assert!(text.contains(needle), "{needle}\n{text}");
        }
        // A part of a loop that panics is lived through: a report, a notice
        // for the operator, and the loop's next part runs.
        assert_eq!(survive("Drawing the window", || 3), Some(3));
        let lived = survive("Drawing the window", || -> u8 { panic!("survived-marker") });
        assert_eq!(lived, None);
        let text = std::fs::read_to_string(dir.join("newq-crash.log")).unwrap();
        assert!(text.contains("survived-marker"), "{text}");
        let notices = take_notices();
        assert!(
            notices.iter().any(|n| n
                .starts_with("Drawing the window met an error and carried on: survived-marker.")
                && n.contains("newq-crash.log")),
            "{notices:?}"
        );
        // A worker thread's panic is told too (it is not the end of the program).
        assert!(
            notices
                .iter()
                .any(|n| n.starts_with("A part of NewQ stopped (output-render): index out of")),
            "{notices:?}"
        );
        // The same fault on every frame leaves two reports, not thousands.
        for _ in 0..50 {
            let _ = survive("The show engine", || -> u8 { panic!("every-frame-marker") });
        }
        let text = std::fs::read_to_string(dir.join("newq-crash.log")).unwrap();
        assert_eq!(text.matches("every-frame-marker").count(), 2, "{text}");
        assert!(panic_count() >= 52);
        // A worker that panics says so on its way out.
        let said = std::sync::Arc::new(AtomicBool::new(false));
        let said2 = said.clone();
        let _ = std::thread::spawn(move || {
            let _g = OnPanic(|| said2.store(true, Ordering::SeqCst));
            panic!("worker-marker");
        })
        .join();
        assert!(said.load(Ordering::SeqCst));
        let quiet = std::sync::Arc::new(AtomicBool::new(false));
        let quiet2 = quiet.clone();
        drop(OnPanic(|| quiet2.store(true, Ordering::SeqCst)));
        assert!(!quiet.load(Ordering::SeqCst), "only when it panics");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fault_that_repeats_is_counted_not_written_out_every_time() {
        assert!(full_report(1, 0) && full_report(2, 1));
        assert!(!full_report(3, 2), "the third at one place is only counted");
        assert!(!full_report(1, REPORTS_PER_RUN), "and a run has a limit");
        let logged: Vec<u32> = (3..=40).filter(|n| repeat_logged(*n)).collect();
        assert_eq!(logged, [4, 8, 16, 32]);
        assert_eq!(test_panic_asked("engine@5"), Some(("engine".into(), 5.0)));
        assert_eq!(test_panic_asked(" ui @ 0.5 "), Some(("ui".into(), 0.5)));
        for bad in ["", "engine", "@3", "ui@x", "ui@-1", "ui@NaN"] {
            assert_eq!(test_panic_asked(bad), None, "{bad}");
        }
        assert_eq!(
            notice("audio-reader", None, "boom\nmore", None),
            "A part of NewQ stopped (audio-reader): boom."
        );
    }

    #[test]
    fn the_time_in_a_report_is_a_real_date() {
        let t = utc_now();
        assert!(t.ends_with(" UTC") && t.len() == 20, "{t}");
        assert!(t.starts_with("20"), "{t}");
    }
}
