//! Breadcrumbs for the main loop: where its time went.
//!
//! When the loop stalls for seconds, "event loop stalled for 4815 ms" says that
//! it did and nothing about what it was doing. The loop leaves a crumb after
//! each of its parts (`lap`), with how long the part took since the last one;
//! when a stall is noticed, `report` names the parts that took any time, and
//! how much of the stall no crumb accounts for (time spent outside NewQ's own
//! code: the operating system, a driver, a window server that did not answer).
//!
//! Cheap enough to leave on: a clock read per part.

use std::time::{Duration, Instant};

/// Parts shorter than this are not worth a mention.
const WORTH_NAMING: Duration = Duration::from_millis(15);

pub struct Crumbs {
    mark: Instant,
    /// What each part took since the cycle began, in order.
    parts: Vec<(&'static str, Duration)>,
}

impl Default for Crumbs {
    fn default() -> Self {
        Crumbs::new()
    }
}

impl Crumbs {
    pub fn new() -> Crumbs {
        Crumbs {
            mark: Instant::now(),
            parts: Vec::new(),
        }
    }

    /// Start timing from now without charging the time since the last crumb to
    /// anyone (an event handler starting after a wait).
    pub fn skip(&mut self) {
        self.mark = Instant::now();
    }

    /// The part just done was `name`.
    pub fn lap(&mut self, name: &'static str) {
        let now = Instant::now();
        let took = now.duration_since(self.mark);
        self.mark = now;
        self.add(name, took);
    }

    fn add(&mut self, name: &'static str, took: Duration) {
        match self.parts.iter_mut().find(|(n, _)| *n == name) {
            Some((_, d)) => *d += took,
            None => self.parts.push((name, took)),
        }
    }

    /// The parts of the cycle that has just ended, as a sentence for a stall of
    /// `total`, and the start of a new cycle.
    pub fn report(&mut self, total: Duration) -> String {
        let mut parts = std::mem::take(&mut self.parts);
        self.mark = Instant::now();
        let accounted: Duration = parts.iter().map(|(_, d)| *d).sum();
        parts.retain(|(_, d)| *d >= WORTH_NAMING);
        parts.sort_by_key(|(_, d)| std::cmp::Reverse(*d));
        let mut out = String::new();
        for (name, d) in parts.iter().take(6) {
            if !out.is_empty() {
                out.push_str(", ");
            }
            out.push_str(&format!("{name} {} ms", d.as_millis()));
        }
        if out.is_empty() {
            out.push_str("no part of the loop took long");
        }
        let outside = total.saturating_sub(accounted);
        out.push_str(&format!(
            "; {} ms of {} ms was outside the loop's own parts (waiting on the system)",
            outside.as_millis(),
            total.as_millis()
        ));
        out
    }

    /// The parts that took any time, slowest first ("a 30 ms, b 20 ms"), or
    /// "no part took long". Doesn't end the cycle.
    pub fn summary(&self) -> String {
        let mut parts: Vec<_> = self
            .parts
            .iter()
            .filter(|(_, d)| *d >= WORTH_NAMING)
            .collect();
        parts.sort_by_key(|(_, d)| std::cmp::Reverse(*d));
        if parts.is_empty() {
            return "no part took long".into();
        }
        parts
            .iter()
            .take(6)
            .map(|(n, d)| format!("{n} {} ms", d.as_millis()))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Forget the cycle (nothing was wrong).
    pub fn next_cycle(&mut self) {
        self.parts.clear();
        self.mark = Instant::now();
    }
}

// ----- Stalls, for Test Mode and the warning ---------------------------------

/// A stall of the main loop, as the warning told it: kept for Test Mode, which
/// reports each with its time and what the loop was doing.
#[derive(Clone, Debug)]
pub struct Stall {
    /// When it ended.
    pub at: Instant,
    pub ms: u64,
    /// The part of the loop that took longest ("test mode", "the show engine").
    pub part: &'static str,
    /// Seconds past the whole minute, by the wall clock, when it began: many
    /// that began just after one are something done once a minute.
    pub past_minute: f64,
    /// The warning's words after "stalled for N ms: ".
    pub text: String,
}

static STALLS: std::sync::Mutex<Vec<Stall>> = std::sync::Mutex::new(Vec::new());
/// Kept until taken, no more than this (a test takes them every frame).
const STALLS_KEPT: usize = 500;

pub fn note_stall(s: Stall) {
    if let Ok(mut v) = STALLS.lock() {
        if v.len() >= STALLS_KEPT {
            v.remove(0);
        }
        v.push(s);
    }
}

/// The stalls noted since the last call.
pub fn take_stalls() -> Vec<Stall> {
    STALLS
        .lock()
        .map(|mut v| std::mem::take(&mut *v))
        .unwrap_or_default()
}

/// The part that took longest so far this cycle.
impl Crumbs {
    pub fn worst(&self) -> Option<&'static str> {
        self.parts.iter().max_by_key(|(_, d)| *d).map(|(n, _)| *n)
    }
}

/// Seconds past the whole minute (by the wall clock) that `ago` before now was.
pub fn past_minute(ago: Duration) -> f64 {
    std::time::SystemTime::now()
        .checked_sub(ago)
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0.0, |d| {
            (d.as_secs() % 60) as f64 + f64::from(d.subsec_millis()) / 1000.0
        })
}

