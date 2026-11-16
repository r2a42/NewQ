pub mod audio;
pub mod camera;
pub mod convert;
pub mod ffmpeg;
pub mod hap;
pub mod playback;
pub mod video;
pub mod waveform;

use crossbeam_channel::{Receiver, Sender, unbounded};
use ffmpeg::{Ffmpeg, MediaInfo};
use parking_lot::{Condvar, Mutex};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub enum ProbeState {
    Pending,
    Ready(MediaInfo),
    Failed(String),
}

/// How many files are probed at once. A show opened with hundreds of files
/// would otherwise start hundreds of ffprobe processes (and threads) together.
const PROBE_WORKERS: usize = 3;

/// Files waiting to be probed, shared with the worker threads.
struct Jobs {
    /// (the path the show names, the file to read, FFmpeg)
    queue: Mutex<VecDeque<(PathBuf, PathBuf, Ffmpeg)>>,
    wake: Condvar,
    closed: AtomicBool,
    /// Tests keep the workers from finishing, to look at what is waiting.
    #[cfg(test)]
    hold: AtomicBool,
}

type Probed = (PathBuf, Result<MediaInfo, String>);

/// Caches ffprobe results. Probes run on a few background threads, the files
/// nearest the playhead first.
pub struct ProbeCache {
    entries: HashMap<PathBuf, ProbeState>,
    /// When each missing file was last looked for.
    rechecked: HashMap<PathBuf, std::time::Instant>,
    tx: Sender<Probed>,
    rx: Receiver<Probed>,
    jobs: Arc<Jobs>,
    workers: usize,
    /// In a show network, files come from the media store (`net::media`):
    /// the paths answered through it, with the store's file read (None: what
    /// the primary said of a file not here yet), and the finder's generation
    /// they were answered at.
    via: HashMap<PathBuf, Option<PathBuf>>,
    via_gen: Option<u64>,
    /// Paths are answered through a finder (since the last look).
    through_store: bool,
    /// A finder of its own (tests), else the app's (`net::media::finder`).
    pub finder: Option<Arc<dyn crate::net::media::Finder>>,
}

impl Default for ProbeCache {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        ProbeCache {
            entries: HashMap::new(),
            rechecked: HashMap::new(),
            tx,
            rx,
            jobs: Arc::new(Jobs {
                queue: Mutex::new(VecDeque::new()),
                wake: Condvar::new(),
                closed: AtomicBool::new(false),
                #[cfg(test)]
                hold: AtomicBool::new(false),
            }),
            workers: 0,
            via: HashMap::new(),
            via_gen: None,
            through_store: false,
            finder: None,
        }
    }
}

impl Drop for ProbeCache {
    fn drop(&mut self) {
        self.jobs.closed.store(true, Ordering::Release);
        self.jobs.wake.notify_all();
    }
}

