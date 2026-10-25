//! Waveforms for audio and video cues, so in and out points can be set by eye.
//!
//! The file's sound is decoded once by ffmpeg to a low-rate mono stream and
//! reduced to its peaks, a few hundred to a few thousand numbers however long
//! the file is. That runs on a background thread, and the result is kept per
//! file, like the file's probe.

use super::ffmpeg::Ffmpeg;
use crossbeam_channel::{Receiver, Sender, unbounded};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Sample rate the sound is decoded at: plenty to see the shape of it.
const RATE: f64 = 4000.0;
/// How many peaks a file gets, by length: this many a second, within these limits.
const PEAKS_PER_SECOND: f64 = 50.0;
const MIN_PEAKS: f64 = 800.0;
const MAX_PEAKS: f64 = 8000.0;

/// The loudness of a file over time.
#[derive(Debug, PartialEq)]
pub struct Waveform {
    /// Length of the sound in seconds.
    pub duration: f64,
    /// The loudest sample (0 to 1) in each equal slice of the file.
    pub peaks: Vec<f32>,
}

/// Reduces a stream of samples to peaks as they arrive.
struct PeakBuilder {
    per_peak: usize,
    count: usize,
    loudest: f32,
    total: usize,
    peaks: Vec<f32>,
}

impl PeakBuilder {
    fn new(per_peak: usize) -> PeakBuilder {
        PeakBuilder {
            per_peak: per_peak.max(1),
            count: 0,
            loudest: 0.0,
            total: 0,
            peaks: Vec::new(),
        }
    }

    fn push(&mut self, sample: f32) {
        self.loudest = self.loudest.max(sample.abs().min(1.0));
        self.count += 1;
        self.total += 1;
        if self.count == self.per_peak {
            self.peaks.push(self.loudest);
            (self.count, self.loudest) = (0, 0.0);
        }
    }

    /// The waveform so far (a short last slice still counts), if any sound arrived.
    fn finish(mut self) -> Option<Waveform> {
        if self.count > 0 {
            self.peaks.push(self.loudest);
        }
        (self.total > 0).then(|| Waveform {
            duration: self.total as f64 / RATE,
            peaks: self.peaks,
        })
    }
}

impl Waveform {
    /// Reduce `samples` to peaks of `per_peak` samples each (the last may be shorter).
    #[cfg(test)]
    pub fn from_samples(samples: impl IntoIterator<Item = f32>, per_peak: usize) -> Waveform {
        let mut b = PeakBuilder::new(per_peak);
        for s in samples {
            b.push(s);
        }
        b.finish().unwrap_or(Waveform {
            duration: 0.0,
            peaks: Vec::new(),
        })
    }

    /// The loudest peak between two times, 0 outside the file.
    pub fn peak_between(&self, from: f64, to: f64) -> f32 {
        if self.peaks.is_empty() || self.duration <= 0.0 {
            return 0.0;
        }
        let n = self.peaks.len();
        let at = |t: f64| {
            ((t / self.duration) * n as f64)
                .floor()
                .clamp(0.0, n as f64)
        };
        let (a, b) = (at(from.min(to)) as usize, at(from.max(to)) as usize);
        self.peaks[a.min(n - 1)..=b.min(n - 1)]
            .iter()
            .copied()
            .fold(0.0, f32::max)
    }
}

