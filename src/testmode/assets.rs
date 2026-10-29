//! The media Test Mode plays: free clips, pictures and sound downloaded once
//! (and kept), or, with no network or when a download fails, made with FFmpeg.
//!
//! Every file is checked with the FFmpeg NewQ uses before it is trusted (a
//! damaged or truncated download, or a codec this FFmpeg can't decode, is
//! replaced by a generated one), so the test show always has something that
//! plays. Downloads resume, retry and can be cancelled; the first network
//! failure of a run stops further attempts, so being offline costs seconds.

use super::Level;
use crate::media::ffmpeg::Ffmpeg;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Video,
    Image,
    /// Sound taken out of a video that has some.
    Audio,
}

/// One downloadable file (or, for `Audio`, one made from a downloaded video).
pub struct Asset {
    /// A short name, used in the show and the log.
    pub id: &'static str,
    /// Its name in the cache folder.
    pub file: &'static str,
    pub kind: Kind,
    /// Where to get it (empty for `Audio`, which comes from `from`).
    pub url: &'static str,
    pub from: Option<&'static str>,
    pub bytes: u64,
    pub sha256: &'static str,
    pub title: &'static str,
    pub credit: &'static str,
    pub licence: &'static str,
    /// Which levels use it (bit 1 Short, 2 Medium, 4 Long).
    pub levels: u8,
}

const BBB: &str = "Big Buck Bunny, (c) Blender Foundation | peach.blender.org";
const SINTEL: &str = "Sintel, (c) Blender Foundation | durian.blender.org";
const JELLY: &str = "Jellyfish clip, test-videos.co.uk";
const CCBY: &str = "Creative Commons Attribution 3.0";
const NASA: &str = "NASA image, public domain";
const TV: &str = "https://test-videos.co.uk/vids";

