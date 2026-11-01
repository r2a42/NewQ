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

    /// Forget the cycle (nothing was wrong).
    pub fn next_cycle(&mut self) {
        self.parts.clear();
        self.mark = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn a_slow_part_is_named_and_the_rest_is_counted_as_outside() {
        let mut c = Crumbs::new();
        c.skip();
        sleep(Duration::from_millis(60));
        c.lap("slow part");
        c.lap("quick part");
        let r = c.report(Duration::from_millis(200));
        assert!(
            r.starts_with("slow part 6") || r.starts_with("slow part 7"),
            "{r}"
        );
        assert!(!r.contains("quick part"), "{r}");
        assert!(r.contains("of 200 ms was outside"), "{r}");
        // A new cycle starts empty.
        assert!(c.report(Duration::from_millis(5)).contains("no part"));
    }

    #[test]
    fn the_same_part_twice_adds_up() {
        let mut c = Crumbs::new();
        for _ in 0..2 {
            sleep(Duration::from_millis(20));
            c.lap("events");
        }
        let r = c.report(Duration::from_millis(100));
        assert!(r.starts_with("events 4"), "{r}");
    }
}
