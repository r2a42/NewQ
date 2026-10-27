//! Effects on a cue: what the picture effects come to at a moment (the
//! numbers the compositor's shader takes), and the sound effects themselves
//! (filters, compressor, echo and reverb) that the mixer runs on a voice.
//!
//! The parameters are `show::Effect`s, whose numbers can be animated; the
//! engine reads them at the cue's own time every tick. Everything here is
//! plain numbers, so it is tested exactly, without a graphics card or a
//! sound device.

use crate::show::{Effect, EffectKind};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

// ----- Picture ------------------------------------------------------------------

/// The picture effects of a layer at one moment, as the shader takes them.
/// Several effects of one kind combine: blurs and sharpens add up, the others
/// take the strongest.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct VideoFx {
    /// Blur radius in pixels of the picture.
    pub blur: f32,
    /// Sharpening amount (0 = none).
    pub sharpen: f32,
    /// Vignette darkness (0 = none) and how soft its edge is.
    pub vignette: f32,
    pub softness: f32,
    /// Size of the blocks in pixels (below 2 = none).
    pub pixel: f32,
    /// How far the colours are inverted (0 = not at all).
    pub negative: f32,
}

impl VideoFx {
    /// The picture effects among `effects`, `t` seconds into the cue.
    pub fn at(effects: &[Effect], t: f64) -> VideoFx {
        let mut fx = VideoFx::default();
        for e in effects.iter().filter(|e| e.enabled && e.kind.is_video()) {
            let v = e.values(t);
            match e.kind {
                EffectKind::Blur => fx.blur += v[0],
                EffectKind::Sharpen => fx.sharpen += v[0],
                EffectKind::Vignette => {
                    if v[0] > fx.vignette {
                        fx.vignette = v[0];
                        fx.softness = v[1];
                    }
                }
                EffectKind::Pixelate => fx.pixel = fx.pixel.max(v[0]),
                EffectKind::Negative => fx.negative = fx.negative.max(v[0]),
                _ => {}
            }
        }
        fx.blur = fx.blur.min(100.0);
        fx
    }

    /// Whether nothing is done to the picture.
    #[cfg(test)]
    pub fn is_none(&self) -> bool {
        self.blur < 0.25
            && self.sharpen < 0.01
            && self.vignette < 0.005
            && self.pixel < 2.0
            && self.negative < 0.005
    }

    /// How many times the layer's picture is read for each pixel drawn: a
    /// rough measure of what the effects cost the graphics card (1 is a
    /// plain layer).
    pub fn taps_per_pixel(&self) -> u32 {
        let mut taps = 1;
        if self.blur >= 0.25 {
            taps += blur_taps(self.blur);
        }
        if self.sharpen >= 0.01 {
            taps += 4;
        }
        taps
    }
}

/// How many samples the shader takes in a disc of `radius` pixels: more for
/// a wider blur, so it stays smooth, up to a cap.
pub fn blur_taps(radius: f32) -> u32 {
    ((radius * 0.7).round() as u32).clamp(8, 48)
}

/// The vignette's darkening at `c`, the position in the picture (0..1 each
/// way): 1 in the middle, falling to `1 - amount` at the corners. (The
/// shader does the same sums.)
#[cfg(test)]
pub fn vignette_factor(c: [f32; 2], amount: f32, softness: f32) -> f32 {
    let (x, y) = ((c[0] - 0.5) * 2.0, (c[1] - 0.5) * 2.0);
    let r = (x * x + y * y).sqrt() / std::f32::consts::SQRT_2;
    let s = softness.clamp(0.05, 1.0);
    let t = ((r - (1.0 - s)) / s).clamp(0.0, 1.0);
    1.0 - amount * t * t * (3.0 - 2.0 * t)
}

// ----- Sound --------------------------------------------------------------------

/// The numbers of a sound effect, shared between the engine (which sets them
/// every tick) and the mixer (which reads them each buffer).
pub struct FxShared {
    slots: Vec<FxSlot>,
}

