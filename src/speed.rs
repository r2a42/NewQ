//! Playback speed that changes over a cue: a map from the cue's time to the
//! file's, and a time stretcher that plays sound along it without changing
//! its pitch.
//!
//! A cue's speed is a number animated over its own time (the time the
//! operator sees run), so what is played at cue time `t` is the file at
//! `media_at(t)`, the integral of the speed. Both readers (the picture's and
//! the sound's) work from the same map, so they stay together.

use crate::show::{Animated, MAX_RATE, MIN_RATE};
use std::sync::Arc;

/// How fine the table of the integral is, in seconds of cue time.
const STEP: f64 = 0.005;
/// Most entries in the table; a longer run of keyframes is tabulated more coarsely.
const MAX_ENTRIES: usize = 400_000;

/// The file's time as a function of the cue's, for a speed that is keyframed.
#[derive(Debug)]
pub struct SpeedMap {
    /// File seconds at 0, `step`, 2 `step`... of cue time, up to the last keyframe.
    cum: Vec<f64>,
    step: f64,
    /// The speed after the last keyframe.
    last: f64,
}

fn clamp_rate(r: f32) -> f64 {
    if r.is_finite() {
        f64::from(r).clamp(MIN_RATE, MAX_RATE)
    } else {
        1.0
    }
}

impl SpeedMap {
    pub fn new(speed: &Animated) -> SpeedMap {
        let end = speed
            .keys
            .last()
            .map_or(0.0, |k| k.t.max(0.0))
            .min(36_000.0);
        let end = if end.is_finite() { end } else { 0.0 };
        let step = STEP.max(end / MAX_ENTRIES as f64);
        let n = (end / step).ceil() as usize + 1;
        let rate_at = |t: f64| clamp_rate(speed.at(t));
        let mut cum = Vec::with_capacity(n + 1);
        cum.push(0.0);
        for k in 1..=n {
            let (a, b) = (rate_at((k - 1) as f64 * step), rate_at(k as f64 * step));
            cum.push(cum[k - 1] + (a + b) * 0.5 * step);
        }
        SpeedMap {
            cum,
            step,
            last: rate_at(end + 1.0),
        }
    }

    /// The file's time, in seconds from where the cue started playing it, at
    /// `t` seconds of cue time.
    pub fn media_at(&self, t: f64) -> f64 {
        let t = t.max(0.0);
        let x = t / self.step;
        let i = x.floor() as usize;
        if i + 1 >= self.cum.len() {
            let end = (self.cum.len() - 1) as f64 * self.step;
            return self.cum[self.cum.len() - 1] + (t - end) * self.last;
        }
        self.cum[i] + (self.cum[i + 1] - self.cum[i]) * (x - i as f64)
    }

    /// The cue time at which `media` seconds of the file have been played.
    pub fn time_for(&self, media: f64) -> f64 {
        if media <= 0.0 {
            return 0.0;
        }
        let top = self.cum[self.cum.len() - 1];
        let end = (self.cum.len() - 1) as f64 * self.step;
        if media >= top {
            return end + (media - top) / self.last;
        }
        let i = self.cum.partition_point(|&c| c <= media).max(1) - 1;
        let span = (self.cum[i + 1] - self.cum[i]).max(1e-12);
        (i as f64 + (media - self.cum[i]) / span) * self.step
    }
}

/// A map and where in the cue a decoder starts, so that a decoder begun
/// part way (a seek) follows the same speed as one begun at the top.
#[derive(Clone, Debug)]
pub struct SpeedPlan {
    pub map: Arc<SpeedMap>,
    /// Cue time at which the decoder starts.
    pub t0: f64,
}

impl SpeedPlan {
    /// File seconds past the decoder's start at `x` seconds after it started.
    pub fn media_after(&self, x: f64) -> f64 {
        self.map.media_at(self.t0 + x) - self.map.media_at(self.t0)
    }

    /// Seconds after the decoder's start at which `media` seconds of the file
    /// have been played.
    pub fn time_after(&self, media: f64) -> f64 {
        self.map.time_for(self.map.media_at(self.t0) + media) - self.t0
    }
}

// ----- Time stretching ---------------------------------------------------