pub static CATALOG: &[Asset] = &[
    Asset {
        id: "bbb_60",
        file: "bbb_1080_60fps_h264.mp4",
        kind: Kind::Video,
        url: "https://test-videos.co.uk/vids/bigbuckbunny/mp4/h264/1080/Big_Buck_Bunny_1080_10s_10MB.mp4",
        from: None,
        bytes: 10_484_984,
        sha256: "1bb2a62abfcb5c4bf2e9216b63ba715b358f22c9fa8e68ca7149c4788b906826",
        title: "Big Buck Bunny, 1080p 60 fps, 10 s",
        credit: BBB,
        licence: CCBY,
        levels: 7,
    },
    Asset {
        id: "sintel_24",
        file: "sintel_1080_24fps_h264.mp4",
        kind: Kind::Video,
        url: "https://test-videos.co.uk/vids/sintel/mp4/h264/1080/Sintel_1080_10s_10MB.mp4",
        from: None,
        bytes: 10_485_062,
        sha256: "6904625485a36ac11177c4fbb24365273ec5385608009a6b3368bdb071ccde7f",
        title: "Sintel, 1080p 24 fps, 10 s",
        credit: SINTEL,
        licence: CCBY,
        levels: 7,
    },
    Asset {
        id: "jelly_30",
        file: "jellyfish_1080_2997fps_h264.mp4",
        kind: Kind::Video,
        url: "https://test-videos.co.uk/vids/jellyfish/mp4/h264/1080/Jellyfish_1080_10s_10MB.mp4",
        from: None,
        bytes: 10_484_793,
        sha256: "7436a25922a296eabb5e536aa3e4420802b33e843fc2f20a2d8f4f7d4bb79a99",
        title: "Jellyfish, 1080p 29.97 fps, 10 s",
        credit: JELLY,
        licence: "Free to use (test-videos.co.uk)",
        levels: 7,
    },
    Asset {
        id: "sintel_hi",
        file: "sintel_1080_24fps_h264_30mb.mp4",
        kind: Kind::Video,
        url: "https://test-videos.co.uk/vids/sintel/mp4/h264/1080/Sintel_1080_10s_30MB.mp4",
        from: None,
        bytes: 31_455_365,
        sha256: "312e8f83a424b56c3fce030806387124354b9dc676e0f77c724235397c0843f6",
        title: "Sintel, 1080p high bitrate (25 Mbit/s), 10 s",
        credit: SINTEL,
        licence: CCBY,
        levels: 4,
    },
    Asset {
        id: "bbb_hevc",
        file: "bbb_1080_30fps_hevc.mp4",
        kind: Kind::Video,
        url: "https://test-videos.co.uk/vids/bigbuckbunny/mp4/h265/1080/Big_Buck_Bunny_1080_10s_10MB.mp4",
        from: None,
        bytes: 10_547_944,
        sha256: "571e07bb108a5445fea2be90c6f4e2f04b15a073309dade6b58ae0f292cc6092",
        title: "Big Buck Bunny, 1080p H.265, 10 s",
        credit: BBB,
        licence: CCBY,
        levels: 4,
    },
    Asset {
        id: "jelly_hevc",
        file: "jellyfish_1080_30fps_hevc.mp4",
        kind: Kind::Video,
        url: "https://test-videos.co.uk/vids/jellyfish/mp4/h265/1080/Jellyfish_1080_10s_10MB.mp4",
        from: None,
        bytes: 10_587_822,
        sha256: "1171f90fa0c34220936531341a30694e1839372c3a8c683a338140ad82309062",
        title: "Jellyfish, 1080p H.265, 10 s",
        credit: JELLY,
        licence: "Free to use (test-videos.co.uk)",
        levels: 4,
    },
    Asset {
        id: "sintel_trailer",
        file: "sintel_trailer_720p.mp4",
        kind: Kind::Video,
        url: "https://download.blender.org/durian/trailer/sintel_trailer-720p.mp4",
        from: None,
        bytes: 7_608_204,
        sha256: "cb0fe73fc0a7d543459996c0cdab730997e6eac1013d3ede18796f777cb7f273",
        title: "Sintel trailer, 720p with stereo sound, 52 s",
        credit: SINTEL,
        licence: CCBY,
        levels: 7,
    },
    Asset {
        id: "bbb_trailer",
        file: "bbb_trailer_480p.mov",
        kind: Kind::Video,
        url: "https://download.blender.org/peach/trailer/trailer_480p.mov",
        from: None,
        bytes: 11_061_011,
        sha256: "36801b74638c12be9aa587e93cd18edfc9bc51a1c089ab2a19ee42beed9f497d",
        title: "Big Buck Bunny trailer, 480p with 5.1 sound, 33 s",
        credit: BBB,
        licence: CCBY,
        levels: 7,
    },
    Asset {
        id: "sintel_sound",
        file: "sintel_trailer_stereo.wav",
        kind: Kind::Audio,
        url: "",
        from: Some("sintel_trailer"),
        bytes: 0,
        sha256: "",
        title: "Sintel trailer sound, stereo",
        credit: SINTEL,
        licence: CCBY,
        levels: 7,
    },
    Asset {
        id: "apollo11",
        file: "apollo11_as11-40-5874.jpg",
        kind: Kind::Image,
        url: "https://images-assets.nasa.gov/image/as11-40-5874/as11-40-5874~orig.jpg",
        from: None,
        bytes: 2_236_993,
        sha256: "9c93e64991da8c71352e8290f4faa57f1239bd87d74f956a8489a93d7201c8bb",
        title: "Apollo 11: Buzz Aldrin on the Moon (3922 x 3882)",
        credit: "NASA, Apollo 11, AS11-40-5874",
        licence: NASA,
        levels: 7,
    },
    Asset {
        id: "bluemarble",
        file: "apollo17_as17-148-22727.jpg",
        kind: Kind::Image,
        url: "https://images-assets.nasa.gov/image/as17-148-22727/as17-148-22727~orig.jpg",
        from: None,
        bytes: 1_915_552,
        sha256: "cb4c3d2da98ee72130ebfd2ea472fc84ede6726002c2a8c9cabd86e4deea9f0f",
        title: "Apollo 17: the Earth, \"Blue Marble\" (4579 x 4579)",
        credit: "NASA, Apollo 17, AS17-148-22727",
        licence: NASA,
        levels: 7,
    },
    Asset {
        id: "earthrise",
        file: "apollo8_as08-14-2383.jpg",
        kind: Kind::Image,
        url: "https://images-assets.nasa.gov/image/as08-14-2383/as08-14-2383~orig.jpg",
        from: None,
        bytes: 494_374,
        sha256: "4b1484e9bb55652e30a56c03c27141938885a09e4b3c0bcabb8841e1e94ed9d9",
        title: "Apollo 8: Earth over the horizon of the Moon (3000 x 3000)",
        credit: "NASA, Apollo 8, AS08-14-2383",
        licence: NASA,
        levels: 6,
    },
    Asset {
        id: "curiosity",
        file: "curiosity_PIA19808.jpg",
        kind: Kind::Image,
        url: "https://images-assets.nasa.gov/image/PIA19808/PIA19808~orig.jpg",
        from: None,
        bytes: 1_609_971,
        sha256: "cdbf121382a2a77777564aaaf2db11d81303c14cd29f56465ef38517e1500412",
        title: "Curiosity rover, Buckskin selfie (4841 x 2949)",
        credit: "NASA/JPL-Caltech/MSSS, PIA19808",
        licence: NASA,
        levels: 6,
    },
    Asset {
        id: "mars",
        file: "mars_PIA00407.jpg",
        kind: Kind::Image,
        url: "https://images-assets.nasa.gov/image/PIA00407/PIA00407~orig.jpg",
        from: None,
        bytes: 3_642_865,
        sha256: "076d0be8f7cf603f05e8b7281709400d9d39f21c6f615e8e385b8dfffc9bafdd",
        title: "Mars, global colour view (6787 x 6787)",
        credit: "NASA/JPL/USGS, PIA00407",
        licence: NASA,
        levels: 4,
    },
    Asset {
        id: "andromeda",
        file: "andromeda_PIA15416.jpg",
        kind: Kind::Image,
        url: "https://images-assets.nasa.gov/image/PIA15416/PIA15416~orig.jpg",
        from: None,
        bytes: 6_756_530,
        sha256: "88ed2cf29704895a32427c02be2daea6204f113e30f11aac4ff789c0de29a87d",
        title: "The Andromeda galaxy (9400 x 7000)",
        credit: "NASA/JPL-Caltech, PIA15416",
        licence: NASA,
        levels: 4,
    },
];

// The unused constant keeps the host in one place for the credits page.
#[allow(dead_code)]
const _HOSTS: &str = TV;

pub fn find(id: &str) -> Option<&'static Asset> {
    CATALOG.iter().find(|a| a.id == id)
}

/// The assets a level uses, with those they are made from.
pub fn wanted(level: Level) -> Vec<&'static Asset> {
    CATALOG
        .iter()
        .filter(|a| a.levels & level.bit() != 0)
        .collect()
}

/// Bytes to download for the files of `assets` that aren't in the cache yet.
pub fn download_size(dir: &Path, assets: &[&Asset]) -> u64 {
    assets
        .iter()
        .filter(|a| a.kind != Kind::Audio && !is_cached(dir, a))
        .map(|a| a.bytes)
        .sum()
}

