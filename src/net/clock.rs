//! A follower's estimate of the primary's clock (the shared clock), from
//! timestamped request and reply exchanges, as NTP makes them: the follower
//! sends at its own `t1`, the primary receives at `t2` and replies at `t3` by
//! the shared clock, and the reply arrives at the follower's `t4`. Pure: the
//! times come in, nothing here reads a clock or a socket.
//!
//! Each exchange gives an offset (shared less local, at the middle of the
//! exchange) and a round trip. A sample's offset is wrong by at most half its
//! round trip, and by less the shorter it was, so the fit weighs samples by how
//! close their round trip is to the shortest seen: a weighted straight line
//! through the offsets over a window (a few minutes) gives the offset now and
//! the drift (the local clock's rate against the shared one). One-way delays
//! that differ steadily (an asymmetric path) shift every sample alike; nothing
//! can see that, and it stays as an error of half the difference.
//!
//! The time given never goes back. Until it is locked it may jump forward to a
//! better estimate; once locked it is slewed (run a little fast or slow, at most
//! `Settings::max_slew` apart) rather than jumped.

use std::cell::Cell;
use std::collections::VecDeque;

/// One request and reply: `t1` and `t4` by the local clock, `t2` and `t3` by
/// the shared one, all in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Exchange {
    pub t1: f64,
    pub t2: f64,
    pub t3: f64,
    pub t4: f64,
}

impl Exchange {
    /// Shared less local, at the middle of the exchange (if the path took as
    /// long each way).
    pub fn offset(&self) -> f64 {
        ((self.t2 - self.t1) + (self.t3 - self.t4)) / 2.0
    }

    /// The round trip, less the time the primary held the request.
    pub fn delay(&self) -> f64 {
        ((self.t4 - self.t1) - (self.t3 - self.t2)).max(0.0)
    }

    /// The middle of the exchange by the local clock.
    pub fn local_mid(&self) -> f64 {
        (self.t1 + self.t4) / 2.0
    }

    fn sane(&self) -> bool {
        [self.t1, self.t2, self.t3, self.t4]
            .iter()
            .all(|t| t.is_finite())
            && self.t4 >= self.t1
            && self.t3 >= self.t2
    }
}

/// The shared time as the follower believes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SharedNow {
    /// Seconds by the shared clock.
    pub time: f64,
    /// How far off it may be, in seconds (half the shortest round trip that
    /// agrees with the fit, plus what drift and an unfinished slew add): a
    /// bound, usually far above the real error.
    pub error: f64,
    /// Settled: corrections since have been small, and are slewed.
    pub locked: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// Samples kept, and for how long (seconds).
    pub window: usize,
    pub window_secs: f64,
    /// The span of samples (seconds) needed before the drift is fitted; until
    /// then the last fitted rate (or none) is kept.
    pub min_span: f64,
    /// Round-trip noise floor (seconds): a sample this much above the
    /// shortest still weighs half as much as the shortest.
    pub floor: f64,
    /// A correction is spread over at least this long (seconds) once locked.
    pub slew_secs: f64,
    /// How much faster or slower than the fitted rate the clock may run while
    /// it corrects: locked, and before.
    pub max_slew: f64,
    pub max_slew_unlocked: f64,
    /// Locked after this many samples, once this many corrections in a row
    /// were under `lock_error` (seconds).
    pub lock_samples: usize,
    pub lock_calm: u32,
    pub lock_error: f64,
    /// Unlocked again after `lock_calm` corrections in a row over this.
    pub unlock_error: f64,
    /// How fast the drift may wander from the fit (per second of age).
    pub drift_doubt: f64,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            window: 256,
            window_secs: 300.0,
            min_span: 10.0,
            floor: 0.000_1,
            slew_secs: 2.0,
            max_slew: 500e-6,
            max_slew_unlocked: 0.01,
            lock_samples: 8,
            lock_calm: 3,
            lock_error: 0.001,
            unlock_error: 0.025,
            drift_doubt: 1e-6,
        }
    }
}

/// How far (seconds) beyond half its round trip and the error bound a sample's
/// offset may be from the fit before it is taken for a step of the primary's clock.
const STEP_TOLERANCE: f64 = 0.005;

#[derive(Clone, Copy, Debug)]
struct Sample {
    mid: f64,
    offset: f64,
    delay: f64,
}

