//! A bytes-per-second cap shared by every transfer that uses it.

use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub struct RateCap {
    bytes_per_second: u64,
    /// When the bytes taken so far are paid for.
    paid_until: Mutex<Option<Instant>>,
}

impl RateCap {
    pub fn new(bytes_per_second: u64) -> RateCap {
        RateCap {
            bytes_per_second: bytes_per_second.max(1),
            paid_until: Mutex::new(None),
        }
    }

    /// Account for `n` bytes, waiting as long as the cap needs (in short
    /// steps, so `stop` is noticed). False if stopped meanwhile.
    pub fn take(&self, n: u64, stop: &AtomicBool) -> bool {
        let cost = Duration::from_secs_f64(n as f64 / self.bytes_per_second as f64);
        let now = Instant::now();
        let start = {
            let mut p = self.paid_until.lock();
            // Credit for time unused is not saved up beyond a quarter second,
            // so a pause isn't followed by a burst.
            let floor = now.checked_sub(Duration::from_millis(250)).unwrap_or(now);
            let start = p.map_or(now, |t| t.max(floor));
            *p = Some(start + cost);
            start
        };
        let until = start + cost;
        loop {
            if stop.load(Ordering::Relaxed) {
                return false;
            }
            let now = Instant::now();
            if now >= until {
                return true;
            }
            std::thread::sleep((until - now).min(Duration::from_millis(100)));
        }
    }
}

/// Take from a cap if there is one.
pub fn take(cap: Option<&RateCap>, n: u64, stop: &AtomicBool) -> bool {
    cap.is_none_or(|c| c.take(n, stop))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cap_spaces_the_bytes_out() {
        let cap = RateCap::new(1_000_000);
        let stop = AtomicBool::new(false);
        let t = Instant::now();
        for _ in 0..10 {
            assert!(cap.take(50_000, &stop));
        }
        // Half a megabyte at a megabyte a second: about half a second, at least
        // a quarter (the credit allowed) less.
        assert!(
            t.elapsed() >= Duration::from_millis(200),
            "{:?}",
            t.elapsed()
        );
        stop.store(true, Ordering::Relaxed);
        assert!(!cap.take(10_000_000, &stop));
    }
}
