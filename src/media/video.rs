//! Video decoding via an ffmpeg subprocess that writes raw YUV 4:2:0 frames to a pipe.

use super::ffmpeg::{ColorMatrix, Ffmpeg, VideoInfo};
use super::hap::{self, HapFormat};
use super::playback::{Children, PassFeed, Playback};
use crate::speed::SpeedPlan;
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

/// Whether the graphics device reads block-compressed textures, which Hap
/// needs (see `Gpu::new`); without, Hap is decoded to a picture like any other.
static BC_SUPPORTED: AtomicBool = AtomicBool::new(false);

pub fn set_bc_supported(on: bool) {
    BC_SUPPORTED.store(on, Ordering::Relaxed);
}

/// Whether a file can be played as Hap frames sent to the card as they are:
/// it is Hap, its picture is a whole number of blocks and has no rotation or
/// odd pixel shape, and the card can read them.
pub fn plays_as_hap(info: &VideoInfo) -> bool {
    BC_SUPPORTED.load(Ordering::Relaxed)
        && std::env::var_os("NEWQ_NO_HAP").is_none()
        && info.codec == "hap"
        && info.coded.0.is_multiple_of(4)
        && info.coded.1.is_multiple_of(4)
        && (info.width, info.height) == info.coded
}

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
    /// A Hap frame: `data` is its texture, of this format, not a picture in the usual sense.
    pub hap: Option<HapFormat>,
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

