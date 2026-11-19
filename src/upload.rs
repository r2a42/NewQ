//! Texture uploads on a thread of their own, so the main loop never waits on
//! the graphics driver.
//!
//! `queue.write_texture` can wait: wgpu takes locks the output and control
//! render threads hold while they submit and present, and the driver can make
//! it wait for memory. Two real tests on a Mac had the main loop held up
//! 100-180 ms at a time "making the pictures" while the main thread worked for
//! 1-5 ms of it: one ordinary 1080p frame's upload waited (twelve different
//! films were "the slowest picture"). Meanwhile cues, the network and the
//! controls waited too. Here the main thread copies the frame (as wgpu itself
//! did) and goes on; this thread writes it and submits, latest frame wins per
//! texture, and a texture is shown once its first picture is in
//! (`LayerTexture::is_filled`).

use crate::gpu::{Gpu, LayerTexture};
use crate::media::video::RgbaImage;
use parking_lot::{Condvar, Mutex};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// What kind of data a texture takes (see `LayerTexture::upload_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Planar Y, U, V.
    Yuv,
    Rgba,
    /// A Hap frame's blocks.
    Bc,
}

/// The bytes of an upload: a copy made for it, or a picture the engine holds.
pub enum Bytes {
    Owned(Vec<u8>),
    Image(Arc<RgbaImage>),
}

impl Bytes {
    fn as_slice(&self) -> &[u8] {
        match self {
            Bytes::Owned(v) => v,
            Bytes::Image(i) => &i.data,
        }
    }
}

pub struct Job {
    pub texture: Arc<LayerTexture>,
    pub kind: Kind,
    pub data: Bytes,
}

impl Job {
    fn key(&self) -> usize {
        Arc::as_ptr(&self.texture) as usize
    }

    fn write(&self, gpu: &Gpu) {
        write_now(gpu, &self.texture, self.kind, self.data.as_slice());
    }
}

/// The uploads waiting, at most one per texture: a newer picture for a
/// texture takes the place of the one not yet written (which `put` gives back
/// so its buffer can be used again). In the order the textures first came.
#[derive(Default)]
pub struct Pending {
    jobs: Vec<Job>,
}

impl Pending {
    pub fn put(&mut self, job: Job) -> Option<Job> {
        let key = job.key();
        match self.jobs.iter_mut().find(|j| j.key() == key) {
            Some(slot) => Some(std::mem::replace(slot, job)),
            None => {
                self.jobs.push(job);
                None
            }
        }
    }

    pub fn take(&mut self) -> Vec<Job> {
        std::mem::take(&mut self.jobs)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }
}

/// Buffers kept for the next copies (a 1080p frame is 3 MB; more than this
/// many are only needed while the thread is held up, and then let go).
const SPARE: usize = 12;

/// How often the device is polled, so a lost device is noticed (`Gpu::lost`).
const POLL_EVERY: Duration = Duration::from_millis(100);

/// A batch that takes this long is logged (the main loop went on meanwhile).
const SLOW_BATCH: Duration = Duration::from_millis(50);
/// At most one such line this often.
const SLOW_LOG_EVERY: Duration = Duration::from_secs(10);

#[derive(Default)]
struct State {
    pending: Pending,
    spare: Vec<Vec<u8>>,
    quit: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    cv: Condvar,
    /// The thread has ended (a panic in the driver): `Uploader::ended`.
    ended: AtomicBool,
    /// Pictures written, for tests.
    written: AtomicU64,
}

/// The upload thread. Dropping it lets the thread go (without waiting for it:
/// it may be stuck in a lost device).
pub struct Uploader {
    shared: Arc<Shared>,
}

impl Uploader {
    pub fn start(gpu: Gpu) -> std::io::Result<Uploader> {
        let shared = Arc::new(Shared::default());
        let s = shared.clone();
        std::thread::Builder::new()
            .name("uploads".into())
            .spawn(move || {
                let s2 = s.clone();
                let _said = crate::crash::OnPanic(move || {
                    s2.ended.store(true, Ordering::Release);
                    s2.cv.notify_all();
                });
                work(&gpu, &s);
                s.ended.store(true, Ordering::Release);
            })?;
        Ok(Uploader { shared })
    }

    /// The thread has stopped (it panicked): start another.
    pub fn ended(&self) -> bool {
        self.shared.ended.load(Ordering::Acquire)
    }

    /// Upload a copy of `data` into `texture` (a video frame: its buffer goes
    /// back to the reader).
    pub fn copy(&self, texture: &Arc<LayerTexture>, kind: Kind, data: &[u8]) {
        let mut buf = self.shared.state.lock().spare.pop().unwrap_or_default();
        buf.clear();
        buf.extend_from_slice(data);
        self.put(Job {
            texture: texture.clone(),
            kind,
            data: Bytes::Owned(buf),
        });
    }

