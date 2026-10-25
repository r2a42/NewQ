//! Clock triggers: a cue that fires when the time of day reaches a set time,
//! such as house-open music at 18:30.
//!
//! Times are local, to the second. Like timecode, a cue fires only for time
//! that passes while NewQ runs: a show opened at 18:31 doesn't start the 18:30
//! cue, a laptop that slept through it doesn't start it on waking, and moving
//! the clock doesn't fire the cues it skips. If NewQ itself was busy for a
//! while (up to a minute) rather than asleep, the time did pass, and the cues
//! that came due meanwhile fire when it looks again. The follower is plain functions
//! on the times it is given; only `now` reads the clock.

use crate::show::CueId;
use serde::{Deserialize, Serialize};
use std::fmt;

const DAY: u32 = 24 * 3600;
/// More than this many seconds between two looks at the clock is a jump
/// (sleep, or the clock set), not time passing...
const JUMP_SECONDS: u32 = 5;
/// ...unless that long really did go by, by the program's own steady clock,
/// and not much more than this: NewQ was busy (a dialog, a stall), not asleep.
const STALL_SECONDS: f64 = 60.0;

/// A time of day, to the second.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimeOfDay(u32);

impl TimeOfDay {
    pub fn new(hours: u32, minutes: u32, seconds: u32) -> Option<TimeOfDay> {
        (hours < 24 && minutes < 60 && seconds < 60)
            .then_some(TimeOfDay(hours * 3600 + minutes * 60 + seconds))
    }

    pub fn hms(self) -> (u32, u32, u32) {
        (self.0 / 3600, self.0 % 3600 / 60, self.0 % 60)
    }

    /// Read "18:30" or "18:30:15".
    pub fn parse(text: &str) -> Option<TimeOfDay> {
        let parts: Vec<u32> = text
            .trim()
            .split(':')
            .map(|p| p.trim().parse().ok())
            .collect::<Option<_>>()?;
        match parts[..] {
            [h, m] => TimeOfDay::new(h, m, 0),
            [h, m, s] => TimeOfDay::new(h, m, s),
            _ => None,
        }
    }

    /// Seconds from `self` forward to `later`, wrapping past midnight.
    fn until(self, later: TimeOfDay) -> u32 {
        (later.0 + DAY - self.0) % DAY
    }
}

impl fmt::Display for TimeOfDay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (h, m, s) = self.hms();
        write!(f, "{h:02}:{m:02}:{s:02}")
    }
}

// Shown in the show file as "18:30:00".
impl Serialize for TimeOfDay {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TimeOfDay {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<TimeOfDay, D::Error> {
        let text = String::deserialize(d)?;
        TimeOfDay::parse(&text).ok_or_else(|| serde::de::Error::custom("not a time of day"))
    }
}

/// The local time of day now.
pub fn now() -> TimeOfDay {
    let t = jiff::Zoned::now().time();
    TimeOfDay::new(
        t.hour().unsigned_abs().into(),
        t.minute().unsigned_abs().into(),
        t.second().unsigned_abs().into(),
    )
    .unwrap_or_default()
}

/// Follows the clock and says which cues it passes.
#[derive(Default)]
pub struct ClockFollower {
    last: Option<TimeOfDay>,
}

impl ClockFollower {
    /// The clock now reads `now`. Returns the cues, from `triggers`, whose time
    /// has passed since the last look, in the order they came due.
    #[cfg(test)]
    pub fn advance(&mut self, triggers: &[(CueId, TimeOfDay)], now: TimeOfDay) -> Vec<CueId> {
        self.advance_after(triggers, now, 0.0)
    }