/// Read a file's frames (at `fps`, in the file's own time) and send the
/// cue's: the cue's frame `j` is at `j / fps` and shows the file's frame the
/// plan has got to by then, so a frame is held while the speed is low and
/// skipped while it is high. A loop's passes (`first_pass` and `per_pass`
/// frames each) are one run of frames, which the plan goes round and round.
/// Ends when the file does, or the passes are used up.
#[allow(clippy::too_many_arguments)]
fn read_at_speed(
    plan: &SpeedPlan,
    fps: f64,
    feed: &mut PassFeed,
    tx: &Sender<VideoFrame>,
    recycle: &Receiver<Vec<u8>>,
    frame_len: usize,
    first_pass: Option<u64>,
    per_pass: Option<u64>,
) {
    let mut current = vec![0u8; frame_len];
    // The file's frame that `current` holds (none yet), counting through the passes.
    let held: std::cell::Cell<Option<u64>> = std::cell::Cell::new(None);
    let mut limit = first_pass;
    let mut in_pass: u64 = 0;
    // The next frame of the file's run, false when there is none.
    let mut next_frame = |feed: &mut PassFeed, current: &mut Vec<u8>| -> bool {
        if limit.is_some_and(|l| in_pass >= l) {
            if !feed.advance() {
                return false;
            }
            in_pass = 0;
            limit = per_pass;
        }
        if limit.is_some_and(|l| in_pass * 2 >= l) {
            feed.prefetch();
        }
        if feed.stdout().read_exact(current).is_err() {
            // A pass that is cut short holds its last frame to its length; one
            // that gave nothing, or no pass at all, is the end.
            if in_pass == 0 || limit.is_none() {
                return false;
            }
        }
        in_pass += 1;
        held.set(Some(held.get().map_or(0, |h| h + 1)));
        true
    };
    let mut j: u64 = 0;
    loop {
        let t = j as f64 / fps;
        let want = (plan.media_after(t) * fps).round().max(0.0) as u64;
        loop {
            // (`held` is only changed by `next_frame`: look at it between calls.)
            if held.get().is_some_and(|h| h >= want) {
                break;
            }
            if !next_frame(feed, &mut current) {
                return;
            }
        }
        let mut buf = recycle.try_recv().unwrap_or_else(|_| vec![0u8; frame_len]);
        buf.resize(frame_len, 0);
        buf.copy_from_slice(&current);
        if tx
            .send(VideoFrame {
                pts: t,
                hap: None,
                data: buf,
            })
            .is_err()
        {
            return; // player dropped
        }
        j += 1;
    }
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
        Self::spawn_at_rate(ff, path, info, start, playback, size, 1.0, None)
    }

    /// `spawn`, playing `rate` times as fast: the picture's time is the file's
    /// divided by `rate`, at the same frame rate (frames are dropped or
    /// repeated to keep it).
    ///
    /// With a `speed` plan the speed changes as the cue plays: the picture is
    /// decoded at its own speed and each frame of the cue shows the frame of
    /// the file the plan has got to (`rate` is then 1).
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_at_rate(
        ff: &Ffmpeg,
        path: &Path,
        info: &VideoInfo,
        start: f64,
        playback: Playback,
        size: (u32, u32),
        rate: f64,
        speed: Option<SpeedPlan>,
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
        let setpts = if (rate - 1.0).abs() < 1e-9 {
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
                    &format!("{setpts}scale={w}:{h}:flags=bicubic:{scale_tail}"),
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
            speed,
        )
    }

    /// Play a Hap file: FFmpeg only demuxes it and the frames, which are
    /// textures, go to the graphics card as they are (see `hap`). Its speed is
    /// in the times of the frames, so a `rate` or a keyframed `speed` costs
    /// nothing here; the player shows whichever frame is due. Loops and the
    /// in and out points are the same as for other video.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_hap(
        ff: &Ffmpeg,
        path: &Path,
        info: &VideoInfo,
        start: f64,
        playback: Playback,
        rate: f64,
        speed: Option<SpeedPlan>,
    ) -> Result<VideoPlayer> {
        let (w, h) = info.coded;
        let fps = info.fps;
        let make = {
            let (ff, path) = (ff.clone(), path.to_path_buf());
            let calls = std::sync::atomic::AtomicUsize::new(0);
            move || {
                let first = calls.fetch_add(1, Ordering::Relaxed) == 0;
                let mut cmd = ff.command();
                cmd.args(if first {
                    playback.first_input_args(start)
                } else {
                    playback.input_args(start)
                });
                cmd.arg("-i").arg(&path);
                // The frames' own times are what is counted, so the limit is in the file's.
                cmd.args(if first {
                    playback.first_limit_args_at(1.0)
                } else {
                    playback.limit_args_at(1.0)
                });
                cmd.args([
                    "-map", "0:v:0", "-an", "-sn", "-dn", "-c:v", "copy", "-f", "rawvideo",
                    "pipe:1",
                ]);
                cmd
            }
        };
        let children = Children::new();
        let error = Arc::new(Mutex::new(None));
        let mut feed = PassFeed::start(&children, playback.passes(), &error, make)?;
        let frames = |l: f64| (l * fps).round().max(1.0) as u64;
        let per_pass = playback.pass_len().map(frames);
        let first_pass = playback.first_pass_len().map(frames);
        let (tx, rx) = bounded::<VideoFrame>(QUEUE_FRAMES);
        let (recycle, recycle_rx) = unbounded::<Vec<u8>>();
        let eof = Arc::new(AtomicBool::new(false));
        let eof2 = eof.clone();
        let error2 = error.clone();
        let reader_done = Arc::new(AtomicBool::new(false));
        let reader_done2 = reader_done.clone();
        std::thread::Builder::new()
            .name("hap-reader".into())
            .spawn(move || {
                let _done = ReaderDone(reader_done2);
                let pts = |g: u64| match &speed {
                    Some(p) => p.time_after(g as f64 / fps),
                    None => g as f64 / (fps * rate),
                };
                let (mut packet, mut bad) = (Vec::new(), 0u32);
                let (mut base, mut first) = (0u64, true);
                'passes: loop {
                    let mut n: u64 = 0;
                    let limit = if first { first_pass } else { per_pass };
                    first = false;
                    while limit.is_none_or(|p| n < p) {
                        if limit.is_some_and(|p| n * 2 >= p) {
                            feed.prefetch();
                        }
                        match hap::read_frame(feed.stdout(), &mut packet) {
                            Ok(Some(())) => {}
                            Ok(None) => break,
                            Err(e) => {
                                *error2.lock() = Some(format!("{e:#}"));
                                break;
                            }
                        }
                        let mut data = recycle_rx.try_recv().unwrap_or_default();
                        let format = match hap::decode_frame(
                            &packet,
                            hap::texture_len_of(&packet, w, h),
                            &mut data,
                        ) {
                            Ok(f) => f,
                            Err(e) => {
                                // A damaged frame is skipped; a stream of them is the end.
                                bad += 1;
                                if bad > 5 {
                                    *error2.lock() = Some(format!("{e:#}"));
                                    break 'passes;
                                }
                                n += 1;
                                continue;
                            }
                        };
                        bad = 0;
                        let frame = VideoFrame {
                            pts: pts(base + n),
                            hap: Some(format),
                            data,
                        };
                        if tx.send(frame).is_err() {
                            return; // player dropped
                        }
                        n += 1;
                    }
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
            rgba: false,
            matrix: info.matrix,
            rx,
            recycle,
            children,
            eof,
            error,
            current: None,
            pending: None,
            serial: 0,
            live: false,
            reader_done,
        })
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
            None,
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
            None,
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
        speed: Option<SpeedPlan>,
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
                if let Some(plan) = speed {
                    read_at_speed(
                        &plan,
                        fps,
                        &mut feed,
                        &tx,
                        &recycle_rx,
                        frame_len,
                        first_pass,
                        per_pass,
                    );
                    eof2.store(true, Ordering::Release);
                    return;
                }
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
                            hap: None,
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

    /// When the next frame is due, if it has arrived (for tests that put the
    /// clock on each frame).
    #[cfg(test)]
    fn next_pts(&mut self) -> Option<f64> {
        self.fill_pending();
        self.pending.as_ref().map(|f| f.pts)
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
    fn a_ramped_speed_shows_the_frames_the_plan_has_got_to() {
        use crate::show::Animated;
        use crate::speed::SpeedMap;
        let Some((ff, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let info = ff.probe(&clip).expect("probe");
        let vi = info.video.as_ref().expect("a video stream");
        // Double speed from the start: ten seconds of file in five.
        let mut a = Animated::new(2.0);
        a.set_key(0.0, 2.0);
        let plan = SpeedPlan {
            map: std::sync::Arc::new(SpeedMap::new(&a)),
            t0: 0.0,
        };
        let mut p = VideoPlayer::spawn_at_rate(
            &ff,
            &clip,
            vi,
            0.0,
            Playback::Once { duration: None },
            (160, 90),
            1.0,
            Some(plan),
        )
        .expect("spawn");
        let t0 = std::time::Instant::now();
        let mut last_pts = -1.0;
        let mut frames = 0u64;
        // Step the clock a frame at a time until the picture has ended.
        let mut t = 0.0;
        loop {
            assert!(t0.elapsed().as_secs() < 30, "never ended");
            p.update(t);
            if let Some(f) = p.current()
                && f.pts > last_pts
            {
                last_pts = f.pts;
                frames += 1;
            }
            if p.finished() {
                break;
            }
            t += 1.0 / vi.fps;
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let secs = last_pts + 1.0 / vi.fps;
        assert!(
            (secs - 5.0).abs() < 0.3,
            "{secs} s of picture, {frames} frames"
        );
    }

    #[test]
    fn a_looping_picture_follows_a_keyframed_speed_through_its_passes() {
        use crate::show::Animated;
        use crate::speed::SpeedMap;
        let Some((ff, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let info = ff.probe(&clip).expect("probe");
        let vi = info.video.as_ref().expect("a video stream");
        // Two passes of two seconds at double speed: two seconds of picture.
        let mut a = Animated::new(2.0);
        a.set_key(0.0, 2.0);
        let plan = SpeedPlan {
            map: std::sync::Arc::new(SpeedMap::new(&a)),
            t0: 0.0,
        };
        let mut p = VideoPlayer::spawn_at_rate(
            &ff,
            &clip,
            vi,
            0.0,
            Playback::Slice {
                len: 2.0,
                passes: Some(2),
            },
            (160, 90),
            1.0,
            Some(plan),
        )
        .expect("spawn");
        let t0 = std::time::Instant::now();
        let (mut last_pts, mut t) = (-1.0, 0.0);
        loop {
            assert!(t0.elapsed().as_secs() < 30, "never ended");
            p.update(t);
            if let Some(f) = p.current() {
                last_pts = f64::max(last_pts, f.pts);
            }
            if p.finished() {
                break;
            }
            t += 1.0 / vi.fps;
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let secs = last_pts + 1.0 / vi.fps;
        assert!((secs - 2.0).abs() < 0.3, "{secs} s of picture");
    }

    /// A two-second Hap clip made with FFmpeg, 640 by 360 at the source's rate.
    fn hap_clip(ff: &Ffmpeg, src: &Path, name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("newq_test_hap_player");
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join(format!("{name}.mov"));
        let ok = ff
            .command()
            .args(["-y", "-i"])
            .arg(src)
            .args([
                "-t",
                "2",
                "-vf",
                "scale=640:360",
                "-c:v",
                "hap",
                "-format",
                name,
                "-an",
            ])
            .arg(&clip)
            .status()
            .unwrap()
            .success();
        assert!(ok, "ffmpeg could not make the Hap clip");
        clip
    }

    /// Every frame a player delivers, as (time, format), until it ends: the
    /// clock is put on each frame as it comes, so none is passed over.
    fn all_frames(p: &mut VideoPlayer) -> Vec<(f64, Option<HapFormat>)> {
        let t0 = std::time::Instant::now();
        let mut out = Vec::new();
        loop {
            assert!(t0.elapsed().as_secs() < 30, "never ended");
            match p.next_pts() {
                Some(t) => {
                    p.update(t);
                    let f = p.current().unwrap();
                    out.push((f.pts, f.hap));
                }
                None if p.finished() => return out,
                None => std::thread::sleep(std::time::Duration::from_millis(1)),
            }
        }
    }

    #[test]
    fn a_hap_file_is_played_as_the_frames_it_holds() {
        use crate::show::Animated;
        use crate::speed::SpeedMap;
        let Some((ff, src)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let clip = hap_clip(&ff, &src, "hap_alpha");
        let info = ff.probe(&clip).expect("probe");
        let vi = info.video.as_ref().expect("a video stream");
        assert_eq!((vi.codec.as_str(), vi.coded), ("hap", (640, 360)));
        // Only a card that reads block-compressed textures plays it this way.
        set_bc_supported(false);
        assert!(!plays_as_hap(vi));
        set_bc_supported(true);
        assert!(plays_as_hap(vi));
        let plain = ff.probe(&src).unwrap().video.unwrap();
        assert!(!plays_as_hap(&plain), "an ordinary file is not");
        set_bc_supported(false);

        let fps = vi.fps;
        let once = Playback::Once { duration: None };
        // At its own speed: a frame every 1/fps, each a Hap Alpha texture.
        let mut p = VideoPlayer::spawn_hap(&ff, &clip, vi, 0.0, once, 1.0, None).unwrap();
        assert_eq!((p.width, p.height), (640, 360));
        let frames = all_frames(&mut p);
        assert!(
            (fps * 2.0 - frames.len() as f64).abs() <= 2.0,
            "{} frames",
            frames.len()
        );
        assert!(frames.iter().all(|f| f.1 == Some(HapFormat::Dxt5)));
        assert!((frames[1].0 - 1.0 / fps).abs() < 1e-9);
        // At double speed the same frames come twice as fast.
        let mut p = VideoPlayer::spawn_hap(&ff, &clip, vi, 0.0, once, 2.0, None).unwrap();
        let fast = all_frames(&mut p);
        assert_eq!(fast.len(), frames.len());
        assert!((fast[1].0 - 0.5 / fps).abs() < 1e-9);
        // Along keyframes: the plan's times.
        let mut a = Animated::new(2.0);
        a.set_key(0.0, 2.0);
        let plan = SpeedPlan {
            map: std::sync::Arc::new(SpeedMap::new(&a)),
            t0: 0.0,
        };
        let mut p = VideoPlayer::spawn_hap(&ff, &clip, vi, 0.0, once, 1.0, Some(plan)).unwrap();
        let planned = all_frames(&mut p);
        assert!((planned.last().unwrap().0 - fast.last().unwrap().0).abs() < 0.01);
        // A loop of the first second, twice: the frames of both passes, one run of times.
        let slice = Playback::Slice {
            len: 1.0,
            passes: Some(2),
        };
        let mut p = VideoPlayer::spawn_hap(&ff, &clip, vi, 0.0, slice, 1.0, None).unwrap();
        let looped = all_frames(&mut p);
        assert!(
            (fps * 2.0 - looped.len() as f64).abs() <= 2.0,
            "{} frames",
            looped.len()
        );
        assert!(
            looped.windows(2).all(|w| w[1].0 > w[0].0),
            "times only go forward"
        );
        assert!((looped.last().unwrap().0 - 2.0).abs() < 0.1);
        // Starting part way in.
        let mut p = VideoPlayer::spawn_hap(&ff, &clip, vi, 1.0, once, 1.0, None).unwrap();
        let later = all_frames(&mut p);
        assert!(
            (fps - later.len() as f64).abs() <= 2.0,
            "{} frames",
            later.len()
        );
        let _ = std::fs::remove_dir_all(clip.parent().unwrap());
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
