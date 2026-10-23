//! Video decoding via an ffmpeg subprocess that writes raw YUV 4:2:0 frames to a pipe.

use super::ffmpeg::{ColorMatrix, Ffmpeg, VideoInfo};
use super::playback::{Children, PassFeed, Playback};
use anyhow::Result;
use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use parking_lot::Mutex;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct VideoFrame {
    /// Presentation time in seconds, relative to the cue's in point.
    pub pts: f64,
    /// Planar Y, U, V (yuv420p).
    pub data: Vec<u8>,
}

/// Frames buffered ahead of presentation. Each 1080p frame is ~3 MB.
const QUEUE_FRAMES: usize = 4;

pub struct VideoPlayer {
    pub width: u32,
    pub height: u32,
    pub matrix: ColorMatrix,
    rx: Receiver<VideoFrame>,
    recycle: Sender<Vec<u8>>,
    children: Children,
    eof: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    current: Option<VideoFrame>,
    pending: Option<VideoFrame>,
    /// Increments every time `current` changes, so renderers know when to re-upload.
    pub serial: u64,
}

/// Compute an even-sized decode resolution no larger than the source, fitted
/// to `max` (so a 4K file playing on a 1080p output decodes at 1080p).
pub fn decode_size(src: (u32, u32), max: (u32, u32), cover: bool) -> (u32, u32) {
    let (w, h) = (src.0.max(2) as f64, src.1.max(2) as f64);
    let sx = max.0.max(2) as f64 / w;
    let sy = max.1.max(2) as f64 / h;
    let s = if cover { sx.max(sy) } else { sx.min(sy) }.min(1.0);
    let even = |v: f64| ((v.round() as u32) / 2 * 2).max(2);
    (even(w * s), even(h * s))
}

impl VideoPlayer {
    /// Start decoding `path` from `start`; `playback` says whether to play
    /// once or loop a slice.
    pub fn spawn(
        ff: &Ffmpeg,
        path: &Path,
        info: &VideoInfo,
        start: f64,
        playback: Playback,
        size: (u32, u32),
    ) -> Result<VideoPlayer> {
        let (w, h) = size;
        let fps = info.fps;
        let make = {
            let (ff, path) = (ff.clone(), path.to_path_buf());
            move || {
                let mut cmd = ff.command();
                cmd.args(playback.input_args(start));
                cmd.arg("-i").arg(&path);
                cmd.args(playback.limit_args());
                cmd.args([
                    "-map",
                    "0:v:0",
                    "-an",
                    "-sn",
                    "-dn",
                    "-vf",
                    &format!("scale={w}:{h}:flags=bicubic:out_range=tv,format=yuv420p"),
                    "-fps_mode",
                    "cfr",
                    "-r",
                    &format!("{fps:.6}"),
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "yuv420p",
                    "pipe:1",
                ]);
                cmd
            }
        };
        let children = Children::new();
        let error = Arc::new(Mutex::new(None));
        let mut feed = PassFeed::start(&children, playback.passes(), &error, make)?;
        // Looping passes are cut to an exact frame count so the timeline
        // stays locked to the audio's, whatever the decoder delivers.
        let per_pass = playback
            .pass_len()
            .map(|l| (l * fps).round().max(1.0) as u64);

        let frame_len = (w * h + 2 * (w / 2) * (h / 2)) as usize;
        let (tx, rx) = bounded::<VideoFrame>(QUEUE_FRAMES);
        let (recycle, recycle_rx) = unbounded::<Vec<u8>>();
        let eof = Arc::new(AtomicBool::new(false));

        let eof2 = eof.clone();
        std::thread::Builder::new()
            .name("video-reader".into())
            .spawn(move || {
                let mut base: u64 = 0; // index of the current pass's first frame
                loop {
                    let mut n: u64 = 0;
                    while per_pass.is_none_or(|p| n < p) {
                        if per_pass.is_some_and(|p| n * 2 >= p) {
                            feed.prefetch();
                        }
                        let mut buf = recycle_rx
                            .try_recv()
                            .unwrap_or_else(|_| vec![0u8; frame_len]);
                        buf.resize(frame_len, 0);
                        if feed.stdout().read_exact(&mut buf).is_err() {
                            break;
                        }
                        let frame = VideoFrame {
                            pts: (base + n) as f64 / fps,
                            data: buf,
                        };
                        if tx.send(frame).is_err() {
                            return; // player dropped
                        }
                        n += 1;
                    }
                    // A pass that decodes nothing is an error, not a reason to retry forever.
                    if n == 0 || !feed.advance() {
                        break;
                    }
                    base += per_pass.unwrap_or(n);
                }
                eof2.store(true, Ordering::Release);
            })?;

        Ok(VideoPlayer {
            width: w,
            height: h,
            matrix: info.matrix,
            rx,
            recycle,
            children,
            eof,
            error,
            current: None,
            pending: None,
            serial: 0,
        })
    }