struct FxSlot {
    enabled: AtomicBool,
    p: [AtomicU32; 4],
}

impl FxShared {
    pub fn new(n: usize) -> FxShared {
        FxShared {
            slots: (0..n)
                .map(|_| FxSlot {
                    enabled: AtomicBool::new(true),
                    p: Default::default(),
                })
                .collect(),
        }
    }

    pub fn set(&self, i: usize, enabled: bool, v: [f32; 4]) {
        if let Some(s) = self.slots.get(i) {
            s.enabled.store(enabled, Ordering::Relaxed);
            for (a, v) in s.p.iter().zip(v) {
                a.store(v.to_bits(), Ordering::Relaxed);
            }
        }
    }

    fn get(&self, i: usize) -> (bool, [f32; 4]) {
        let s = &self.slots[i];
        (
            s.enabled.load(Ordering::Relaxed),
            [0, 1, 2, 3].map(|k| f32::from_bits(s.p[k].load(Ordering::Relaxed))),
        )
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }
}

/// The sound effects of one voice, in order.
pub struct AudioChain {
    effects: Vec<(AudioFx, bool)>,
    last: Vec<(bool, [u32; 4])>,
}

enum AudioFx {
    Eq(Eq),
    Filter(Filter),
    Compressor(Compressor),
    Echo(Echo),
    Reverb(Reverb),
}

impl AudioChain {
    /// A chain of `kinds` (the sound ones; picture kinds are skipped) for a
    /// voice at `rate` Hz. All the memory is taken here, so playing never
    /// has to.
    pub fn new(kinds: &[EffectKind], rate: u32) -> AudioChain {
        let effects: Vec<(AudioFx, bool)> = kinds
            .iter()
            .filter_map(|k| {
                Some(match k {
                    EffectKind::Eq => AudioFx::Eq(Eq::new(rate)),
                    EffectKind::Filter => AudioFx::Filter(Filter::new(rate)),
                    EffectKind::Compressor => AudioFx::Compressor(Compressor::new(rate)),
                    EffectKind::Echo => AudioFx::Echo(Echo::new(rate)),
                    EffectKind::Reverb => AudioFx::Reverb(Reverb::new(rate)),
                    _ => return None,
                })
            })
            .map(|e| (e, true))
            .collect();
        let n = effects.len();
        AudioChain {
            effects,
            last: vec![(true, [0; 4]); n],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.effects.is_empty()
    }

    /// Take the numbers the engine has set. Cheap when nothing changed.
    pub fn sync(&mut self, shared: &FxShared) {
        for i in 0..self.effects.len().min(shared.len()) {
            let (on, v) = shared.get(i);
            let bits = v.map(f32::to_bits);
            if self.last[i] != (on, bits) {
                self.last[i] = (on, bits);
                self.effects[i].1 = on;
                match &mut self.effects[i].0 {
                    AudioFx::Eq(e) => e.set(v),
                    AudioFx::Filter(e) => e.set(v),
                    AudioFx::Compressor(e) => e.set(v),
                    AudioFx::Echo(e) => e.set(v),
                    AudioFx::Reverb(e) => e.set(v),
                }
            }
        }
    }

    /// One stereo frame through the effects that are on.
    pub fn process(&mut self, mut f: [f32; 2]) -> [f32; 2] {
        for (fx, on) in &mut self.effects {
            if !*on {
                continue;
            }
            f = match fx {
                AudioFx::Eq(e) => e.process(f),
                AudioFx::Filter(e) => e.process(f),
                AudioFx::Compressor(e) => e.process(f),
                AudioFx::Echo(e) => e.process(f),
                AudioFx::Reverb(e) => e.process(f),
            };
        }
        f
    }
}

/// A second-order filter section (RBJ's cookbook), for one channel pair.
#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z: [[f32; 2]; 2],
    /// Does nothing (flat).
    off: bool,
}

