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

/// Whether files are decoded by the graphics hardware where it can (a
/// setting of this computer, see `prefs`).
static HW_DECODE: AtomicBool = AtomicBool::new(false);

pub fn set_hw_decode(on: bool) {
    HW_DECODE.store(on, Ordering::Relaxed);
}

/// FFmpeg's input options for hardware decoding, if it is on. It falls back to
/// the processor for a file the hardware can't do.
fn hw_input_args(on: bool) -> &'static [&'static str] {
    if on { &["-hwaccel", "auto"] } else { &[] }
}

pub struct VideoFrame {
    /// Presentation time in seconds, relative to the cue's in point.
    pub pts: f64,
    /// Planar Y, U, V (yuv420p), or for a picture with transparency RGBA.
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
    /// Frames are RGBA (the picture has transparency), not yuv420p.
    pub rgba: bool,
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
    #[cfg(test)]
    pub fn spawn(
        ff: &Ffmpeg,
        path: &Path,
        info: &VideoInfo,
        start: f64,
        playback: Playback,
        size: (u32, u32),
    ) -> Result<VideoPlayer> {
        Self::spawn_at_rate(ff, path, info, start, playback, size, 1.0)
    }

    /// `spawn`, playing `rate` times as fast: the picture's time is the file's
    /// divided by `rate`, at the same frame rate (frames are dropped or
    /// repeated to keep it).
    pub fn spawn_at_rate(
        ff: &Ffmpeg,
        path: &Path,
        info: &VideoInfo,
        start: f64,
        playback: Playback,
        size: (u32, u32),
        rate: f64,
    ) -> Result<VideoPlayer> {
        let (w, h) = size;
        let fps = info.fps;
        // A picture with transparency is decoded to RGBA, with the matrix and
        // range its file says, since the compositor's yuv path has no alpha.
        let alpha = info.has_alpha;
        let matrix_name = match info.matrix {
            ColorMatrix::Bt601 => "bt601",
            ColorMatrix::Bt709 => "bt709",
            ColorMatrix::Bt2020 => "bt2020",
        };
        let (scale_tail, pix_fmt) = if alpha {
            (
                format!("in_color_matrix={matrix_name}:out_range=pc,format=rgba"),
                "rgba",
            )
        } else {
            ("out_range=tv,format=yuv420p".to_string(), "yuv420p")
        };
        let speed = if (rate - 1.0).abs() < 1e-9 {
            String::new()
        } else {
            format!("setpts=PTS/{rate:.6},")
        };
        let make = {
            let (ff, path) = (ff.clone(), path.to_path_buf());
            // The first pass of a jump into a loop starts part way in.
            let calls = std::sync::atomic::AtomicUsize::new(0);
            move || {
                let first = calls.fetch_add(1, Ordering::Relaxed) == 0;
                let mut cmd = ff.command();
                cmd.args(if first {
                    playback.first_input_args(start)
                } else {
                    playback.input_args(start)
                });
                cmd.args(hw_input_args(HW_DECODE.load(Ordering::Relaxed)));
                cmd.arg("-i").arg(&path);
                cmd.args(if first {
                    playback.first_limit_args_at(rate)
                } else {
                    playback.limit_args_at(rate)
                });
                cmd.args([
                    "-map",
                    "0:v:0",
                    "-an",
                    "-sn",
                    "-dn",
                    "-vf",
                    &format!("{speed}scale={w}:{h}:flags=bicubic:{scale_tail}"),
                    "-fps_mode",
                    "cfr",
                    "-r",
                    &format!("{fps:.6}"),
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    pix_fmt,
                    "pipe:1",
                ]);
                cmd
            }
        };
        // Looping passes are cut to an exact frame count so the timeline
        // stays locked to the audio's, whatever the decoder delivers.
        let frames = |l: f64| (l / rate * fps).round().max(1.0) as u64;
        let per_pass = playback.pass_len().map(frames);
        let first_pass = playback.first_pass_len().map(frames);
        Self::start(
            make,
            playback.passes(),
            per_pass,
            first_pass,
            fps,
            info.matrix,
            size,
            false,
            alpha,
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
            None,
            f64::from(fps.max(1)),
            matrix,
            size,
            true,
            false,
        )
    }