/// One probe worker: takes the next file off the queue until the cache is gone.
fn probe_worker(jobs: &Jobs, tx: &Sender<Probed>) {
    loop {
        let (path, file, ff) = {
            let mut queue = jobs.queue.lock();
            loop {
                if jobs.closed.load(Ordering::Acquire) {
                    return;
                }
                if let Some(job) = queue.pop_front() {
                    break job;
                }
                jobs.wake.wait(&mut queue);
            }
        };
        #[cfg(test)]
        while jobs.hold.load(Ordering::Acquire) && !jobs.closed.load(Ordering::Acquire) {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let r = ff.probe(&file).map_err(|e| format!("{e:#}"));
        if tx.send((path, r)).is_err() {
            return;
        }
    }
}

/// What a lookup of a file that isn't there says.
const NOT_FOUND: &str = "file not found";

/// How often a file that was missing is looked for again.
const RECHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(2);

impl ProbeCache {
    /// Forget that a file was missing once it is there (a drive mounted, a
    /// copy finished); otherwise it would stay missing until NewQ restarted.
    /// Looked for every couple of seconds, not every time it is asked about:
    /// the cue list asks for every cue every frame, and asking the disk about
    /// a file on a drive that has gone can take a long time.
    fn recheck(&mut self, path: &Path) {
        if !matches!(self.entries.get(path), Some(ProbeState::Failed(e)) if e == NOT_FOUND) {
            return;
        }
        let now = std::time::Instant::now();
        let due = self
            .rechecked
            .get(path)
            .is_none_or(|at| now.duration_since(*at) >= RECHECK_EVERY);
        if !due {
            return;
        }
        self.rechecked.insert(path.to_path_buf(), now);
        if path.is_file() {
            self.entries.remove(path);
            self.rechecked.remove(path);
        }
    }

    /// Look for the missing files again at the next ask (for tests).
    #[cfg(test)]
    pub fn recheck_now(&mut self) {
        self.rechecked.clear();
    }

    pub fn poll(&mut self) {
        self.follow_store();
        while let Ok((path, result)) = self.rx.try_recv() {
            // In a show network, an answer for a path no longer read from
            // the store's file is stale.
            if !matches!(self.via.get(&path), Some(Some(_))) && self.finder().is_some() {
                continue;
            }
            let state = match result {
                Ok(info) => ProbeState::Ready(info),
                Err(e) => ProbeState::Failed(e),
            };
            self.entries.insert(path, state);
        }
    }

    /// Returns the cached state, starting a background probe if needed.
    pub fn get(&mut self, ff: Option<&Ffmpeg>, path: &Path) -> &ProbeState {
        self.lookup(ff, path, false)
    }

    /// Like `get`, but a probe that is wanted soon (a cue about to be fired)
    /// goes to the front of the queue.
    pub fn get_urgent(&mut self, ff: Option<&Ffmpeg>, path: &Path) -> &ProbeState {
        self.lookup(ff, path, true)
    }

    /// What is known about a file, without starting anything.
    pub fn peek(&self, path: &Path) -> Option<&ProbeState> {
        self.entries.get(path)
    }

    /// The finder in use: this cache's own, else the app's.
    fn finder(&self) -> Option<Arc<dyn crate::net::media::Finder>> {
        self.finder.clone().or_else(crate::net::media::finder)
    }

    /// In a show network: when the store has changed, the paths answered
    /// through it whose file is now another (it arrived, or a new version)
    /// are read again.
    fn follow_store(&mut self) {
        let Some(f) = self.finder() else {
            self.through_store = false;
            if !self.via.is_empty() {
                for p in std::mem::take(&mut self.via).into_keys() {
                    self.entries.remove(&p);
                }
                self.via_gen = None;
            }
            return;
        };
        // Just joined a show network: everything known was read from the
        // paths themselves, and is asked again through the store.
        if !self.through_store {
            self.through_store = true;
            self.entries.clear();
            self.rechecked.clear();
            self.via.clear();
        }
        let g = f.generation();
        if self.via_gen == Some(g) {
            return;
        }
        self.via_gen = Some(g);
        let stale: Vec<PathBuf> = self
            .via
            .iter()
            .filter(|(p, was)| f.local(p).here().map(Path::to_path_buf) != **was)
            .map(|(p, _)| p.clone())
            .collect();
        for p in stale {
            self.via.remove(&p);
            self.entries.remove(&p);
        }
    }

    /// Whether what is known of `path` was read from a file here (not what
    /// the primary said of one on its way).
    pub fn read_here(&self, path: &Path) -> bool {
        !matches!(self.via.get(path), Some(None))
    }

    fn lookup(&mut self, ff: Option<&Ffmpeg>, path: &Path, urgent: bool) -> &ProbeState {
        self.poll();
        if let Some(f) = self.finder() {
            if !self.entries.contains_key(path) {
                let state = match (f.local(path), ff) {
                    (_, None) => ProbeState::Failed("ffmpeg not found".into()),
                    (crate::net::media::Local::Here(file), Some(ff)) => {
                        self.queue(ff.clone(), path.to_path_buf(), file.clone(), urgent);
                        self.via.insert(path.to_path_buf(), Some(file));
                        ProbeState::Pending
                    }
                    // Not here yet: what the primary read from it.
                    (_, Some(_)) => {
                        self.via.insert(path.to_path_buf(), None);
                        ProbeState::Ready(crate::net::media::card::info_from(
                            f.facts(path).as_ref(),
                        ))
                    }
                };
                self.entries.insert(path.to_path_buf(), state);
            }
            return &self.entries[path];
        }
        self.recheck(path);
        if !self.entries.contains_key(path) {
            let state = match ff {
                None => ProbeState::Failed("ffmpeg not found".into()),
                Some(_) if !path.is_file() => {
                    self.rechecked
                        .insert(path.to_path_buf(), std::time::Instant::now());
                    ProbeState::Failed(NOT_FOUND.into())
                }
                Some(ff) => {
                    self.queue(ff.clone(), path.to_path_buf(), path.to_path_buf(), urgent);
                    ProbeState::Pending
                }
            };
            self.entries.insert(path.to_path_buf(), state);
        } else if urgent && matches!(self.entries.get(path), Some(ProbeState::Pending)) {
            // Already waiting its turn: move it up.
            let mut queue = self.jobs.queue.lock();
            if let Some(i) = queue.iter().position(|(p, _, _)| p == path)
                && let Some(job) = queue.remove(i)
            {
                queue.push_front(job);
            }
        }
        &self.entries[path]
    }

    /// Put a file in the queue, and start a worker if fewer than the limit run.
    fn queue(&mut self, ff: Ffmpeg, path: PathBuf, file: PathBuf, urgent: bool) {
        {
            let mut queue = self.jobs.queue.lock();
            if urgent {
                queue.push_front((path, file, ff));
            } else {
                queue.push_back((path, file, ff));
            }
        }
        if self.workers < PROBE_WORKERS {
            let (jobs, tx) = (self.jobs.clone(), self.tx.clone());
            let spawned = std::thread::Builder::new()
                .name("probe".into())
                .spawn(move || probe_worker(&jobs, &tx));
            if spawned.is_ok() {
                self.workers += 1;
            }
        }
        self.jobs.wake.notify_one();
    }

    /// Worker threads started, and files waiting for one (for tests).
    #[cfg(test)]
    pub fn load(&self) -> (usize, usize) {
        (self.workers, self.jobs.queue.lock().len())
    }

    /// Keep the workers from finishing (for tests).
    #[cfg(test)]
    pub fn hold(&self, on: bool) {
        self.jobs.hold.store(on, Ordering::Release);
    }

    /// Forget a path so it is probed again (e.g. after the user relinks a file).
    pub fn invalidate(&mut self, path: &Path) {
        self.entries.remove(path);
        self.via.remove(path);
        self.rechecked.remove(path);
    }
}

/// Locate ffmpeg and a file in `test_media/` for integration tests. Returns
/// None (test skips) if either is missing, unless `NEWQ_REQUIRE_MEDIA` is set,
/// as it is in CI, in which case a missing dependency fails the test.
#[cfg(test)]
pub fn test_media(file: &str) -> Option<(Ffmpeg, PathBuf)> {
    let found = Ffmpeg::locate().and_then(|ff| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("test_media")
            .join(file);
        path.is_file().then_some((ff, path))
    });
    if found.is_none() && std::env::var_os("NEWQ_REQUIRE_MEDIA").is_some() {
        panic!("NEWQ_REQUIRE_MEDIA is set but ffmpeg or test_media/{file} is missing");
    }
    found
}