impl Biquad {
    fn identity() -> Biquad {
        Biquad {
            b0: 1.0,
            off: true,
            ..Default::default()
        }
    }

    fn set(&mut self, b: [f32; 3], a: [f32; 3]) {
        self.b0 = b[0] / a[0];
        self.b1 = b[1] / a[0];
        self.b2 = b[2] / a[0];
        self.a1 = a[1] / a[0];
        self.a2 = a[2] / a[0];
        self.off = false;
    }

    fn low_shelf(&mut self, f0: f32, gain_db: f32, rate: f32) {
        if gain_db.abs() < 0.05 {
            return self.reset_flat();
        }
        let a = 10f32.powf(gain_db / 40.0);
        let w = std::f32::consts::TAU * f0 / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / 2.0 * std::f32::consts::SQRT_2;
        let k = 2.0 * a.sqrt() * alpha;
        self.set(
            [
                a * ((a + 1.0) - (a - 1.0) * cos + k),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                a * ((a + 1.0) - (a - 1.0) * cos - k),
            ],
            [
                (a + 1.0) + (a - 1.0) * cos + k,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                (a + 1.0) + (a - 1.0) * cos - k,
            ],
        );
    }

    fn high_shelf(&mut self, f0: f32, gain_db: f32, rate: f32) {
        if gain_db.abs() < 0.05 {
            return self.reset_flat();
        }
        let a = 10f32.powf(gain_db / 40.0);
        let w = std::f32::consts::TAU * f0 / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / 2.0 * std::f32::consts::SQRT_2;
        let k = 2.0 * a.sqrt() * alpha;
        self.set(
            [
                a * ((a + 1.0) + (a - 1.0) * cos + k),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                a * ((a + 1.0) + (a - 1.0) * cos - k),
            ],
            [
                (a + 1.0) - (a - 1.0) * cos + k,
                2.0 * ((a - 1.0) - (a + 1.0) * cos),
                (a + 1.0) - (a - 1.0) * cos - k,
            ],
        );
    }

    fn peaking(&mut self, f0: f32, gain_db: f32, q: f32, rate: f32) {
        if gain_db.abs() < 0.05 {
            return self.reset_flat();
        }
        let a = 10f32.powf(gain_db / 40.0);
        let w = std::f32::consts::TAU * f0 / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        self.set(
            [1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a],
            [1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a],
        );
    }