    /// `advance`, given how many seconds went by since the last look on the
    /// program's own steady clock. A longer gap than a moment that the clock
    /// agrees with (within a couple of seconds, and under a minute) is the
    /// program having been busy, so the time counts as having passed; one it
    /// doesn't agree with is a sleep or the clock being set.
    pub fn advance_after(
        &mut self,
        triggers: &[(CueId, TimeOfDay)],
        now: TimeOfDay,
        gap: f64,
    ) -> Vec<CueId> {
        let fired = match self.last {
            Some(prev) if prev != now => {
                let ahead = prev.until(now);
                // Behind by less than half a day is the clock set back, not a
                // new day; and ahead by more than a moment is a jump.
                let set_back = ahead > DAY / 2;
                let busy = gap <= STALL_SECONDS && (f64::from(ahead) - gap).abs() <= 2.0;
                if set_back || (ahead > JUMP_SECONDS && !busy) {
                    Vec::new()
                } else {
                    let mut due: Vec<(u32, CueId)> = triggers
                        .iter()
                        .filter_map(|(id, t)| {
                            let d = prev.until(*t);
                            (d > 0 && d <= ahead).then_some((d, *id))
                        })
                        .collect();
                    due.sort();
                    due.into_iter().map(|(_, id)| id).collect()
                }
            }
            _ => Vec::new(),
        };
        self.last = Some(now);
        fired
    }
}

/// The next trigger to come due at or after `now`, and how many seconds away
/// it is (a cue set for this very second is a day away once it has passed).
pub fn upcoming(
    triggers: &[(CueId, TimeOfDay)],
    now: TimeOfDay,
) -> Option<(CueId, TimeOfDay, u32)> {
    triggers
        .iter()
        .map(|(id, t)| {
            let d = now.until(*t);
            (*id, *t, if d == 0 { DAY } else { d })
        })
        .min_by_key(|(_, _, d)| *d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(h: u32, m: u32, s: u32) -> TimeOfDay {
        TimeOfDay::new(h, m, s).unwrap()
    }

    #[test]
    fn times_read_print_and_serialise() {
        assert_eq!(TimeOfDay::parse("18:30"), Some(t(18, 30, 0)));
        assert_eq!(TimeOfDay::parse(" 7:05:09 "), Some(t(7, 5, 9)));
        assert_eq!(TimeOfDay::parse("00:00:00"), Some(t(0, 0, 0)));
        assert_eq!(TimeOfDay::parse("23:59:59"), Some(t(23, 59, 59)));
        for bad in [
            "", "18", "24:00", "12:60", "12:00:60", "a:b", "1:2:3:4", "-1:00", "12:",
        ] {
            assert_eq!(TimeOfDay::parse(bad), None, "{bad:?}");
        }
        assert_eq!(t(7, 5, 9).to_string(), "07:05:09");
        assert_eq!(t(18, 30, 0).hms(), (18, 30, 0));
        assert!(t(9, 0, 0) < t(18, 30, 0));
        let json = serde_json::to_string(&t(18, 30, 5)).unwrap();
        assert_eq!(json, "\"18:30:05\"");
        assert_eq!(
            serde_json::from_str::<TimeOfDay>(&json).unwrap(),
            t(18, 30, 5)
        );
        assert!(serde_json::from_str::<TimeOfDay>("\"later\"").is_err());
    }

    fn cues(times: &[TimeOfDay]) -> Vec<(CueId, TimeOfDay)> {
        times
            .iter()
            .enumerate()
            .map(|(i, x)| (CueId(i as u64 + 1), *x))
            .collect()
    }

    /// Look at the clock every second from `from` for `secs` seconds.
    fn run(
        f: &mut ClockFollower,
        trig: &[(CueId, TimeOfDay)],
        from: TimeOfDay,
        secs: u32,
    ) -> Vec<CueId> {
        let mut out = Vec::new();
        let mut now = from.0;
        for _ in 0..secs {
            now = (now + 1) % DAY;
            out.extend(f.advance(trig, TimeOfDay(now)));
        }
        out
    }

    #[test]
    fn a_cue_fires_once_as_the_clock_passes_it() {
        let trig = cues(&[t(18, 30, 0), t(18, 30, 3)]);
        let mut f = ClockFollower::default();
        f.advance(&trig, t(18, 29, 55));
        let fired = run(&mut f, &trig, t(18, 29, 55), 10);
        assert_eq!(fired, [CueId(1), CueId(2)], "each once, in time order");
        assert!(
            run(&mut f, &trig, t(18, 30, 5), 60).is_empty(),
            "and never again today"
        );
    }

    #[test]
    fn looking_more_often_or_less_often_makes_no_difference() {
        let trig = cues(&[t(12, 0, 0)]);
        // Many looks in the same second.
        let mut f = ClockFollower::default();
        f.advance(&trig, t(11, 59, 59));
        let mut n = 0;
        for _ in 0..50 {
            n += f.advance(&trig, t(12, 0, 0)).len();
        }
        assert_eq!(n, 1);
        // A look only every 3 seconds.
        let mut f = ClockFollower::default();
        f.advance(&trig, t(11, 59, 58));
        assert!(f.advance(&trig, t(12, 0, 1)).len() == 1);
        assert!(f.advance(&trig, t(12, 0, 4)).is_empty());
    }

    #[test]
    fn nothing_fires_on_the_first_look() {
        let trig = cues(&[t(18, 30, 0)]);
        let mut f = ClockFollower::default();
        assert!(
            f.advance(&trig, t(18, 30, 0)).is_empty(),
            "opened at the time itself"
        );
        let mut f = ClockFollower::default();
        assert!(
            f.advance(&trig, t(18, 31, 0)).is_empty(),
            "opened a minute late"
        );
    }

    #[test]
    fn jumps_and_sleeps_do_not_fire_what_they_skip() {
        let trig = cues(&[t(18, 30, 0), t(19, 0, 0)]);
        let mut f = ClockFollower::default();
        f.advance(&trig, t(18, 0, 0));
        // The laptop sleeps from 18:00 to 18:45 (or the clock is set forward).
        assert!(f.advance(&trig, t(18, 45, 0)).is_empty());
        // Playing on from there reaches the 19:00 cue.
        assert_eq!(run(&mut f, &trig, t(18, 45, 0), 15 * 60), [CueId(2)]);
        // A look 5 seconds on still counts as time passing; 6 does not.
        let trig = cues(&[t(10, 0, 3)]);
        let mut f = ClockFollower::default();
        f.advance(&trig, t(10, 0, 0));
        assert_eq!(f.advance(&trig, t(10, 0, 5)).len(), 1);
        let mut f = ClockFollower::default();
        f.advance(&trig, t(9, 59, 58));
        assert!(f.advance(&trig, t(10, 0, 4)).is_empty());
    }

    #[test]
    fn setting_the_clock_back_fires_nothing_and_carries_on() {
        let trig = cues(&[t(10, 0, 0)]);
        let mut f = ClockFollower::default();
        f.advance(&trig, t(10, 0, 30));
        assert!(
            f.advance(&trig, t(9, 0, 0)).is_empty(),
            "an hour back: a resync"
        );
        // It then fires as the clock comes up to the time again, as it is a new pass.
        assert_eq!(run(&mut f, &trig, t(9, 59, 58), 4), [CueId(1)]);
    }

    #[test]
    fn midnight_is_just_another_second() {
        let trig = cues(&[t(23, 59, 59), t(0, 0, 0), t(0, 0, 2)]);
        let mut f = ClockFollower::default();
        f.advance(&trig, t(23, 59, 57));
        let fired = run(&mut f, &trig, t(23, 59, 57), 6);
        assert_eq!(fired, [CueId(1), CueId(2), CueId(3)]);
    }

    #[test]
    fn the_next_cue_due_and_how_long_to_wait() {
        let trig = cues(&[t(18, 30, 0), t(9, 0, 0), t(23, 0, 0)]);
        assert_eq!(
            upcoming(&trig, t(8, 0, 0)),
            Some((CueId(2), t(9, 0, 0), 3600))
        );
        assert_eq!(
            upcoming(&trig, t(10, 0, 0)),
            Some((CueId(1), t(18, 30, 0), 8 * 3600 + 1800))
        );
        // After the last of the day, the first of tomorrow.
        assert_eq!(
            upcoming(&trig, t(23, 30, 0)),
            Some((CueId(2), t(9, 0, 0), 9 * 3600 + 1800))
        );
        // One due this very second has passed: a day away.
        assert_eq!(
            upcoming(&cues(&[t(9, 0, 0)]), t(9, 0, 0)),
            Some((CueId(1), t(9, 0, 0), DAY))
        );
        assert_eq!(upcoming(&[], t(9, 0, 0)), None);
    }

    #[test]
    fn the_real_clock_gives_a_time_of_day() {
        let a = now();
        let (h, m, s) = a.hms();
        assert!(h < 24 && m < 60 && s < 60);
        // It agrees with itself a moment later (barring midnight).
        let b = now();
        assert!(a.until(b) < 5, "{a} then {b}");
    }

    #[test]
    fn time_the_program_was_busy_still_counts_but_a_sleep_does_not() {
        let trig = cues(&[t(18, 30, 0)]);
        // A dialog held the program up for 30 seconds across the cue's time:
        // the clock and the steady clock agree on how long, so it fires.
        let mut f = ClockFollower::default();
        f.advance(&trig, t(18, 29, 50));
        assert_eq!(f.advance_after(&trig, t(18, 30, 20), 30.0), [CueId(1)]);
        // The same jump with the steady clock saying it was a moment: the
        // laptop slept or the clock was set forward.
        let mut f = ClockFollower::default();
        f.advance(&trig, t(18, 29, 50));
        assert!(f.advance_after(&trig, t(18, 30, 20), 0.02).is_empty());
        // Slept for an hour on a system whose steady clock runs on through it.
        let mut f = ClockFollower::default();
        f.advance(&trig, t(17, 30, 0));
        assert!(f.advance_after(&trig, t(18, 30, 30), 3630.0).is_empty());
        // Busy for ten minutes is too long to be sure of: nothing fires.
        let mut f = ClockFollower::default();
        f.advance(&trig, t(18, 25, 0));
        assert!(f.advance_after(&trig, t(18, 35, 0), 600.0).is_empty());
        // A look 30 s on where the steady clock says 10: the clock was moved.
        let mut f = ClockFollower::default();
        f.advance(&trig, t(18, 29, 50));
        assert!(f.advance_after(&trig, t(18, 30, 20), 10.0).is_empty());
        // Nothing changes for the ordinary case of a look every frame.
        let mut f = ClockFollower::default();
        f.advance(&trig, t(18, 29, 59));
        assert_eq!(f.advance_after(&trig, t(18, 30, 0), 0.016), [CueId(1)]);
    }
}
