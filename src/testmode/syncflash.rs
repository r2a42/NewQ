//! The network test's "Sync flash": a clip a person can film. Every frame
//! carries its number in large digits, a bar that steps one slot a frame along
//! a row of sixty, and the first frame of each second is white all over. Played
//! on every display of every computer at the same shared time, a phone's
//! slow-motion video across two projectors shows how many frames apart they
//! are: read the two numbers in one shot of the video, or compare where the
//! two bars are, and the flashes should land in the same shot.
//!
//! The frames are drawn here (`frame`, pure and tested) and encoded by our
//! FFmpeg on the thread that makes the test's media; nothing depends on
//! FFmpeg's text drawing, which not every build has.

use crate::media::ffmpeg::Ffmpeg;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub const WIDTH: u32 = 1280;
pub const HEIGHT: u32 = 720;
pub const FPS: u32 = 60;
/// Seconds of clip (it loops).
pub const SECONDS: u32 = 10;
/// Its name in the test media folder (the version in the name: a change to the
/// picture makes it again).
pub const FILE: &str = "generated_sync_flash_v1_60fps.mp4";

const DARK: [u8; 3] = [20, 20, 24];
const LIGHT: [u8; 3] = [235, 235, 235];
const DIM: [u8; 3] = [70, 70, 78];
const MARK: [u8; 3] = [255, 200, 40];

/// Which of the seven segments (a top, b top right, c bottom right, d bottom,
/// e bottom left, f top left, g middle) each digit lights.
const SEGMENTS: [[bool; 7]; 10] = [
    [true, true, true, true, true, true, false],
    [false, true, true, false, false, false, false],
    [true, true, false, true, true, false, true],
    [true, true, true, true, false, false, true],
    [false, true, true, false, false, true, true],
    [true, false, true, true, false, true, true],
    [true, false, true, true, true, true, true],
    [true, true, true, false, false, false, false],
    [true, true, true, true, true, true, true],
    [true, true, true, true, false, true, true],
];

struct Canvas<'a> {
    px: &'a mut [u8],
    w: u32,
    h: u32,
}

impl Canvas<'_> {
    fn rect(&mut self, x: i64, y: i64, w: i64, h: i64, c: [u8; 3]) {
        let x0 = x.clamp(0, i64::from(self.w)) as usize;
        let x1 = (x + w).clamp(0, i64::from(self.w)) as usize;
        let y0 = y.clamp(0, i64::from(self.h)) as usize;
        let y1 = (y + h).clamp(0, i64::from(self.h)) as usize;
        for row in y0..y1 {
            let start = (row * self.w as usize + x0) * 3;
            let end = (row * self.w as usize + x1) * 3;
            for p in self.px[start..end].as_chunks_mut::<3>().0 {
                p.copy_from_slice(&c);
            }
        }
    }

    /// A digit `h` high with its top left at (x, y).
    fn digit(&mut self, d: usize, x: i64, y: i64, h: i64, c: [u8; 3]) {
        let w = h * 11 / 20;
        let t = (h / 9).max(2);
        let half = h / 2;
        let on = SEGMENTS[d % 10];
        let segs: [(i64, i64, i64, i64); 7] = [
            (x + t, y, w - 2 * t, t),
            (x + w - t, y + t, t, half - t - t / 2),
            (x + w - t, y + half + t / 2, t, half - t - t / 2),
            (x + t, y + h - t, w - 2 * t, t),
            (x, y + half + t / 2, t, half - t - t / 2),
            (x, y + t, t, half - t - t / 2),
            (x + t, y + half - t / 2, w - 2 * t, t),
        ];
        for (i, s) in segs.iter().enumerate() {
            if on[i] {
                self.rect(s.0, s.1, s.2, s.3, c);
            }
        }
    }
}

/// Frame `n` of the clip, `w`×`h` RGB.
pub fn frame(n: u32, w: u32, h: u32) -> Vec<u8> {
    let mut px = vec![0u8; (w * h * 3) as usize];
    let flash = n.is_multiple_of(FPS);
    let (bg, fg) = if flash { (LIGHT, DARK) } else { (DARK, LIGHT) };
    let mut c = Canvas { px: &mut px, w, h };
    c.rect(0, 0, i64::from(w), i64::from(h), bg);
    let (wi, hi) = (i64::from(w), i64::from(h));
    // The number: three digits, as large as fits above the bar.
    let dh = hi * 11 / 20;
    let dw = dh * 11 / 20;
    let gap = dh / 6;
    let total = 3 * dw + 2 * gap;
    let x0 = (wi - total) / 2;
    let y0 = hi / 12;
    let digits = [(n / 100) % 10, (n / 10) % 10, n % 10];
    for (i, d) in digits.iter().enumerate() {
        c.digit(*d as usize, x0 + i as i64 * (dw + gap), y0, dh, fg);
    }
    // The bar: sixty slots, one a frame, the tens marked longer.
    let slot = n % FPS;
    let margin = wi / 20;
    let row_w = wi - 2 * margin;
    let sw = row_w / i64::from(FPS);
    let by = hi * 3 / 4;
    let bh = hi / 10;
    for i in 0..i64::from(FPS) {
        let x = margin + i * sw;
        let tick = if i % 10 == 0 { bh / 2 } else { bh / 5 };
        c.rect(
            x,
            by + bh + 4,
            (sw / 6).max(1),
            tick,
            if flash { DARK } else { DIM },
        );
    }
    c.rect(
        margin + i64::from(slot) * sw,
        by,
        sw,
        bh,
        if flash { DARK } else { MARK },
    );
    px
}

