//! How fast pictures are being drawn: a frame counter that reads out once a
//! second, for the Preview pane's readout and the overlay on an output.

use std::time::{Duration, Instant};

/// Frames further apart than this are not "playing continuously": the count
/// starts again.
const GAP: Duration = Duration::from_millis(400);

/// How long a reading goes on being shown after the last frame.
pub const FRESH: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct FpsMeter {
    began: Option<Instant>,
    last: Option<Instant>,
    frames: u32,
}

impl FpsMeter {
    /// A frame was drawn at `now`. Once a second of continuous frames has been
    /// seen, returns the rate over it.
    pub fn tick(&mut self, now: Instant) -> Option<f32> {
        let gap = self.last.map(|l| now.duration_since(l));
        self.last = Some(now);
        if gap.is_none_or(|g| g > GAP) {
            self.began = Some(now);
            self.frames = 0;
            return None;
        }
        self.frames += 1;
        let began = self.began?;
        let span = now.duration_since(began);
        if span >= Duration::from_secs(1) {
            let fps = self.frames as f32 / span.as_secs_f32();
            self.began = Some(now);
            self.frames = 0;
            return Some(fps);
        }
        None
    }
}

/// A reading as the pane and the overlay write it.
pub fn text(fps: f32) -> String {
    format!("{fps:.1} fps")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(m: &mut FpsMeter, start: Instant, every_ms: u64, count: u64) -> Vec<f32> {
        (0..count)
            .filter_map(|i| m.tick(start + Duration::from_millis(i * every_ms)))
            .collect()
    }

    #[test]
    fn steady_frames_read_out_their_rate_once_a_second() {
        let mut m = FpsMeter::default();
        let r = feed(&mut m, Instant::now(), 20, 260);
        assert_eq!(r.len(), 5, "{r:?}");
        for v in r {
            assert!((v - 50.0).abs() < 1.5, "{v}");
        }
    }

    #[test]
    fn a_pause_starts_the_count_again_and_reads_nothing() {
        let mut m = FpsMeter::default();
        let t = Instant::now();
        assert!(feed(&mut m, t, 100, 5).is_empty());
        // Nothing for two seconds, then frames: the old ones don't count.
        let r = feed(&mut m, t + Duration::from_secs(3), 10, 250);
        assert_eq!(r.len(), 2, "{r:?}");
        assert!((r[0] - 100.0).abs() < 3.0, "{r:?}");
    }

    #[test]
    fn the_text_has_one_decimal() {
        assert_eq!(text(59.94), "59.9 fps");
    }
}
