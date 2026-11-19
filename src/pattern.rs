//! The test patterns a display can show to be lined up: drawn on the CPU into
//! an RGBA picture the size of the display, which then plays like an image.
//! (`show::PatternKind` lists them and says what each is for.)

use crate::show::PatternKind;

const WHITE: [u8; 3] = [255, 255, 255];
const BLACK: [u8; 3] = [0, 0, 0];

/// Draw `kind` at `w` by `h` pixels. `number` is the display's number, drawn
/// large on the patterns that have room for it (0 = draw none). The app draws
/// through `render_shaped`; this is for a picture seen at the size it is drawn.
#[cfg(test)]
pub fn render(kind: PatternKind, w: u32, h: u32, number: usize) -> Vec<u8> {
    render_shaped(kind, w, h, number, w.max(1) as f32 / h.max(1) as f32)
}

/// `render` for a picture that is stretched to a display of the shape `shape`
/// (width over height): the patterns are drawn at a rounded size, and the SMPTE
/// bars need the shape they will be seen at to stay 4:3.
pub fn render_shaped(kind: PatternKind, w: u32, h: u32, number: usize, shape: f32) -> Vec<u8> {
    let (w, h) = (w.max(2), h.max(2));
    let shape = if shape.is_finite() && shape > 0.0 {
        shape
    } else {
        w as f32 / h as f32
    };
    let digits = Digits::new(w, h, number);
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let c = colour(kind, x, y, w, h, shape, &digits);
            let i = ((y * w + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    px
}

fn colour(
    kind: PatternKind,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    shape: f32,
    digits: &Digits,
) -> [u8; 3] {
    let number = |c: [u8; 3]| match digits.at(x as f32, y as f32) {
        Some(true) => WHITE,
        Some(false) => BLACK,
        None => c,
    };
    match kind {
        PatternKind::SmpteHd => number(smpte_hd(x, y, w, h, shape)),
        PatternKind::Smpte => number(smpte(x, y, w, h, shape)),
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

/// SMPTE colour bars (ECR 1-1978, as RP 219 lays them out for wide pictures):
/// a 4:3 picture's worth of bars in the middle with 40% grey at the sides, as
/// wide as the display's `shape` (width over height) leaves, so the bars are 4:3
/// on any display; one narrower than 4:3 is filled with them.
/// From the top: seven bars at 75%, a row of reversed blue bars, then the
/// -I, white and +Q blocks and the PLUGE strips for setting the black level.
///
/// Full-range RGB, as a computer sends it: 75% is 191 and black is 0 wherever the
/// pattern means black (the reversed row's black bars, the bottom row's black and
/// its sides), so it is one black on the display. The PLUGE "above black" strip is
/// `ABOVE_BLACK`, about 4% (just visible when the black level is right); "below
/// black" can't be sent in full range (it would be under 0), so that strip is black
/// too, where studio-range bars would show 4% under it.
fn smpte(x: u32, y: u32, w: u32, h: u32, shape: f32) -> [u8; 3] {
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
    let side = ((1.0 - (4.0 / 3.0) / shape) / 2.0).clamp(0.0, 0.49);
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

/// The HD colour bars (ARIB STD-B28, adopted as SMPTE RP 219), from the
/// standard's own figure for 1920 by 1080: four rows of 7/12, 1/12, 1/12 and 3/12
/// of the height, a 4:3 area in the middle seven bars wide, and side areas `d`
/// (240 of 1920 at 16:9).
///
/// 1. The 75% bars, 40% grey beside them.
/// 2. 75% white under the bars (the standard lets its first cell be 75% white, 100%
///    white or +I; NewQ uses 75% white), 100% cyan left and 100% blue right.
/// 3. A ramp of brightness from black to white, one 10-bit level a sample at 1920
///    (876 of the 1440 samples of the 4:3 area), centred on the screen, black before
///    it and white after; 100% yellow left and 100% red right.
/// 4. Black for 3/2 of a bar, 100% white for 2 bars, black for 5/6 of a bar, the
///    black-level strips -2%, 0, +2%, 0, +4% a third of a bar each, black for a
///    bar; 15% grey beside them.
///
/// On a display wider than 16:9 the cyan, blue, yellow and red ends stay 16:9's
/// width beside the bars and the rest is 40% grey (15% in the bottom row); on a
/// narrower one the side areas are what the shape leaves, none at 4:3. Full-range
/// RGB, as `smpte`: -2% would be under black, so it is black (`BELOW_BLACK`); +2%
/// is 5 and +4% `ABOVE_BLACK`.
fn smpte_hd(x: u32, y: u32, w: u32, h: u32, shape: f32) -> [u8; 3] {
    const GREY40: [u8; 3] = [102, 102, 102];
    const GREY15: [u8; 3] = [38, 38, 38];
    const WHITE75: [u8; 3] = [191, 191, 191];
    const PLUS2: [u8; 3] = [5, 5, 5];
    const BARS: [[u8; 3]; 7] = [
        WHITE75,
        [191, 191, 0],
        [0, 191, 191],
        [0, 191, 0],
        [191, 0, 191],
        [191, 0, 0],
        [0, 0, 191],
    ];
    // Across, in the display's own shape: the 4:3 area's left edge and width, and
    // how far the coloured ends reach out from it (16:9's side area at most).
    let shape = shape.max(0.01);
    let side = ((1.0 - (4.0 / 3.0) / shape) / 2.0).clamp(0.0, 0.49);
    let ends = (((16.0 / 9.0 - 4.0 / 3.0) / 2.0) / shape).min(side);
    let fx = (x as f32 + 0.5) / w as f32;
    let fy = (y as f32 + 0.5) / h as f32;
    // In bar widths across the 4:3 area (0..7), and where in the side areas.
    let t = (fx - side) / (1.0 - 2.0 * side) * 7.0;
    let inside = (0.0..7.0).contains(&t);
    let left = fx < side;
    let at_end =
        !inside && (fx >= side - ends - 1e-6 && left || fx < 1.0 - side + ends + 1e-6 && !left);
    if fy < 7.0 / 12.0 {
        return if inside {
            BARS[(t as usize).min(6)]
        } else {
            GREY40
        };
    }
    if fy < 8.0 / 12.0 {
        return match (inside, at_end, left) {
            (true, ..) => WHITE75,
            (_, true, true) => [0, 255, 255],
            (_, true, false) => [0, 0, 255],
            _ => GREY40,
        };
    }
    if fy < 9.0 / 12.0 {
        if inside {
            // 876 of the 4:3 area's 1440 samples, centred: in bar widths.
            let span = 7.0 * 876.0 / 1440.0;
            let from = (7.0 - span) / 2.0;
            let v = ((t - from) / span).clamp(0.0, 1.0);
            let v = (v * 255.0).round() as u8;
            return [v, v, v];
        }
        return match (at_end, left) {
            (true, true) => [255, 255, 0],
            (true, false) => [255, 0, 0],
            _ => GREY40,
        };
    }
    if !inside {
        return GREY15;
    }
    let third = 1.0 / 3.0;
    let pluge = 1.5 + 2.0 + 5.0 / 6.0;
    match t {
        t if t < 1.5 => BLACK,
        t if t < 3.5 => WHITE,
        t if t < pluge => BLACK,
        t if t < pluge + third => BELOW_BLACK,
        t if t < pluge + 2.0 * third => BLACK,
        t if t < pluge + 3.0 * third => PLUS2,
        t if t < pluge + 4.0 * third => BLACK,
        t if t < pluge + 5.0 * third => ABOVE_BLACK,
        _ => BLACK,
    }
}

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
        let list: Vec<u32> = if number == 0 {
            Vec::new()
        } else {
            number
                .to_string()
                .chars()
                .filter_map(|c| c.to_digit(10))
                .collect()
        };
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
    fn there_are_fourteen_with_names_and_every_one_draws_a_whole_opaque_picture() {
        assert_eq!(PatternKind::ALL.len(), 14);
        let mut labels: Vec<&str> = PatternKind::ALL.iter().map(|k| k.label()).collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), 14, "labels are different");
        for k in PatternKind::ALL {
            assert!(!k.description().is_empty());
            let p = pic(k, 1);
            assert_eq!(p.len(), (W * H * 4) as usize, "{k:?}");
            assert!(p.chunks(4).all(|c| c[3] == 255), "{k:?} is opaque");
        }
    }

    #[test]
    fn no_two_patterns_are_the_same_picture() {
        let all: Vec<Vec<u8>> = PatternKind::ALL.iter().map(|k| pic(*k, 1)).collect();
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                assert_ne!(
                    all[i],
                    all[j],
                    "{:?} and {:?}",
                    PatternKind::ALL[i],
                    PatternKind::ALL[j]
                );
            }
        }
    }

    /// The HD bars at 1920 by 1080, pixel by pixel against ARIB STD-B28's figure
    /// and its table of stripe widths (Table A-5, the ideal widths).
    #[test]
    fn the_hd_bars_are_the_default_and_follow_arib_std_b28_at_1920_by_1080() {
        assert_eq!(PatternKind::default(), PatternKind::SmpteHd);
        let (w, h) = (1920, 1080);
        let p = render(PatternKind::SmpteHd, w, h, 0);
        let px = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            [p[i], p[i + 1], p[i + 2]]
        };
        // Rows of 630, 90, 90 and 270 lines.
        let (r1, r2, r3, r4) = (300, 675, 765, 1000);
        assert_eq!(px(629, 629), [191, 191, 0], "row 1 ends at 630");
        assert_eq!(px(629, 631), [191, 191, 191], "row 2 starts at 630");
        assert_eq!(px(629, 719), [191, 191, 191]);
        assert_ne!(px(629, 721), [191, 191, 191], "row 3 starts at 720");
        // Row 1: 40% grey sides of 240, the bars about 206 wide from 240.
        assert_eq!(px(100, r1), [102, 102, 102]);
        assert_eq!(px(1820, r1), [102, 102, 102]);
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
            assert_eq!(px(240 + 206 * i as u32 + 100, r1), *c, "bar {i}");
        }
        // Row 2: 100% cyan, 75% white, 100% blue.
        assert_eq!(px(100, r2), [0, 255, 255]);
        assert_eq!(px(960, r2), [191, 191, 191]);
        assert_eq!(px(1820, r2), [0, 0, 255]);
        // Row 3: 100% yellow, the ramp (black up to 522, white from 1398, rising
        // one level of 255 about every 3.4 pixels between), 100% red.
        assert_eq!(px(100, r3), [255, 255, 0]);
        assert_eq!(px(1820, r3), [255, 0, 0]);
        assert_eq!(px(400, r3), [0, 0, 0]);
        assert_eq!(px(1500, r3), [255, 255, 255]);
        let mid = px(960, r3)[0];
        assert!((126..=129).contains(&mid), "{mid}");
        let mut last = 0;
        for x in 522..1398 {
            let v = px(x, r3)[0];
            assert!(v >= last, "the ramp only rises ({x})");
            last = v;
        }
        // Row 4: 15% grey sides, black 309, 100% white 411, black 171, then the
        // black-level strips of 69/68 (-2 0 +2 0 +4) and black 206.
        assert_eq!(px(100, r4), [38, 38, 38]);
        assert_eq!(px(1820, r4), [38, 38, 38]);
        assert_eq!(px(240 + 150, r4), [0, 0, 0]);
        assert_eq!(px(549 + 200, r4), [255, 255, 255]);
        assert_eq!(px(960 + 80, r4), [0, 0, 0]);
        let strips = 1131;
        assert_eq!(px(strips + 34, r4), BELOW_BLACK, "-2% shows as black");
        assert_eq!(px(strips + 103, r4), [0, 0, 0]);
        assert_eq!(px(strips + 171, r4), [5, 5, 5], "+2%");
        assert_eq!(px(strips + 240, r4), [0, 0, 0]);
        assert_eq!(px(strips + 308, r4), ABOVE_BLACK, "+4%");
        assert_eq!(px(1600, r4), [0, 0, 0]);
    }

    #[test]
    fn the_hd_bars_keep_4_3_and_their_coloured_ends_on_any_display() {
        // Where the bars start and end in row 1, and what is at the far left of
        // rows 1 to 3, for displays of every shape.
        for (w, h) in [
            (1920, 1200),
            (2560, 1080),
            (3840, 1080),
            (1024, 768),
            (540, 960),
        ] {
            let p = render(PatternKind::SmpteHd, w, h, 0);
            let px = |x: u32, y: u32| {
                let i = ((y * w + x) * 4) as usize;
                [p[i], p[i + 1], p[i + 2]]
            };
            let row = |y: u32| (0..w).map(|x| px(x, y)).collect::<Vec<_>>();
            let top = row(h / 4);
            let a = top.iter().position(|c| *c == [191, 191, 191]).unwrap() as u32;
            let b = top.iter().rposition(|c| *c == [0, 0, 191]).unwrap() as u32 + 1;
            let bars = (b - a) as f32 / h as f32;
            if w * 3 >= h * 4 {
                assert!((bars - 4.0 / 3.0).abs() < 0.01, "{w}x{h}: {bars}");
            } else {
                assert_eq!((a, b), (0, w), "{w}x{h}: narrower than 4:3 is all bars");
            }
            let cyan = row(h * 15 / 24)
                .iter()
                .filter(|c| **c == [0, 255, 255])
                .count() as f32;
            let reach = (16.0 / 9.0 - 4.0 / 3.0) / 2.0 * h as f32;
            let side = a as f32;
            // The cyan end is as wide as 16:9's side area, or what there is.
            assert!(
                (cyan - reach.min(side)).abs() <= 2.0,
                "{w}x{h}: cyan {cyan}"
            );
            if side > reach + 2.0 {
                assert_eq!(px(0, h * 15 / 24), [102, 102, 102], "{w}x{h}: grey beyond");
                assert_eq!(px(0, h * 17 / 24), [102, 102, 102], "{w}x{h}");
                assert_eq!(px(0, h - 2), [38, 38, 38], "{w}x{h}");
            }
        }
    }

    #[test]
    fn the_classic_smpte_bars_have_the_standard_layout() {
        let p = pic(PatternKind::Smpte, 1);
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
        let p = pic(PatternKind::Smpte, 0);
        for px in p.chunks(4) {
            let c = [px[0], px[1], px[2]];
            assert!(
                c.iter().all(|v| *v == 0) || c == [10, 10, 10] || c.iter().any(|v| *v > 30),
                "{c:?}"
            );
        }
    }

    #[test]
    fn the_smpte_bars_are_4_3_on_any_display_with_grey_beside_them() {
        // Where the first bar starts and the last ends, in a picture `w` wide of
        // the shape `shape` (drawn at that size, or stretched to it).
        let edges = |w: u32, h: u32, shape: f32| {
            let p = render_shaped(PatternKind::Smpte, w, h, 0, shape);
            let row: Vec<[u8; 3]> = (0..w)
                .map(|x| {
                    let i = ((5 * w + x) * 4) as usize;
                    [p[i], p[i + 1], p[i + 2]]
                })
                .collect();
            let first = row.iter().position(|c| *c == [191, 191, 191]).unwrap();
            let last = row.iter().rposition(|c| *c == [0, 0, 191]).unwrap();
            (first as u32, last as u32 + 1, row[0])
        };
        // 16:9, 16:10, 21:9, 32:9 and 4:3 displays: the bars' block is 4:3 of the
        // height it is seen at, and the rest of the width is grey (none at 4:3).
        for (w, h) in [
            (1920, 1080),
            (1920, 1200),
            (2560, 1080),
            (3840, 1080),
            (1024, 768),
        ] {
            let (a, b, side) = edges(w, h, w as f32 / h as f32);
            let block = (b - a) as f32 / h as f32;
            assert!((block - 4.0 / 3.0).abs() < 0.01, "{w}x{h}: {block}");
            assert!((a as i32 - (w - b) as i32).abs() <= 1, "{w}x{h} centred");
            let grey = a > 0;
            assert_eq!(grey, w * 3 > h * 4, "{w}x{h}");
            if grey {
                assert_eq!(side, [102, 102, 102], "{w}x{h}");
            }
        }
        // Narrower than 4:3 (a portrait sub display): bars across the whole width.
        let (a, b, _) = edges(540, 960, 540.0 / 960.0);
        assert_eq!((a, b), (0, 540));
        // Drawn small and stretched to the display: the shape it is seen at counts,
        // not the size it was drawn at (a 16:10 display drawn at a 16:9 size).
        let (a, b, _) = edges(1280, 720, 16.0 / 10.0);
        let seen = (b - a) as f32 / 1280.0 * 16.0 / 10.0;
        assert!((seen - 4.0 / 3.0).abs() < 0.01, "{seen}");
        // Nothing divides by 0 or panics on a nonsense shape.
        for shape in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(
                render_shaped(PatternKind::Smpte, 20, 10, 1, shape).len(),
                800
            );
        }
        // The number is drawn over them, as on the other patterns.
        assert_ne!(
            render(PatternKind::Smpte, 192, 108, 1),
            render(PatternKind::Smpte, 192, 108, 8)
        );
    }

    #[test]
    fn number_0_draws_no_number_and_leaves_the_pattern_whole() {
        for k in [
            PatternKind::Smpte,
            PatternKind::Bars,
            PatternKind::Grid,
            PatternKind::Circles,
            PatternKind::Frame,
        ] {
            assert_ne!(pic(k, 0), pic(k, 1), "{k:?} shows a number when asked");
        }
        // Where the number goes, the SMPTE bars show their own green bar.
        assert_eq!(at(&pic(PatternKind::Smpte, 0), 96, 40), [0, 191, 0]);
        assert_ne!(at(&pic(PatternKind::Smpte, 1), 96, 40), [0, 191, 0]);
        // Tiny pictures with no number don't panic either.
        assert_eq!(render(PatternKind::Smpte, 2, 2, 0).len(), 16);
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
            PatternKind::SmpteHd,
            PatternKind::Smpte,
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