/// Read `r` to its end, keeping only the last `keep` bytes of it as text.
fn read_tail(mut r: impl Read, keep: usize) -> String {
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

/// Decode `path`'s first audio stream and reduce it to peaks. `duration_hint`
/// (from a probe) picks how fine they are.
pub fn read(ff: &Ffmpeg, path: &Path, duration_hint: Option<f64>) -> Result<Waveform, String> {
    let peaks = duration_hint.map_or(MIN_PEAKS, |d| {
        (d * PEAKS_PER_SECOND).clamp(MIN_PEAKS, MAX_PEAKS)
    });
    let per_peak = duration_hint
        .map_or(80.0, |d| (d * RATE / peaks).round())
        .max(1.0) as usize;
    let mut cmd = ff.command();
    cmd.arg("-i").arg(path).args([
        "-map",
        "0:a:0",
        "-vn",
        "-sn",
        "-dn",
        "-ac",
        "1",
        "-ar",
        &(RATE as u32).to_string(),
        "-f",
        "f32le",
        "pipe:1",
    ]);
    let mut child = cmd.spawn().map_err(|e| format!("starting ffmpeg: {e}"))?;
    let mut stdout = child.stdout.take().ok_or("no ffmpeg output")?;
    // ffmpeg's complaints are read as they come, on their own thread: if they
    // piled up unread (a damaged file can produce thousands of lines) it would
    // block writing them, and this thread would block waiting for its sound.
    let stderr = child.stderr.take();
    let errors = std::thread::spawn(move || stderr.map(|e| read_tail(e, 4096)).unwrap_or_default());
    let mut builder = PeakBuilder::new(per_peak);
    let mut buf = vec![0u8; 16 * 1024];
    let mut leftover: Vec<u8> = Vec::new();
    loop {
        let n = stdout.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        leftover.extend_from_slice(&buf[..n]);
        let whole = leftover.len() / 4 * 4;
        for chunk in leftover[..whole].as_chunks::<4>().0 {
            builder.push(f32::from_le_bytes(*chunk));
        }
        leftover.drain(..whole);
    }
    let _ = child.wait();
    builder.finish().ok_or_else(|| {
        let text = errors.join().unwrap_or_default();
        text.lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("no sound")
            .to_string()
    })
}

pub enum WaveState {
    Pending,
    Ready(Arc<Waveform>),
    Failed(String),
}

type Finished = (PathBuf, Result<Waveform, String>);

/// Waveforms of the files the show uses, read on background threads.
pub struct WaveformCache {
    entries: HashMap<PathBuf, WaveState>,
    tx: Sender<Finished>,
    rx: Receiver<Finished>,
}

impl Default for WaveformCache {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        WaveformCache {
            entries: HashMap::new(),
            tx,
            rx,
        }
    }
}

impl WaveformCache {
    pub fn poll(&mut self) {
        while let Ok((path, result)) = self.rx.try_recv() {
            let state = match result {
                Ok(w) => WaveState::Ready(Arc::new(w)),
                Err(e) => WaveState::Failed(e),
            };
            self.entries.insert(path, state);
        }
    }

    /// The waveform of `path`, starting to read it in the background if need be.
    pub fn get(&mut self, ff: Option<&Ffmpeg>, path: &Path, duration: Option<f64>) -> &WaveState {
        if !self.entries.contains_key(path) {
            let state = match ff {
                None => WaveState::Failed("ffmpeg not found".into()),
                Some(_) if !path.is_file() => WaveState::Failed("file not found".into()),
                Some(ff) => {
                    let (ff, tx, p) = (ff.clone(), self.tx.clone(), path.to_path_buf());
                    std::thread::spawn(move || {
                        let r = read(&ff, &p, duration);
                        let _ = tx.send((p, r));
                    });
                    WaveState::Pending
                }
            };
            self.entries.insert(path.to_path_buf(), state);
        }
        &self.entries[path]
    }