/// The processor time the calling thread has had: how much of a stall the
/// main thread spent working, and how much waiting (or not given a processor).
#[cfg(unix)]
pub fn thread_cpu() -> Option<Duration> {
    let mut t = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: `t` is a valid timespec that lives through the call.
    let ok = unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
    (ok == 0).then(|| Duration::new(t.tv_sec as u64, t.tv_nsec as u32))
}

#[cfg(windows)]
pub fn thread_cpu() -> Option<Duration> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut made, mut ended, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: the pseudo handle for this thread is always valid, and the four
    // times live through the call.
    let ok = unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut made,
            &mut ended,
            &mut kernel,
            &mut user,
        )
    };
    let ticks = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    // In units of 100 ns (counted in whole scheduler quanta, about 16 ms).
    (ok != 0).then(|| Duration::from_nanos((ticks(kernel) + ticks(user)) * 100))
}

#[cfg(not(any(unix, windows)))]
pub fn thread_cpu() -> Option<Duration> {
    None
}

/// The most a thread that only sleeps for 10 ms at a time has overslept since
/// the last call. Large while the main loop stalls: the whole program (or the
/// computer) was held up, not the main thread alone.
pub fn pulse_late() -> Duration {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LATE_US: AtomicU64 = AtomicU64::new(0);
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let _ = std::thread::Builder::new()
            .name("stall-pulse".into())
            .spawn(|| {
                let step = Duration::from_millis(10);
                loop {
                    let t = Instant::now();
                    std::thread::sleep(step);
                    let late = t.elapsed().saturating_sub(step);
                    LATE_US.fetch_max(late.as_micros() as u64, Ordering::Relaxed);
                }
            });
    });
    Duration::from_micros(LATE_US.swap(0, Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Times are put in by hand: a runner that oversleeps would otherwise
    // make the sums wrong.
    #[test]
    fn a_slow_part_is_named_and_the_rest_is_counted_as_outside() {
        let mut c = Crumbs::new();
        c.add("slow part", Duration::from_millis(60));
        c.add("quick part", Duration::from_millis(2));
        let r = c.report(Duration::from_millis(200));
        assert!(r.starts_with("slow part 60 ms"), "{r}");
        assert!(!r.contains("quick part"), "{r}");
        assert!(r.contains("138 ms of 200 ms was outside"), "{r}");
        // A new cycle starts empty.
        assert!(c.report(Duration::from_millis(5)).contains("no part"));
    }

    #[test]
    fn the_same_part_twice_adds_up_and_the_worst_comes_first() {
        let mut c = Crumbs::new();
        c.add("events", Duration::from_millis(20));
        c.add("engine", Duration::from_millis(30));
        c.add("events", Duration::from_millis(20));
        let r = c.report(Duration::from_millis(100));
        assert!(r.starts_with("events 40 ms, engine 30 ms"), "{r}");
    }

    #[test]
    fn a_summary_names_the_slow_parts_and_keeps_the_cycle() {
        let mut c = Crumbs::new();
        assert_eq!(c.summary(), "no part took long");
        c.add("quick", Duration::from_millis(1));
        c.add("slow", Duration::from_millis(900));
        assert_eq!(c.summary(), "slow 900 ms");
        assert_eq!(c.parts.len(), 2);
        assert_eq!(c.worst(), Some("slow"));
    }

    #[test]
    fn stalls_are_kept_until_taken() {
        let _ = take_stalls();
        note_stall(Stall {
            at: Instant::now(),
            ms: 120,
            part: "test mode",
            past_minute: 0.02,
            text: "test mode 119 ms".into(),
        });
        let got = take_stalls();
        assert!(got.iter().any(|s| s.ms == 120 && s.part == "test mode"));
        assert!(take_stalls().iter().all(|s| s.ms != 120));
    }

    #[test]
    fn a_busy_thread_has_processor_time_and_a_minute_has_sixty_seconds() {
        let Some(before) = thread_cpu() else { return };
        let t = Instant::now();
        let mut x = 0u64;
        while t.elapsed() < Duration::from_millis(60) {
            x = x.wrapping_add(std::hint::black_box(1));
        }
        assert!(x > 0);
        let used = thread_cpu().unwrap() - before;
        assert!(used >= Duration::from_millis(15), "{used:?}");
        // Asleep is not working.
        let before = thread_cpu().unwrap();
        std::thread::sleep(Duration::from_millis(80));
        assert!(thread_cpu().unwrap() - before < Duration::from_millis(40));
        let p = past_minute(Duration::from_secs(3));
        assert!((0.0..60.0).contains(&p), "{p}");
    }

    #[test]
    fn a_lap_charges_the_time_since_the_last_one() {
        let mut c = Crumbs::new();
        c.lap("first");
        // Whatever the clock did, the names are there once and in order.
        c.lap("second");
        assert_eq!(c.parts.len(), 2);
    }
}