/// The offset as a line: `a + b * (local - at)`.
#[derive(Clone, Copy, Debug)]
struct Fit {
    a: f64,
    b: f64,
    at: f64,
}

impl Fit {
    fn offset(&self, local: f64) -> f64 {
        self.a + self.b * (local - self.at)
    }
}

/// A follower's shared clock.
#[derive(Debug)]
pub struct SharedClock {
    s: Settings,
    samples: VecDeque<Sample>,
    fit: Option<Fit>,
    /// The clock given: `base_shared` at `base_local`, running at `rate` plus
    /// `slew` until `slew_until` (local), at `rate` after.
    base_local: f64,
    base_shared: f64,
    rate: f64,
    slew: f64,
    slew_until: f64,
    started: bool,
    locked: bool,
    calm: u32,
    wild: u32,
    /// The error bound at `bound_at` (local).
    bound: f64,
    bound_at: f64,
    /// The highest time given, so none is ever given lower.
    given: Cell<f64>,
    /// Samples that disagree with the fit by more than their round trip
    /// allows, kept aside: `Settings::lock_calm` of them in a row mean the
    /// primary's clock was stepped (or restarted), and they replace the rest.
    suspects: Vec<Sample>,
    /// Corrections made (for the trial's report): steps and their sizes.
    pub steps: u32,
}

impl Default for SharedClock {
    fn default() -> SharedClock {
        SharedClock::new(Settings::default())
    }
}

impl SharedClock {
    pub fn new(s: Settings) -> SharedClock {
        SharedClock {
            s,
            samples: VecDeque::new(),
            fit: None,
            base_local: 0.0,
            base_shared: 0.0,
            rate: 1.0,
            slew: 0.0,
            slew_until: 0.0,
            started: false,
            locked: false,
            calm: 0,
            wild: 0,
            bound: f64::INFINITY,
            bound_at: 0.0,
            given: Cell::new(f64::NEG_INFINITY),
            suspects: Vec::new(),
            steps: 0,
        }
    }

    pub fn locked(&self) -> bool {
        self.locked
    }