    fn low_pass(&mut self, f0: f32, rate: f32) {
        let w = std::f32::consts::TAU * f0 / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        self.set(
            [(1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        );
    }

    fn high_pass(&mut self, f0: f32, rate: f32) {
        let w = std::f32::consts::TAU * f0 / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        self.set(
            [(1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0],
            [1.0 + alpha, -2.0 * cos, 1.0 - alpha],
        );
    }

    fn reset_flat(&mut self) {
        let z = self.z;
        *self = Biquad::identity();
        self.z = z;
    }

    #[inline]
    fn process(&mut self, x: [f32; 2]) -> [f32; 2] {
        if self.off {
            return x;
        }
        let mut y = [0.0; 2];
        for c in 0..2 {
            let out = self.b0 * x[c] + self.z[c][0];
            self.z[c][0] = self.b1 * x[c] - self.a1 * out + self.z[c][1];
            self.z[c][1] = self.b2 * x[c] - self.a2 * out;
            y[c] = out;
        }
        y
    }
}

struct Eq {
    rate: f32,
    low: Biquad,
    mid: Biquad,
    high: Biquad,
}

impl Eq {
    fn new(rate: u32) -> Eq {
        Eq {
            rate: rate as f32,
            low: Biquad::identity(),
            mid: Biquad::identity(),
            high: Biquad::identity(),
        }
    }

    fn set(&mut self, v: [f32; 4]) {
        let top = (self.rate * 0.45).min(16_000.0);
        self.low.low_shelf(120.0, v[0], self.rate);
        self.mid.peaking(1_000.0, v[1], 0.9, self.rate);
        self.high.high_shelf(6_000.0_f32.min(top), v[2], self.rate);
    }

    fn process(&mut self, x: [f32; 2]) -> [f32; 2] {
        self.high.process(self.mid.process(self.low.process(x)))
    }
}

struct Filter {
    rate: f32,
    hp: Biquad,
    lp: Biquad,
}

impl Filter {
    fn new(rate: u32) -> Filter {
        Filter {
            rate: rate as f32,
            hp: Biquad::identity(),
            lp: Biquad::identity(),
        }
    }

    fn set(&mut self, v: [f32; 4]) {
        let nyquist = self.rate * 0.49;
        if v[0] > 20.5 {
            self.hp.high_pass(v[0].min(nyquist), self.rate);
        } else {
            self.hp.reset_flat();
        }
        if v[1] < 19_500.0 {
            self.lp.low_pass(v[1].clamp(20.0, nyquist), self.rate);
        } else {
            self.lp.reset_flat();
        }
    }

    fn process(&mut self, x: [f32; 2]) -> [f32; 2] {
        self.lp.process(self.hp.process(x))
    }
}

struct Compressor {
    rate: f32,
    threshold_db: f32,
    ratio: f32,
    makeup: f32,
    attack: f32,
    release: f32,
    env: f32,
}

impl Compressor {
    fn new(rate: u32) -> Compressor {
        let mut c = Compressor {
            rate: rate as f32,
            threshold_db: -18.0,
            ratio: 4.0,
            makeup: 1.0,
            attack: 0.0,
            release: 0.0,
            env: 0.0,
        };
        c.set([-18.0, 4.0, 0.0, 150.0]);
        c
    }

    fn set(&mut self, v: [f32; 4]) {
        self.threshold_db = v[0];
        self.ratio = v[1].max(1.0);
        self.makeup = 10f32.powf(v[2] / 20.0);
        self.attack = (-1.0 / (0.005 * self.rate)).exp();
        self.release = (-1.0 / (v[3].max(1.0) / 1000.0 * self.rate)).exp();
    }

    fn process(&mut self, x: [f32; 2]) -> [f32; 2] {
        // Both channels are reduced together, so the picture of the sound
        // doesn't lean to one side.
        let level = x[0].abs().max(x[1].abs());
        let coef = if level > self.env {
            self.attack
        } else {
            self.release
        };
        self.env = level + coef * (self.env - level);
        let db = 20.0 * self.env.max(1e-6).log10();
        let over = db - self.threshold_db;
        let reduction = if over > 0.0 {
            over * (1.0 - 1.0 / self.ratio)
        } else {
            0.0
        };
        let gain = 10f32.powf(-reduction / 20.0) * self.makeup;
        [x[0] * gain, x[1] * gain]
    }
}

struct Echo {
    rate: f32,
    buf: [Vec<f32>; 2],
    write: usize,
    /// The delay now and the one asked for, in samples (it glides, so
    /// changing the time doesn't click).
    delay: f32,
    target: f32,
    /// Whether the time has been set once (the first time is not glided to).
    primed: bool,
    feedback: f32,
    mix: f32,
}

impl Echo {
    fn new(rate: u32) -> Echo {
        let len = rate as usize * 2 + 4;
        let mut e = Echo {
            rate: rate as f32,
            buf: [vec![0.0; len], vec![0.0; len]],
            write: 0,
            delay: 0.35 * rate as f32,
            target: 0.35 * rate as f32,
            primed: false,
            feedback: 0.4,
            mix: 0.35,
        };
        e.set([350.0, 0.4, 0.35, 0.0]);
        e.primed = false;
        e
    }

    fn set(&mut self, v: [f32; 4]) {
        let max = (self.buf[0].len() - 3) as f32;
        self.target = (v[0] / 1000.0 * self.rate).clamp(1.0, max);
        if !self.primed {
            self.primed = true;
            self.delay = self.target;
        }
        self.feedback = v[1].clamp(0.0, 0.95);
        self.mix = v[2].clamp(0.0, 1.0);
    }

    fn process(&mut self, x: [f32; 2]) -> [f32; 2] {
        // Glide to the delay asked for over about a tenth of a second.
        self.delay += (self.target - self.delay) * (10.0 / self.rate).min(1.0);
        let len = self.buf[0].len();
        let read = self.write as f32 - self.delay;
        let read = if read < 0.0 { read + len as f32 } else { read };
        let (i0, frac) = (read as usize % len, read.fract());
        let i1 = (i0 + 1) % len;
        let mut out = [0.0; 2];
        for c in 0..2 {
            let wet = self.buf[c][i0] * (1.0 - frac) + self.buf[c][i1] * frac;
            self.buf[c][self.write] = x[c] + wet * self.feedback;
            out[c] = x[c] + wet * self.mix;
        }
        self.write = (self.write + 1) % len;
        out
    }
}

/// Freeverb: eight comb filters in parallel, four all-pass in series, each
/// side a little different from the other.
struct Reverb {
    combs: [Vec<Comb>; 2],
    allpass: [Vec<AllPass>; 2],
    feedback: f32,
    damp: f32,
    mix: f32,
}

struct Comb {
    buf: Vec<f32>,
    i: usize,
    store: f32,
}

struct AllPass {
    buf: Vec<f32>,
    i: usize,
}

impl Reverb {
    fn new(rate: u32) -> Reverb {
        let scale = rate as f32 / 44_100.0;
        let size = |n: usize| ((n as f32 * scale).round() as usize).max(2);
        const COMBS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
        const ALLPASS: [usize; 4] = [556, 441, 341, 225];
        const SPREAD: usize = 23;
        let side = |extra: usize| {
            (
                COMBS
                    .iter()
                    .map(|&n| Comb {
                        buf: vec![0.0; size(n + extra)],
                        i: 0,
                        store: 0.0,
                    })
                    .collect::<Vec<_>>(),
                ALLPASS
                    .iter()
                    .map(|&n| AllPass {
                        buf: vec![0.0; size(n + extra)],
                        i: 0,
                    })
                    .collect::<Vec<_>>(),
            )
        };
        let ((cl, al), (cr, ar)) = (side(0), side(SPREAD));
        let mut r = Reverb {
            combs: [cl, cr],
            allpass: [al, ar],
            feedback: 0.0,
            damp: 0.0,
            mix: 0.0,
        };
        r.set([0.6, 0.5, 0.25, 0.0]);
        r
    }

    fn set(&mut self, v: [f32; 4]) {
        self.feedback = 0.7 + 0.28 * v[0].clamp(0.0, 1.0);
        self.damp = 0.4 * v[1].clamp(0.0, 1.0);
        self.mix = v[2].clamp(0.0, 1.0);
    }

    fn process(&mut self, x: [f32; 2]) -> [f32; 2] {
        let input = (x[0] + x[1]) * 0.015;
        let mut out = [0.0; 2];
        for c in 0..2 {
            let mut wet = 0.0;
            for comb in &mut self.combs[c] {
                let y = comb.buf[comb.i];
                comb.store = y * (1.0 - self.damp) + comb.store * self.damp;
                comb.buf[comb.i] = input + comb.store * self.feedback;
                comb.i = (comb.i + 1) % comb.buf.len();
                wet += y;
            }
            for ap in &mut self.allpass[c] {
                let b = ap.buf[ap.i];
                let y = -wet + b;
                ap.buf[ap.i] = wet + b * 0.5;
                ap.i = (ap.i + 1) % ap.buf.len();
                wet = y;
            }
            // Freeverb's own gain on the wet sound.
            out[c] = x[c] * (1.0 - self.mix) + wet * 3.0 * self.mix;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::{Animated, Curve, Keyframe};

    const RATE: u32 = 48_000;

    fn chain(kind: EffectKind, v: [f32; 4]) -> AudioChain {
        let mut c = AudioChain::new(&[kind], RATE);
        let shared = FxShared::new(1);
        shared.set(0, true, v);
        c.sync(&shared);
        c
    }

    /// The level of a steady sine of `hz` through the chain, once it has settled.
    fn gain_at(chain: &mut AudioChain, hz: f32) -> f32 {
        let n = RATE as usize;
        let amp = 0.05;
        let mut sum = 0.0f64;
        for i in 0..n {
            let s = (std::f32::consts::TAU * hz * i as f32 / RATE as f32).sin() * amp;
            let y = chain.process([s, s]);
            if i >= n / 2 {
                sum += f64::from(y[0]).powi(2);
            }
        }
        // The level of the sine from its mean square.
        ((sum / (n - n / 2) as f64).sqrt() * std::f64::consts::SQRT_2) as f32 / amp
    }

    #[test]
    fn a_number_without_keys_is_its_value_and_with_keys_follows_them() {
        let mut a = Animated::new(5.0);
        assert_eq!((a.at(0.0), a.at(99.0)), (5.0, 5.0));
        a.set_key(2.0, 10.0);
        a.set_key(4.0, 20.0);
        // Before the first and after the last, the nearest key's value.
        assert_eq!((a.at(0.0), a.at(2.0), a.at(9.0)), (10.0, 10.0, 20.0));
        assert_eq!(a.at(3.0), 15.0);
        // Out of order is put in order, and a key at the same time is replaced.
        a.set_key(1.0, 0.0);
        assert_eq!(
            a.keys.iter().map(|k| k.t).collect::<Vec<_>>(),
            [1.0, 2.0, 4.0]
        );
        a.set_key(2.0, 12.0);
        assert_eq!(a.keys.len(), 3);
        assert_eq!(a.at(1.5), 6.0);
        // Moving past a neighbour keeps them in order; removing works.
        let at = a.move_key(0, 5.0, 1.0);
        assert_eq!(at, 2);
        assert_eq!(a.keys.last().unwrap().v, 1.0);
        a.remove_key(0);
        assert_eq!(a.keys.len(), 2);
    }

    #[test]
    fn a_key_can_ease_into_the_next() {
        let mut a = Animated::new(0.0);
        a.keys = vec![
            Keyframe {
                t: 0.0,
                v: 0.0,
                curve: Curve::SCurve,
            },
            Keyframe {
                t: 2.0,
                v: 100.0,
                curve: Curve::Linear,
            },
        ];
        assert_eq!(a.at(1.0), 50.0);
        assert!(a.at(0.5) < 25.0 && a.at(1.5) > 75.0, "eased at both ends");
    }

    #[test]
    fn effect_numbers_start_at_their_defaults_and_stay_in_range() {
        let mut e = Effect::new(EffectKind::Blur);
        assert_eq!(e.values(0.0)[0], 8.0);
        e.p[0].set_key(0.0, 500.0);
        assert_eq!(e.values(0.0)[0], 100.0);
        // A file with too few numbers is filled in with the defaults.
        e.p.clear();
        assert_eq!(e.values(0.0)[0], 8.0);
        for k in EffectKind::VIDEO.iter().chain(&EffectKind::AUDIO) {
            let e = Effect::new(*k);
            assert_eq!(e.p.len(), k.params().len());
            assert!(k.params().len() <= 4);
            for s in k.params() {
                assert!(s.min <= s.default && s.default <= s.max, "{k:?} {}", s.name);
            }
        }
    }

    #[test]
    fn picture_effects_combine_and_follow_their_keys() {
        let mut blur = Effect::new(EffectKind::Blur);
        blur.p[0].set_key(0.0, 0.0);
        blur.p[0].set_key(2.0, 40.0);
        let mut off = Effect::new(EffectKind::Sharpen);
        off.enabled = false;
        let fx = VideoFx::at(&[blur.clone(), off, Effect::new(EffectKind::Blur)], 1.0);
        // Half way through the first blur (20) plus the second's 8.
        assert_eq!(fx.blur, 28.0);
        assert_eq!(fx.sharpen, 0.0, "a disabled effect does nothing");
        assert!(VideoFx::default().is_none());
        assert!(!fx.is_none());
        // Sound effects don't touch the picture.
        assert!(VideoFx::at(&[Effect::new(EffectKind::Reverb)], 0.0).is_none());
        // More blur costs more reads.
        assert!(
            VideoFx::at(&[blur], 2.0).taps_per_pixel()
                > VideoFx::at(&[Effect::new(EffectKind::Pixelate)], 0.0).taps_per_pixel()
        );
        assert_eq!(blur_taps(1.0), 8);
        assert_eq!(blur_taps(100.0), 48);
    }

    #[test]
    fn the_vignette_is_clear_in_the_middle_and_darkest_in_the_corners() {
        assert_eq!(vignette_factor([0.5, 0.5], 0.8, 0.5), 1.0);
        let corner = vignette_factor([0.0, 0.0], 0.8, 0.5);
        assert!((corner - 0.2).abs() < 1e-5, "{corner}");
        let edge = vignette_factor([1.0, 0.5], 0.8, 0.5);
        assert!(edge > corner && edge < 1.0);
        assert_eq!(vignette_factor([0.0, 0.0], 0.0, 0.5), 1.0);
    }

    #[test]
    fn the_eq_boosts_and_cuts_its_own_part_of_the_sound() {
        let mut c = chain(EffectKind::Eq, [12.0, 0.0, 0.0, 0.0]);
        let low = gain_at(&mut c, 40.0);
        let high = gain_at(&mut c, 8_000.0);
        assert!(low > 3.0 && low < 4.6, "40 Hz up 12 dB is ×4: {low}");
        assert!((high - 1.0).abs() < 0.05, "highs untouched: {high}");
        let mut c = chain(EffectKind::Eq, [0.0, 0.0, -12.0, 0.0]);
        assert!(gain_at(&mut c, 12_000.0) < 0.35);
        assert!((gain_at(&mut c, 200.0) - 1.0).abs() < 0.05);
        let mut c = chain(EffectKind::Eq, [0.0, 9.0, 0.0, 0.0]);
        assert!(gain_at(&mut c, 1_000.0) > 2.4);
        // Flat is exactly the sound that went in.
        let mut flat = chain(EffectKind::Eq, [0.0; 4]);
        assert_eq!(flat.process([0.3, -0.2]), [0.3, -0.2]);
    }

    #[test]
    fn the_filter_cuts_above_and_below_its_frequencies() {
        let mut c = chain(EffectKind::Filter, [20.0, 500.0, 0.0, 0.0]);
        assert!(gain_at(&mut c, 5_000.0) < 0.02, "low-pass at 500 Hz");
        assert!((gain_at(&mut c, 100.0) - 1.0).abs() < 0.05);
        let mut c = chain(EffectKind::Filter, [2_000.0, 20_000.0, 0.0, 0.0]);
        assert!(gain_at(&mut c, 100.0) < 0.02, "high-pass at 2 kHz");
        assert!((gain_at(&mut c, 8_000.0) - 1.0).abs() < 0.05);
        let mut open = chain(EffectKind::Filter, [20.0, 20_000.0, 0.0, 0.0]);
        assert_eq!(open.process([0.5, 0.5]), [0.5, 0.5]);
    }

    #[test]
    fn the_compressor_pulls_down_what_is_over_the_threshold_only() {
        // Threshold -20 dB, ratio 4: a 0 dB sine goes down 15 dB.
        let mut c = chain(EffectKind::Compressor, [-20.0, 4.0, 0.0, 100.0]);
        let loud = {
            let mut peak = 0.0f32;
            for i in 0..RATE as usize {
                let s = (std::f32::consts::TAU * 440.0 * i as f32 / RATE as f32).sin();
                let y = c.process([s, s]);
                if i > RATE as usize / 2 {
                    peak = peak.max(y[0].abs());
                }
            }
            peak
        };
        let db = 20.0 * loud.log10();
        assert!((db + 15.0).abs() < 1.5, "{db} dB");
        // Quiet sound is left alone, and make-up gain lifts it.
        let mut quiet = chain(EffectKind::Compressor, [-20.0, 4.0, 0.0, 100.0]);
        assert!((gain_at(&mut quiet, 440.0) - 1.0).abs() < 0.02);
        let mut up = chain(EffectKind::Compressor, [0.0, 4.0, 6.0, 100.0]);
        assert!((gain_at(&mut up, 440.0) - 2.0).abs() < 0.05);
    }

    #[test]
    fn the_echo_repeats_after_its_time_and_dies_away_by_its_feedback() {
        let mut c = chain(EffectKind::Echo, [100.0, 0.5, 1.0, 0.0]);
        let mut out = Vec::new();
        for i in 0..RATE as usize / 2 {
            let x = if i == 0 { 1.0 } else { 0.0 };
            out.push(c.process([x, x])[0]);
        }
        let d = (0.1 * RATE as f32) as usize;
        assert_eq!(out[0], 1.0);
        assert!((out[d] - 1.0).abs() < 0.01, "first echo: {}", out[d]);
        assert!((out[2 * d] - 0.5).abs() < 0.01, "second: {}", out[2 * d]);
        assert!((out[3 * d] - 0.25).abs() < 0.01);
        assert!(out[d / 2].abs() < 1e-6, "silence between");
        // No mix: nothing is added to the sound.
        let mut dry = chain(EffectKind::Echo, [100.0, 0.5, 0.0, 0.0]);
        assert_eq!(dry.process([0.4, 0.4]), [0.4, 0.4]);
    }

    #[test]
    fn the_reverb_leaves_a_tail_and_none_at_zero_mix() {
        let mut c = chain(EffectKind::Reverb, [0.7, 0.5, 0.5, 0.0]);
        let mut tail = 0.0f32;
        let mut peak = 0.0f32;
        for i in 0..RATE as usize {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let y = c.process([x, x]);
            if i > 4_000 {
                tail = tail.max(y[0].abs());
            }
            peak = peak.max(y[0].abs().max(y[1].abs()));
        }
        assert!(tail > 0.001, "there is a tail: {tail}");
        assert!(peak < 2.0 && peak.is_finite());
        let mut dry = chain(EffectKind::Reverb, [0.7, 0.5, 0.0, 0.0]);
        assert_eq!(dry.process([0.3, 0.1]), [0.3, 0.1]);
        // The left and right of a mono sound are not the same.
        let mut c = chain(EffectKind::Reverb, [0.7, 0.5, 1.0, 0.0]);
        let mut differ = false;
        for i in 0..8_000 {
            let x = if i == 0 { 1.0 } else { 0.0 };
            let y = c.process([x, x]);
            differ |= (y[0] - y[1]).abs() > 1e-6;
        }
        assert!(differ);
    }

    #[test]
    fn effects_can_be_changed_and_switched_off_while_playing() {
        let mut c = AudioChain::new(&[EffectKind::Filter], RATE);
        let shared = FxShared::new(1);
        shared.set(0, true, [20.0, 500.0, 0.0, 0.0]);
        c.sync(&shared);
        assert!(gain_at(&mut c, 5_000.0) < 0.02);
        shared.set(0, false, [20.0, 500.0, 0.0, 0.0]);
        c.sync(&shared);
        assert!(
            (gain_at(&mut c, 5_000.0) - 1.0).abs() < 1e-3,
            "switched off"
        );
        shared.set(0, true, [20.0, 20_000.0, 0.0, 0.0]);
        c.sync(&shared);
        assert!((gain_at(&mut c, 5_000.0) - 1.0).abs() < 0.05);
        // Picture effects are no part of a sound chain.
        assert!(AudioChain::new(&[EffectKind::Blur], RATE).is_empty());
    }
}
