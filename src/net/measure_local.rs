//! What a follower measures of ITSELF while the primary runs a test across the
//! show network: the same figures Test Mode measures on a computer alone
//! (memory, processor, FFmpeg processes, picture layers, late frames, the
//! sound's load and dropouts, the slowest frame), each output's frame rate,
//! and the sound device it plays on. `net::measure` is how it keeps to the
//! primary; this is how it copes. The primary judges both per computer
//! (`testmode::fleet`).
//!
//! Pure: the app hands in the rows (`stats::StatsLog`, on its own thread) and
//! the counts; this keeps them and makes the reports (`NetTest::Local`), one
//! every `SEND_EVERY`.

use serde::{Deserialize, Serialize};

/// A report is sent this often (seconds).
pub const SEND_EVERY: f64 = 2.0;
/// A row of measurements is asked for this often (seconds).
pub const ROW_EVERY: u64 = 2;
/// At most this many rows or dropouts in one report (a burst is cut).
const MAX_ROWS: usize = 120;

/// One row of a follower's own measurements (as `stats::Sample` has them).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalSample {
    /// Seconds into the test, by the shared clock.
    pub t: f64,
    pub rss_mb: u64,
    pub cpu_pct: f32,
    pub ffmpeg: usize,
    pub ffmpeg_rss_mb: u64,
    pub ffmpeg_cpu_pct: f32,
    pub threads: Option<usize>,
    pub playing: usize,
    /// FFmpeg processes started since NewQ started.
    pub started_total: u64,
    pub layers: u32,
    pub late_permille: u32,
    pub audio_permille: u32,
    pub audio_errors: u64,
    pub stall_ms: u64,
    pub gpu_mb: Option<u64>,
    pub commit_mb: Option<u64>,
}

impl LocalSample {
    /// From a row of the app's log, `t` seconds into the test.
    pub fn of(t: f64, s: &crate::stats::Sample) -> LocalSample {
        LocalSample {
            t: (t * 100.0).round() / 100.0,
            rss_mb: s.rss_mb,
            cpu_pct: s.cpu_pct,
            ffmpeg: s.ffmpeg,
            ffmpeg_rss_mb: s.ffmpeg_rss_mb,
            ffmpeg_cpu_pct: s.ffmpeg_cpu_pct,
            threads: s.threads,
            playing: s.inputs.playing,
            started_total: s.inputs.started_total,
            layers: s.inputs.layers,
            late_permille: s.inputs.late_permille,
            audio_permille: s.inputs.audio_permille,
            audio_errors: s.inputs.audio_errors,
            stall_ms: s.stall_ms,
            gpu_mb: s.gpu_mb,
            commit_mb: s.commit_mb,
        }
    }

    /// Back into a row, for Test Mode's own judging (`testmode::analysis`).
    pub fn sample(&self) -> crate::stats::Sample {
        crate::stats::Sample {
            uptime_s: self.t.max(0.0) as u64,
            rss_mb: self.rss_mb,
            cpu_pct: self.cpu_pct,
            ffmpeg: self.ffmpeg,
            ffmpeg_rss_mb: self.ffmpeg_rss_mb,
            ffmpeg_cpu_pct: self.ffmpeg_cpu_pct,
            fds: None,
            threads: self.threads,
            inputs: crate::stats::Inputs {
                playing: self.playing,
                started_total: self.started_total,
                layers: self.layers,
                late_permille: self.late_permille,
                audio_permille: self.audio_permille,
                audio_errors: self.audio_errors,
            },
            stall_ms: self.stall_ms,
            gpu_mb: self.gpu_mb,
            commit_mb: self.commit_mb,
        }
    }
}

/// One output window's frame rate, as a computer measured it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputRate {
    pub name: String,
    pub fps: f32,
    /// Its monitor's refresh rate (Hz), if known.
    pub refresh_hz: Option<f32>,
}

impl OutputRate {
    pub fn new(name: &str, fps: f32, refresh_hz: Option<f32>) -> OutputRate {
        OutputRate {
            name: name.into(),
            fps,
            refresh_hz,
        }
    }
}

/// What a follower measured of itself since its last report.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalReport {
    pub run: u64,
    /// Counts this computer's local reports of the run.
    pub seq: u32,
    pub samples: Vec<LocalSample>,
    /// Seconds into the test of each sound dropout since the last report.
    pub dropouts: Vec<f64>,
    /// How many of them came with the system calling for sound late, not with
    /// NewQ's mixing being slow (`audio::cause`; none from 0.24.3 and before).
    pub dropouts_late: u32,
    /// Each of its output windows now: its name, frames a second and refresh.
    pub outputs: Vec<OutputRate>,
    /// The sound device it plays on, if one is working.
    pub sound_device: Option<String>,
    /// Its memory, in MB.
    pub ram_mb: u64,
    /// The last report of the run.
    pub last: bool,
}

/// A follower's own measuring, between the primary's `Begin` and `End`.
#[derive(Default)]
pub struct LocalMeter {
    run: Option<(u64, f64)>,
    pending: LocalReport,
    /// The sound devices' error count when last looked at, and how many of
    /// them came with the system calling late.
    errors: Option<u64>,
    errors_late: Option<u64>,
    next_send: f64,
    seq: u32,
}

impl LocalMeter {
    /// Start for `run` (the same run is not started twice).
    pub fn begin(&mut self, run: u64, start: f64) {
        if self.running() == Some(run) {
            return;
        }
        *self = LocalMeter {
            run: Some((run, start)),
            ..LocalMeter::default()
        };
    }

