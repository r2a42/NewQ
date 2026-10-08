pub mod audio;
pub mod ffmpeg;
pub mod video;

use crossbeam_channel::{Receiver, Sender, unbounded};
use ffmpeg::{Ffmpeg, MediaInfo};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub enum ProbeState {
    Pending,
    Ready(MediaInfo),
    Failed(String),
}

/// Caches ffprobe results; probes run on background threads.
pub struct ProbeCache {
    entries: HashMap<PathBuf, ProbeState>,
    tx: Sender<(PathBuf, Result<MediaInfo, String>)>,
    rx: Receiver<(PathBuf, Result<MediaInfo, String>)>,
}

impl Default for ProbeCache {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        ProbeCache {
            entries: HashMap::new(),
            tx,
            rx,
        }
    }
}

impl ProbeCache {
    pub fn poll(&mut self) {
        while let Ok((path, result)) = self.rx.try_recv() {
            let state = match result {
                Ok(info) => ProbeState::Ready(info),
                Err(e) => ProbeState::Failed(e),
            };
            self.entries.insert(path, state);
        }
    }

    /// Returns the cached state, starting a background probe if needed.
    pub fn get(&mut self, ff: Option<&Ffmpeg>, path: &Path) -> &ProbeState {
        if !self.entries.contains_key(path) {
            let state = match ff {
                None => ProbeState::Failed("ffmpeg not found".into()),
                Some(_) if !path.is_file() => ProbeState::Failed("file not found".into()),
                Some(ff) => {
                    let ff = ff.clone();
                    let tx = self.tx.clone();
                    let p = path.to_path_buf();
                    std::thread::spawn(move || {
                        let r = ff.probe(&p).map_err(|e| format!("{e:#}"));
                        let _ = tx.send((p, r));
                    });
                    ProbeState::Pending
                }
            };
            self.entries.insert(path.to_path_buf(), state);
        }
        &self.entries[path]
    }

    /// Blocking probe, used when a cue is fired before its background probe finished.
    pub fn get_blocking(&mut self, ff: &Ffmpeg, path: &Path) -> Result<MediaInfo, String> {
        self.poll();
        match self.entries.get(path) {
            Some(ProbeState::Ready(info)) => return Ok(info.clone()),
            Some(ProbeState::Failed(e)) => return Err(e.clone()),
            _ => {}
        }
        let r = ff.probe(path).map_err(|e| format!("{e:#}"));
        let state = match &r {
            Ok(info) => ProbeState::Ready(info.clone()),
            Err(e) => ProbeState::Failed(e.clone()),
        };
        self.entries.insert(path.to_path_buf(), state);
        r
    }

    /// Forget a path so it is probed again (e.g. after the user relinks a file).
    pub fn invalidate(&mut self, path: &Path) {
        self.entries.remove(path);
    }
}

#[cfg(test)]
mod tests {
    //! These run real ffmpeg against `test_media/`; they skip if either is missing.
    use super::ffmpeg::Ffmpeg;
    use super::video::{self, VideoPlayer};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn setup(file: &str) -> Option<(Ffmpeg, PathBuf)> {
        let ff = Ffmpeg::locate()?;
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("test_media")
            .join(file);
        path.is_file().then_some((ff, path))
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
        let mut p = VideoPlayer::spawn(&ff, &path, vi, 2.0, Some(1.0), false, size).unwrap();
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