/// Like `test_media`, for tests that make a Hap file with FFmpeg: None (the
/// test skips) when this FFmpeg has no `hap` encoder, as on Homebrew's build.
#[cfg(test)]
pub fn test_media_with_hap(file: &str) -> Option<(Ffmpeg, PathBuf)> {
    let (ff, path) = test_media(file)?;
    if ff.has_encoder("hap") {
        Some((ff, path))
    } else {
        eprintln!("skipped: this FFmpeg has no hap encoder");
        None
    }
}

/// Read `r` to its end, keeping only the last `keep` bytes of it as text.
pub(crate) fn read_tail(mut r: impl std::io::Read, keep: usize) -> String {
    let mut tail: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    while let Ok(n) = r.read(&mut buf) {
        if n == 0 {
            break;
        }
        tail.extend_from_slice(&buf[..n]);
        if tail.len() > keep {
            tail.drain(..tail.len() - keep);
        }
    }
    String::from_utf8_lossy(&tail).into_owned()
}

#[cfg(test)]
mod tests {
    //! These run real ffmpeg against `test_media/`; they skip if either is missing.
    use super::ffmpeg::Ffmpeg;
    use super::playback::Playback;
    use super::video::{self, VideoPlayer};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn setup(file: &str) -> Option<(Ffmpeg, PathBuf)> {
        super::test_media(file)
    }