    /// Upload a picture the engine holds, without copying it.
    pub fn image(&self, texture: &Arc<LayerTexture>, image: &Arc<RgbaImage>) {
        self.put(Job {
            texture: texture.clone(),
            kind: Kind::Rgba,
            data: Bytes::Image(image.clone()),
        });
    }

    /// Upload RGBA bytes made for it (a pattern, a label, a card).
    pub fn rgba(&self, texture: &Arc<LayerTexture>, data: Vec<u8>) {
        self.put(Job {
            texture: texture.clone(),
            kind: Kind::Rgba,
            data: Bytes::Owned(data),
        });
    }

    fn put(&self, job: Job) {
        let mut st = self.shared.state.lock();
        if let Some(old) = st.pending.put(job) {
            keep_spare(&mut st.spare, old.data);
        }
        drop(st);
        self.shared.cv.notify_one();
    }

    /// Pictures written so far.
    #[cfg(test)]
    fn written(&self) -> u64 {
        self.shared.written.load(Ordering::Acquire)
    }
}

impl Drop for Uploader {
    fn drop(&mut self) {
        self.shared.state.lock().quit = true;
        self.shared.cv.notify_all();
    }
}

/// Hand `data` to `up`, or write it here when there is no upload thread
/// (it could not be started).
pub fn copy(
    up: Option<&Uploader>,
    gpu: &Gpu,
    texture: &Arc<LayerTexture>,
    kind: Kind,
    data: &[u8],
) {
    match up {
        Some(up) => up.copy(texture, kind, data),
        None => write_now(gpu, texture, kind, data),
    }
}

/// `copy` of a picture the engine holds.
pub fn image(up: Option<&Uploader>, gpu: &Gpu, texture: &Arc<LayerTexture>, img: &Arc<RgbaImage>) {
    match up {
        Some(up) => up.image(texture, img),
        None => write_now(gpu, texture, Kind::Rgba, &img.data),
    }
}

/// `copy` of RGBA bytes made for it.
pub fn rgba(up: Option<&Uploader>, gpu: &Gpu, texture: &Arc<LayerTexture>, data: Vec<u8>) {
    match up {
        Some(up) => up.rgba(texture, data),
        None => write_now(gpu, texture, Kind::Rgba, &data),
    }
}

fn write_now(gpu: &Gpu, texture: &LayerTexture, kind: Kind, data: &[u8]) {
    match kind {
        Kind::Yuv => texture.upload_yuv(&gpu.queue, data),
        Kind::Rgba => texture.upload_rgba(&gpu.queue, data),
        Kind::Bc => texture.upload_bc(&gpu.queue, data),
    }
}

fn keep_spare(spare: &mut Vec<Vec<u8>>, data: Bytes) {
    if let Bytes::Owned(v) = data
        && spare.len() < SPARE
    {
        spare.push(v);
    }
}

