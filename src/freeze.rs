//! Seeing what the program is doing while it is frozen.
//!
//! A freeze of several seconds with "0 ms outside the loop's own parts" in the
//! log says the main thread was stuck in a call that didn't return, but not
//! which one. On a Mac the program `sample` can look at a process while it
//! runs and write the stack of every thread. A watchdog thread does that when
//! the main loop has stopped for `FREEZE_AFTER` while a display is showing, and
//! leaves the answer in the log folder as `newq-freeze-<time>.txt`; Test Mode
//! copies the ones made during a run into its folder.
//!
//! The decision is a plain function and tested; the sampling itself is Mac only.

use std::time::Duration;

/// How long the main loop must have been silent before the program is sampled.
/// (When idle it looks up at least every half second.)
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const FREEZE_AFTER: Duration = Duration::from_millis(1500);

/// The most samples taken in one run of the program: each is a file and a
/// couple of seconds of work for the system.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const MAX_SAMPLES: u32 = 5;

/// Whether to sample now: the loop has been quiet for `quiet_for`, this very
/// freeze hasn't been sampled yet, and not too many have been taken.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn should_sample(quiet_for: Duration, sampled_this_freeze: bool, taken: u32) -> bool {
    quiet_for >= FREEZE_AFTER && !sampled_this_freeze && taken < MAX_SAMPLES
}

/// The file a sample taken at `unix_secs` is written to.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn sample_file_name(unix_secs: u64) -> String {
    format!("newq-freeze-{unix_secs}.txt")
}

/// Whether a file name is one of the samples.
pub fn is_sample_file(name: &str) -> bool {
    name.starts_with("newq-freeze-") && name.ends_with(".txt")
}

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Once;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::{Instant, SystemTime, UNIX_EPOCH};

    /// When the main loop last looked up, in milliseconds since `start`
    /// (0 = not yet).
    static BEAT: AtomicU64 = AtomicU64::new(0);
    /// Sample only while this is set (a display is showing): a dialog or a
    /// menu being held open also stops the loop, and is not a freeze.
    static WATCHING: AtomicBool = AtomicBool::new(false);
    static STARTED: Once = Once::new();

    fn now_ms(start: Instant) -> u64 {
        start.elapsed().as_millis() as u64 + 1
    }

    fn start_instant() -> Instant {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        *START.get_or_init(Instant::now)
    }

    /// The main loop is alive.
    pub fn beat() {
        BEAT.store(now_ms(start_instant()), Ordering::Release);
    }

    /// Say whether a freeze now would matter (outputs are open).
    pub fn watch(on: bool) {
        WATCHING.store(on, Ordering::Release);
    }

    /// Start the watchdog (once), writing samples into `dir`.
    pub fn start_watching(dir: PathBuf) {
        STARTED.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("freeze-watch".into())
                .spawn(move || {
                    let start = start_instant();
                    let (mut sampled_for, mut taken) = (0u64, 0u32);
                    loop {
                        std::thread::sleep(Duration::from_millis(250));
                        let beat = BEAT.load(Ordering::Acquire);
                        if beat == 0 || !WATCHING.load(Ordering::Acquire) {
                            continue;
                        }
                        let quiet = Duration::from_millis(now_ms(start).saturating_sub(beat));
                        if !should_sample(quiet, sampled_for == beat, taken) {
                            continue;
                        }
                        sampled_for = beat;
                        taken += 1;
                        let secs = SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .map_or(0, |d| d.as_secs());
                        let path = dir.join(sample_file_name(secs));
                        log::warn!(
                            "the main loop has been silent for {} ms: sampling what the program is doing into {}",
                            quiet.as_millis(),
                            path.display()
                        );
                        let status = std::process::Command::new("/usr/bin/sample")
                            .arg(std::process::id().to_string())
                            .args(["2", "-file"])
                            .arg(&path)
                            .stdin(std::process::Stdio::null())
                            .stdout(std::process::Stdio::null())
                            .stderr(std::process::Stdio::null())
                            .status();
                        match status {
                            Ok(s) if s.success() => {}
                            other => {
                                log::warn!("`sample` could not look at the program ({other:?})")
                            }
                        }
                    }
                });
        });
    }
}

#[cfg(target_os = "macos")]
pub use mac::{beat, start_watching, watch};

/// Copy the samples written since `since` from `from` into `to`; returns their
/// names.
pub fn copy_samples(
    from: &std::path::Path,
    to: &std::path::Path,
    since: std::time::SystemTime,
) -> Vec<String> {
    let mut copied = Vec::new();
    let Ok(entries) = std::fs::read_dir(from) else {
        return copied;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_sample_file(&name) {
            continue;
        }
        let recent = entry
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t >= since);
        if recent && std::fs::copy(entry.path(), to.join(&name)).is_ok() {
            copied.push(name);
        }
    }
    copied.sort();
    copied
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_silent_loop_is_sampled_once_per_freeze_and_not_forever() {
        let ms = Duration::from_millis;
        assert!(!should_sample(ms(400), false, 0), "an idle moment");
        assert!(!should_sample(ms(1499), false, 0));
        assert!(should_sample(ms(1500), false, 0));
        assert!(
            !should_sample(ms(6000), true, 1),
            "this freeze has been sampled"
        );
        assert!(should_sample(ms(2000), false, MAX_SAMPLES - 1));
        assert!(!should_sample(ms(2000), false, MAX_SAMPLES), "enough taken");
    }

    #[test]
    fn samples_are_told_from_other_files_and_copied_when_recent() {
        assert!(is_sample_file(&sample_file_name(1_762_000_000)));
        assert!(!is_sample_file("newq.log"));
        assert!(!is_sample_file("newq-crash.log"));
        let base = std::env::temp_dir().join(format!("newq_freeze_{}", std::process::id()));
        let (from, to) = (base.join("from"), base.join("to"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        std::fs::write(from.join("newq.log"), "x").unwrap();
        std::fs::write(from.join(sample_file_name(5)), "stack").unwrap();
        let before = std::time::SystemTime::now() - Duration::from_secs(60);
        let after = std::time::SystemTime::now() + Duration::from_secs(60);
        assert_eq!(copy_samples(&from, &to, before), vec![sample_file_name(5)]);
        assert!(to.join(sample_file_name(5)).exists() && !to.join("newq.log").exists());
        // One from before the run began is left out.
        assert!(copy_samples(&from, &to, after).is_empty());
        let _ = std::fs::remove_dir_all(base);
    }
}