    /// Ask until the answer for `path` is in.
    fn wait_for(cache: &mut super::ProbeCache, ff: &Ffmpeg, path: &std::path::Path) -> bool {
        let t0 = Instant::now();
        loop {
            match cache.get(Some(ff), path) {
                super::ProbeState::Ready(_) => return true,
                super::ProbeState::Failed(_) => return false,
                super::ProbeState::Pending => {}
            }
            assert!(t0.elapsed() < Duration::from_secs(10), "never answered");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_file_that_turns_up_later_is_no_longer_missing() {
        let Some((ff, clip)) = setup("testsrc_10s.mp4") else {
            return;
        };
        let later = std::env::temp_dir().join("newq_probe_later.mp4");
        let _ = std::fs::remove_file(&later);
        let mut cache = super::ProbeCache::default();
        assert!(matches!(
            cache.get(Some(&ff), &later),
            super::ProbeState::Failed(_)
        ));
        // The drive is mounted / the copy finishes.
        std::fs::copy(&clip, &later).unwrap();
        // Asked again at once, the disk isn't troubled (the cue list asks
        // every frame); when it is time to look again, the file is found.
        assert!(matches!(
            cache.get(Some(&ff), &later),
            super::ProbeState::Failed(_)
        ));
        cache.recheck_now();
        assert!(
            !matches!(cache.get(Some(&ff), &later), super::ProbeState::Failed(_)),
            "found now"
        );
        assert!(wait_for(&mut cache, &ff, &later), "and readable");
        let _ = std::fs::remove_file(&later);
    }

    #[test]
    fn many_files_are_probed_a_few_at_a_time_and_the_urgent_one_first() {
        let Some((ff, clip)) = setup("testsrc_10s.mp4") else {
            return;
        };
        let dir = std::env::temp_dir().join("newq_probe_pool");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let files: Vec<PathBuf> = (0..30)
            .map(|i| {
                let f = dir.join(format!("clip{i}.mp4"));
                std::fs::hard_link(&clip, &f)
                    .or_else(|_| std::fs::copy(&clip, &f).map(|_| ()))
                    .unwrap();
                f
            })
            .collect();
        let mut cache = super::ProbeCache::default();
        cache.hold(true);
        for f in &files {
            assert!(matches!(
                cache.get(Some(&ff), f),
                super::ProbeState::Pending
            ));
        }
        // The three workers each take a file as they start, which a loaded
        // machine does in its own time: wait for it, not for a tenth of a second.
        let began = std::time::Instant::now();
        while cache.load().1 > 27 && began.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let (workers, waiting) = cache.load();
        assert_eq!(workers, 3, "three at once, not thirty");
        assert_eq!(waiting, 27, "the rest wait their turn");
        // The one wanted soonest jumps the queue, wherever it was.
        cache.get_urgent(Some(&ff), &files[29]);
        cache.hold(false);
        assert!(wait_for(&mut cache, &ff, &files[29]));
        // And every file is answered in the end.
        for f in &files {
            assert!(wait_for(&mut cache, &ff, f), "{}", f.display());
        }
        assert_eq!(cache.load(), (3, 0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn decodes_video_frames_in_order() {
        let Some((ff, path)) = setup("testsrc_10s.mp4") else {
            return;
        };
        let info = ff.probe(&path).unwrap();
        let vi = info.video.as_ref().unwrap();
        assert_eq!((vi.width, vi.height), (1280, 720));
        assert!((info.duration.unwrap() - 10.0).abs() < 0.1);
        assert!(info.has_audio);

        let size = video::decode_size((vi.width, vi.height), (640, 360), false);
        let mut p = VideoPlayer::spawn(
            &ff,
            &path,
            vi,
            2.0,
            Playback::Once {
                duration: Some(1.0),
            },
            size,
        )
        .unwrap();
        let t0 = Instant::now();
        while !p.ready() {
            assert!(t0.elapsed() < Duration::from_secs(10), "no first frame");
            std::thread::sleep(Duration::from_millis(5));
        }
        p.update(0.0);
        let f = p.current().unwrap();
        assert_eq!(f.data.len(), 640 * 360 * 3 / 2);
        assert!(
            f.data[..640 * 360].iter().any(|&y| y > 100),
            "luma is blank"
        );
        // Drain: a 1 s clip at 30 fps yields ~30 frames, then EOF.
        let mut frames = 1;
        let mut last = p.serial;
        while !p.finished() {
            assert!(t0.elapsed() < Duration::from_secs(10), "never finished");
            p.update(1e9);
            if p.serial != last {
                frames += p.serial - last;
                last = p.serial;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!((28..=32).contains(&frames), "got {frames} frames");
    }

    /// Run the player until it finishes or `limit` frames have been shown.
    /// Returns how many frames were shown and the last frame's pts.
    fn drain(p: &mut VideoPlayer, limit: Option<u64>) -> (u64, f64) {
        let t0 = Instant::now();
        loop {
            assert!(
                t0.elapsed() < Duration::from_secs(20),
                "stalled at {} frames",
                p.serial
            );
            p.update(1e9);
            if limit.is_some_and(|l| p.serial >= l) || p.finished() {
                return (p.serial, p.current().map_or(0.0, |f| f.pts));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn loops_a_slice_a_set_number_of_times() {
        let Some((ff, path)) = setup("testsrc_10s.mp4") else {
            return;
        };
        let info = ff.probe(&path).unwrap();
        let vi = info.video.as_ref().unwrap();
        let size = video::decode_size((vi.width, vi.height), (320, 180), false);
        // 1 s slice at 30 fps, three plays: exactly 90 frames on one timeline.
        let pb = Playback::Slice {
            len: 1.0,
            passes: Some(3),
        };
        let mut p = VideoPlayer::spawn(&ff, &path, vi, 2.0, pb, size).unwrap();
        let (frames, last) = drain(&mut p, None);
        assert!((85..=90).contains(&frames), "got {frames} frames");
        // The last frame sits a frame short of 3 s: every pass kept its length.
        assert!((2.9..3.0).contains(&last), "last pts {last}");
    }

    #[test]
    fn loops_a_slice_forever_and_stops_cleanly() {
        let Some((ff, path)) = setup("testsrc_10s.mp4") else {
            return;
        };
        let info = ff.probe(&path).unwrap();
        let vi = info.video.as_ref().unwrap();
        let size = video::decode_size((vi.width, vi.height), (320, 180), false);
        let pb = Playback::Slice {
            len: 0.5,
            passes: None,
        };
        let mut p = VideoPlayer::spawn(&ff, &path, vi, 1.0, pb, size).unwrap();
        // Five passes' worth of frames and still going.
        let (frames, _) = drain(&mut p, Some(75));
        assert!(frames >= 75);
        assert!(!p.finished());
        let t0 = Instant::now();
        drop(p);
        assert!(t0.elapsed() < Duration::from_secs(2), "drop was slow");
    }

    #[test]
    fn loads_image_as_rgba() {
        let Some((ff, path)) = setup("color_red.png") else {
            return;
        };
        let info = ff.probe(&path).unwrap();
        assert!(info.is_still);
        let rx = video::load_image(&ff, &path, (64, 36));
        let img = rx.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
        assert_eq!(img.data.len(), 64 * 36 * 4);
        assert!(img.data[0] > 200 && img.data[1] < 30 && img.data[3] == 255);
    }

    #[test]
    fn probes_audio_file() {
        let Some((ff, path)) = setup("tone_5s.wav") else {
            return;
        };
        let info = ff.probe(&path).unwrap();
        assert!(info.video.is_none());
        assert!(info.has_audio);
        assert!((info.duration.unwrap() - 5.0).abs() < 0.05);
    }
}
