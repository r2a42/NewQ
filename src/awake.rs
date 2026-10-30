//! Keeping the computer awake while something that must not be interrupted
//! runs (a test). A computer that goes to sleep mid-test stops the show and
//! its timing: on a Mac, an hour-long test took 69 minutes by the wall clock,
//! and a lap took twice as long, because the Mac slept (and woke briefly now
//! and then) with the lid closed or the screen off.
//!
//! Best effort: where it can't be done the test says so and goes on.

use std::time::{Duration, Instant, SystemTime};

/// Holds the computer awake until it is dropped.
pub struct KeepAwake {
    /// What was done, for the report ("caffeinate", "SetThreadExecutionState"),
    /// or why nothing was.
    how: String,
    held: bool,
    #[cfg(not(windows))]
    child: Option<std::process::Child>,
}

impl KeepAwake {
    /// Ask the system not to sleep or switch the display off. `why` is what it
    /// tells the user who looks.
    pub fn new(why: &str) -> KeepAwake {
        let _ = why;
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Power::{
                ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
            };
            // Per thread: this is called on the thread that drops it, the main one.
            let before = unsafe {
                SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED)
            };
            let held = before != 0;
            KeepAwake {
                how: if held {
                    "SetThreadExecutionState".into()
                } else {
                    "the system refused (SetThreadExecutionState)".into()
                },
                held,
            }
        }
        #[cfg(target_os = "macos")]
        {
            // caffeinate holds the assertions for as long as this process lives.
            let spawned = std::process::Command::new("/usr/bin/caffeinate")
                .args([
                    "-d",
                    "-i",
                    "-m",
                    "-s",
                    "-w",
                    &std::process::id().to_string(),
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            match spawned {
                Ok(child) => KeepAwake {
                    how: "caffeinate".into(),
                    held: true,
                    child: Some(child),
                },
                Err(e) => KeepAwake {
                    how: format!("couldn't start caffeinate: {e}"),
                    held: false,
                    child: None,
                },
            }
        }
        #[cfg(all(not(windows), not(target_os = "macos")))]
        {
            let spawned = std::process::Command::new("systemd-inhibit")
                .args([
                    "--what=idle:sleep",
                    "--who=NewQ",
                    &format!("--why={why}"),
                    "sleep",
                    "86400",
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            match spawned {
                Ok(child) => KeepAwake {
                    how: "systemd-inhibit".into(),
                    held: true,
                    child: Some(child),
                },
                Err(_) => KeepAwake {
                    how: "not available on this system".into(),
                    held: false,
                    child: None,
                },
            }
        }
    }

    pub fn held(&self) -> bool {
        self.held
    }

    pub fn how(&self) -> &str {
        &self.how
    }
}

impl Drop for KeepAwake {
    fn drop(&mut self) {
        #[cfg(windows)]
        if self.held {
            use windows_sys::Win32::System::Power::{ES_CONTINUOUS, SetThreadExecutionState};
            unsafe {
                SetThreadExecutionState(ES_CONTINUOUS);
            }
        }
        #[cfg(not(windows))]
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Notices that the computer slept: the clock on the wall moved on by more
/// than the clock that stops while it sleeps (`Instant` does not count the
/// time on a Mac or under Linux).
pub struct SleepWatch {
    mono: Instant,
    wall: SystemTime,
}

/// More than this between the two clocks is a sleep, not clock jitter or a
/// small correction of the time.
const SLEEP_GAP: Duration = Duration::from_secs(15);

impl SleepWatch {
    pub fn new() -> SleepWatch {
        SleepWatch {
            mono: Instant::now(),
            wall: SystemTime::now(),
        }
    }

    /// Look at the clocks: how long the computer was asleep since the last
    /// look, if it was.
    pub fn check(&mut self, mono: Instant, wall: SystemTime) -> Option<Duration> {
        let awake = mono.saturating_duration_since(self.mono);
        let passed = wall.duration_since(self.wall).unwrap_or(Duration::ZERO);
        self.mono = mono;
        self.wall = wall;
        (passed > awake + SLEEP_GAP).then(|| passed - awake)
    }
}

impl Default for SleepWatch {
    fn default() -> Self {
        SleepWatch::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sleep_is_the_wall_clock_running_ahead_of_the_one_that_stops_in_sleep() {
        let (m0, w0) = (Instant::now(), SystemTime::now());
        let mut w = SleepWatch { mono: m0, wall: w0 };
        // An ordinary second of both.
        let t = |s: u64| (m0 + Duration::from_secs(s), w0 + Duration::from_secs(s));
        let (m, wl) = t(1);
        assert_eq!(w.check(m, wl), None);
        // Ten seconds on the monitor clock, five minutes on the wall: it slept for 4:50.
        let m = m0 + Duration::from_secs(11);
        let wl = w0 + Duration::from_secs(301);
        assert_eq!(w.check(m, wl), Some(Duration::from_secs(290)));
        // Small differences (clock corrections, a busy moment) are not sleep.
        let m = m + Duration::from_secs(10);
        let wl = wl + Duration::from_secs(20);
        assert_eq!(w.check(m, wl), None);
        // The wall clock set back is not sleep either.
        assert_eq!(
            w.check(m + Duration::from_secs(1), wl - Duration::from_secs(100)),
            None
        );
    }

    #[test]
    fn keeping_awake_can_be_asked_for_and_given_back() {
        let a = KeepAwake::new("a test of NewQ");
        // Whether it could be done depends on the system; what was done is said.
        assert!(!a.how().is_empty());
        let held = a.held();
        drop(a);
        // Asking again after letting go works the same.
        let b = KeepAwake::new("a test of NewQ");
        assert_eq!(b.held(), held);
    }
}
