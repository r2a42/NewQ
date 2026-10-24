//! Colour adjustment and chroma key for visual cues.
//!
//! `apply` is the reference for what the compositor's shader does to each
//! pixel; the matrix and key numbers it uses are the ones sent to the GPU.
//! Working on plain numbers means the maths is tested exactly, here, without
//! a GPU.

use crate::show::{ChromaKey, ColorAdjust};

/// The brightness, contrast, saturation and hue of a cue as a 3×4 matrix:
/// each row is (r, g, b, offset), applied to a colour in 0..1. The identity
/// when nothing is adjusted. Saturation and hue use the same constants as
/// SVG's `feColorMatrix`.
pub fn color_matrix(c: &ColorAdjust) -> [[f32; 4]; 3] {
    let s = c.saturation;
    let (sin, cos) = c.hue.to_radians().sin_cos();
    let sat = [
        [0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s],
        [0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s],
    ];
    let hue = [
        [
            0.213 + cos * 0.787 - sin * 0.213,
            0.715 - cos * 0.715 - sin * 0.715,
            0.072 - cos * 0.072 + sin * 0.928,
        ],
        [
            0.213 - cos * 0.213 + sin * 0.143,
            0.715 + cos * 0.285 + sin * 0.140,
            0.072 - cos * 0.072 - sin * 0.283,
        ],
        [
            0.213 - cos * 0.213 - sin * 0.787,
            0.715 - cos * 0.715 + sin * 0.715,
            0.072 + cos * 0.928 + sin * 0.072,
        ],
    ];
    // hue × saturation, then contrast about mid grey, then brightness.
    let offset = 0.5 * (1.0 - c.contrast) + c.brightness;
    let mut m = [[0.0f32; 4]; 3];
    for (r, row) in m.iter_mut().enumerate() {
        for col in 0..3 {
            row[col] = c.contrast * (0..3).map(|k| hue[r][k] * sat[k][col]).sum::<f32>();
        }
        row[3] = offset;
    }
    m
}

/// A key colour's position on the chroma plane (blue and red difference,
/// each −0.5 to 0.5), which is what the key measures distance in, so
/// brightness doesn't matter.
pub fn chroma_of(rgb: [f32; 3]) -> [f32; 2] {
    let [r, g, b] = rgb;
    [
        -0.168_736 * r - 0.331_264 * g + 0.5 * b,
        0.5 * r - 0.418_688 * g - 0.081_312 * b,
    ]
}

/// The key as the shader takes it: (cb, cr, tolerance, softness) and
/// (mask r, g, b, spill), where the mask picks out the key's dominant
/// channel, the one whose spill is suppressed.
pub fn key_uniforms(k: &ChromaKey) -> ([f32; 4], [f32; 4]) {
    let rgb = k.color.map(|v| f32::from(v) / 255.0);
    let [cb, cr] = chroma_of(rgb);
    // The channel that most exceeds the average of the other two.
    let dominance = [
        rgb[0] - (rgb[1] + rgb[2]) / 2.0,
        rgb[1] - (rgb[0] + rgb[2]) / 2.0,
        rgb[2] - (rgb[0] + rgb[1]) / 2.0,
    ];
    let mut best = 0;
    for i in 1..3 {
        if dominance[i] > dominance[best] {
            best = i;
        }
    }
    let mut mask = [0.0; 4];
    mask[best] = 1.0;
    mask[3] = k.spill;
    ([cb, cr, k.tolerance, k.softness.max(1e-4)], mask)
}