fn work(gpu: &Gpu, s: &Shared) {
    let mut last_said: Option<Instant> = None;
    let mut last_poll = Instant::now();
    loop {
        let jobs = {
            let mut st = s.state.lock();
            while st.pending.is_empty() && !st.quit {
                if s.cv.wait_for(&mut st, POLL_EVERY).timed_out() && st.pending.is_empty() {
                    // Off the main thread, as is everything here.
                    drop(st);
                    last_poll = Instant::now();
                    let _ = gpu.device.poll(wgpu::PollType::Poll);
                    st = s.state.lock();
                }
            }
            if st.quit {
                return;
            }
            st.pending.take()
        };
        let began = Instant::now();
        let n = jobs.len();
        let mut spent = Vec::with_capacity(n);
        for job in jobs {
            job.write(gpu);
            spent.push(job.data);
        }
        let written = began.elapsed();
        // The staging copies are held until something submits, and the
        // windows that would may not be drawing (a Mac stops presenting a
        // window in the background): a minute of video was gigabytes.
        gpu.flush_uploads();
        s.written.fetch_add(n as u64, Ordering::AcqRel);
        // (A submit to a lost device fails without saying so: the poll does.)
        if last_poll.elapsed() >= POLL_EVERY {
            last_poll = Instant::now();
            let _ = gpu.device.poll(wgpu::PollType::Poll);
        }
        let took = began.elapsed();
        if took >= SLOW_BATCH && last_said.is_none_or(|t| t.elapsed() >= SLOW_LOG_EVERY) {
            last_said = Some(Instant::now());
            log::info!(
                "uploading {n} picture{} to the graphics card took {} ms ({} ms writing, {} ms submitting); the main loop went on meanwhile",
                if n == 1 { "" } else { "s" },
                took.as_millis(),
                written.as_millis(),
                took.saturating_sub(written).as_millis()
            );
        }
        let mut st = s.state.lock();
        for data in spent {
            keep_spare(&mut st.spare, data);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::ffmpeg::ColorMatrix;

    fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
        let began = Instant::now();
        while !ok() {
            assert!(began.elapsed() < Duration::from_secs(30), "{what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_newer_picture_for_a_texture_takes_the_place_of_the_one_waiting() {
        let Some(gpu) = Gpu::headless() else {
            assert!(std::env::var_os("NEWQ_REQUIRE_GPU").is_none(), "no GPU");
            return;
        };
        let a = Arc::new(LayerTexture::new_rgba(&gpu, 2, 2));
        let b = Arc::new(LayerTexture::new_rgba(&gpu, 2, 2));
        let job = |t: &Arc<LayerTexture>, v: u8| Job {
            texture: t.clone(),
            kind: Kind::Rgba,
            data: Bytes::Owned(vec![v; 16]),
        };
        let mut p = Pending::default();
        assert!(p.put(job(&a, 1)).is_none());
        assert!(p.put(job(&b, 2)).is_none());
        let old = p.put(job(&a, 3)).expect("the first of a's is given back");
        assert_eq!(old.data.as_slice()[0], 1);
        assert_eq!(p.len(), 2);
        let jobs = p.take();
        assert!(p.is_empty());
        // a keeps its place in the order, with the newer picture.
        assert!(Arc::ptr_eq(&jobs[0].texture, &a));
        assert_eq!(jobs[0].data.as_slice()[0], 3);
        assert!(Arc::ptr_eq(&jobs[1].texture, &b));
    }

    #[test]
    fn a_texture_is_filled_only_once_its_picture_is_written() {
        let Some(gpu) = Gpu::headless() else {
            assert!(std::env::var_os("NEWQ_REQUIRE_GPU").is_none(), "no GPU");
            return;
        };
        let up = Uploader::start(gpu.clone()).expect("thread");
        let tex = Arc::new(LayerTexture::new_yuv(&gpu, 64, 32, ColorMatrix::Bt709));
        assert!(!tex.is_filled(), "a new texture holds nothing to show");
        // Too short a frame is not written, and doesn't count as a picture.
        up.copy(&tex, Kind::Yuv, &[0u8; 10]);
        wait_for("the short frame was looked at", || up.written() == 1);
        assert!(!tex.is_filled());
        up.copy(&tex, Kind::Yuv, &vec![128u8; 64 * 32 * 3 / 2]);
        wait_for("the frame was written", || tex.is_filled());
        let img = Arc::new(RgbaImage {
            width: 4,
            height: 4,
            data: vec![255; 64],
        });
        let still = Arc::new(LayerTexture::new_rgba(&gpu, 4, 4));
        up.image(&still, &img);
        wait_for("the picture was written", || still.is_filled());
    }

    /// The point of it all: whatever holds the upload thread up (here it
    /// can't submit while a surface is being configured), the main thread's
    /// side never waits, and only the newest picture is written afterwards.
    #[test]
    fn handing_over_never_waits_for_the_graphics_card() {
        let Some(gpu) = Gpu::headless() else {
            assert!(std::env::var_os("NEWQ_REQUIRE_GPU").is_none(), "no GPU");
            return;
        };
        let up = Arc::new(Uploader::start(gpu.clone()).expect("thread"));
        let tex = Arc::new(LayerTexture::new_rgba(&gpu, 16, 16));
        up.rgba(&tex, vec![1u8; 16 * 16 * 4]);
        wait_for("the first picture was written", || up.written() == 1);
        let held = gpu.gate.write();
        // Held up in its submit after writing the next one.
        up.rgba(&tex, vec![2u8; 16 * 16 * 4]);
        wait_for("the thread took the picture", || {
            up.shared.state.lock().pending.is_empty()
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let (up2, tex2) = (up.clone(), tex.clone());
        std::thread::spawn(move || {
            for i in 0..50u8 {
                up2.copy(&tex2, Kind::Rgba, &vec![i; 16 * 16 * 4]);
            }
            let _ = tx.send(());
        });
        assert!(
            rx.recv_timeout(Duration::from_secs(20)).is_ok(),
            "handing over pictures waited for the graphics card"
        );
        assert_eq!(up.shared.state.lock().pending.len(), 1, "the newest only");
        drop(held);
        wait_for("the newest was written", || up.written() == 3);
        assert!(up.shared.state.lock().spare.len() <= SPARE);
    }
}
