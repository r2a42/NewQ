//! Drawing a line-up's crosses on a projector (mapping plan 4.4), pure: the
//! little sprite of each cross and its number, where a sprite goes, and the
//! model's outline as a whole-screen picture.
//!
//! How the app uses them (`app.rs`, the overlay's group in
//! `update_outputs`): while a line-up overlay is held for a display, its
//! monitor's window is sent black (or its usual list dimmed) and then one
//! last group whose canvas is the monitor's own size in raw pixels (no
//! keystone, mesh, map or fine-tune; the window's edge blend off). In that
//! group: the outline picture first (`outline_picture`, made at the
//! monitor's size, `Fit::Stretch`, drawn again only when a new outline
//! comes), then one layer per mark: its sprite (`sprite`, one texture per
//! number and look, made once) with `Fit::Stretch` and `sprite_transform`.
//! So a nudge changes a few numbers and uploads nothing.
//!
//! What is drawn (straight alpha, transparent around):
//! - every point is a ring with its number above and to the right of it
//!   (`Look::Plain` white, `Look::Off` grey, dimmer);
//! - the point being moved (`Look::Active`) is yellow, with a cross through
//!   the ring whose middle is the point itself, so the operator can put it
//!   on a corner they can see on the wall;
//! - the numbers are a built-in 5 × 7 figure font, so the same pictures are
//!   made on every computer whatever fonts it has;
//! - the outline is lines `OUTLINE_WIDTH` monitor pixels wide (more on a big
//!   monitor), cut at the picture's edge before they are stepped along, so a
//!   line with an end far outside costs no more than one across the picture.

// Drawn by the app's overlay group (round 2, agent B); until then only the
// tests use it.
#![allow(dead_code)]

use crate::media::video::RgbaImage;
use crate::show::LayerTransform;

/// A sprite's side in pixels (square).
pub const SPRITE: u32 = 64;

/// The ring's radius and its line's width (sprite pixels).
pub const RING: f32 = 13.0;
const RING_WIDTH: f32 = 2.2;
/// The cross's half-length and half-width (sprite pixels).
const CROSS: f32 = 30.0;
const CROSS_HALF_WIDTH: f32 = 1.0;

/// The colours (straight RGBA).
pub const PLAIN: [u8; 4] = [255, 255, 255, 255];
pub const ACTIVE: [u8; 4] = [255, 220, 0, 255];
pub const OFF: [u8; 4] = [120, 120, 120, 255];
pub const OUTLINE: [u8; 4] = [0, 220, 255, 255];
/// The edge round the figures, so they read on any picture.
const EDGE: [u8; 4] = [0, 0, 0, 255];

/// The outline's width on a 1080-high monitor (pixels); in proportion on
/// others, never under one.
pub const OUTLINE_WIDTH: f32 = 2.0;

/// How a cross is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Look {
    Plain,
    /// The point the operator is moving.
    Active,
    /// A point switched off: dim.
    Off,
}

impl Look {
    pub fn colour(self) -> [u8; 4] {
        match self {
            Look::Plain => PLAIN,
            Look::Active => ACTIVE,
            Look::Off => OFF,
        }
    }
}