    fn fill_pending(&mut self) {
        if self.pending.is_none() {
            self.pending = self.rx.try_recv().ok();
        }
    }

    /// True once the first frame has been decoded.
    pub fn ready(&mut self) -> bool {
        self.fill_pending();
        self.current.is_some() || self.pending.is_some()
    }

    /// Advance to the frame that should be on screen at media time `t`.
    pub fn update(&mut self, t: f64) {
        loop {
            self.fill_pending();
            let due = match &self.pending {
                Some(f) => f.pts <= t || self.current.is_none(),
                None => false,
            };
            if !due {
                break;
            }
            let next = self.pending.take();
            if let Some(old) = std::mem::replace(&mut self.current, next) {
                let _ = self.recycle.send(old.data);
            }
            self.serial += 1;
        }
    }

    pub fn current(&self) -> Option<&VideoFrame> {
        self.current.as_ref()
    }

    /// All frames have been decoded and presented.
    pub fn finished(&mut self) -> bool {
        self.fill_pending();
        self.eof.load(Ordering::Acquire) && self.pending.is_none() && self.rx.is_empty()
    }

    pub fn error(&self) -> Option<String> {
        self.error.lock().clone()
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.children.close();
    }
}

/// Keep ffmpeg's stderr pipe from filling up, remembering the last error line.
pub fn spawn_stderr_drain(
    stderr: Option<std::process::ChildStderr>,
    error: Arc<Mutex<Option<String>>>,
) {
    let Some(mut stderr) = stderr else { return };
    let _ = std::thread::Builder::new()
        .name("ffmpeg-stderr".into())
        .spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            if let Some(line) = text.lines().rev().find(|l| !l.trim().is_empty()) {
                log::warn!("ffmpeg: {line}");
                *error.lock() = Some(line.to_string());
            }
        });
}

pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

/// Decode a still image to RGBA on a background thread.
pub fn load_image(
    ff: &Ffmpeg,
    path: &Path,
    size: (u32, u32),
) -> Receiver<Result<RgbaImage, String>> {
    let (tx, rx) = bounded(1);
    let mut cmd = ff.command();
    cmd.arg("-i").arg(path).args([
        "-frames:v",
        "1",
        "-vf",
        &format!("scale={}:{}:flags=bicubic", size.0, size.1),
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgba",
        "pipe:1",
    ]);
    let _ = std::thread::Builder::new()
        .name("image-loader".into())
        .spawn(move || {
            let result = (|| {
                let out = cmd.output().map_err(|e| format!("starting ffmpeg: {e}"))?;
                let expected = (size.0 * size.1 * 4) as usize;
                if out.stdout.len() < expected {
                    return Err(format!(
                        "image decode failed: {}",
                        String::from_utf8_lossy(&out.stderr).trim()
                    ));
                }
                let mut data = out.stdout;
                data.truncate(expected);
                Ok(RgbaImage {
                    width: size.0,
                    height: size.1,
                    data,
                })
            })();
            let _ = tx.send(result);
        });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_size_only_downscales() {
        assert_eq!(decode_size((1280, 720), (1920, 1080), false), (1280, 720));
        assert_eq!(decode_size((3840, 2160), (1920, 1080), false), (1920, 1080));
        assert_eq!(decode_size((1920, 1080), (1080, 1920), false), (1080, 608));
        assert_eq!(decode_size((1920, 1080), (1080, 1920), true), (1920, 1080));
        assert_eq!(decode_size((641, 361), (4000, 4000), false), (640, 360));
    }
}