    pub fn running(&self) -> Option<u64> {
        self.run.map(|(r, _)| r)
    }

    fn at(&self, now: f64) -> f64 {
        self.run.map_or(0.0, |(_, s)| now - s)
    }

    /// A row of measurements, made at shared time `now`.
    pub fn sample(&mut self, now: f64, s: &crate::stats::Sample) {
        if self.run.is_some() && self.pending.samples.len() < MAX_ROWS {
            let t = self.at(now);
            self.pending.samples.push(LocalSample::of(t, s));
        }
    }

    /// The sound devices' error count now, `late` of them with the system
    /// calling for sound late: the new ones are dropouts.
    pub fn errors(&mut self, now: f64, count: u64, late: u64) {
        if self.run.is_none() {
            return;
        }
        let seen = *self.errors.get_or_insert(count);
        let seen_late = *self.errors_late.get_or_insert(late);
        let t = (self.at(now) * 100.0).round() / 100.0;
        let mut added = 0u64;
        for _ in seen..count {
            if self.pending.dropouts.len() < MAX_ROWS {
                self.pending.dropouts.push(t);
                added += 1;
            }
        }
        let late_new = late.saturating_sub(seen_late).min(added);
        self.pending.dropouts_late += late_new as u32;
        self.errors = Some(count.max(seen));
        self.errors_late = Some(late.max(seen_late));
    }

    /// The report to send now, if one is due (or `last`).
    pub fn report(
        &mut self,
        now: f64,
        outputs: Vec<OutputRate>,
        sound_device: Option<String>,
        ram_mb: u64,
        last: bool,
    ) -> Option<LocalReport> {
        let (run, _) = self.run?;
        if self.next_send > now + SEND_EVERY {
            self.next_send = now;
        }
        if !last && now < self.next_send {
            return None;
        }
        self.next_send = now + SEND_EVERY;
        self.seq += 1;
        let mut r = std::mem::take(&mut self.pending);
        r.run = run;
        r.seq = self.seq;
        r.outputs = outputs;
        r.sound_device = sound_device;
        r.ram_mb = ram_mb;
        r.last = last;
        Some(r)
    }

    /// The test is over: the last report (if `run` is the one measured).
    pub fn end(
        &mut self,
        run: u64,
        now: f64,
        outputs: Vec<OutputRate>,
        sound_device: Option<String>,
        ram_mb: u64,
    ) -> Option<LocalReport> {
        if self.running() != Some(run) {
            return None;
        }
        let r = self.report(now, outputs, sound_device, ram_mb, true);
        *self = LocalMeter::default();
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(rss: u64, errors: u64) -> crate::stats::Sample {
        crate::stats::Sample {
            rss_mb: rss,
            cpu_pct: 40.0,
            ffmpeg: 3,
            inputs: crate::stats::Inputs {
                playing: 2,
                late_permille: 5,
                audio_errors: errors,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn a_followers_own_rows_and_dropouts_go_in_reports_until_the_end() {
        let mut m = LocalMeter::default();
        // Nothing before the test.
        m.sample(10.0, &row(100, 0));
        assert!(m.report(10.0, vec![], None, 0, false).is_none());
        m.begin(7, 100.0);
        m.errors(100.0, 4, 1);
        m.sample(101.0, &row(300, 4));
        m.errors(102.5, 6, 2);
        let r = m
            .report(
                102.5,
                vec![OutputRate::new("Left 1", 59.9, Some(60.0))],
                Some("Speakers".into()),
                8192,
                false,
            )
            .expect("the first report is due at once");
        assert_eq!((r.run, r.seq), (7, 1));
        assert_eq!(r.samples.len(), 1);
        assert_eq!(r.samples[0].t, 1.0);
        assert_eq!(r.samples[0].rss_mb, 300);
        // Dropouts from before the test aren't its.
        assert_eq!(r.dropouts, vec![2.5, 2.5]);
        assert_eq!(
            r.dropouts_late, 1,
            "one of the two with the system calling late"
        );
        assert_eq!(r.outputs, vec![OutputRate::new("Left 1", 59.9, Some(60.0))]);
        // Not again until SEND_EVERY has passed.
        m.sample(103.0, &row(310, 6));
        assert!(m.report(103.0, vec![], None, 0, false).is_none());
        let last = m.end(7, 103.5, vec![], None, 8192).expect("the last one");
        assert!(last.last && last.seq == 2 && last.samples.len() == 1);
        assert!(m.running().is_none());
        // Another run's end is not this one's.
        m.begin(8, 0.0);
        assert!(m.end(9, 1.0, vec![], None, 0).is_none());
        assert_eq!(m.running(), Some(8));
    }

    #[test]
    fn a_row_goes_there_and_back_unchanged_for_the_judging() {
        let s = row(512, 3);
        let l = LocalSample::of(12.345, &s);
        assert_eq!(l.t, 12.35);
        let back = l.sample();
        assert_eq!(back.rss_mb, 512);
        assert_eq!(back.inputs.audio_errors, 3);
        assert_eq!(back.inputs.playing, 2);
        assert_eq!(back.uptime_s, 12);
        let json = serde_json::to_string(&LocalReport {
            samples: vec![l],
            ..Default::default()
        })
        .unwrap();
        let r: LocalReport = serde_json::from_str(&json).unwrap();
        assert_eq!(r.samples[0], l);
    }
}