    /// The fitted drift: shared seconds per local second, less 1.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn drift(&self) -> f64 {
        self.fit.map_or(0.0, |f| f.b)
    }

    /// The clock given, before the never-back guard.
    fn raw(&self, local: f64) -> f64 {
        let fast = self.rate + self.slew;
        if local <= self.slew_until {
            self.base_shared + (local - self.base_local) * fast
        } else {
            let to = (self.slew_until - self.base_local).max(0.0);
            self.base_shared + to * fast + (local - self.base_local - to) * self.rate
        }
    }

    /// The shared time at `local` (the follower's own clock, seconds), once
    /// there has been an exchange. Never lower than a time given before.
    pub fn now(&self, local: f64) -> Option<SharedNow> {
        if !self.started || !local.is_finite() {
            return None;
        }
        let time = self.raw(local).max(self.given.get());
        self.given.set(time);
        let left = self.slew.abs() * (self.slew_until - local).max(0.0);
        let error = self.bound + self.s.drift_doubt * (local - self.bound_at).abs() + left;
        Some(SharedNow {
            time,
            error,
            locked: self.locked,
        })
    }

    /// Take an exchange that completed by `local` (normally its `t4`).
    pub fn update(&mut self, ex: Exchange, local: f64) {
        if !ex.sane() || !local.is_finite() {
            return;
        }
        let sample = Sample {
            mid: ex.local_mid(),
            offset: ex.offset(),
            delay: ex.delay(),
        };
        // An offset that neither its round trip nor the fit's own doubt can
        // explain: the primary's clock moved.
        let odd = self.fit.is_some_and(|f| {
            let room = sample.delay / 2.0 + self.bound + STEP_TOLERANCE;
            (sample.offset - f.offset(sample.mid)).abs() > room
        });
        if odd {
            self.suspects.push(sample);
            if self.suspects.len() < self.s.lock_calm as usize {
                return;
            }
            // A line through both sides of a step would be wrong on each.
            self.samples.clear();
            self.samples.extend(self.suspects.drain(..));
        } else {
            self.suspects.clear();
            self.samples.push_back(sample);
        }
        while self.samples.len() > self.s.window
            || self
                .samples
                .front()
                .is_some_and(|f| f.mid < local - self.s.window_secs)
        {
            self.samples.pop_front();
        }
        let fit = self.fit_line(local);
        self.fit = Some(fit);
        let target = local + fit.offset(local);
        let rate = 1.0 + fit.b;
        if !self.started {
            self.started = true;
            self.base_local = local;
            self.base_shared = target;
            self.rate = rate;
            self.slew = 0.0;
            self.slew_until = local;
            self.bound = self.bound_now(&fit, local);
            self.bound_at = local;
            return;
        }
        let current = self.raw(local).max(self.given.get());
        let e = target - current;
        self.base_local = local;
        self.base_shared = current;
        self.rate = rate;
        if !self.locked && e > 0.0 {
            // Before it is locked, forward is a jump.
            self.base_shared += e;
            self.slew = 0.0;
            self.slew_until = local;
            if e > self.s.lock_error {
                self.steps += 1;
            }
        } else {
            let most = if self.locked {
                self.s.max_slew
            } else {
                self.s.max_slew_unlocked
            };
            let secs = (e.abs() / most).max(self.s.slew_secs);
            self.slew = e / secs;
            self.slew_until = local + secs;
        }
        if e.abs() < self.s.lock_error {
            self.calm += 1;
        } else {
            self.calm = 0;
        }
        if !self.locked
            && self.samples.len() >= self.s.lock_samples
            && self.calm >= self.s.lock_calm
        {
            self.locked = true;
            self.wild = 0;
        } else if self.locked {
            if e.abs() > self.s.unlock_error {
                self.wild += 1;
                if self.wild >= self.s.lock_calm {
                    self.locked = false;
                    self.calm = 0;
                }
            } else {
                self.wild = 0;
            }
        }
        self.bound = self.bound_now(&fit, local);
        self.bound_at = local;
    }

    /// The weighted line through the samples' offsets.
    fn fit_line(&self, local: f64) -> Fit {
        let shortest = self
            .samples
            .iter()
            .map(|s| s.delay)
            .fold(f64::INFINITY, f64::min);
        let weight = |s: &Sample| {
            let over = s.delay - shortest + self.s.floor;
            1.0 / (over * over)
        };
        let (first, last) = (
            self.samples.front().map_or(local, |s| s.mid),
            self.samples.back().map_or(local, |s| s.mid),
        );
        let at = last;
        let prior = self.fit.map_or(0.0, |f| f.b);
        let (mut sw, mut sx, mut sy, mut sxx, mut sxy) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for s in &self.samples {
            let w = weight(s);
            let x = s.mid - at;
            sw += w;
            sx += w * x;
            sy += w * s.offset;
            sxx += w * x * x;
            sxy += w * x * s.offset;
        }
        let det = sw * sxx - sx * sx;
        if last - first >= self.s.min_span && self.samples.len() >= 4 && det > 0.0 {
            let b = (sw * sxy - sx * sy) / det;
            // A wild slope (too few good samples) is not believed.
            if b.abs() < 0.001 {
                let a = (sy - b * sx) / sw;
                return Fit { a, b, at };
            }
        }
        // Too short to see the drift: the offset alone, with the rate as it was.
        let a = (sy - prior * sx) / sw;
        Fit { a, b: prior, at }
    }

    /// Half the round trip of the sample that best agrees with the fit, plus
    /// how far its offset (carried to `local` by the fit's rate) is from it.
    fn bound_now(&self, fit: &Fit, local: f64) -> f64 {
        let at = fit.offset(local);
        self.samples
            .iter()
            .map(|s| {
                let carried = s.offset + fit.b * (local - s.mid);
                s.delay / 2.0 + (carried - at).abs() + self.s.drift_doubt * (local - s.mid).abs()
            })
            .fold(f64::INFINITY, f64::min)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An exchange with the local clock at `offset` behind the shared one and
    /// running `ppm` fast, sent at true time `t`, `out` and `back` long.
    fn exchange(t: f64, offset: f64, ppm: f64, out: f64, back: f64) -> Exchange {
        shifted(t, offset, ppm, out, back, 0.0)
    }

    /// The same with the primary's clock put `shift` ahead of true time.
    fn shifted(t: f64, offset: f64, ppm: f64, out: f64, back: f64, shift: f64) -> Exchange {
        let local = |true_t: f64| true_t * (1.0 + ppm * 1e-6) - offset;
        Exchange {
            t1: local(t),
            t2: t + out + shift,
            t3: t + out + 0.000_05 + shift,
            t4: local(t + out + 0.000_05 + back),
        }
    }

    #[test]
    fn an_exchange_gives_the_offset_and_the_round_trip() {
        let ex = exchange(10.0, 3.0, 0.0, 0.002, 0.002);
        assert!((ex.offset() - 3.0).abs() < 1e-9);
        assert!((ex.delay() - 0.004).abs() < 1e-9);
        // An asymmetric path is wrong by half the difference, and nothing can tell.
        let ex = exchange(10.0, 3.0, 0.0, 0.003, 0.001);
        assert!((ex.offset() - 3.001).abs() < 1e-9);
    }

    #[test]
    fn nothing_is_given_before_an_exchange_and_bad_times_are_ignored() {
        let mut c = SharedClock::default();
        assert_eq!(c.now(1.0), None);
        c.update(
            Exchange {
                t1: f64::NAN,
                t2: 0.0,
                t3: 0.0,
                t4: 1.0,
            },
            1.0,
        );
        assert_eq!(c.now(1.0), None);
    }

    #[test]
    fn it_finds_the_offset_and_the_drift_and_locks() {
        let (offset, ppm) = (5.25, 50.0);
        let mut c = SharedClock::default();
        let mut t: f64 = 0.0;
        let mut locked_at = None;
        while t < 120.0 {
            let ex = exchange(t, offset, ppm, 0.001, 0.001);
            c.update(ex, ex.t4);
            if c.locked() && locked_at.is_none() {
                locked_at = Some(t);
            }
            t += 0.5;
        }
        assert!(locked_at.is_some_and(|t| t < 10.0), "{locked_at:?}");
        assert!((c.drift() + 50e-6).abs() < 1e-6, "{}", c.drift());
        let local = 120.0 * (1.0 + ppm * 1e-6) - offset;
        let now = c.now(local).unwrap();
        assert!((now.time - 120.0).abs() < 50e-6, "{}", now.time - 120.0);
        assert!(now.locked && now.error < 0.002, "{now:?}");
    }

    #[test]
    fn the_time_never_goes_back_and_once_locked_never_jumps() {
        let mut c = SharedClock::default();
        let mut last = f64::NEG_INFINITY;
        let mut t: f64 = 0.0;
        let offset = 2.0;
        let mut shift = 0.0;
        while t < 200.0 {
            // The primary's clock is put back by 40 ms half way: a correction
            // that must be slewed, not stepped back.
            if (t - 100.0).abs() < 1e-9 {
                shift = -0.04;
            }
            let ex = shifted(t, offset, 0.0, 0.001, 0.001, shift);
            c.update(ex, ex.t4);
            for k in 0..10 {
                let local = t + 0.05 * k as f64 + 0.01 - offset;
                let now = c.now(local).unwrap();
                assert!(now.time >= last, "went back at {t}");
                if last.is_finite() && t > 5.0 {
                    // At most 0.05 s of clock and the largest slew between looks.
                    assert!(now.time - last < 0.05 * 1.011, "jumped at {t}");
                }
                last = now.time;
            }
            t += 0.5;
        }
        // It has come round to the primary's clock by now.
        let now = c.now(200.0 - offset).unwrap();
        assert!((now.time - 199.96).abs() < 0.001, "{}", now.time);
    }

    #[test]
    fn short_round_trips_win_over_long_ones() {
        // Every third reply is held up 30 ms on the way back: those samples are
        // 15 ms off, and are not believed.
        let mut c = SharedClock::default();
        let mut t: f64 = 0.0;
        let mut k = 0;
        while t < 60.0 {
            let back = if k % 3 == 0 { 0.031 } else { 0.001 };
            let ex = exchange(t, 1.0, 20.0, 0.001, back);
            c.update(ex, ex.t4);
            t += 0.5;
            k += 1;
        }
        let local = 60.0 * (1.0 + 20e-6) - 1.0;
        let now = c.now(local).unwrap();
        assert!((now.time - 60.0).abs() < 200e-6, "{}", now.time - 60.0);
    }
}