    /// Forget a file so it is read again (it was replaced or relinked).
    pub fn invalidate(&mut self, path: &Path) {
        self.entries.remove(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flood_of_error_text_is_read_through_but_only_the_end_is_kept() {
        // Far more than a pipe holds (64 KB), one line at a time.
        let flood: Vec<u8> = (0..50_000)
            .flat_map(|n| format!("error {n}\n").into_bytes())
            .collect();
        assert!(flood.len() > 400_000);
        let tail = read_tail(std::io::Cursor::new(flood), 4096);
        assert!(tail.len() <= 4096);
        assert_eq!(tail.lines().last(), Some("error 49999"));
        // Short text is kept whole.
        assert_eq!(
            read_tail(std::io::Cursor::new(b"bad file\n".to_vec()), 4096),
            "bad file\n"
        );
    }

    #[test]
    fn samples_reduce_to_their_loudest_in_each_slice() {
        let samples = [0.1, -0.5, 0.2, 0.0, 0.0, 0.0, -0.9, 0.3, 0.7];
        let w = Waveform::from_samples(samples, 3);
        assert_eq!(w.peaks, vec![0.5, 0.0, 0.9]);
        assert!((w.duration - 9.0 / RATE).abs() < 1e-9);
        // A short last slice still counts; loud samples are capped at 1.
        let w = Waveform::from_samples([0.2, 2.0, -3.0, 0.1], 3);
        assert_eq!(w.peaks, vec![1.0, 0.1]);
        // Nothing in, nothing out; a slice size of 0 is treated as 1.
        assert!(Waveform::from_samples([], 4).peaks.is_empty());
        assert_eq!(
            Waveform::from_samples([0.5, 0.25], 0).peaks,
            vec![0.5, 0.25]
        );
    }

    #[test]
    fn the_loudest_peak_between_two_times() {
        // 10 peaks over 10 seconds: only the one at 4 s to 5 s is loud.
        let mut peaks = vec![0.0; 10];
        peaks[4] = 0.8;
        let w = Waveform {
            duration: 10.0,
            peaks,
        };
        assert_eq!(w.peak_between(0.0, 3.9), 0.0);
        assert_eq!(w.peak_between(4.2, 4.3), 0.8);
        assert_eq!(w.peak_between(0.0, 10.0), 0.8);
        assert_eq!(w.peak_between(5.0, 4.0), 0.8, "either way round");
        // Outside the file is clamped to its ends, not a panic.
        assert_eq!(w.peak_between(-5.0, 100.0), 0.8);
        assert_eq!(w.peak_between(20.0, 30.0), 0.0);
        let empty = Waveform {
            duration: 0.0,
            peaks: vec![],
        };
        assert_eq!(empty.peak_between(0.0, 1.0), 0.0);
    }

    #[test]
    fn reads_a_real_file() {
        let Some((ff, path)) = super::super::test_media("tone_5s.wav") else {
            return;
        };
        let w = read(&ff, &path, Some(5.0)).unwrap();
        assert!((w.duration - 5.0).abs() < 0.05, "{}", w.duration);
        // 50 peaks a second, within limits: 5 s gives the minimum of 800.
        assert!(
            (780..=820).contains(&w.peaks.len()),
            "{} peaks",
            w.peaks.len()
        );
        // A steady tone: loud throughout, never clipped.
        let quiet = w.peaks.iter().filter(|p| **p < 0.05).count();
        assert!(quiet < 10, "{quiet} quiet peaks in a steady tone");
        assert!(w.peaks.iter().all(|p| *p <= 1.0));
        assert!(w.peak_between(1.0, 2.0) > 0.1);
        // Without a duration hint it still reads the whole file.
        let w2 = read(&ff, &path, None).unwrap();
        assert!((w2.duration - w.duration).abs() < 0.01);
    }

    #[test]
    fn reads_the_sound_of_a_video_and_says_when_there_is_none() {
        let Some((ff, video)) = super::super::test_media("testsrc_10s.mp4") else {
            return;
        };
        let w = read(&ff, &video, Some(10.0)).unwrap();
        assert!((w.duration - 10.0).abs() < 0.1);
        // A picture has no sound to draw.
        let Some((_, image)) = super::super::test_media("color_red.png") else {
            return;
        };
        assert!(read(&ff, &image, None).is_err());
        // Neither does a file that isn't media at all.
        let dir = std::env::temp_dir().join("newq_wave_text");
        std::fs::create_dir_all(&dir).unwrap();
        let text = dir.join("not_media.txt");
        std::fs::write(&text, "hello").unwrap();
        assert!(read(&ff, &text, None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_cache_reads_in_the_background_and_remembers() {
        let Some((ff, path)) = super::super::test_media("tone_5s.wav") else {
            return;
        };
        let mut cache = WaveformCache::default();
        assert!(matches!(
            cache.get(Some(&ff), &path, None),
            WaveState::Pending
        ));
        let t0 = std::time::Instant::now();
        loop {
            cache.poll();
            if let WaveState::Ready(w) = cache.get(Some(&ff), &path, None) {
                assert!(!w.peaks.is_empty());
                break;
            }
            assert!(t0.elapsed().as_secs() < 20, "never finished");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Forgetting a file reads it afresh.
        cache.invalidate(&path);
        assert!(matches!(
            cache.get(Some(&ff), &path, None),
            WaveState::Pending
        ));
        // A missing file or no ffmpeg fails at once, without a thread.
        assert!(matches!(
            cache.get(Some(&ff), Path::new("/no/such/file.wav"), None),
            WaveState::Failed(_)
        ));
        assert!(matches!(
            cache.get(None, Path::new("/other.wav"), None),
            WaveState::Failed(_)
        ));
    }
}