pub fn is_cached(dir: &Path, a: &Asset) -> bool {
    std::fs::metadata(dir.join(a.file)).is_ok_and(|m| m.is_file() && m.len() > 0)
}

/// Where the downloads are kept.
pub fn cache_dir(log_dir: &Path) -> PathBuf {
    log_dir.join("test-mode").join("media")
}

/// What the credits say, for the log and the window.
pub fn credits(assets: &[&Asset]) -> String {
    let mut out = String::new();
    for a in assets {
        out += &format!("{}\n    {} | {}\n", a.title, a.credit, a.licence);
    }
    out
}

// ----- What a fetch has done -----------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    /// Already in the cache from an earlier run.
    Cached,
    Downloaded,
    /// Made with FFmpeg, and why the real one could not be used.
    Generated(String),
}

impl Source {
    pub fn label(&self) -> &'static str {
        match self {
            Source::Cached => "ready",
            Source::Downloaded => "downloaded",
            Source::Generated(_) => "generated",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Waiting,
    Downloading { done: u64, total: u64 },
    Checking,
    Ready(Source),
}

/// A file of the fetch, as the window lists it.
#[derive(Clone, Debug)]
pub struct Item {
    pub id: &'static str,
    pub title: &'static str,
    pub bytes: u64,
    pub state: State,
}

/// What the fetch knows about a file it has made ready.
#[derive(Clone, Debug)]
pub struct Resolved {
    pub path: PathBuf,
    pub source: Source,
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub audio_channels: u16,
    pub codec: String,
}

#[derive(Default)]
pub struct Status {
    pub items: Vec<Item>,
    /// Said as it happens, newest last: what the window shows below the list.
    pub notes: Vec<String>,
    pub offline: bool,
    pub finished: bool,
    pub cancelled: bool,
    pub resolved: HashMap<&'static str, Resolved>,
}

pub struct Fetch {
    pub status: Arc<Mutex<Status>>,
    cancel: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Fetch {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn finished(&self) -> bool {
        self.status.lock().finished
    }
}

impl Drop for Fetch {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Make the files of `assets` ready in `dir` on a thread of its own: download
/// what is missing if `online`, and make anything that can't be got.
pub fn start(ff: Ffmpeg, dir: PathBuf, assets: Vec<&'static Asset>, online: bool) -> Fetch {
    let status = Arc::new(Mutex::new(Status {
        items: assets
            .iter()
            .map(|a| Item {
                id: a.id,
                title: a.title,
                bytes: a.bytes,
                state: State::Waiting,
            })
            .collect(),
        ..Status::default()
    }));
    let cancel = Arc::new(AtomicBool::new(false));
    let (st, c) = (status.clone(), cancel.clone());
    let thread = std::thread::Builder::new()
        .name("test-mode-media".into())
        .spawn(move || {
            run(&ff, &dir, &assets, online, &st, &c);
            st.lock().finished = true;
        })
        .ok();
    if thread.is_none() {
        let mut s = status.lock();
        s.notes.push("Could not start the download thread.".into());
        s.finished = true;
    }
    Fetch {
        status,
        cancel,
        thread,
    }
}

fn note(status: &Mutex<Status>, text: impl Into<String>) {
    let text = text.into();
    log::info!("test mode media: {text}");
    status.lock().notes.push(text);
}

fn set_state(status: &Mutex<Status>, id: &str, state: State) {
    if let Some(i) = status.lock().items.iter_mut().find(|i| i.id == id) {
        i.state = state;
    }
}

fn run(
    ff: &Ffmpeg,
    dir: &Path,
    assets: &[&'static Asset],
    mut online: bool,
    status: &Mutex<Status>,
    cancel: &AtomicBool,
) {
    if let Err(e) = std::fs::create_dir_all(dir) {
        note(
            status,
            format!(
                "Can't use {} ({e}): the test media is made fresh each time.",
                dir.display()
            ),
        );
    }
    if !online {
        status.lock().offline = true;
        note(
            status,
            "Not downloading: the media is made with FFmpeg instead.",
        );
    }
    // Videos first: the sound is taken out of two of them.
    let (derived, plain): (Vec<&'static Asset>, Vec<&'static Asset>) =
        assets.iter().copied().partition(|a| a.from.is_some());
    for a in plain.into_iter().chain(derived) {
        if cancel.load(Ordering::Relaxed) {
            status.lock().cancelled = true;
            return;
        }
        let resolved = if let Some(from) = a.from {
            derive(ff, dir, a, from, status)
        } else {
            fetch_one(ff, dir, a, &mut online, status, cancel)
        };
        match resolved {
            Some(r) => {
                set_state(status, a.id, State::Ready(r.source.clone()));
                status.lock().resolved.insert(a.id, r);
            }
            None => {
                if cancel.load(Ordering::Relaxed) {
                    status.lock().cancelled = true;
                    return;
                }
                note(
                    status,
                    format!(
                        "{}: could not be made ready; the test goes without it.",
                        a.title
                    ),
                );
                set_state(
                    status,
                    a.id,
                    State::Ready(Source::Generated("unavailable".into())),
                );
            }
        }
    }
}

/// One downloaded file: from the cache, the network, or generated.
fn fetch_one(
    ff: &Ffmpeg,
    dir: &Path,
    a: &'static Asset,
    online: &mut bool,
    status: &Mutex<Status>,
    cancel: &AtomicBool,
) -> Option<Resolved> {
    let path = dir.join(a.file);
    let why;
    set_state(status, a.id, State::Checking);
    if is_cached(dir, a) {
        match check(ff, &path, a.kind) {
            Ok(r) => return Some(with_source(r, path, Source::Cached)),
            Err(e) => {
                note(
                    status,
                    format!(
                        "{}: the kept copy is no good ({e}); getting it again.",
                        a.title
                    ),
                );
                let _ = std::fs::remove_file(&path);
            }
        }
    }
    if *online {
        match download_with_retries(a, &path, status, cancel) {
            Ok(()) => {
                set_state(status, a.id, State::Checking);
                match check(ff, &path, a.kind) {
                    Ok(r) => {
                        let sum = sha256_of(&path).unwrap_or_default();
                        if !a.sha256.is_empty() && sum != a.sha256 {
                            note(
                                status,
                                format!(
                                    "{}: the file differs from the one this version of NewQ was made with (it may have been updated); it plays, so it is used.",
                                    a.title
                                ),
                            );
                        }
                        return Some(with_source(r, path, Source::Downloaded));
                    }
                    Err(e) => {
                        why = format!("the download can't be used by this FFmpeg: {e}");
                        let _ = std::fs::remove_file(&path);
                    }
                }
            }
            Err(DownloadError::Cancelled) => return None,
            Err(DownloadError::Network(e)) => {
                *online = false;
                status.lock().offline = true;
                why = format!("no network: {e}");
                note(
                    status,
                    "Can't reach the download sites; the rest of the media is made with FFmpeg instead.",
                );
            }
            Err(DownloadError::Other(e)) => why = e,
        }
    } else {
        why = "not downloaded".into();
    }
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    set_state(status, a.id, State::Checking);
    match generate(ff, dir, a) {
        Ok(p) => match check(ff, &p, a.kind) {
            Ok(r) => Some(with_source(r, p, Source::Generated(why))),
            Err(e) => {
                note(
                    status,
                    format!("{}: the generated file is no good either ({e}).", a.title),
                );
                None
            }
        },
        Err(e) => {
            note(
                status,
                format!("{}: could not be generated ({e}).", a.title),
            );
            None
        }
    }
}

fn with_source(mut r: Resolved, path: PathBuf, source: Source) -> Resolved {
    r.path = path;
    r.source = source;
    r
}

/// The sound of a downloaded video, as a WAV file.
fn derive(
    ff: &Ffmpeg,
    dir: &Path,
    a: &'static Asset,
    from: &str,
    status: &Mutex<Status>,
) -> Option<Resolved> {
    let path = dir.join(a.file);
    set_state(status, a.id, State::Checking);
    if is_cached(dir, a)
        && let Ok(r) = check(ff, &path, Kind::Audio)
    {
        return Some(with_source(r, path, Source::Cached));
    }
    let source = status.lock().resolved.get(from).cloned();
    let result = match source {
        Some(r) if r.audio_channels > 0 && !matches!(r.source, Source::Generated(_)) => {
            let tmp = dir.join(format!("partial_{}", a.file));
            let out = ff
                .command()
                .args(["-y", "-i"])
                .arg(&r.path)
                .args(["-vn", "-c:a", "pcm_s16le"])
                .arg(&tmp)
                .output();
            match out {
                Ok(o) if o.status.success() => std::fs::rename(&tmp, &path)
                    .map_err(|e| e.to_string())
                    .and_then(|()| check(ff, &path, Kind::Audio))
                    .map(|r| with_source(r, path.clone(), Source::Downloaded)),
                Ok(o) => {
                    let _ = std::fs::remove_file(&tmp);
                    Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
                }
                Err(e) => Err(e.to_string()),
            }
        }
        _ => Err("the video it comes from is not available".into()),
    };
    match result {
        Ok(r) => Some(r),
        Err(why) => {
            note(
                status,
                format!("{}: made with FFmpeg instead ({why}).", a.title),
            );
            match generate(ff, dir, a).and_then(|p| {
                check(ff, &p, Kind::Audio).map(|r| with_source(r, p, Source::Generated(why)))
            }) {
                Ok(r) => Some(r),
                Err(e) => {
                    note(
                        status,
                        format!("{}: could not be generated ({e}).", a.title),
                    );
                    None
                }
            }
        }
    }
}

// ----- Checking a file ------------------------------------------------------------

/// Whether FFmpeg can read and decode `path` as `kind`, and what it is.
pub fn check(ff: &Ffmpeg, path: &Path, kind: Kind) -> Result<Resolved, String> {
    let info = ff.probe(path).map_err(|e| format!("{e:#}"))?;
    let (width, height, fps, codec) = info.video.as_ref().map_or((0, 0, 0.0, String::new()), |v| {
        (v.width, v.height, v.fps, v.codec.clone())
    });
    match kind {
        Kind::Video if info.video.is_none() => return Err("no picture in it".into()),
        Kind::Image if info.video.is_none() => return Err("not a picture".into()),
        Kind::Audio if !info.has_audio => return Err("no sound in it".into()),
        _ => {}
    }
    let duration = info.duration.unwrap_or(0.0);
    if kind == Kind::Video && duration < 1.0 {
        return Err(format!("only {duration:.1} s long"));
    }
    // Decode a little of it: a codec this FFmpeg lacks, or a damaged file,
    // fails here and not in the middle of the test.
    let mut cmd = ff.command();
    cmd.args(["-v", "error", "-i"]).arg(path);
    match kind {
        Kind::Audio => cmd.args(["-t", "2", "-vn", "-f", "null", "-"]),
        _ => cmd.args(["-frames:v", "10", "-an", "-f", "null", "-"]),
    };
    let out = cmd.output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        let text = String::from_utf8_lossy(&out.stderr);
        let first = text
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("it does not decode");
        return Err(first.trim().to_string());
    }
    Ok(Resolved {
        path: path.to_path_buf(),
        source: Source::Cached,
        duration,
        width,
        height,
        fps,
        audio_channels: info.audio_channels,
        codec,
    })
}

fn sha256_of(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

// ----- Downloading ------------------------------------------------------------------

#[derive(Debug)]
pub enum DownloadError {
    Cancelled,
    /// Could not reach the server at all (no point trying the others).
    Network(String),
    /// The server answered, but not with the file.
    Other(String),
}

const ATTEMPTS: u32 = 3;

fn download_with_retries(
    a: &'static Asset,
    dest: &Path,
    status: &Mutex<Status>,
    cancel: &AtomicBool,
) -> Result<(), DownloadError> {
    let mut last = DownloadError::Other("not tried".into());
    for attempt in 1..=ATTEMPTS {
        if cancel.load(Ordering::Relaxed) {
            return Err(DownloadError::Cancelled);
        }
        let progress = |done: u64, total: u64| {
            set_state(status, a.id, State::Downloading { done, total });
        };
        match download(a.url, dest, a.bytes, cancel, &progress) {
            Ok(()) => return Ok(()),
            Err(DownloadError::Cancelled) => return Err(DownloadError::Cancelled),
            // Not reachable: another attempt would only wait again.
            Err(e @ DownloadError::Network(_)) => return Err(e),
            Err(e) => {
                note(
                    status,
                    format!(
                        "{}: attempt {attempt} of {ATTEMPTS} failed ({}).",
                        a.title,
                        describe(&e)
                    ),
                );
                last = e;
                if attempt < ATTEMPTS {
                    // Short pauses, but watch for the cancel button.
                    let until = Instant::now() + Duration::from_secs(u64::from(attempt) * 2);
                    while Instant::now() < until && !cancel.load(Ordering::Relaxed) {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }
        }
    }
    Err(last)
}

fn describe(e: &DownloadError) -> String {
    match e {
        DownloadError::Cancelled => "cancelled".into(),
        DownloadError::Network(s) | DownloadError::Other(s) => s.clone(),
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(20)))
        .timeout_global(Some(Duration::from_secs(20 * 60)))
        .user_agent(concat!(
            "NewQ/",
            env!("CARGO_PKG_VERSION"),
            " (test mode; https://github.com/r2a42/NewQ)"
        ))
        .build()
        .into()
}

/// Get `url` into `dest` (through `dest.part`, which a later try resumes from).
/// `expected` is the size the catalog knows; the file must be about that long
/// when the server says its length, and the whole body must arrive.
pub fn download(
    url: &str,
    dest: &Path,
    expected: u64,
    cancel: &AtomicBool,
    progress: &dyn Fn(u64, u64),
) -> Result<(), DownloadError> {
    let part = dest.with_extension(format!(
        "{}.part",
        dest.extension().and_then(|e| e.to_str()).unwrap_or("dl")
    ));
    let have = std::fs::metadata(&part).map_or(0, |m| m.len());
    let agent = agent();
    let mut req = agent.get(url);
    if have > 0 {
        req = req.header("Range", format!("bytes={have}-"));
    }
    let resp = req.call().map_err(|e| match e {
        ureq::Error::StatusCode(code) => {
            DownloadError::Other(format!("the server answered {code}"))
        }
        // Anything else is the network: DNS, connect, TLS, timeout.
        other => DownloadError::Network(other.to_string()),
    })?;
    let status = resp.status().as_u16();
    // 206: the rest of the file. 200: all of it, from the start.
    let (mut file, mut done) = if status == 206 && have > 0 {
        let f = std::fs::OpenOptions::new()
            .append(true)
            .open(&part)
            .map_err(|e| DownloadError::Other(format!("can't write {}: {e}", part.display())))?;
        (f, have)
    } else {
        let f = std::fs::File::create(&part)
            .map_err(|e| DownloadError::Other(format!("can't write {}: {e}", part.display())))?;
        (f, 0)
    };
    let length = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());
    let total = length.map_or(expected, |l| l + done);
    let mut body = resp.into_body().into_reader();
    let mut buf = vec![0u8; 64 * 1024];
    let mut last_report = Instant::now();
    progress(done, total);
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(DownloadError::Cancelled);
        }
        let n = body
            .read(&mut buf)
            .map_err(|e| DownloadError::Other(format!("the download broke off: {e}")))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| DownloadError::Other(format!("can't write {}: {e}", part.display())))?;
        done += n as u64;
        if last_report.elapsed() > Duration::from_millis(100) {
            last_report = Instant::now();
            progress(done, total);
        }
    }
    file.flush().ok();
    drop(file);
    if total > 0 && length.is_some() && done != total {
        return Err(DownloadError::Other(format!(
            "only {done} of {total} bytes arrived"
        )));
    }
    if done == 0 {
        let _ = std::fs::remove_file(&part);
        return Err(DownloadError::Other("the server sent nothing".into()));
    }
    progress(done, done);
    std::fs::rename(&part, dest)
        .map_err(|e| DownloadError::Other(format!("can't keep {}: {e}", dest.display())))
}

// ----- Making them when they can't be got -------------------------------------------------

/// The file name a generated stand-in has: never the real one's, so a
/// download later is not shadowed by it.
fn generated_name(a: &Asset) -> String {
    let stem = Path::new(a.file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(a.id);
    let ext = match a.kind {
        Kind::Video => "mp4",
        Kind::Image => "png",
        Kind::Audio => "wav",
    };
    format!("generated_{stem}.{ext}")
}

fn generate(ff: &Ffmpeg, dir: &Path, a: &Asset) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let out = dir.join(generated_name(a));
    if out.is_file() && check(ff, &out, a.kind).is_ok() {
        return Ok(out);
    }
    let tmp = dir.join(format!("partial_{}", generated_name(a)));
    let seed =
        a.id.bytes()
            .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    let mut args: Vec<String> = Vec::new();
    fn add(args: &mut Vec<String>, s: &[&str]) {
        args.extend(s.iter().map(|x| x.to_string()));
    }
    match a.kind {
        Kind::Video => {
            // A distinct moving pattern and frame rate for each, and 10 s,
            // like the clips they stand in for.
            let (pattern, fps) = match a.id {
                "bbb_60" => ("testsrc2=size=1920x1080:rate=60", 60),
                "sintel_24" | "sintel_hi" | "sintel_trailer" => {
                    ("smptehdbars=size=1920x1080:rate=24", 24)
                }
                "jelly_30" => ("mandelbrot=size=1920x1080:rate=30000/1001", 30),
                "bbb_trailer" => ("testsrc2=size=1280x720:rate=25", 25),
                _ => ("rgbtestsrc=size=1920x1080:rate=30", 30),
            };
            let _ = fps;
            add(&mut args, &["-f", "lavfi", "-i", pattern]);
            if matches!(a.id, "sintel_trailer" | "bbb_trailer") {
                add(
                    &mut args,
                    &[
                        "-f",
                        "lavfi",
                        "-i",
                        "sine=frequency=330:sample_rate=48000",
                        "-ac",
                        "2",
                    ],
                );
            }
            add(&mut args, &["-t", "12"]);
            let h264 = crate::soak::h264_args(|n| ff.has_encoder(n));
            args.extend(h264);
            if matches!(a.id, "sintel_trailer" | "bbb_trailer") {
                add(&mut args, &["-c:a", "aac", "-shortest"]);
            }
        }
        Kind::Image => {
            add(
                &mut args,
                &[
                    "-f",
                    "lavfi",
                    "-i",
                    &format!(
                        "gradients=size=3840x2160:seed={}:duration=1:speed=0.00001",
                        seed % 10_000
                    ),
                    "-frames:v",
                    "1",
                ],
            );
        }
        Kind::Audio => {
            let music = "aevalsrc=0.3*sin(2*PI*(220+40*sin(2*PI*t/7))*t)|0.3*sin(2*PI*(330+30*sin(2*PI*t/5))*t):s=48000:d=52";
            add(
                &mut args,
                &["-f", "lavfi", "-i", music, "-c:a", "pcm_s16le"],
            );
        }
    }
    let result = ff
        .command()
        .args(["-y"])
        .args(&args)
        .arg(&tmp)
        .output()
        .map_err(|e| e.to_string())?;
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
    use std::net::TcpListener;

    #[derive(Clone, Copy, PartialEq)]
    enum Mode {
        /// The whole file, honouring Range.
        Normal,
        /// A Range header is ignored: the whole file comes with 200.
        IgnoreRange,
        /// Says it will send everything, sends half, and hangs up.
        Truncate,
        NotFound,
    }

    /// A tiny HTTP server on loopback for the tests.
    struct Server {
        url: String,
        mode: Arc<Mutex<Mode>>,
        stop: Arc<AtomicBool>,
        hits: Arc<Mutex<Vec<Option<String>>>>,
    }

    impl Server {
        fn start(body: Vec<u8>) -> Server {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let url = format!("http://{}/file.bin", listener.local_addr().unwrap());
            let mode = Arc::new(Mutex::new(Mode::Normal));
            let stop = Arc::new(AtomicBool::new(false));
            let hits = Arc::new(Mutex::new(Vec::new()));
            let (m, s, h) = (mode.clone(), stop.clone(), hits.clone());
            std::thread::spawn(move || {
                while !s.load(Ordering::Relaxed) {
                    let Ok((mut conn, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    conn.set_nonblocking(false).ok();
                    conn.set_read_timeout(Some(Duration::from_secs(5))).ok();
                    let mut head = Vec::new();
                    let mut b = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") {
                        if conn.read(&mut b).unwrap_or(0) == 0 {
                            break;
                        }
                        head.push(b[0]);
                    }
                    let text = String::from_utf8_lossy(&head).to_string();
                    let range = text
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("range: ")
                                .or_else(|| l.strip_prefix("Range: "))
                        })
                        .map(|r| r.trim().to_string());
                    h.lock().push(range.clone());
                    let mode = *m.lock();
                    let start = match (&range, mode) {
                        (Some(r), Mode::Normal) => r
                            .trim_start_matches("bytes=")
                            .trim_end_matches('-')
                            .parse::<usize>()
                            .unwrap_or(0),
                        _ => 0,
                    };
                    let reply = |code: &str, payload: &[u8], claimed: usize| {
                        let mut out = format!(
                            "HTTP/1.1 {code}\r\nContent-Length: {claimed}\r\nConnection: close\r\n\r\n"
                        )
                        .into_bytes();
                        out.extend_from_slice(payload);
                        out
                    };
                    let out = match mode {
                        Mode::NotFound => reply("404 Not Found", b"no", 2),
                        Mode::Truncate => {
                            let half = &body[..body.len() / 2];
                            reply("200 OK", half, body.len())
                        }
                        _ if start > 0 => {
                            reply("206 Partial Content", &body[start..], body.len() - start)
                        }
                        _ => reply("200 OK", &body, body.len()),
                    };
                    let _ = conn.write_all(&out);
                }
            });
            Server {
                url,
                mode,
                stop,
                hits,
            }
        }

        fn set(&self, mode: Mode) {
            *self.mode.lock() = mode;
        }
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("newq_assets_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn body(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 7 % 251) as u8).collect()
    }

    #[test]
    fn the_catalog_is_complete_and_consistent() {
        let mut ids = std::collections::HashSet::new();
        let mut files = std::collections::HashSet::new();
        for a in CATALOG {
            assert!(ids.insert(a.id), "{} twice", a.id);
            assert!(files.insert(a.file), "{} twice", a.file);
            assert!(!a.title.is_empty() && !a.credit.is_empty() && !a.licence.is_empty());
            assert_ne!(a.levels & 7, 0, "{} is in no level", a.id);
            match a.kind {
                Kind::Audio => {
                    let from = find(a.from.expect("sound comes from a video")).unwrap();
                    assert_eq!(from.kind, Kind::Video);
                    assert_eq!(
                        a.levels & !from.levels,
                        0,
                        "{}: its video is not in its levels",
                        a.id
                    );
                }
                _ => {
                    assert!(a.url.starts_with("https://"), "{}", a.id);
                    assert!(a.bytes > 100_000, "{}", a.id);
                    assert_eq!(a.sha256.len(), 64, "{}", a.id);
                    assert!(a.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
                }
            }
        }
        // Each level has pictures, video and sound, and each gets bigger.
        let mut last = 0;
        for level in Level::ALL {
            let w = wanted(level);
            for kind in [Kind::Video, Kind::Image, Kind::Audio] {
                assert!(
                    w.iter().any(|a| a.kind == kind),
                    "{level:?} has no {kind:?}"
                );
            }
            let size: u64 = w.iter().map(|a| a.bytes).sum();
            assert!(size > last, "{level:?} is not bigger than the one before");
            last = size;
        }
        assert!(credits(&wanted(Level::Long)).contains("NASA"));
    }

    #[test]
    fn a_file_is_downloaded_whole_with_progress() {
        let data = body(300_000);
        let server = Server::start(data.clone());
        let dir = scratch("whole");
        let dest = dir.join("x.bin");
        let seen = Mutex::new(Vec::new());
        download(&server.url, &dest, 0, &AtomicBool::new(false), &|d, t| {
            seen.lock().push((d, t))
        })
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert!(!dir.join("x.bin.part").exists());
        let seen = seen.lock();
        assert_eq!(*seen.last().unwrap(), (300_000, 300_000));
        assert!(seen.iter().all(|(d, t)| d <= t));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_broken_download_is_resumed_from_where_it_stopped() {
        let data = body(300_000);
        let server = Server::start(data.clone());
        let dir = scratch("resume");
        let dest = dir.join("x.bin");
        server.set(Mode::Truncate);
        let err = download(&server.url, &dest, 0, &AtomicBool::new(false), &|_, _| {}).unwrap_err();
        assert!(matches!(err, DownloadError::Other(_)), "{err:?}");
        assert!(!dest.exists(), "a half file is never taken for the file");
        let part = dir.join("x.bin.part");
        assert_eq!(std::fs::metadata(&part).unwrap().len(), 150_000);
        // The server comes back: the rest is asked for, not the whole.
        server.set(Mode::Normal);
        download(&server.url, &dest, 0, &AtomicBool::new(false), &|_, _| {}).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        assert_eq!(
            server.hits.lock().last().unwrap().as_deref(),
            Some("bytes=150000-")
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_server_that_ignores_the_range_gets_the_file_started_again() {
        let data = body(100_000);
        let server = Server::start(data.clone());
        let dir = scratch("norange");
        let dest = dir.join("x.bin");
        std::fs::write(dir.join("x.bin.part"), vec![9u8; 40_000]).unwrap();
        server.set(Mode::IgnoreRange);
        download(&server.url, &dest, 0, &AtomicBool::new(false), &|_, _| {}).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_missing_file_a_dead_server_and_a_cancel_are_told_apart() {
        let server = Server::start(body(1000));
        let dir = scratch("errors");
        server.set(Mode::NotFound);
        let e = download(
            &server.url,
            &dir.join("a"),
            0,
            &AtomicBool::new(false),
            &|_, _| {},
        )
        .unwrap_err();
        assert!(
            matches!(&e, DownloadError::Other(s) if s.contains("404")),
            "{e:?}"
        );
        // Nothing listening: the network, so no retries are worth it.
        let dead = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}/x", l.local_addr().unwrap())
        };
        let e = download(
            &dead,
            &dir.join("b"),
            0,
            &AtomicBool::new(false),
            &|_, _| {},
        )
        .unwrap_err();
        assert!(matches!(e, DownloadError::Network(_)), "{e:?}");
        server.set(Mode::Normal);
        let e = download(
            &server.url,
            &dir.join("c"),
            0,
            &AtomicBool::new(true),
            &|_, _| {},
        )
        .unwrap_err();
        assert!(matches!(e, DownloadError::Cancelled), "{e:?}");
        assert!(!dir.join("c").exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn leak_asset(id: &'static str, file: &'static str, kind: Kind, url: String) -> &'static Asset {
        Box::leak(Box::new(Asset {
            id,
            file,
            kind,
            url: Box::leak(url.into_boxed_str()),
            from: None,
            bytes: 1000,
            sha256: "",
            title: "A test file",
            credit: "test",
            licence: "test",
            levels: 7,
        }))
    }

    fn wait(fetch: &Fetch) {
        let t = Instant::now();
        while !fetch.finished() {
            assert!(
                t.elapsed() < Duration::from_secs(120),
                "the fetch never finished"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn media_is_downloaded_checked_kept_and_replaced_by_generated_when_bad_or_offline() {
        let Some((ff, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let good = std::fs::read(&clip).unwrap();
        let good_server = Server::start(good);
        let junk_server = Server::start(body(50_000));
        let dead = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            format!("http://{}/x", l.local_addr().unwrap())
        };
        let ok = leak_asset("t_ok", "t_ok.mp4", Kind::Video, good_server.url.clone());
        let bad = leak_asset("t_bad", "t_bad.mp4", Kind::Video, junk_server.url.clone());
        let dir = scratch("fetch");
        let fetch = start(ff.clone(), dir.clone(), vec![ok, bad], true);
        wait(&fetch);
        {
            let s = fetch.status.lock();
            assert!(!s.cancelled && !s.offline);
            let r = &s.resolved["t_ok"];
            assert_eq!(r.source, Source::Downloaded);
            assert!(r.duration > 9.0 && r.width == 1280, "{r:?}");
            // Junk that is not a video is not used; a generated clip is.
            let r = &s.resolved["t_bad"];
            assert!(matches!(r.source, Source::Generated(_)), "{r:?}");
            assert!(
                r.path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("generated_")
            );
            assert!(r.duration > 9.0);
        }
        drop(fetch);
        // A second run finds the good one kept, and doesn't ask the server again.
        let before = good_server.hits.lock().len();
        let fetch = start(ff.clone(), dir.clone(), vec![ok], true);
        wait(&fetch);
        assert_eq!(fetch.status.lock().resolved["t_ok"].source, Source::Cached);
        assert_eq!(good_server.hits.lock().len(), before);
        drop(fetch);
        // Offline: everything generated, quickly, and said so.
        let gone = leak_asset("t_off", "t_off.mp4", Kind::Video, dead);
        let img = leak_asset(
            "t_img",
            "t_img.jpg",
            Kind::Image,
            "http://127.0.0.1:9/x".into(),
        );
        let dir2 = scratch("fetch_off");
        let t = Instant::now();
        let fetch = start(ff.clone(), dir2.clone(), vec![gone, img], true);
        wait(&fetch);
        assert!(t.elapsed() < Duration::from_secs(60));
        let s = fetch.status.lock();
        assert!(s.offline);
        assert!(
            s.notes.iter().any(|n| n.contains("Can't reach")),
            "{:?}",
            s.notes
        );
        for id in ["t_off", "t_img"] {
            assert!(
                matches!(s.resolved[id].source, Source::Generated(_)),
                "{id}"
            );
        }
        assert!(s.resolved["t_img"].width >= 1920);
        drop(s);
        drop(fetch);
        // Told not to download at all.
        let fetch = start(ff, scratch("fetch_none"), vec![ok], false);
        wait(&fetch);
        assert!(matches!(
            fetch.status.lock().resolved["t_ok"].source,
            Source::Generated(_)
        ));
        for d in [dir, dir2] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn the_sound_of_a_video_is_taken_out_and_generated_when_there_is_none() {
        let Some((ff, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let server = Server::start(std::fs::read(&clip).unwrap());
        let video = leak_asset("t_av", "t_av.mp4", Kind::Video, server.url.clone());
        let sound: &'static Asset = Box::leak(Box::new(Asset {
            from: Some("t_av"),
            kind: Kind::Audio,
            url: "",
            id: "t_snd",
            file: "t_snd.wav",
            ..clone_asset(video)
        }));
        let dir = scratch("derive");
        let fetch = start(ff.clone(), dir.clone(), vec![sound, video], true);
        wait(&fetch);
        let s = fetch.status.lock();
        let r = &s.resolved["t_snd"];
        assert!(r.audio_channels >= 1 && r.duration > 9.0, "{r:?}");
        assert_eq!(r.source, Source::Downloaded);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Downloads the real files for every level: run by hand with
    /// `cargo test -- --ignored real_downloads` to check the catalog against
    /// the sites (it needs the network and about 140 MB).
    #[test]
    #[ignore]
    fn real_downloads_match_the_catalog() {
        let Some(ff) = Ffmpeg::locate() else {
            panic!("no ffmpeg")
        };
        let dir = scratch("real");
        let fetch = start(ff, dir.clone(), wanted(Level::Long), true);
        wait(&fetch);
        let s = fetch.status.lock();
        for a in wanted(Level::Long) {
            let r = s
                .resolved
                .get(a.id)
                .unwrap_or_else(|| panic!("{} missing: {:?}", a.id, s.notes));
            eprintln!(
                "{:16} {:?} {:?} {}x{} {:.1}s {}ch {}",
                a.id,
                r.source,
                r.codec,
                r.width,
                r.height,
                r.duration,
                r.audio_channels,
                r.path.display()
            );
            assert!(
                !matches!(r.source, Source::Generated(_)),
                "{}: {:?}",
                a.id,
                r.source
            );
            if a.kind != Kind::Audio {
                assert_eq!(
                    sha256_of(&r.path).unwrap(),
                    a.sha256,
                    "{} has changed upstream",
                    a.id
                );
            }
        }
        eprintln!("notes: {:?}", s.notes);
    }

    fn clone_asset(a: &Asset) -> Asset {
        Asset {
            id: a.id,
            file: a.file,
            kind: a.kind,
            url: a.url,
            from: a.from,
            bytes: a.bytes,
            sha256: a.sha256,
            title: a.title,
            credit: a.credit,
            licence: a.licence,
            levels: a.levels,
        }
    }
}
