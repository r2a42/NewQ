//! What a test notes of the program's hitches as they happen: the main loop's
//! stalls of 100 ms or more (`crumbs::take_stalls`, with what the loop was
//! doing) and the sound device's dropouts (`audio::stream_errors`), each with
//! its time in the test. Cheap: a lock and an atomic each frame. Shared by the
//! cue and timeline tests and the full check; `analysis` judges what it keeps.

use super::analysis::{self, Finding, StallNote};
use std::time::Instant;

pub struct Hitches {
    started: Instant,
    pub stalls: Vec<StallNote>,
    /// Seconds into the test of each dropout.
    pub dropouts: Vec<f64>,
    errors: u64,
}

impl Hitches {
    /// From now: what came before the test is not its.
    pub fn new(started: Instant) -> Hitches {
        let _ = crate::crumbs::take_stalls();
        Hitches {
            started,
            stalls: Vec::new(),
            dropouts: Vec::new(),
            errors: crate::media::audio::stream_errors(),
        }
    }

    /// Take note of what has happened since the last look: the lines for
    /// `events.log`, each with its time in the test.
    pub fn tick(&mut self, now: Instant) -> Vec<(f64, String)> {
        let mut lines = Vec::new();
        for s in crate::crumbs::take_stalls() {
            let t = s.at.saturating_duration_since(self.started).as_secs_f64();
            if s.at < self.started {
                continue;
            }
            lines.push((
                t,
                format!("The main loop stalled for {} ms: {}", s.ms, s.text),
            ));
            self.stalls.push(StallNote {
                t,
                ms: s.ms,
                part: s.part.to_string(),
                past_minute: s.past_minute,
            });
        }
        self.noted_errors(crate::media::audio::stream_errors(), now, &mut lines);
        lines
    }

    fn noted_errors(&mut self, count: u64, now: Instant, lines: &mut Vec<(f64, String)>) {
        let t = now.saturating_duration_since(self.started).as_secs_f64();
        while self.errors < count {
            self.errors += 1;
            self.dropouts.push(t);
            lines.push((
                t,
                format!(
                    "The sound device reported a dropout (a buffer underrun), {} in this test so far; newq.log says more",
                    self.dropouts.len()
                ),
            ));
        }
    }

    /// The findings, for a run `hours` long playing on `device` (None: no
    /// working sound device).
    pub fn findings(&self, hours: f64, device: Option<&str>) -> Vec<Finding> {
        let mut f = analysis::analyse_stalls(&self.stalls, hours);
        f.extend(analysis::analyse_dropouts(&self.dropouts, hours, device));
        f
    }

    /// The report's lines.
    pub fn summary(&self, device: Option<&str>) -> [String; 2] {
        [
            analysis::stall_summary(&self.stalls),
            analysis::dropout_summary(&self.dropouts, device),
        ]
    }
}

/// The sound device a test plays on, if it is working.
pub fn working_device(engine: &crate::engine::Engine) -> Option<String> {
    engine
        .audio
        .device_status(None)
        .filter(|d| d.error.is_none())
        .map(|d| d.name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn dropouts_are_noted_with_their_time_and_one_line_each() {
        let t0 = Instant::now();
        let mut h = Hitches::new(t0);
        h.errors = 5;
        let mut lines = Vec::new();
        h.noted_errors(5, t0 + Duration::from_secs(3), &mut lines);
        assert!(lines.is_empty());
        h.noted_errors(7, t0 + Duration::from_secs(90), &mut lines);
        assert_eq!(h.dropouts, vec![90.0, 90.0]);
        assert_eq!(lines.len(), 2);
        assert!(lines[1].1.contains("2 in this test so far"), "{:?}", lines);
        let [stalls, sound] = h.summary(Some("Speakers"));
        assert!(sound.contains("2 on Speakers"), "{sound}");
        assert!(stalls.contains("never") || stalls.contains("held up"));
    }
}
