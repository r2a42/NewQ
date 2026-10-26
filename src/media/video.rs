//! Video decoding via an ffmpeg subprocess that writes raw YUV 4:2:0 frames to a pipe.

use super::ffmpeg::{ColorMatrix, Ffmpeg, VideoInfo};
use super::playback::{Children, PassFeed, Playback};
use anyhow::Result;
use crossbeam_channel::{Receiver, Sender, TrySendError, bounded, unbounded};
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

/// Marks the reader thread as ended however it ends.
struct ReaderDone(Arc<AtomicBool>);

impl Drop for ReaderDone {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

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
    /// A camera: show the newest frame, whatever its time stamp says.
    live: bool,
    /// Set by the reader thread as it ends (only tests look).
    #[cfg_attr(not(test), allow(dead_code))]
    reader_done: Arc<AtomicBool>,
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
        // Looping passes are cut to an exact frame count so the timeline
        // stays locked to the audio's, whatever the decoder delivers.
        let per_pass = playback
            .pass_len()
            .map(|l| (l * fps).round().max(1.0) as u64);
        Self::start(
            make,
            playback.passes(),
            per_pass,
            fps,
            info.matrix,
            size,
            false,
        )
    }

    /// Open a camera with the given ffmpeg input options (see
    /// `camera::input_args`), delivering frames of `size`. Nothing waits for
    /// a slow viewer: the oldest frame is dropped to make room for a new one.
    pub fn spawn_live(
        ff: &Ffmpeg,
        input: Vec<String>,
        matrix: ColorMatrix,
        fps: u32,
        size: (u32, u32),
    ) -> Result<VideoPlayer> {
        anyhow::ensure!(!input.is_empty(), "no camera chosen");
        let (w, h) = size;
        let make = {
            let ff = ff.clone();
            move || {
                let mut cmd = ff.command();
                cmd.args(["-fflags", "nobuffer", "-flags", "low_delay"]);
                cmd.args(&input);
                cmd.args([
                    "-map",
                    "0:v:0",
                    "-an",
                    "-sn",
                    "-dn",
                    "-vf",
                    &format!("scale={w}:{h}:flags=bicubic:out_range=tv,format=yuv420p"),
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    "yuv420p",
                    "pipe:1",
                ]);
                cmd
            }
        };
        Self::start(
            make,
            Some(1),
            None,
            f64::from(fps.max(1)),
            matrix,
            size,
            true,
        )
    }

    fn start(
        make: impl Fn() -> std::process::Command + Send + 'static,
        passes: Option<u32>,
        per_pass: Option<u64>,
        fps: f64,
        matrix: ColorMatrix,
        size: (u32, u32),
        live: bool,
    ) -> Result<VideoPlayer> {
        let (w, h) = size;
        let children = Children::new();
        let error = Arc::new(Mutex::new(None));
        let mut feed = PassFeed::start(&children, passes, &error, make)?;

        let frame_len = (w * h + 2 * (w / 2) * (h / 2)) as usize;
        let (tx, rx) = bounded::<VideoFrame>(QUEUE_FRAMES);
        let (recycle, recycle_rx) = unbounded::<Vec<u8>>();
        let eof = Arc::new(AtomicBool::new(false));
        // Only a camera's reader takes frames back out of the queue (to drop
        // the oldest). A file's reader must not hold a receiver: it would keep
        // the channel open, and its blocked `send` could never fail once the
        // player is gone, so the thread and its frames would leak.
        let rx_reader = live.then(|| rx.clone());
        let recycle_back = recycle.clone();

        let eof2 = eof.clone();
        let reader_done = Arc::new(AtomicBool::new(false));
        let reader_done2 = reader_done.clone();
        std::thread::Builder::new()
            .name("video-reader".into())
            .spawn(move || {
                let _done = ReaderDone(reader_done2);
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
                        if live {
                            // Newest wins: make room by dropping the oldest frame.
                            let mut frame = frame;
                            loop {
                                match tx.try_send(frame) {
                                    Ok(()) => break,
                                    Err(TrySendError::Full(f)) => {
                                        frame = f;
                                        if let Some(old) =
                                            rx_reader.as_ref().and_then(|r| r.try_recv().ok())
                                        {
                                            let _ = recycle_back.send(old.data);
                                        }
                                    }
                                    Err(TrySendError::Disconnected(_)) => return,
                                }
                            }
                        } else if tx.send(frame).is_err() {
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
            matrix,
            rx,
            recycle,
            children,
            eof,
            error,
            current: None,
            pending: None,
            serial: 0,
            live,
            reader_done,
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
                Some(f) => self.live || f.pts <= t || self.current.is_none(),
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

    /// A camera, which has no end of its own.
    pub fn is_live(&self) -> bool {
        self.live
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

    /// Whether the reader thread has ended (for tests of leaks).
    #[cfg(test)]
    pub fn reader_done(&self) -> Arc<AtomicBool> {
        self.reader_done.clone()
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
            // Only the tail matters (it holds the error), and a chatty ffmpeg
            // can write a lot over a long show.
            let text = super::read_tail(&mut stderr, 8192);
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

    /// Wait up to `secs` for every flag to be set.
    fn all_set(flags: &[Arc<AtomicBool>], secs: u64) -> bool {
        let t0 = std::time::Instant::now();
        while t0.elapsed().as_secs() < secs {
            if flags.iter().all(|f| f.load(Ordering::Acquire)) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn dropping_a_player_ends_its_reader_thread() {
        let Some((ff, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let info = ff.probe(&clip).expect("probe");
        let vi = info.video.as_ref().expect("a video stream");
        let mut flags = Vec::new();
        for _ in 0..3 {
            let mut p = VideoPlayer::spawn(
                &ff,
                &clip,
                vi,
                0.0,
                Playback::Once { duration: None },
                (320, 180),
            )
            .expect("spawn");
            // Let the decoder get ahead, so the frame queue is full and the
            // reader is blocked on it.
            let t0 = std::time::Instant::now();
            while !p.ready() {
                assert!(t0.elapsed().as_secs() < 10, "no first frame");
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            std::thread::sleep(std::time::Duration::from_millis(400));
            flags.push(p.reader_done());
            drop(p);
        }
        assert!(all_set(&flags, 5), "a reader thread outlived its player");
    }

    #[test]
    fn decode_size_only_downscales() {
        assert_eq!(decode_size((1280, 720), (1920, 1080), false), (1280, 720));
        assert_eq!(decode_size((3840, 2160), (1920, 1080), false), (1920, 1080));
        assert_eq!(decode_size((1920, 1080), (1080, 1920), false), (1080, 608));
        assert_eq!(decode_size((1920, 1080), (1080, 1920), true), (1920, 1080));
        assert_eq!(decode_size((641, 361), (4000, 4000), false), (640, 360));
    }
}
