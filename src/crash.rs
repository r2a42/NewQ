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
    asked || (os == Os::Windows && !debug_build)
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
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let what = format!(
            "{}:\n{}",
            info.location()
                .map_or("an unknown place".to_string(), ToString::to_string),
            info.payload()
                .downcast_ref::<&str>()
                .map(ToString::to_string)
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "(no message)".into())
        );
        let text = report(
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            &utc_now(),
            thread.name().unwrap_or("unnamed"),
            &what,
            &std::backtrace::Backtrace::force_capture().to_string(),
        );
        let saved = write_report(&dir, &text);
        log::error!("{text}");
        if dialog && !told.swap(true, Ordering::SeqCst) {
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
        assert!(!log_to_file(Os::Mac, false, false));
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
        let text = std::fs::read_to_string(dir.join("newq-crash.log")).expect("a report");
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
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_time_in_a_report_is_a_real_date() {
        let t = utc_now();
        assert!(t.ends_with(" UTC") && t.len() == 20, "{t}");
        assert!(t.starts_with("20"), "{t}");
    }
}