/// Frames in a window.
const WINDOW: usize = 2048;
/// Output frames each window adds (half a window).
const HOP: usize = WINDOW / 2;
/// How far, in frames, a window may be moved from where the speed puts it to
/// find a better join.
const SEARCH: usize = 400;

/// Plays sound along a `SpeedPlan` without changing its pitch (waveform
/// similarity overlap-add): windows of the input are laid down every `HOP`
/// frames of output, each taken from where the speed says the input has got
/// to, nudged to where it fits the last one best.
pub struct Stretcher {
    channels: usize,
    plan: SpeedPlan,
    sample_rate: f64,
    /// Interleaved input not yet dropped, and the same as one channel.
    input: Vec<f32>,
    mono: Vec<f32>,
    /// The input frame that `input` starts at.
    base: usize,
    ended: bool,
    window: Vec<f32>,
    /// Output (interleaved) still waiting for the next window to add to it.
    tail: Vec<f32>,
    /// Which window is next.
    step: u64,
    /// Where the last window began, in input frames.
    last: Option<usize>,
    done: bool,
}

impl Stretcher {
    pub fn new(channels: usize, sample_rate: u32, plan: SpeedPlan) -> Stretcher {
        let window = (0..WINDOW)
            .map(|i| {
                // Periodic Hann: windows half a window apart add up to 1.
                let x = i as f32 / WINDOW as f32;
                0.5 - 0.5 * (2.0 * std::f32::consts::PI * x).cos()
            })
            .collect();
        Stretcher {
            channels: channels.max(1),
            plan,
            sample_rate: f64::from(sample_rate.max(1)),
            input: Vec::new(),
            mono: Vec::new(),
            base: 0,
            ended: false,
            window,
            tail: vec![0.0; (WINDOW - HOP) * channels.max(1)],
            step: 0,
            last: None,
            done: false,
        }
    }

    /// Give it more input (interleaved).
    pub fn push(&mut self, samples: &[f32]) {
        let c = self.channels;
        let whole = samples.len() / c * c;
        for frame in samples[..whole].chunks_exact(c) {
            self.mono.push(frame.iter().sum::<f32>() / c as f32);
        }
        self.input.extend_from_slice(&samples[..whole]);
    }

    /// There is no more input.
    pub fn finish(&mut self) {
        self.ended = true;
    }

    /// Whether all of the output has been produced.
    #[cfg(test)]
    pub fn is_done(&self) -> bool {
        self.done
    }

    fn available(&self) -> usize {
        self.base + self.mono.len()
    }

    /// Where the speed puts the input at output frame `n`.
    fn wanted(&self, n: u64) -> f64 {
        self.plan.media_after(n as f64 / self.sample_rate) * self.sample_rate
    }

    /// The window to use for the next step: its start in the input.
    fn choose(&self, wanted: usize) -> usize {
        let Some(last) = self.last else {
            return wanted;
        };
        // What the last window leads on into, to join to.
        let lead = last + HOP;
        let lo = wanted.saturating_sub(SEARCH).max(self.base);
        let hi = wanted + SEARCH;
        let overlap = WINDOW - HOP;
        let at = |i: usize| self.mono.get(i - self.base).copied().unwrap_or(0.0);
        let score = |c: usize| -> f32 {
            let (mut dot, mut norm) = (0.0f32, 1e-9f32);
            for k in (0..overlap).step_by(2) {
                let (a, b) = (at(lead + k), at(c + k));
                dot += a * b;
                norm += b * b;
            }
            dot / norm.sqrt()
        };
        // Coarsely, then closer in.
        let mut best = (f32::MIN, wanted.clamp(lo, hi));
        let mut c = lo;
        while c <= hi {
            let s = score(c);
            if s > best.0 {
                best = (s, c);
            }
            c += 8;
        }
        let (from, to) = (best.1.saturating_sub(8).max(lo), (best.1 + 8).min(hi));
        for c in from..=to {
            let s = score(c);
            if s > best.0 {
                best = (s, c);
            }
        }
        best.1
    }