#[cfg(test)]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// What the compositor does to one pixel's colour: key it, then adjust it.
/// (The shader does this on the GPU; this is the same maths for the tests.)
#[cfg(test)]
/// Returns the colour and the alpha the key leaves (1 with no key).
pub fn apply(rgb: [f32; 3], c: &ColorAdjust) -> ([f32; 3], f32) {
    let mut rgb = rgb;
    let mut alpha = 1.0;
    if let Some(k) = &c.key {
        let (kc, ks) = key_uniforms(k);
        let [cb, cr] = chroma_of(rgb);
        let d = ((cb - kc[0]).powi(2) + (cr - kc[1]).powi(2)).sqrt();
        alpha = smoothstep(kc[2], kc[2] + kc[3], d);
        // Pull the key colour's spill out of what is kept: cap its dominant
        // channel at the strongest of the other two.
        let other = (rgb[0] * (1.0 - ks[0]))
            .max(rgb[1] * (1.0 - ks[1]))
            .max(rgb[2] * (1.0 - ks[2]));
        let dom = rgb[0] * ks[0] + rgb[1] * ks[1] + rgb[2] * ks[2];
        let pull = (dom.min(other) - dom) * ks[3];
        for (v, m) in rgb.iter_mut().zip(&ks) {
            *v += m * pull;
        }
    }
    let m = color_matrix(c);
    let out = [0, 1, 2].map(|r| {
        (m[r][0] * rgb[0] + m[r][1] * rgb[1] + m[r][2] * rgb[2] + m[r][3]).clamp(0.0, 1.0)
    });
    (out, alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: [f32; 3], b: [f32; 3]) {
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < 0.01, "{a:?} vs {b:?}");
        }
    }

    fn adjust(f: impl FnOnce(&mut ColorAdjust)) -> ColorAdjust {
        let mut c = ColorAdjust::default();
        f(&mut c);
        c
    }

    #[test]
    fn no_adjustment_is_the_identity() {
        let m = color_matrix(&ColorAdjust::default());
        for (r, row) in m.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                let want = if c == r { 1.0 } else { 0.0 };
                assert!((v - want).abs() < 1e-5, "{m:?}");
            }
        }
        for px in [[0.0, 0.0, 0.0], [1.0, 0.5, 0.25], [0.3, 0.9, 0.6]] {
            near(apply(px, &ColorAdjust::default()).0, px);
        }
        assert_eq!(apply([0.1, 0.2, 0.3], &ColorAdjust::default()).1, 1.0);
        assert!(ColorAdjust::default().is_default());
    }

    #[test]
    fn brightness_and_contrast() {
        let up = adjust(|c| c.brightness = 0.2);
        near(apply([0.3, 0.5, 0.7], &up).0, [0.5, 0.7, 0.9]);
        // Pushed past the ends it clips.
        near(apply([0.9, 1.0, 0.0], &up).0, [1.0, 1.0, 0.2]);
        let contrast = adjust(|c| c.contrast = 2.0);
        // Mid grey stays; others move away from it.
        near(apply([0.5, 0.5, 0.5], &contrast).0, [0.5; 3]);
        near(apply([0.25, 0.5, 0.75], &contrast).0, [0.0, 0.5, 1.0]);
        let flat = adjust(|c| c.contrast = 0.0);
        near(apply([0.1, 0.9, 0.4], &flat).0, [0.5; 3]);
    }

    #[test]
    fn saturation_keeps_grey_and_luma() {
        let grey = adjust(|c| c.saturation = 0.0);
        let out = apply([1.0, 0.0, 0.0], &grey).0;
        near(out, [out[0]; 3]);
        assert!((out[0] - 0.213).abs() < 0.01, "red's luma: {out:?}");
        // Greys are the same at any saturation.
        for s in [0.0, 0.5, 2.0] {
            near(apply([0.4; 3], &adjust(|c| c.saturation = s)).0, [0.4; 3]);
        }
        // More saturation moves a colour away from grey, with the same brightness.
        let more = apply([0.6, 0.4, 0.4], &adjust(|c| c.saturation = 2.0)).0;
        assert!(more[0] - more[1] > 0.2 - 0.001);
    }

    #[test]
    fn hue_rotates_around_the_greys() {
        let by = |deg| adjust(move |c| c.hue = deg);
        // A full turn is no change; greys never change.
        near(apply([0.8, 0.2, 0.1], &by(360.0)).0, [0.8, 0.2, 0.1]);
        near(apply([0.5; 3], &by(120.0)).0, [0.5; 3]);
        // Red turned 120 degrees is mostly green; 240 mostly blue.
        let g = apply([1.0, 0.0, 0.0], &by(120.0)).0;
        assert!(g[1] > g[0] && g[1] > g[2], "{g:?}");
        let b = apply([1.0, 0.0, 0.0], &by(240.0)).0;
        assert!(b[2] > b[0] && b[2] > b[1], "{b:?}");
        // Opposite turns cancel.
        let there = apply([0.7, 0.3, 0.2], &by(50.0)).0;
        near(apply(there, &by(-50.0)).0, [0.7, 0.3, 0.2]);
    }

    fn green_key(tolerance: f32, softness: f32, spill: f32) -> ColorAdjust {
        adjust(|c| {
            c.key = Some(ChromaKey {
                color: [0, 255, 0],
                tolerance,
                softness,
                spill,
            })
        })
    }

    #[test]
    fn the_key_removes_its_colour_and_keeps_the_rest() {
        let k = green_key(0.2, 0.1, 0.0);
        // The key colour, and lighter or washed-out versions of it, go clear.
        for g in [[0.0, 1.0, 0.0], [0.2, 0.9, 0.2], [0.1, 0.8, 0.1]] {
            assert!(apply(g, &k).1 < 0.01, "{g:?}");
        }
        // A much darker green has little colour left to key on: mostly clear.
        assert!(apply([0.0, 0.6, 0.0], &k).1 < 0.1);
        // Skin, red, blue, white and black stay.
        for keep in [
            [0.9, 0.6, 0.5],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0; 3],
            [0.0; 3],
        ] {
            assert!(apply(keep, &k).1 > 0.99, "{keep:?}");
        }
        // No key, nothing removed.
        assert_eq!(apply([0.0, 1.0, 0.0], &ColorAdjust::default()).1, 1.0);
    }

    #[test]
    fn softness_makes_a_gradual_edge() {
        let hard = green_key(0.2, 0.0, 0.0);
        let soft = green_key(0.2, 0.3, 0.0);
        // A greenish colour just outside the tolerance.
        let px = [0.35, 0.75, 0.35];
        let d = {
            let [kcb, kcr] = chroma_of([0.0, 1.0, 0.0]);
            let [cb, cr] = chroma_of(px);
            ((cb - kcb).powi(2) + (cr - kcr).powi(2)).sqrt()
        };
        assert!(d > 0.2 && d < 0.5, "test colour at {d}");
        assert!(apply(px, &hard).1 > 0.99);
        let a = apply(px, &soft).1;
        assert!(a > 0.0 && a < 1.0, "partly clear: {a}");
        // Further from the key is more opaque.
        assert!(apply([0.6, 0.6, 0.6], &soft).1 > a);
        // Alpha goes up smoothly with distance.
        let mut last = 0.0;
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            let a = apply([t, 1.0 - t * 0.4, t], &soft).1;
            assert!(a + 1e-4 >= last, "not monotonic at {t}");
            last = a;
        }
    }

    #[test]
    fn spill_suppression_takes_green_out_of_edges() {
        // A skin tone with green light spilled on it.
        let px = [0.7, 0.85, 0.5];
        let off = green_key(0.05, 0.05, 0.0);
        let on = green_key(0.05, 0.05, 1.0);
        let (kept_off, _) = apply(px, &off);
        let (kept_on, _) = apply(px, &on);
        near(kept_off, px);
        // Green is brought down to the stronger of red and blue; red and blue are untouched.
        near(kept_on, [0.7, 0.7, 0.5]);
        // Half as much spill suppression goes half way.
        near(apply(px, &green_key(0.05, 0.05, 0.5)).0, [0.7, 0.775, 0.5]);
        // A colour with no excess green is left alone.
        near(apply([0.8, 0.3, 0.2], &on).0, [0.8, 0.3, 0.2]);
    }

    #[test]
    fn the_key_picks_its_dominant_channel() {
        let mask = |color| {
            key_uniforms(&ChromaKey {
                color,
                ..ChromaKey::default()
            })
            .1
        };
        assert_eq!(&mask([0, 255, 0])[..3], [0.0, 1.0, 0.0]);
        assert_eq!(&mask([0, 0, 255])[..3], [0.0, 0.0, 1.0]);
        assert_eq!(&mask([255, 30, 30])[..3], [1.0, 0.0, 0.0]);
        // A blue-ish screen is blue; a yellow-green one is mostly green.
        assert_eq!(&mask([40, 90, 230])[..3], [0.0, 0.0, 1.0]);
        assert_eq!(&mask([100, 220, 40])[..3], [0.0, 1.0, 0.0]);
    }

    #[test]
    fn key_then_adjust() {
        // Adjustments apply to what the key keeps.
        let mut c = green_key(0.15, 0.1, 0.0);
        c.brightness = 0.1;
        let (rgb, a) = apply([0.2, 0.3, 0.9], &c);
        near(rgb, [0.3, 0.4, 1.0]);
        assert!(a > 0.99);
    }
}