    /// Take an NDI source as a live picture of `size`. NewQ's own helper
    /// program talks to NDI and writes yuv420p frames (see `ndi.rs`).
    pub fn spawn_ndi(
        source: &str,
        matrix: ColorMatrix,
        fps: u32,
        size: (u32, u32),
    ) -> Result<VideoPlayer> {
        let args: Vec<String> = [
            "--ndi-receive",
            source,
            &size.0.to_string(),
            &size.1.to_string(),
            if matrix == ColorMatrix::Bt601 {
                "601"
            } else {
                "709"
            },
        ]
        .map(String::from)
        .to_vec();
        let make = move || {
            // `current_exe` can fail only if the program was removed under
            // us; then the start below fails with the reason.
            crate::ndi::helper(&args).unwrap_or_else(|_| std::process::Command::new("false"))
        };
        Self::start(
            make,
            Some(1),
            None,
            None,
            f64::from(fps.max(1)),
            matrix,
            size,
            true,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn start(
        make: impl Fn() -> std::process::Command + Send + 'static,
        passes: Option<u32>,
        per_pass: Option<u64>,
        first_pass: Option<u64>,
        fps: f64,
        matrix: ColorMatrix,
        size: (u32, u32),
        live: bool,
        rgba: bool,
    ) -> Result<VideoPlayer> {
        let (w, h) = size;
        let children = Children::new();
        let error = Arc::new(Mutex::new(None));
        let mut feed = PassFeed::start(&children, passes, &error, make)?;

        let frame_len = if rgba {
            (w * h * 4) as usize
        } else {
            (w * h + 2 * (w / 2) * (h / 2)) as usize
        };
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
                let mut first = true;
                loop {
                    let mut n: u64 = 0;
                    // The first pass of a jump into a loop is short.
                    let limit = if first { first_pass } else { per_pass };
                    first = false;
                    while limit.is_none_or(|p| n < p) {
                        if limit.is_some_and(|p| n * 2 >= p) {
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
                    base += limit.unwrap_or(n);
                }
                eof2.store(true, Ordering::Release);
            })?;

        Ok(VideoPlayer {
            width: w,
            height: h,
            rgba,
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
    fn a_clip_with_transparency_is_decoded_to_rgba() {
        let Some((ff, _)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let dir = std::env::temp_dir().join("newq_test_alpha");
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("alpha.mov");
        // Half-transparent red, in ProRes 4444.
        let made = ff
            .command()
            .args(["-y", "-f", "lavfi", "-i"])
            .arg("color=c=red@0.5:s=64x64:d=1:r=25,format=yuva444p10le")
            .args(["-c:v", "prores_ks", "-profile:v", "4444"])
            .args(["-pix_fmt", "yuva444p10le"])
            .arg(&clip)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !made {
            // A build of ffmpeg without the ProRes encoder can't make the clip.
            assert!(
                std::env::var_os("NEWQ_REQUIRE_MEDIA").is_none(),
                "ffmpeg could not make the alpha clip"
            );
            return;
        }
        let info = ff.probe(&clip).expect("probe");
        let vi = info.video.as_ref().expect("a video stream");
        assert!(vi.has_alpha, "ProRes 4444 has an alpha channel");
        let mut p = VideoPlayer::spawn(
            &ff,
            &clip,
            vi,
            0.0,
            Playback::Once { duration: None },
            (64, 64),
        )
        .expect("spawn");
        let t0 = std::time::Instant::now();
        while !p.ready() {
            assert!(t0.elapsed().as_secs() < 10, "no first frame");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        p.update(0.0);
        assert!(p.rgba);
        let f = p.current().expect("a frame");
        assert_eq!(f.data.len(), 64 * 64 * 4);
        let (r, a) = (f.data[0], f.data[3]);
        assert!(r > 200, "red: {r}");
        assert!((i32::from(a) - 127).abs() <= 2, "half transparent: {a}");
        // A picture without transparency stays on the yuv path.
        let (_, plain) = crate::media::test_media("testsrc_10s.mp4").unwrap();
        assert!(!ff.probe(&plain).unwrap().video.unwrap().has_alpha);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hardware_decoding_asks_ffmpeg_and_still_decodes_what_it_cannot() {
        assert_eq!(hw_input_args(true), ["-hwaccel", "auto"]);
        assert!(hw_input_args(false).is_empty());
        let Some((ff, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let info = ff.probe(&clip).expect("probe");
        let vi = info.video.as_ref().expect("a video stream");
        // Where there is no hardware (as here) FFmpeg decodes in software.
        set_hw_decode(true);
        let p = VideoPlayer::spawn(
            &ff,
            &clip,
            vi,
            0.0,
            Playback::Once { duration: None },
            (320, 180),
        );
        let mut p = p.expect("spawn");
        let t0 = std::time::Instant::now();
        let ok = loop {
            if p.ready() {
                break true;
            }
            if t0.elapsed().as_secs() > 10 {
                break false;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        set_hw_decode(false);
        assert!(ok, "no frame with hardware decoding asked for");
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
