//! Drawing a text cue: the words laid out, wrapped and rasterized into an
//! RGBA picture the size of the output, which then plays like any image.

use crate::media::video::RgbaImage;
use crate::show::{TextAlign, TextCue, TextVAlign};
use ab_glyph::{Font, FontArc, PxScale, ScaleFont, point};
use std::sync::OnceLock;

/// Gap kept between text and the edge of the output when it sits at the top or bottom.
const MARGIN: f32 = 0.04;
/// Line spacing as a multiple of the font's own line height.
const LINE_SPACING: f32 = 1.05;
const SHADOW_ALPHA: f32 = 0.75;

/// The fonts egui ships with, in the order it prefers them, so a character
/// missing from one (a symbol, say) is looked for in the next.
fn fonts() -> &'static [FontArc] {
    static FONTS: OnceLock<Vec<FontArc>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let defs = egui::FontDefinitions::default();
        let names = defs
            .families
            .get(&egui::FontFamily::Proportional)
            .cloned()
            .unwrap_or_default();
        names
            .iter()
            .filter_map(|n| defs.font_data.get(n))
            .filter_map(|d| FontArc::try_from_vec(d.font.to_vec()).ok())
            .collect()
    })
}

struct Glyph {
    font: usize,
    ch: char,
    /// Distance from the start of the line.
    x: f32,
}

struct Line {
    glyphs: Vec<Glyph>,
    width: f32,
}

/// Which font draws `ch`: the first that has it, else the first.
fn font_for(fonts: &[FontArc], ch: char) -> usize {
    fonts
        .iter()
        .position(|f| f.glyph_id(ch).0 != 0)
        .unwrap_or(0)
}

/// Lay `text` out in lines no wider than `max_width` pixels. Words longer
/// than a line are broken where they overflow.
fn layout(fonts: &[FontArc], text: &str, scale: PxScale, max_width: f32) -> Vec<Line> {
    let advance = |ch: char, prev: Option<(usize, char)>| -> (usize, f32) {
        let fi = font_for(fonts, ch);
        let sf = fonts[fi].as_scaled(scale);
        let mut a = sf.h_advance(sf.glyph_id(ch));
        if let Some((pf, pc)) = prev
            && pf == fi
        {
            a += sf.kern(sf.glyph_id(pc), sf.glyph_id(ch));
        }
        (fi, a)
    };
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = Line {
            glyphs: Vec::new(),
            width: 0.0,
        };
        let mut prev: Option<(usize, char)> = None;
        let flush = |line: &mut Line, lines: &mut Vec<Line>| {
            // Trailing spaces would push a centred line off-centre.
            while line.glyphs.last().is_some_and(|g| g.ch == ' ') {
                let g = line.glyphs.pop().expect("checked");
                let (_, a) = advance(g.ch, None);
                line.width -= a;
            }
            lines.push(std::mem::replace(
                line,
                Line {
                    glyphs: Vec::new(),
                    width: 0.0,
                },
            ));
        };
        let words: Vec<&str> = paragraph.split(' ').collect();
        for (wi, word) in words.iter().enumerate() {
            let sep = if wi + 1 < words.len() { " " } else { "" };
            let token: Vec<char> = word.chars().chain(sep.chars()).collect();
            // Width of the word on its own, to decide whether it fits here.
            let mut w = 0.0;
            let mut p = None;
            for &ch in &token {
                let (fi, a) = advance(ch, p);
                w += a;
                p = Some((fi, ch));
            }
            let word_w = w - if sep.is_empty() {
                0.0
            } else {
                advance(' ', None).1
            };
            if !line.glyphs.is_empty() && line.width + word_w > max_width {
                flush(&mut line, &mut lines);
                prev = None;
            }
            for &ch in &token {
                let (fi, a) = advance(ch, prev);
                // A word wider than a whole line breaks wherever it runs out.
                if line.width + a > max_width && !line.glyphs.is_empty() && ch != ' ' {
                    flush(&mut line, &mut lines);
                }
                line.glyphs.push(Glyph {
                    font: fi,
                    ch,
                    x: line.width,
                });
                line.width += a;
                prev = Some((fi, ch));
            }
        }
        flush(&mut line, &mut lines);
    }
    lines
}

/// Blend a colour of coverage `a` over a straight-alpha RGBA pixel.
fn blend(dst: &mut [u8], rgb: [u8; 3], a: f32) {
    let a = a.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let da = f32::from(dst[3]) / 255.0;
    let out = a + da * (1.0 - a);
    for i in 0..3 {
        let c = (f32::from(rgb[i]) * a + f32::from(dst[i]) * da * (1.0 - a)) / out;
        dst[i] = c.round().clamp(0.0, 255.0) as u8;
    }
    dst[3] = (out * 255.0).round().clamp(0.0, 255.0) as u8;
}