/// Make the clip in `dir` (or find it made), with `ff`. Slow (a few seconds):
/// on a thread.
pub fn make(ff: &Ffmpeg, dir: &Path, cancel: &AtomicBool) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let out = dir.join(FILE);
    if out.is_file() && super::assets::check(ff, &out, super::assets::Kind::Video).is_ok() {
        return Ok(out);
    }
    let tmp = dir.join(format!("partial_{FILE}"));
    let size = format!("{WIDTH}x{HEIGHT}");
    let rate = FPS.to_string();
    let mut cmd = ff.command();
    cmd.args([
        "-y", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", &size, "-r", &rate,
    ])
    .args(["-i", "pipe:0"])
    .args(crate::soak::h264_args(|n| ff.has_encoder(n)))
    .args(["-g", &rate, "-movflags", "+faststart"])
    .arg(&tmp)
    .stdin(std::process::Stdio::piped())
    .stdout(std::process::Stdio::null());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("FFmpeg didn't start: {e}"))?;
    crate::reaper::watch(&child);
    let mut stdin = child.stdin.take().ok_or("no pipe to FFmpeg")?;
    let mut failed = None;
    for n in 0..FPS * SECONDS {
        if cancel.load(Ordering::Relaxed) {
            failed = Some("cancelled".to_string());
            break;
        }
        if let Err(e) = stdin.write_all(&frame(n, WIDTH, HEIGHT)) {
            failed = Some(format!("FFmpeg stopped taking frames: {e}"));
            break;
        }
    }
    drop(stdin);
    let result = child.wait_with_output().map_err(|e| e.to_string())?;
    if let Some(why) = failed {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!(
            "{why} {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    if !result.status.success() {
        let _ = std::fs::remove_file(&tmp);
        return Err(String::from_utf8_lossy(&result.stderr).trim().to_string());
    }
    std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(px: &[u8], w: u32, x: u32, y: u32) -> [u8; 3] {
        let i = ((y * w + x) * 3) as usize;
        [px[i], px[i + 1], px[i + 2]]
    }

    fn light_share(px: &[u8]) -> f64 {
        let n = px.len() / 3;
        px.as_chunks::<3>().0.iter().filter(|p| p[0] > 128).count() as f64 / n as f64
    }

    #[test]
    fn the_first_frame_of_each_second_flashes_white_and_the_others_are_dark() {
        let (w, h) = (320, 180);
        assert!(light_share(&frame(0, w, h)) > 0.6);
        assert!(light_share(&frame(60, w, h)) > 0.6);
        assert!(light_share(&frame(1, w, h)) < 0.3);
        assert!(light_share(&frame(59, w, h)) < 0.3);
    }

    #[test]
    fn the_bar_steps_one_slot_a_frame_along_sixty() {
        let (w, h) = (1280, 720);
        let margin = w / 20;
        let sw = (w - 2 * margin) / FPS;
        let y = h * 3 / 4 + 5;
        for n in [1u32, 7, 30, 59, 61] {
            let f = frame(n, w, h);
            let slot = n % FPS;
            let x = margin + slot * sw + sw / 2;
            assert_eq!(at(&f, w, x, y), MARK, "frame {n}");
            let other = margin + ((slot + 30) % FPS) * sw + sw / 2;
            assert_eq!(at(&f, w, other, y), DARK, "frame {n}");
        }
    }

    #[test]
    fn the_number_is_drawn_in_seven_segments() {
        // 8 lights every segment, 1 only the right two: 888 is the brightest.
        let lit = |n| {
            frame(n, 640, 360)
                .as_chunks::<3>()
                .0
                .iter()
                .filter(|p| p[0] > 200 && p[2] > 200)
                .count()
        };
        assert!(lit(888) > lit(111) * 2);
        assert!(lit(108) > lit(101));
        assert_ne!(frame(123, 64, 36), frame(124, 64, 36));
    }
}