/// The figures 0 to 9, 5 wide and 7 high, a row a byte (the low 5 bits,
/// the leftmost the highest).
const FIGURES: [[u8; 7]; 10] = [
    [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
    [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
    [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
    [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
    [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
    [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
    [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
    [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
    [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
    [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
];

/// Where figure `d`'s cell `(x, y)` (0..5, 0..7) is set.
fn figure_bit(d: u8, x: usize, y: usize) -> bool {
    FIGURES[d as usize % 10][y] & (0x10 >> x) != 0
}

fn blank(w: u32, h: u32) -> RgbaImage {
    RgbaImage {
        width: w,
        height: h,
        data: vec![0; w as usize * h as usize * 4],
    }
}

/// Lay `c` with coverage `a` (0..1) over pixel `(x, y)`, straight alpha.
fn put(img: &mut RgbaImage, x: i64, y: i64, c: [u8; 4], a: f32) {
    if x < 0 || y < 0 || x >= img.width as i64 || y >= img.height as i64 {
        return;
    }
    let a = (a.clamp(0.0, 1.0) * c[3] as f32 / 255.0).clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let i = (y as usize * img.width as usize + x as usize) * 4;
    let d = &mut img.data[i..i + 4];
    let da = d[3] as f32 / 255.0;
    let out = a + da * (1.0 - a);
    for k in 0..3 {
        let v = (c[k] as f32 * a + d[k] as f32 * da * (1.0 - a)) / out;
        d[k] = v.round().clamp(0.0, 255.0) as u8;
    }
    d[3] = (out * 255.0).round().clamp(0.0, 255.0) as u8;
}

/// The cross for point `n` (its number as the operator sees it), with its
/// ring and number, `SPRITE` square, its middle the point, transparent
/// around.
pub fn sprite(n: u16, look: Look) -> RgbaImage {
    let side = SPRITE;
    let mut img = blank(side, side);
    let c = look.colour();
    let mid = side as f32 / 2.0;
    // The ring, with a dark edge either side of it.
    for y in 0..side {
        for x in 0..side {
            let (px, py) = (x as f32 + 0.5 - mid, y as f32 + 0.5 - mid);
            let r = (px * px + py * py).sqrt();
            let off = (r - RING).abs();
            let edge = (RING_WIDTH / 2.0 + 1.2 - off).clamp(0.0, 1.0);
            put(&mut img, x as i64, y as i64, EDGE, edge * 0.8);
            let ink = (RING_WIDTH / 2.0 + 0.5 - off).clamp(0.0, 1.0);
            put(&mut img, x as i64, y as i64, c, ink);
        }
    }
    if look == Look::Active {
        // A cross through the ring, its two lines meeting at the middle.
        for y in 0..side {
            for x in 0..side {
                let (px, py) = (x as f32 + 0.5 - mid, y as f32 + 0.5 - mid);
                let along = px.abs().max(py.abs());
                let across = px.abs().min(py.abs());
                if along <= CROSS {
                    let ink = (CROSS_HALF_WIDTH + 0.5 - across).clamp(0.0, 1.0);
                    put(&mut img, x as i64, y as i64, c, ink);
                }
            }
        }
    }
    number(&mut img, n, c);
    img
}

/// Point `n`'s figures above and to the right of the ring.
fn number(img: &mut RgbaImage, n: u16, c: [u8; 4]) {
    let digits: Vec<u8> = n.to_string().bytes().map(|b| b - b'0').collect();
    // Two figures at twice the size fit beside the ring; more at one.
    let scale: i64 = if digits.len() <= 2 { 2 } else { 1 };
    let cell = 6 * scale;
    let x0 = SPRITE as i64 / 2 + RING as i64 - 2;
    let x0 = x0
        .min(SPRITE as i64 - cell * digits.len() as i64 - 1)
        .max(0);
    let y0 = 2;
    let set = |d: u8, fx: i64, fy: i64| -> bool {
        (0..5).contains(&fx) && (0..7).contains(&fy) && figure_bit(d, fx as usize, fy as usize)
    };
    for (k, d) in digits.iter().enumerate() {
        let left = x0 + k as i64 * cell;
        // The edge first: every pixel next to a set one.
        for fy in -1..8 {
            for fx in -1..6 {
                let near = (-1..=1).any(|dy| (-1..=1).any(|dx| set(*d, fx + dx, fy + dy)));
                if !near {
                    continue;
                }
                let on = set(*d, fx, fy);
                for sy in 0..scale {
                    for sx in 0..scale {
                        let (x, y) = (left + fx * scale + sx, y0 + fy * scale + sy);
                        if on {
                            put(img, x, y, c, 1.0);
                        } else {
                            put(img, x, y, EDGE, 0.85);
                        }
                    }
                }
            }
        }
    }
}

/// Where a sprite goes: its layer is `Fit::Stretch` in a group whose canvas
/// is `monitor` (raw pixels), so this puts its middle at `at` (pixels in a
/// picture `pixels` big, scaled onto the monitor) at `SPRITE` monitor pixels
/// square. A picture or monitor with no size gives a sprite out of sight.
pub fn sprite_transform(at: [f32; 2], pixels: (u32, u32), monitor: (u32, u32)) -> LayerTransform {
    if pixels.0 == 0 || pixels.1 == 0 || monitor.0 == 0 || monitor.1 == 0 {
        return LayerTransform {
            scale_x: 0.0,
            scale_y: 0.0,
            ..LayerTransform::default()
        };
    }
    LayerTransform {
        x: at[0] / pixels.0 as f32 - 0.5,
        y: at[1] / pixels.1 as f32 - 0.5,
        scale_x: SPRITE as f32 / monitor.0 as f32,
        scale_y: SPRITE as f32 / monitor.1 as f32,
        ..LayerTransform::default()
    }
}

/// The segment `a`–`b` cut to the box `lo..hi` both ways (Liang–Barsky), or
/// None when none of it is inside.
fn clip(a: [f32; 2], b: [f32; 2], lo: [f32; 2], hi: [f32; 2]) -> Option<([f32; 2], [f32; 2])> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for k in 0..2 {
        for (p, q) in [(-d[k], a[k] - lo[k]), (d[k], hi[k] - a[k])] {
            if p == 0.0 {
                if q < 0.0 {
                    return None;
                }
            } else {
                let r = q / p;
                if p < 0.0 {
                    t0 = t0.max(r);
                } else {
                    t1 = t1.min(r);
                }
                if t0 > t1 {
                    return None;
                }
            }
        }
    }
    let at = |t: f32| [a[0] + d[0] * t, a[1] + d[1] * t];
    Some((at(t0), at(t1)))
}

/// A line `width` wide from `a` to `b` (pixels of `img`), its ends round.
fn line(img: &mut RgbaImage, a: [f32; 2], b: [f32; 2], width: f32, c: [u8; 4]) {
    let half = width / 2.0;
    let pad = half + 1.0;
    let lo = [-pad, -pad];
    let hi = [img.width as f32 + pad, img.height as f32 + pad];
    let Some((a, b)) = clip(a, b, lo, hi) else {
        return;
    };
    let d = [b[0] - a[0], b[1] - a[1]];
    let len2 = d[0] * d[0] + d[1] * d[1];
    let x_lo = (a[0].min(b[0]) - pad).floor().max(0.0) as i64;
    let x_hi = (a[0].max(b[0]) + pad).ceil().min(img.width as f32) as i64;
    let y_lo = (a[1].min(b[1]) - pad).floor().max(0.0) as i64;
    let y_hi = (a[1].max(b[1]) + pad).ceil().min(img.height as f32) as i64;
    let slope_wide = d[0].abs() >= d[1].abs();
    // Along the long axis, only the pixels near the line (not its box).
    let (outer_lo, outer_hi, inner_lo, inner_hi) = if slope_wide {
        (x_lo, x_hi, y_lo, y_hi)
    } else {
        (y_lo, y_hi, x_lo, x_hi)
    };
    for o in outer_lo..outer_hi {
        let c_o = o as f32 + 0.5;
        // Where the line is at this column (or row).
        let (k, ko) = if slope_wide { (0, 1) } else { (1, 0) };
        let t = if d[k] == 0.0 {
            0.0
        } else {
            ((c_o - a[k]) / d[k]).clamp(0.0, 1.0)
        };
        let at = a[ko] + d[ko] * t;
        let from = ((at - pad).floor() as i64).max(inner_lo);
        let to = ((at + pad).ceil() as i64).min(inner_hi);
        for i in from..to {
            let (x, y) = if slope_wide { (o, i) } else { (i, o) };
            let p = [x as f32 + 0.5, y as f32 + 0.5];
            let s = if len2 > 0.0 {
                (((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let q = [a[0] + d[0] * s - p[0], a[1] + d[1] * s - p[1]];
            let dist = (q[0] * q[0] + q[1] * q[1]).sqrt();
            put(img, x, y, c, half + 0.5 - dist);
        }
    }
}

/// The model's outline (pieces of lines, in pixels of a picture `pixels`
/// big) drawn into a picture `size` big (the monitor's), transparent
/// elsewhere.
pub fn outline_picture(
    outline: &[Vec<[f32; 2]>],
    pixels: (u32, u32),
    size: (u32, u32),
) -> RgbaImage {
    let (w, h) = (size.0.max(1), size.1.max(1));
    let mut img = blank(w, h);
    if pixels.0 == 0 || pixels.1 == 0 {
        return img;
    }
    let sx = w as f32 / pixels.0 as f32;
    let sy = h as f32 / pixels.1 as f32;
    let width = (OUTLINE_WIDTH * h as f32 / 1080.0).max(1.0);
    let finite = |p: &[f32; 2]| p[0].is_finite() && p[1].is_finite();
    for piece in outline {
        for pair in piece.windows(2) {
            if !(finite(&pair[0]) && finite(&pair[1])) {
                continue;
            }
            let a = [pair[0][0] * sx, pair[0][1] * sy];
            let b = [pair[1][0] * sx, pair[1][1] * sy];
            line(&mut img, a, b, width, OUTLINE);
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(img: &RgbaImage, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * img.width + x) * 4) as usize;
        [
            img.data[i],
            img.data[i + 1],
            img.data[i + 2],
            img.data[i + 3],
        ]
    }

    fn inked(img: &RgbaImage) -> usize {
        img.data.chunks(4).filter(|p| p[3] > 0).count()
    }

    #[test]
    fn a_sprite_is_square_and_its_middle_is_the_point() {
        let s = sprite(1, Look::Active);
        assert_eq!((s.width, s.height), (SPRITE, SPRITE));
        assert_eq!(s.data.len(), (SPRITE * SPRITE * 4) as usize);
        // The cross's lines meet at the middle: the four pixels round it
        // are the active colour, solid.
        let m = SPRITE / 2;
        for (x, y) in [(m - 1, m - 1), (m, m - 1), (m - 1, m), (m, m)] {
            assert_eq!(at(&s, x, y), ACTIVE, "({x}, {y})");
        }
        // And it is symmetrical about the middle (but for the number, top
        // right): the bottom half mirrors the top's left side.
        for y in 0..m {
            for x in 0..m {
                assert_eq!(
                    at(&s, x, y)[3],
                    at(&s, x, SPRITE - 1 - y)[3],
                    "({x}, {y}) against its mirror"
                );
            }
        }
        // Transparent in the corners.
        for (x, y) in [(0, SPRITE - 1), (SPRITE - 1, SPRITE - 1), (0, 0)] {
            assert_eq!(at(&s, x, y)[3], 0, "({x}, {y})");
        }
    }

    #[test]
    fn a_plain_point_is_a_ring_with_its_middle_clear() {
        let s = sprite(3, Look::Plain);
        let m = SPRITE / 2;
        // Nothing at the point itself: the wall shows through.
        assert_eq!(at(&s, m, m)[3], 0);
        // The ring, straight below the middle.
        let y = (m as f32 + RING) as u32;
        assert_eq!(at(&s, m, y), PLAIN);
        // Off is the same ring, grey.
        let off = sprite(3, Look::Off);
        assert_eq!(at(&off, m, m)[3], 0);
        assert_eq!(at(&off, m, y), OFF);
    }

    #[test]
    fn each_number_draws_its_own_figures() {
        let mut seen = std::collections::HashSet::new();
        for n in [1u16, 2, 7, 10, 12, 99, 100, 65535] {
            let s = sprite(n, Look::Plain);
            assert!(seen.insert(s.data.clone()), "{n} looks like another number");
        }
        // The figures are in the top right, above the ring.
        let s = sprite(8, Look::Plain);
        let ring_only = {
            let mut i = blank(SPRITE, SPRITE);
            let c = sprite(8, Look::Plain);
            for y in 20..SPRITE {
                for x in 0..SPRITE {
                    let k = ((y * SPRITE + x) * 4) as usize;
                    i.data[k..k + 4].copy_from_slice(&c.data[k..k + 4]);
                }
            }
            i
        };
        assert!(inked(&s) > inked(&ring_only) + 20, "figures above the ring");
        // Every figure stays inside the sprite, however long.
        let wide = sprite(65535, Look::Active);
        assert_eq!(wide.data.len(), (SPRITE * SPRITE * 4) as usize);
    }

    #[test]
    fn a_sprite_goes_where_its_point_is_on_any_monitor() {
        // The middle of a 1920 x 1080 picture on a 1920 x 1080 monitor: no
        // offset, a 64-pixel sprite.
        let t = sprite_transform([960.0, 540.0], (1920, 1080), (1920, 1080));
        assert_eq!((t.x, t.y), (0.0, 0.0));
        assert!((t.scale_x - 64.0 / 1920.0).abs() < 1e-7);
        assert!((t.scale_y - 64.0 / 1080.0).abs() < 1e-7);
        // The top-left corner is half the monitor left and up.
        let t = sprite_transform([0.0, 0.0], (1920, 1080), (3840, 2160));
        assert_eq!((t.x, t.y), (-0.5, -0.5));
        // On a bigger monitor the sprite is as many pixels, so less of it.
        assert!((t.scale_x - 64.0 / 3840.0).abs() < 1e-7);
        // No size: out of sight.
        let t = sprite_transform([5.0, 5.0], (0, 1080), (1920, 1080));
        assert_eq!((t.scale_x, t.scale_y), (0.0, 0.0));
    }

    #[test]
    fn the_outline_is_drawn_where_its_pixels_say_scaled_to_the_monitor() {
        // A box round the middle of a 3840 x 2160 picture, drawn on a
        // monitor half that size.
        let square = vec![vec![
            [960.0, 540.0],
            [2880.0, 540.0],
            [2880.0, 1620.0],
            [960.0, 1620.0],
            [960.0, 540.0],
        ]];
        let img = outline_picture(&square, (3840, 2160), (1920, 1080));
        assert_eq!((img.width, img.height), (1920, 1080));
        // On the line (half the place), solid.
        for (x, y) in [(960, 269), (960, 270), (479, 540), (480, 540), (1439, 600)] {
            assert_eq!(at(&img, x, y), OUTLINE, "({x}, {y})");
        }
        // Off it, nothing: inside the box, outside it, the corners.
        for (x, y) in [(960, 540), (100, 100), (1800, 1000), (0, 0), (1919, 1079)] {
            assert_eq!(at(&img, x, y)[3], 0, "({x}, {y})");
        }
        // Thinner on a smaller monitor, never under a pixel.
        let small = outline_picture(&square, (3840, 2160), (480, 270));
        assert!(at(&small, 240, 67)[3] > 0 || at(&small, 240, 68)[3] > 0);
        // About two pixels wide all along: four sides of about 960 or 540
        // pixels.
        let n = inked(&img);
        let long = 2 * (960 + 540);
        assert!(n > long && n < long * 3, "{n} pixels inked");
    }

    #[test]
    fn an_outline_far_outside_or_broken_costs_nothing_and_never_panics() {
        let wild = vec![
            vec![[-1e6, -1e6], [1e6, 1e6]],
            vec![[f32::NAN, 3.0], [5.0, 5.0]],
            vec![[1e6, -5.0], [2e6, -5.0]],
            vec![[3.0, 3.0]],
            vec![],
        ];
        let img = outline_picture(&wild, (100, 100), (100, 100));
        // Only the first (the diagonal through the picture) is drawn.
        assert_eq!(at(&img, 50, 50), OUTLINE);
        assert_eq!(at(&img, 90, 10)[3], 0);
        let n = inked(&img);
        assert!(n > 100 && n < 500, "{n}");
        // A picture with no size draws nothing; no monitor size is one pixel.
        let img = outline_picture(&wild, (0, 0), (0, 0));
        assert_eq!((img.width, img.height), (1, 1));
        assert_eq!(inked(&img), 0);
    }

    #[test]
    fn clipping_keeps_what_is_inside() {
        let lo = [0.0, 0.0];
        let hi = [10.0, 10.0];
        assert_eq!(
            clip([-5.0, 5.0], [15.0, 5.0], lo, hi),
            Some(([0.0, 5.0], [10.0, 5.0]))
        );
        assert_eq!(clip([-5.0, -5.0], [-1.0, 20.0], lo, hi), None);
        assert_eq!(
            clip([2.0, 2.0], [3.0, 3.0], lo, hi),
            Some(([2.0, 2.0], [3.0, 3.0]))
        );
    }
}