/// Draw one line of glyphs with its baseline at `y`, starting at `x`.
fn draw_line(
    img: &mut RgbaImage,
    fonts: &[FontArc],
    line: &Line,
    scale: PxScale,
    (x, y): (f32, f32),
    rgb: [u8; 3],
    alpha: f32,
) {
    let (w, h) = (img.width as i32, img.height as i32);
    for g in &line.glyphs {
        let font = &fonts[g.font];
        let glyph = font
            .glyph_id(g.ch)
            .with_scale_and_position(scale, point(x + g.x, y));
        let Some(outlined) = font.outline_glyph(glyph) else {
            continue;
        };
        let bounds = outlined.px_bounds();
        outlined.draw(|gx, gy, c| {
            let px = bounds.min.x as i32 + gx as i32;
            let py = bounds.min.y as i32 + gy as i32;
            if px < 0 || py < 0 || px >= w || py >= h {
                return;
            }
            let i = ((py * w + px) * 4) as usize;
            blend(&mut img.data[i..i + 4], rgb, c * alpha);
        });
    }
}

/// Draw a text cue on a transparent picture of `size` (the output's size).
pub fn render(cue: &TextCue, size: (u32, u32)) -> RgbaImage {
    let (w, h) = (size.0.max(2), size.1.max(2));
    let mut img = RgbaImage {
        width: w,
        height: h,
        data: vec![0; (w * h * 4) as usize],
    };
    let fonts = fonts();
    if fonts.is_empty() || cue.text.trim().is_empty() {
        return img;
    }
    let px = (cue.size.clamp(0.005, 1.0) * h as f32).max(4.0);
    let scale = PxScale::from(px);
    let region_w = (cue.width.clamp(0.05, 1.0) * w as f32).max(px);
    let lines = layout(fonts, &cue.text, scale, region_w);

    let sf = fonts[0].as_scaled(scale);
    let line_h = (sf.ascent() - sf.descent() + sf.line_gap()) * LINE_SPACING;
    let block_h = line_h * lines.len() as f32;
    let margin = MARGIN * h as f32;
    let top = match cue.valign {
        TextVAlign::Top => margin,
        TextVAlign::Middle => (h as f32 - block_h) / 2.0,
        TextVAlign::Bottom => h as f32 - margin - block_h,
    };
    let left = (w as f32 - region_w) / 2.0;
    let color = [cue.color[0], cue.color[1], cue.color[2]];
    let alpha = f32::from(cue.color[3]) / 255.0;

    let place = |i: usize, line: &Line| {
        let x = match cue.align {
            TextAlign::Left => left,
            TextAlign::Center => left + (region_w - line.width) / 2.0,
            TextAlign::Right => left + region_w - line.width,
        };
        (x, top + i as f32 * line_h + sf.ascent())
    };
    // Faux bold: the same letters again, nudged a fraction of a pixel.
    let e = (px * 0.03).max(0.7);
    let nudges: &[(f32, f32)] = if cue.bold {
        &[(0.0, 0.0), (e, 0.0), (0.0, e), (e, e)]
    } else {
        &[(0.0, 0.0)]
    };
    if cue.shadow {
        let d = (px * 0.05).max(1.0);
        for (i, line) in lines.iter().enumerate() {
            let (x, y) = place(i, line);
            for (nx, ny) in nudges {
                let at = (x + d + nx, y + d + ny);
                draw_line(
                    &mut img,
                    fonts,
                    line,
                    scale,
                    at,
                    [0, 0, 0],
                    SHADOW_ALPHA * alpha,
                );
            }
        }
    }
    for (i, line) in lines.iter().enumerate() {
        let (x, y) = place(i, line);
        for (nx, ny) in nudges {
            draw_line(&mut img, fonts, line, scale, (x + nx, y + ny), color, alpha);
        }
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (u32, u32) = (640, 360);

    fn cue(text: &str) -> TextCue {
        let mut c = TextCue::new(None);
        c.text = text.into();
        c.shadow = false;
        c
    }

    /// Bounding box (x0, y0, x1, y1) of the pixels that aren't transparent.
    fn ink(img: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
        let mut b: Option<(u32, u32, u32, u32)> = None;
        for y in 0..img.height {
            for x in 0..img.width {
                if img.data[((y * img.width + x) * 4 + 3) as usize] > 8 {
                    b = Some(match b {
                        None => (x, y, x, y),
                        Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
                    });
                }
            }
        }
        b
    }

    #[test]
    fn draws_text_on_a_transparent_picture() {
        let img = render(&cue("Hello"), SIZE);
        assert_eq!((img.width, img.height), SIZE);
        assert_eq!(img.data.len(), (SIZE.0 * SIZE.1 * 4) as usize);
        let (x0, y0, x1, y1) = ink(&img).expect("something was drawn");
        assert!(x1 > x0 + 40 && y1 > y0 + 10, "{x0},{y0} {x1},{y1}");
        // Corners stay empty.
        assert_eq!(img.data[3], 0);
    }

    #[test]
    fn empty_text_draws_nothing() {
        assert!(ink(&render(&cue(""), SIZE)).is_none());
        assert!(ink(&render(&cue("  \n "), SIZE)).is_none());
    }

    #[test]
    fn alignment_places_the_text() {
        let mut c = cue("Hello");
        c.align = TextAlign::Center;
        c.valign = TextVAlign::Bottom;
        let (x0, _, x1, y1) = ink(&render(&c, SIZE)).unwrap();
        assert!(((x0 + x1) as f32 / 2.0 - 320.0).abs() < 8.0, "{x0}..{x1}");
        assert!(y1 > 360 * 3 / 4, "bottom aligned, ended at {y1}");

        c.valign = TextVAlign::Top;
        c.align = TextAlign::Left;
        let (x0, y0, _, _) = ink(&render(&c, SIZE)).unwrap();
        assert!(y0 < 360 / 6 && x0 < 640 / 10, "{x0},{y0}");

        c.valign = TextVAlign::Middle;
        c.align = TextAlign::Right;
        let (_, y0, x1, y1) = ink(&render(&c, SIZE)).unwrap();
        assert!(x1 > 640 * 8 / 10, "right aligned, ended at {x1}");
        assert!(((y0 + y1) as f32 / 2.0 - 180.0).abs() < 20.0, "{y0}..{y1}");
    }

    #[test]
    fn long_text_wraps_and_newlines_break() {
        let one = ink(&render(&cue("one two three four five six"), SIZE)).unwrap();
        let mut narrow = cue("one two three four five six");
        narrow.width = 0.3;
        let wrapped = ink(&render(&narrow, SIZE)).unwrap();
        assert!(wrapped.3 - wrapped.1 > (one.3 - one.1) * 3 / 2, "no wrap");
        assert!(
            wrapped.2 - wrapped.0 <= (640.0 * 0.3) as u32 + 2,
            "too wide"
        );

        let two = ink(&render(&cue("a\nb"), SIZE)).unwrap();
        let single = ink(&render(&cue("a b"), SIZE)).unwrap();
        assert!(two.3 - two.1 > single.3 - single.1);
    }

    #[test]
    fn a_word_wider_than_the_line_still_fits() {
        let mut c = cue("Supercalifragilisticexpialidocious");
        c.width = 0.2;
        let (x0, _, x1, _) = ink(&render(&c, SIZE)).unwrap();
        assert!(x1 - x0 <= (640.0 * 0.2) as u32 + 2, "{x0}..{x1}");
    }

    #[test]
    fn colour_and_shadow() {
        let mut c = cue("MMMM");
        c.color = [255, 0, 0, 255];
        let img = render(&c, SIZE);
        let solid: Vec<&[u8]> = img.data.chunks(4).filter(|p| p[3] == 255).collect();
        assert!(!solid.is_empty());
        assert!(solid.iter().all(|p| p[0] > 240 && p[1] < 15 && p[2] < 15));

        // A shadow adds dark pixels around the same letters.
        c.shadow = true;
        let with = render(&c, SIZE);
        let dark = |i: &RgbaImage| i.data.chunks(4).filter(|p| p[3] > 100 && p[0] < 60).count();
        assert_eq!(dark(&img), 0);
        assert!(dark(&with) > 20);
        // Transparent text colour draws nothing visible.
        c.color[3] = 0;
        c.shadow = false;
        assert!(ink(&render(&c, SIZE)).is_none());
    }

    #[test]
    fn bold_thickens_the_letters() {
        let mut c = cue("Hello");
        let weight = |c: &TextCue| -> u32 {
            render(c, SIZE)
                .data
                .chunks(4)
                .map(|p| u32::from(p[3]))
                .sum()
        };
        let light = weight(&c);
        c.bold = true;
        assert!(
            weight(&c) as f32 > light as f32 * 1.15,
            "bold is not heavier"
        );
    }

    #[test]
    fn size_scales_with_the_output() {
        let c = cue("Hello");
        let small = ink(&render(&c, (320, 180))).unwrap();
        let big = ink(&render(&c, (1280, 720))).unwrap();
        let (sw, bw) = (small.2 - small.0, big.2 - big.0);
        assert!((bw as f32 / sw as f32 - 4.0).abs() < 0.3, "{sw} vs {bw}");
    }
}
