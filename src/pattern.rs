//! The test patterns a display can show to be lined up: drawn on the CPU into
//! an RGBA picture the size of the display, which then plays like an image.
//! (`show::PatternKind` lists them and says what each is for.)

use crate::show::PatternKind;

const WHITE: [u8; 3] = [255, 255, 255];
const BLACK: [u8; 3] = [0, 0, 0];

/// Draw `kind` at `w` by `h` pixels. `number` is the display's number, drawn
/// large on the patterns that have room for it.
pub fn render(kind: PatternKind, w: u32, h: u32, number: usize) -> Vec<u8> {
    let (w, h) = (w.max(2), h.max(2));
    let kind = resolve(kind, w, h);
    let digits = Digits::new(w, h, number);
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let c = colour(kind, x, y, w, h, &digits);
            let i = ((y * w + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    px
}

fn colour(kind: PatternKind, x: u32, y: u32, w: u32, h: u32, digits: &Digits) -> [u8; 3] {
    let number = |c: [u8; 3]| match digits.at(x as f32, y as f32) {
        Some(true) => WHITE,
        Some(false) => BLACK,
        None => c,
    };
    match kind {
        PatternKind::Smpte | PatternKind::Smpte169 => number(smpte(x, y, w, h, 16.0 / 9.0)),
        PatternKind::Smpte1610 => number(smpte(x, y, w, h, 16.0 / 10.0)),
        PatternKind::Bars => number(bars(x, y, w, h)),
        PatternKind::Grid => number(grid(x, y, w, h)),
        PatternKind::Crosshatch => crosshatch(x, y, w),
        PatternKind::Checkerboard => checkerboard(x, y, w),
        PatternKind::Circles => number(circles(x, y, w, h)),
        PatternKind::GreySteps => grey_steps(x, w),
        PatternKind::Ramp => {
            let v = (x as f32 / (w - 1) as f32 * 255.0).round() as u8;
            [v, v, v]
        }
        PatternKind::White => WHITE,
        PatternKind::Black => BLACK,
        PatternKind::Colours => colours(x, y, w, h),
        PatternKind::Frame => number(frame(x, y, w, h)),
        PatternKind::Focus => focus(x, y, w, h),
    }
}

/// The SMPTE bars in the shape of the display when the choice is the automatic
/// one: 16:9, or 16:10 when the display's shape is nearer that (the halfway
/// point is about 1.69). The other patterns are left as they are.
pub fn resolve(kind: PatternKind, w: u32, h: u32) -> PatternKind {
    if kind != PatternKind::Smpte {
        return kind;
    }
    let aspect = w.max(1) as f32 / h.max(1) as f32;
    if (aspect - 16.0 / 10.0).abs() < (aspect - 16.0 / 9.0).abs() {
        PatternKind::Smpte1610
    } else {
        PatternKind::Smpte169
    }
}

/// SMPTE colour bars (ECR 1-1978, as RP 219 lays them out for wide pictures):
/// a 4:3 picture's worth of bars in the middle with 40% grey at the sides, so
/// `aspect` (16:9 or 16:10) only changes how wide the side panels are.
/// From the top: seven bars at 75%, a row of reversed blue bars, then the
/// -I, white and +Q blocks and the PLUGE strips for setting the black level.
///
/// Full-range RGB, as a computer sends it: 75% is 191 and black is 0 wherever the
/// pattern means black (the reversed row's black bars, the bottom row's black and
/// its sides), so it is one black on the display. The PLUGE "above black" strip is
/// `ABOVE_BLACK`, about 4% (just visible when the black level is right); "below
/// black" can't be sent in full range (it would be under 0), so that strip is black
/// too, where studio-range bars would show 4% under it.
fn smpte(x: u32, y: u32, w: u32, h: u32, aspect: f32) -> [u8; 3] {
    const GREY: [u8; 3] = [102, 102, 102];
    const BARS: [[u8; 3]; 7] = [
        [191, 191, 191],
        [191, 191, 0],
        [0, 191, 191],
        [0, 191, 0],
        [191, 0, 191],
        [191, 0, 0],
        [0, 0, 191],
    ];
    const REVERSED: [[u8; 3]; 7] = [
        [0, 0, 191],
        BLACK,
        [191, 0, 191],
        BLACK,
        [0, 191, 191],
        BLACK,
        [191, 191, 191],
    ];
    let side = ((1.0 - (4.0 / 3.0) / aspect) / 2.0).clamp(0.0, 0.25);
    let (fx, fy) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
    let u = (fx - side) / (1.0 - 2.0 * side);
    let inside = (0.0..1.0).contains(&u);
    let column = (u * 7.0) as usize;
    if fy < 2.0 / 3.0 {
        return if inside { BARS[column.min(6)] } else { GREY };
    }
    if fy < 3.0 / 4.0 {
        return if inside {
            REVERSED[column.min(6)]
        } else {
            GREY
        };
    }
    if !inside {
        return BLACK;
    }
    // The bottom row in 28ths of the width of the bars.
    match (u * 28.0) as u32 {
        0..=4 => [0, 33, 76],
        5..=9 => WHITE,
        10..=14 => [50, 0, 106],
        // Black, then PLUGE: below black (black here, see above), black, above black.
        15..=19 => BLACK,
        20..=21 => BELOW_BLACK,
        22..=23 => BLACK,
        24..=25 => ABOVE_BLACK,
        _ => BLACK,
    }
}

/// The SMPTE bars' PLUGE strip just above black: 10 of 255, about 4%.
const ABOVE_BLACK: [u8; 3] = [10, 10, 10];
/// And the one below black, which full-range RGB can't show: black.
const BELOW_BLACK: [u8; 3] = BLACK;

/// How thick a line is: one pixel on small pictures, more on big ones so it
/// can still be seen.
fn line(w: u32) -> u32 {
    (w / 960).max(1)
}

fn bars(x: u32, y: u32, w: u32, h: u32) -> [u8; 3] {
    const BARS: [[u8; 3]; 7] = [
        [192, 192, 192],
        [192, 192, 0],
        [0, 192, 192],
        [0, 192, 0],
        [192, 0, 192],
        [192, 0, 0],
        [0, 0, 192],
    ];
    let mut c = BARS[(x * 7 / w) as usize];
    if x.is_multiple_of((w / 16).max(1)) || y.is_multiple_of((h / 9).max(1)) {
        c = WHITE;
    }
    if x < 4 || y < 4 || x >= w - 4 || y >= h - 4 {
        c = [255, 0, 0];
    }
    c
}

fn grid(x: u32, y: u32, w: u32, h: u32) -> [u8; 3] {
    let t = line(w);
    let near = |v: u32, at: u32| v >= at.saturating_sub(t / 2) && v < at.saturating_sub(t / 2) + t;
    let (cx, cy) = (w / 2, h / 2);
    if x < t * 3 || y < t * 3 || x + t * 3 >= w || y + t * 3 >= h {
        return [255, 0, 0];
    }
    // The centre cross is thicker and yellow.
    if x.abs_diff(cx) < t * 2 || y.abs_diff(cy) < t * 2 {
        return [255, 255, 0];
    }
    let (sx, sy) = ((w / 16).max(2), (h / 9).max(2));
    if x % sx < t || y % sy < t || near(x % sx, 0) {
        return [200, 200, 200];
    }
    BLACK
}

/// Square cells a few pixels across, lines one pixel wide.
fn crosshatch(x: u32, y: u32, w: u32) -> [u8; 3] {
    let cell = (w / 48).clamp(8, 64);
    if x.is_multiple_of(cell) || y.is_multiple_of(cell) {
        WHITE
    } else {
        BLACK
    }
}

fn checkerboard(x: u32, y: u32, w: u32) -> [u8; 3] {
    let cell = (w / 16).max(2);
    if (x / cell + y / cell).is_multiple_of(2) {
        WHITE
    } else {
        BLACK
    }
}

fn circles(x: u32, y: u32, w: u32, h: u32) -> [u8; 3] {
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
    let r = dx.hypot(dy);
    let t = line(w) as f32;
    // Rings every tenth of the height; the one that touches top and bottom
    // is the fifth, so it is brighter and thicker.
    let step = h as f32 / 10.0;
    let ring = (r / step).round();
    if ring >= 1.0 {
        let off = (r - ring * step).abs();
        if ring as u32 == 5 && off < t * 1.5 {
            return [255, 255, 0];
        }
        if off < t {
            return WHITE;
        }
    }
    // The corner to corner diagonals.
    let diagonal = (dx * h as f32 - dy * w as f32).abs() / (w as f32).hypot(h as f32);
    let other = (dx * h as f32 + dy * w as f32).abs() / (w as f32).hypot(h as f32);
    if diagonal < t / 2.0 + 0.5 || other < t / 2.0 + 0.5 {
        return [90, 90, 90];
    }
    if x < 3 || y < 3 || x + 3 >= w || y + 3 >= h {
        return [255, 0, 0];
    }
    BLACK
}

/// Eleven bars from black to white.
fn grey_steps(x: u32, w: u32) -> [u8; 3] {
    let step = (x * 11 / w).min(10);
    let v = (step as f32 * 255.0 / 10.0).round() as u8;
    [v, v, v]
}

/// Red, green and blue above; cyan, magenta and yellow below.
fn colours(x: u32, y: u32, w: u32, h: u32) -> [u8; 3] {
    let col = (x * 3 / w).min(2);
    if y < h / 2 {
        [[255, 0, 0], [0, 255, 0], [0, 0, 255]][col as usize]
    } else {
        [[0, 255, 255], [255, 0, 255], [255, 255, 0]][col as usize]
    }
}

/// Frames at the edge, at 90% and at 80%, diagonals and a centre cross.
fn frame(x: u32, y: u32, w: u32, h: u32) -> [u8; 3] {
    let t = line(w);
    let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
    let edge_dist = fx.min(1.0 - fx).min(fy).min(1.0 - fy);
    let (px, py) = (x.min(w - 1 - x), y.min(h - 1 - y));
    if px.min(py) < t * 2 {
        return WHITE;
    }
    // Inset by 5% and 10% of each side.
    let on = |inset: f32| {
        let (ix, iy) = ((w as f32 * inset) as u32, (h as f32 * inset) as u32);
        let near = |v: u32, at: u32| v >= at && v < at + t;
        let inside = x >= ix && x + ix < w && y >= iy && y + iy < h;
        inside && (near(x, ix) || near(x + ix + 1, w) || near(y, iy) || near(y + iy + 1, h))
    };
    if on(0.05) {
        return [0, 200, 255];
    }
    if on(0.10) {
        return [255, 160, 0];
    }
    let (dx, dy) = (x as f32 - w as f32 / 2.0, y as f32 - h as f32 / 2.0);
    let len = (w as f32).hypot(h as f32);
    let d1 = (dx * h as f32 - dy * w as f32).abs() / len;
    let d2 = (dx * h as f32 + dy * w as f32).abs() / len;
    if d1 < t as f32 || d2 < t as f32 || dx.abs() < t as f32 || dy.abs() < t as f32 {
        return [110, 110, 110];
    }
    let _ = edge_dist;
    BLACK
}

/// Four quadrants of one and two pixel lines, vertical and horizontal.
fn focus(x: u32, y: u32, w: u32, h: u32) -> [u8; 3] {
    let (right, bottom) = (x >= w / 2, y >= h / 2);
    let on = match (right, bottom) {
        (false, false) => x.is_multiple_of(2),
        (true, false) => y.is_multiple_of(2),
        (false, true) => x % 4 < 2,
        (true, true) => y % 4 < 2,
    };
    // A thin dividing line between the quadrants.
    if x == w / 2 || y == h / 2 {
        return [255, 0, 0];
    }
    if on { WHITE } else { BLACK }
}

/// The display's number in big seven segment digits, over the middle of the
/// picture, with a black box behind so it can be read over anything.
struct Digits {
    list: Vec<u32>,
    x0: f32,
    y0: f32,
    dw: f32,
    dh: f32,
    gap: f32,
}

impl Digits {
    fn new(w: u32, h: u32, number: usize) -> Digits {
        let list: Vec<u32> = number
            .to_string()
            .chars()
            .filter_map(|c| c.to_digit(10))
            .collect();
        // As tall as fits, also on narrow sub displays.
        let n = list.len() as f32;
        let dh = (h as f32 * 0.4).min(w as f32 * 0.7 / (n * 0.55 + (n - 1.0).max(0.0) * 0.165));
        let dw = dh * 0.55;
        let gap = dw * 0.3;
        let total = n * dw + (list.len().saturating_sub(1)) as f32 * gap;
        Digits {
            x0: (w as f32 - total) / 2.0,
            y0: (h as f32 - dh) / 2.0,
            list,
            dw,
            dh,
            gap,
        }
    }

    /// Whether (x, y) is on a digit (Some(true)), in the box around one
    /// (Some(false)), or outside them all (None).
    fn at(&self, x: f32, y: f32) -> Option<bool> {
        let mut found = None;
        for (n, d) in self.list.iter().enumerate() {
            let dx = self.x0 + n as f32 * (self.dw + self.gap);
            if x >= dx - self.dw * 0.15
                && x < dx + self.dw * 1.15
                && y >= self.y0 - self.dw * 0.15
                && y < self.y0 + self.dh + self.dw * 0.15
            {
                found = Some(seven_segment(
                    *d,
                    (x - dx) / self.dw,
                    (y - self.y0) / self.dh,
                ));
            }
        }
        found
    }
}

fn seven_segment(digit: u32, u: f32, v: f32) -> bool {
    // Segments: a (top), b (top-right), c (bottom-right), d (bottom), e (bottom-left),
    // f (top-left), g (middle).
    const SEGS: [u8; 10] = [
        0b0111111, 0b0000110, 0b1011011, 0b1001111, 0b1100110, 0b1101101, 0b1111101, 0b0000111,
        0b1111111, 0b1101111,
    ];
    let on = SEGS[digit as usize % 10];
    let t = 0.16; // stroke thickness relative to width
    let tv = t * 0.55; // ... and relative to height
    let horiz = |cy: f32| (0.0..=1.0).contains(&u) && (v - cy).abs() < tv / 2.0;
    let vert = |cx: f32, y0: f32, y1: f32| (u - cx).abs() < t / 2.0 && v >= y0 && v <= y1;
    let segs = [
        horiz(tv / 2.0),
        vert(1.0 - t / 2.0, 0.0, 0.5),
        vert(1.0 - t / 2.0, 0.5, 1.0),
        horiz(1.0 - tv / 2.0),
        vert(t / 2.0, 0.5, 1.0),
        vert(t / 2.0, 0.0, 0.5),
        horiz(0.5),
    ];
    segs.iter()
        .enumerate()
        .any(|(i, &hit)| hit && on & (1 << i) != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: u32 = 192;
    const H: u32 = 108;

    fn pic(kind: PatternKind, number: usize) -> Vec<u8> {
        render(kind, W, H, number)
    }

    fn at(p: &[u8], x: u32, y: u32) -> [u8; 3] {
        let i = ((y * W + x) * 4) as usize;
        [p[i], p[i + 1], p[i + 2]]
    }

    #[test]
    fn there_are_fifteen_with_names_and_every_one_draws_a_whole_opaque_picture() {
        assert_eq!(PatternKind::ALL.len(), 15);
        let mut labels: Vec<&str> = PatternKind::ALL.iter().map(|k| k.label()).collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), 15, "labels are different");
        for k in PatternKind::ALL {
            assert!(!k.description().is_empty());
            let p = pic(k, 1);
            assert_eq!(p.len(), (W * H * 4) as usize, "{k:?}");
            assert!(p.chunks(4).all(|c| c[3] == 255), "{k:?} is opaque");
        }
    }

    #[test]
    fn no_two_patterns_are_the_same_picture() {
        // The automatic SMPTE bars are one of the two shapes on this 16:9
        // picture, so they are the 16:9 ones; the rest all differ.
        let all: Vec<Vec<u8>> = PatternKind::ALL
            .iter()
            .filter(|k| **k != PatternKind::Smpte)
            .map(|k| pic(*k, 1))
            .collect();
        let kinds: Vec<PatternKind> = PatternKind::ALL
            .into_iter()
            .filter(|k| *k != PatternKind::Smpte)
            .collect();
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                assert_ne!(all[i], all[j], "{:?} and {:?}", kinds[i], kinds[j]);
            }
        }
    }

    #[test]
    fn the_smpte_bars_are_the_default_and_have_the_standard_layout() {
        assert_eq!(PatternKind::default(), PatternKind::Smpte);
        let p = pic(PatternKind::Smpte169, 1);
        // 16:9: side panels of 12.5% (24 of 192 pixels) in 40% grey, then the
        // seven bars at 75%, white to blue (each 24 pixels wide, from 24).
        let top = 5;
        assert_eq!(at(&p, 5, top), [102, 102, 102]);
        assert_eq!(at(&p, W - 5, top), [102, 102, 102]);
        let bars = [
            [191, 191, 191],
            [191, 191, 0],
            [0, 191, 191],
            [0, 191, 0],
            [191, 0, 191],
            [191, 0, 0],
            [0, 0, 191],
        ];
        for (i, c) in bars.iter().enumerate() {
            let x = 24 + (144 / 7) * i as u32 + 4;
            assert_eq!(at(&p, x, top), *c, "bar {i}");
        }
        // The reversed blue bars row (blue, black, magenta ...) at 2/3 to 3/4.
        let castle = H * 17 / 24;
        assert_eq!(at(&p, 24 + 4, castle), [0, 0, 191]);
        assert_eq!(at(&p, 24 + 144 / 7 + 4, castle), [0, 0, 0]);
        assert_eq!(at(&p, 24 + 144 / 7 * 2 + 4, castle), [191, 0, 191]);
        // The bottom row: -I, 100% white, +Q, black and the PLUGE strips.
        let low = H - 5;
        assert_eq!(at(&p, 24 + 4, low), [0, 33, 76]);
        assert_eq!(at(&p, 24 + 144 * 7 / 28 + 2, low), WHITE);
        assert_eq!(at(&p, 24 + 144 * 12 / 28, low), [50, 0, 106]);
        assert_eq!(at(&p, 24 + 144 * 17 / 28, low), BLACK);
        // Full range: below black can't be shown, so it is black; above black is 4%.
        assert_eq!(at(&p, 24 + 144 * 20 / 28 + 2, low), BLACK, "below black");
        assert_eq!(at(&p, 24 + 144 * 22 / 28 + 2, low), BLACK, "black");
        assert_eq!(
            at(&p, 24 + 144 * 24 / 28 + 2, low),
            [10, 10, 10],
            "above black"
        );
        assert_eq!(at(&p, 24 + 144 * 27 / 28, low), BLACK);
        // The bottom row's sides are black too, beside the grey of the rows above.
        assert_eq!(at(&p, 5, low), BLACK);
        assert_eq!(at(&p, W - 5, low), BLACK);
        assert_eq!(at(&p, 5, castle), [102, 102, 102]);
        // One black: every dark pixel is black or the above-black strip, nowhere
        // a studio-range black (16) beside a full-range one.
        let p = pic(PatternKind::Smpte169, 0);
        for px in p.chunks(4) {
            let c = [px[0], px[1], px[2]];
            assert!(
                c.iter().all(|v| *v == 0) || c == [10, 10, 10] || c.iter().any(|v| *v > 30),
                "{c:?}"
            );
        }
    }

    #[test]
    fn the_smpte_bars_pick_16_9_or_16_10_to_suit_the_display_and_can_be_forced() {
        // 16:9 and the shapes near it; 16:10 and the shapes near that.
        for (w, h) in [(1920, 1080), (1366, 768), (3840, 2160), (1360, 768)] {
            assert_eq!(resolve(PatternKind::Smpte, w, h), PatternKind::Smpte169);
        }
        for (w, h) in [(1280, 800), (1440, 900), (1680, 1050), (2560, 1600)] {
            assert_eq!(resolve(PatternKind::Smpte, w, h), PatternKind::Smpte1610);
        }
        // Wider and narrower than either take the nearer; nothing divides by 0.
        assert_eq!(
            resolve(PatternKind::Smpte, 3440, 1440),
            PatternKind::Smpte169
        );
        assert_eq!(
            resolve(PatternKind::Smpte, 1024, 768),
            PatternKind::Smpte1610
        );
        assert_eq!(resolve(PatternKind::Smpte, 0, 0), PatternKind::Smpte1610);
        // A choice that isn't the automatic one is left alone.
        assert_eq!(
            resolve(PatternKind::Smpte1610, 1920, 1080),
            PatternKind::Smpte1610
        );
        assert_eq!(
            resolve(PatternKind::Smpte169, 1280, 800),
            PatternKind::Smpte169
        );
        assert_eq!(resolve(PatternKind::Grid, 1280, 800), PatternKind::Grid);
        // The pictures: automatic is what it resolves to, and the two shapes
        // differ (the side panels are 12.5% and 8.3% of the width).
        assert_eq!(
            render(PatternKind::Smpte, 192, 108, 1),
            render(PatternKind::Smpte169, 192, 108, 1)
        );
        assert_eq!(
            render(PatternKind::Smpte, 160, 100, 2),
            render(PatternKind::Smpte1610, 160, 100, 2)
        );
        assert_ne!(
            render(PatternKind::Smpte169, 192, 108, 1),
            render(PatternKind::Smpte1610, 192, 108, 1)
        );
        let p = render(PatternKind::Smpte1610, 160, 100, 2);
        let px = |x: u32, y: u32| {
            let i = ((y * 160 + x) * 4) as usize;
            [p[i], p[i + 1], p[i + 2]]
        };
        assert_eq!(px(10, 5), [102, 102, 102], "inside the 13 pixel side panel");
        assert_eq!(px(16, 5), [191, 191, 191], "the first bar");
        // The number is drawn over them, as on the other patterns.
        assert_ne!(
            render(PatternKind::Smpte169, 192, 108, 1),
            render(PatternKind::Smpte169, 192, 108, 8)
        );
    }

    #[test]
    fn the_older_colour_bars_are_still_there() {
        let p = pic(PatternKind::Bars, 1);
        // Red border, then the first bar grey and the last blue, away from the
        // grid lines.
        assert_eq!(at(&p, 0, 50), [255, 0, 0]);
        assert_eq!(at(&p, 5, 6), [192, 192, 192]);
        assert_eq!(at(&p, 185, 6), [0, 0, 192]);
    }

    #[test]
    fn white_and_black_are_all_white_and_all_black() {
        assert!(
            pic(PatternKind::White, 7)
                .chunks(4)
                .all(|c| c[..3] == WHITE)
        );
        assert!(
            pic(PatternKind::Black, 7)
                .chunks(4)
                .all(|c| c[..3] == BLACK)
        );
    }

    #[test]
    fn grey_steps_has_eleven_even_steps_from_black_to_white() {
        let p = pic(PatternKind::GreySteps, 1);
        let row: Vec<u8> = (0..W).map(|x| at(&p, x, 50)[0]).collect();
        assert_eq!((row[0], row[W as usize - 1]), (0, 255));
        assert!(row.windows(2).all(|w| w[0] <= w[1]), "never gets darker");
        let mut steps = row.clone();
        steps.dedup();
        assert_eq!(steps.len(), 11);
        // Grey: red, green and blue agree.
        assert!((0..W).all(|x| {
            let c = at(&p, x, 50);
            c[0] == c[1] && c[1] == c[2]
        }));
    }

    #[test]
    fn the_ramp_rises_smoothly_across_and_is_the_same_down() {
        let p = pic(PatternKind::Ramp, 1);
        assert_eq!((at(&p, 0, 3)[0], at(&p, W - 1, 3)[0]), (0, 255));
        assert!((1..W).all(|x| at(&p, x, 40)[0] >= at(&p, x - 1, 40)[0]));
        assert!((0..W).all(|x| at(&p, x, 3) == at(&p, x, H - 4)));
        let mut levels: Vec<u8> = (0..W).map(|x| at(&p, x, 3)[0]).collect();
        levels.dedup();
        assert!(levels.len() > 150, "{} levels", levels.len());
    }

    #[test]
    fn the_colours_are_pure() {
        let p = pic(PatternKind::Colours, 1);
        let (l, m, r) = (W / 6, W / 2, W - W / 6);
        let (top, bottom) = (H / 4, H - H / 4);
        assert_eq!(at(&p, l, top), [255, 0, 0]);
        assert_eq!(at(&p, m, top), [0, 255, 0]);
        assert_eq!(at(&p, r, top), [0, 0, 255]);
        assert_eq!(at(&p, l, bottom), [0, 255, 255]);
        assert_eq!(at(&p, m, bottom), [255, 0, 255]);
        assert_eq!(at(&p, r, bottom), [255, 255, 0]);
    }

    #[test]
    fn the_checkerboard_alternates_in_square_cells() {
        let p = pic(PatternKind::Checkerboard, 1);
        let cell = W / 16;
        assert_eq!(at(&p, 0, 0), WHITE);
        assert_eq!(at(&p, cell, 0), BLACK);
        assert_eq!(at(&p, 0, cell), BLACK);
        assert_eq!(at(&p, cell, cell), WHITE);
        assert_eq!(
            at(&p, cell - 1, cell - 1),
            WHITE,
            "the whole cell is one colour"
        );
    }

    #[test]
    fn the_crosshatch_is_one_pixel_lines_in_square_cells() {
        let p = pic(PatternKind::Crosshatch, 1);
        let cell = (W / 48).clamp(8, 64);
        assert_eq!(at(&p, 0, 5), WHITE);
        assert_eq!(at(&p, cell, 5), WHITE);
        assert_eq!(at(&p, 1, 5), BLACK, "lines are a pixel wide");
        assert_eq!(at(&p, 3, cell), WHITE);
        assert_eq!(at(&p, 3, cell + 1), BLACK);
    }

    #[test]
    fn the_focus_lines_are_one_and_two_pixels_wide_both_ways() {
        let p = pic(PatternKind::Focus, 1);
        // Top left: vertical lines, one pixel on, one off.
        assert_eq!(
            [at(&p, 10, 10), at(&p, 11, 10), at(&p, 12, 10)],
            [WHITE, BLACK, WHITE]
        );
        assert_eq!(at(&p, 10, 10), at(&p, 10, 30), "constant down a column");
        // Top right: horizontal lines.
        let x = W / 2 + 10;
        assert_eq!(
            [at(&p, x, 10), at(&p, x, 11), at(&p, x, 12)],
            [WHITE, BLACK, WHITE]
        );
        // Bottom left: vertical, two pixels on and two off.
        let y = H / 2 + 10;
        assert_eq!(
            [at(&p, 8, y), at(&p, 9, y), at(&p, 10, y), at(&p, 11, y)],
            [WHITE, WHITE, BLACK, BLACK]
        );
        // Bottom right: horizontal, two and two (rows from a multiple of four).
        let r = (H / 2 / 4 + 2) * 4;
        assert_eq!(
            [
                at(&p, x, r),
                at(&p, x, r + 1),
                at(&p, x, r + 2),
                at(&p, x, r + 3)
            ],
            [WHITE, WHITE, BLACK, BLACK]
        );
    }

    #[test]
    fn the_circles_are_round_and_touch_the_top_and_bottom() {
        let p = pic(PatternKind::Circles, 1);
        let (cx, cy) = (W / 2, H / 2);
        // The fifth ring has a radius of half the height: lit straight up and
        // straight across the same distance from the centre, so it is a circle.
        let r = H / 2;
        let lit = |c: [u8; 3]| c != BLACK;
        assert!(lit(at(&p, cx, cy - r + 1)), "touches the top");
        assert!(
            lit(at(&p, cx + r - 1, cy)) || lit(at(&p, cx + r, cy)),
            "same distance across"
        );
        assert!(lit(at(&p, cx - r + 1, cy)) || lit(at(&p, cx - r, cy)));
        // Between rings it is dark.
        assert_eq!(
            at(&p, cx + H / 20 + 1, cy + 3),
            at(&p, cx - H / 20 - 1, cy + 3)
        );
    }

    #[test]
    fn the_frame_has_a_line_at_the_edge_and_at_90_and_80_percent() {
        let p = pic(PatternKind::Frame, 1);
        assert_eq!(at(&p, 0, 50), WHITE);
        assert_eq!(at(&p, W - 1, 50), WHITE);
        assert_eq!(at(&p, 50, 0), WHITE);
        let (ix, iy) = ((W as f32 * 0.05) as u32, (H as f32 * 0.05) as u32);
        assert_eq!(at(&p, ix, 40), [0, 200, 255], "the 90% frame");
        assert_eq!(at(&p, 60, iy), [0, 200, 255]);
        // ... and the right and bottom sides, the same distance in.
        assert_eq!(at(&p, W - 1 - ix, 40), [0, 200, 255]);
        assert_eq!(at(&p, 60, H - 1 - iy), [0, 200, 255]);
        let jx = (W as f32 * 0.10) as u32;
        assert_eq!(at(&p, jx, 40), [255, 160, 0], "the 80% frame");
        let jy = (H as f32 * 0.10) as u32;
        assert_eq!(at(&p, W - 1 - jx, 40), [255, 160, 0]);
        assert_eq!(at(&p, 60, H - 1 - jy), [255, 160, 0]);
        // Dark in between.
        assert_eq!(at(&p, 6, 40), BLACK);
    }

    #[test]
    fn the_grid_has_a_centre_cross_and_a_border() {
        let p = pic(PatternKind::Grid, 1);
        assert_eq!(at(&p, 0, 40), [255, 0, 0]);
        assert_eq!(at(&p, W / 2, 20), [255, 255, 0], "centre line");
        assert_eq!(at(&p, 20, H / 2), [255, 255, 0]);
        assert_ne!(at(&p, 24, 24), BLACK, "a grid line");
    }

    #[test]
    fn the_number_is_drawn_where_there_is_room_and_nowhere_else() {
        for k in [
            PatternKind::Smpte,
            PatternKind::Smpte169,
            PatternKind::Smpte1610,
            PatternKind::Bars,
            PatternKind::Grid,
            PatternKind::Circles,
            PatternKind::Frame,
        ] {
            assert_ne!(pic(k, 1), pic(k, 8), "{k:?} shows the number");
        }
        for k in [
            PatternKind::Crosshatch,
            PatternKind::Checkerboard,
            PatternKind::GreySteps,
            PatternKind::Ramp,
            PatternKind::White,
            PatternKind::Black,
            PatternKind::Colours,
            PatternKind::Focus,
        ] {
            assert_eq!(pic(k, 1), pic(k, 8), "{k:?} is left as it is");
        }
    }

    #[test]
    fn tiny_and_odd_sizes_do_not_panic() {
        for k in PatternKind::ALL {
            for (w, h) in [(2, 2), (3, 17), (1, 1), (33, 2), (640, 9)] {
                let p = render(k, w, h, 12);
                assert_eq!(p.len(), (w.max(2) * h.max(2) * 4) as usize, "{k:?} {w}x{h}");
            }
        }
    }

    #[test]
    fn only_the_single_pixel_patterns_ask_for_the_displays_own_size() {
        let exact: Vec<PatternKind> = PatternKind::ALL
            .into_iter()
            .filter(|k| k.pixel_exact())
            .collect();
        assert_eq!(exact, [PatternKind::Crosshatch, PatternKind::Focus]);
    }
}