    /// Produce what output there now is the input for, appending it
    /// (interleaved) to `out`.
    pub fn pull(&mut self, out: &mut Vec<f32>) {
        let c = self.channels;
        while !self.done {
            let wanted = self.wanted(self.step * HOP as u64).round().max(0.0) as usize;
            let total = self.available();
            // Enough input for the window and where it might be moved to.
            let need = wanted + SEARCH + WINDOW;
            if total < need && !self.ended {
                return;
            }
            if self.ended && wanted >= total {
                // The input is used up: what is left of the last window is the end.
                out.extend_from_slice(&self.tail);
                self.done = true;
                return;
            }
            let start = self.choose(wanted.min(total.saturating_sub(1)));
            // Lay the window down on the tail.
            let mut frame = std::mem::take(&mut self.tail);
            frame.resize(WINDOW * c, 0.0);
            for k in 0..WINDOW {
                let w = self.window[k];
                let src = start + k;
                if src >= self.base + self.mono.len() {
                    break;
                }
                let at = (src - self.base) * c;
                for ch in 0..c {
                    frame[k * c + ch] += self.input[at + ch] * w;
                }
            }
            out.extend_from_slice(&frame[..HOP * c]);
            self.tail = frame[HOP * c..].to_vec();
            self.last = Some(start);
            self.step += 1;
            // Drop what nothing will look at again.
            let keep_from = (start + HOP).min(wanted.saturating_sub(SEARCH + HOP));
            if keep_from > self.base + 65_536 {
                let drop = keep_from - self.base;
                self.input.drain(..drop * c);
                self.mono.drain(..drop);
                self.base = keep_from;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::Keyframe;

    fn speed(keys: &[(f64, f32)]) -> Animated {
        let mut a = Animated::new(1.0);
        for &(t, v) in keys {
            a.set_key(t, v);
        }
        a
    }

    #[test]
    fn a_constant_speed_maps_time_in_proportion() {
        let m = SpeedMap::new(&Animated::new(2.0));
        // Without keyframes the value is the speed (clamped); one key is the same.
        assert!((m.media_at(3.0) - 6.0).abs() < 1e-6);
        assert!((m.time_for(6.0) - 3.0).abs() < 1e-6);
        let m = SpeedMap::new(&speed(&[(0.0, 0.5)]));
        assert!((m.media_at(10.0) - 5.0).abs() < 1e-6);
        assert!((m.time_for(5.0) - 10.0).abs() < 1e-6);
    }

    #[test]
    fn a_ramp_is_integrated_and_the_map_can_be_inverted() {
        // 1x at 0 s rising to 3x at 4 s, then 3x: 8 s of file by 4 s, then 3 a second.
        let m = SpeedMap::new(&speed(&[(0.0, 1.0), (4.0, 3.0)]));
        assert!((m.media_at(4.0) - 8.0).abs() < 1e-3, "{}", m.media_at(4.0));
        assert!((m.media_at(6.0) - 14.0).abs() < 1e-3);
        for t in [0.0, 0.7, 2.5, 4.0, 9.3] {
            let back = m.time_for(m.media_at(t));
            assert!((back - t).abs() < 1e-3, "{t} -> {back}");
        }
        // Speeds are kept in range: a 100x key is 4x, a 0 is a quarter.
        let m = SpeedMap::new(&speed(&[(0.0, 100.0)]));
        assert!((m.media_at(1.0) - MAX_RATE).abs() < 1e-6);
        let m = SpeedMap::new(&speed(&[(0.0, 0.0)]));
        assert!((m.media_at(4.0) - 1.0).abs() < 1e-6);
        // A stalled or odd keyframe set can't break it.
        let odd = Animated {
            value: 1.0,
            keys: vec![Keyframe {
                t: 2.0,
                v: f32::NAN,
                curve: Default::default(),
            }],
        };
        assert!(SpeedMap::new(&odd).media_at(5.0).is_finite());
    }

    #[test]
    fn a_decoder_started_part_way_follows_the_same_speed() {
        let map = Arc::new(SpeedMap::new(&speed(&[(0.0, 1.0), (4.0, 3.0)])));
        let from_top = SpeedPlan {
            map: map.clone(),
            t0: 0.0,
        };
        let later = SpeedPlan { map, t0: 4.0 };
        // Three more seconds from 4 s in is 9 s of file; and it is begun 8 s in.
        assert!((later.media_after(3.0) - 9.0).abs() < 1e-3);
        assert!((from_top.media_after(7.0) - (8.0 + 9.0)).abs() < 1e-3);
        assert!((later.time_after(9.0) - 3.0).abs() < 1e-3);
    }

    /// Frequency of a signal, from its upward zero crossings.
    fn frequency(x: &[f32], sr: f64) -> f64 {
        let n = x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        n as f64 / (x.len() as f64 / sr)
    }

    fn run(input: &[f32], plan: SpeedPlan, chunk: usize) -> Vec<f32> {
        let mut s = Stretcher::new(1, 48_000, plan);
        let mut out = Vec::new();
        for c in input.chunks(chunk) {
            s.push(c);
            s.pull(&mut out);
        }
        s.finish();
        s.pull(&mut out);
        assert!(s.is_done());
        out
    }

    fn sine(hz: f64, secs: f64) -> Vec<f32> {
        (0..(secs * 48_000.0) as usize)
            .map(|i| (i as f64 / 48_000.0 * hz * std::f64::consts::TAU).sin() as f32 * 0.5)
            .collect()
    }

    fn plan(keys: &[(f64, f32)], value: f32) -> SpeedPlan {
        let mut a = speed(keys);
        a.value = value;
        SpeedPlan {
            map: Arc::new(SpeedMap::new(&a)),
            t0: 0.0,
        }
    }

    #[test]
    fn normal_speed_passes_the_sound_through() {
        let input = sine(440.0, 1.0);
        let out = run(&input, plan(&[], 1.0), 4800);
        assert!((out.len() as i64 - input.len() as i64).abs() < 3 * HOP as i64);
        // The middle of it is the input, sample for sample.
        let (a, b) = (20_000, 30_000);
        let worst = (a..b)
            .map(|i| (out[i] - input[i]).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 0.01, "{worst}");
    }

    #[test]
    fn faster_is_shorter_and_slower_longer_and_the_pitch_stays() {
        let input = sine(440.0, 2.0);
        for (rate, expect) in [(2.0f32, 1.0), (0.5, 4.0)] {
            let out = run(&input, plan(&[], rate), 4800);
            let secs = out.len() as f64 / 48_000.0;
            assert!((secs - expect).abs() < 0.12, "x{rate}: {secs} s");
            // The middle, away from the ends.
            let mid = &out[10_000..out.len() - 10_000];
            let f = frequency(mid, 48_000.0);
            assert!((f - 440.0).abs() < 8.0, "x{rate}: {f} Hz");
        }
    }

    #[test]
    fn a_ramp_in_speed_makes_the_length_the_map_says() {
        let input = sine(440.0, 6.0);
        let p = plan(&[(0.0, 1.0), (3.0, 2.0)], 1.0);
        let expect = p.map.time_for(6.0);
        let out = run(&input, p, 1000);
        let secs = out.len() as f64 / 48_000.0;
        assert!((secs - expect).abs() < 0.12, "{secs} s, not {expect}");
        let f = frequency(&out[10_000..out.len() - 10_000], 48_000.0);
        assert!((f - 440.0).abs() < 10.0, "{f} Hz");
        // How the input is handed over makes no difference to the output.
        let again = run(&input, plan(&[(0.0, 1.0), (3.0, 2.0)], 1.0), 48_000);
        assert_eq!(out.len(), again.len());
        assert!(out.iter().zip(&again).all(|(a, b)| (a - b).abs() < 1e-5));
    }

    #[test]
    fn several_channels_are_moved_together() {
        let left = sine(440.0, 1.0);
        let mut s = Stretcher::new(2, 48_000, plan(&[], 2.0));
        let both: Vec<f32> = left.iter().flat_map(|&l| [l, -l]).collect();
        s.push(&both);
        s.finish();
        let mut out = Vec::new();
        s.pull(&mut out);
        assert!(s.is_done());
        assert!(
            out.as_chunks::<2>()
                .0
                .iter()
                .all(|f| (f[0] + f[1]).abs() < 1e-5)
        );
        let secs = out.len() as f64 / 2.0 / 48_000.0;
        assert!((secs - 0.5).abs() < 0.12, "{secs}");
    }
}
