//! The network test (stage 6 of `.claude/notes/network-plan.md`): Test Mode on
//! a primary with computers in step. It plays a short show of its own on every
//! display of every computer and has each computer measure how it keeps to
//! the primary (`net::measure`), then judges what they sent (`net_judge`).
//!
//! 1. The media is made ready here (`assets`, and the Sync flash clip,
//!    `syncflash`), as the other tests do.
//! 2. The user's show is put aside (`UiState::begin_test`) and the test show
//!    put in its place; show sync takes it to the computers and the media
//!    manager its files. The test waits until every computer has them all
//!    ("all here"), saying how far each has got, and notes each transfer's rate.
//! 3. It tells the computers to measure (`NetTest::Begin`) and plays its
//!    script by the primary's own inputs (GO, fire, a timeline seek, a pause,
//!    Stop All), which reach the computers as timed commands like any other.
//! 4. It tells them it is over (`NetTest::End`), waits a moment for their last
//!    reports and the end of their logs, puts the user's show back (which
//!    show sync then takes to every computer, and to one that was away when it
//!    comes back), writes the run folder and makes the zip.
//!
//! The test show keeps the user's displays and sound outputs (and which
//! computer each is on), adds a display for each monitor of a computer that
//! the show gives none (so every projector can be filmed), and holds the
//! user's own media in a list that never plays, so a computer that keeps only
//! one version of the media doesn't throw the show's away for the test's.
//!
//! Nothing here touches a socket: the messages go through `TestMode::net_in`
//! and `net_out`, which the app moves (`app/play.rs`).

use super::analysis::{Finding, Severity, Verdict};
use super::assets::{self, Asset, Fetch, Resolved};
use super::net_judge::{self, ComputerNotes, Judged, Transfer};
use super::session::{Done, End, unix_now};
use crate::awake::KeepAwake;
use crate::engine::Engine;
use crate::media::ffmpeg::Ffmpeg;
use crate::net::hub::{NetView, PeerView};
use crate::net::measure::NetTest;
use crate::net::media::manager::{MediaReport, MediaView};
use crate::net::node::NodeId;
use crate::net::store::paths::path_key;
use crate::show::*;
use crate::timeline::TimelineKey;
use crate::ui::{TestStash, UiState};
use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The media the test plays (with the Sync flash, made here).
pub const MEDIA: [&str; 5] = [
    "bbb_60",
    "sintel_24",
    "apollo11",
    "sintel_trailer",
    "sintel_sound",
];
/// How long the media may take to reach every computer before the test gives up.
pub const MEDIA_LIMIT: Duration = Duration::from_secs(20 * 60);
/// After the end, the computers' last reports are waited for this long.
pub const ENDING_LIMIT: f64 = 8.0;
/// The soak plays the lap this many times (about 20 minutes).
pub const SOAK_LAPS: u32 = 10;
/// The name the test show goes by (a follower doesn't keep it as its show).
pub const SHOW_NAME: &str = "NewQ network test";
/// The list that holds the user's media, never played.
pub const KEPT_LIST: &str = "Your show's media (kept, not played)";
/// A computer silent this long is said in the window to be away (s).
const AWAY_SECS: f64 = 3.0;

/// Whether `show` is the network test's own (a follower doesn't keep it for
/// its next start).
pub fn is_test_show(show: &Show) -> bool {
    show.name == SHOW_NAME
}

/// Why the network test can't start here, if it can't: it runs on a primary
/// with at least one computer in step.
pub fn refusal(st: &UiState) -> Option<String> {
    if let Some(why) = st.test_refusal_base() {
        return Some(why.to_string());
    }
    let v = &st.net;
    if !(v.me.on && v.is_primary()) {
        return Some(
            "The network test runs on the computer that runs the show network, with at least one other computer joined. Switch on \"Run the show network from this computer\" in Settings ▸ Show network, join the other computers to it, and come back here. (On a computer alone, use the tests above.)".into(),
        );
    }
    if in_step(v).is_empty() {
        return Some(
            "No computer of this show network is in step yet. Wait until the Show network page says \"In step\" for at least one, then start the test.".into(),
        );
    }
    None
}

/// The computers of the network that are connected and in step.
pub fn in_step(v: &NetView) -> Vec<&PeerView> {
    v.peers
        .iter()
        .filter(|p| p.ours && p.connected)
        .filter(|p| p.status.as_ref().is_some_and(|s| s.in_step))
        .collect()
}

/// What the test plays at each moment of a lap.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    /// GO (the first lap) or fire the Sync flash (the later ones).
    Begin,
    Fire(Which),
    Seek(f64),
    Pause(bool),
    StopAll(f64),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Which {
    Film,
    Sound,
    Timeline,
    FlashAgain,
}

/// One lap: about two minutes.
pub fn lap() -> Vec<(f64, Act)> {
    vec![
        (0.0, Act::Begin),
        (25.0, Act::Fire(Which::Film)),
        (25.0, Act::Fire(Which::Sound)),
        (45.0, Act::Fire(Which::Timeline)),
        (55.0, Act::Seek(14.0)),
        (62.0, Act::Pause(true)),
        (66.0, Act::Pause(false)),
        (78.0, Act::StopAll(1.0)),
        (80.0, Act::Fire(Which::FlashAgain)),
        (110.0, Act::StopAll(2.0)),
    ]
}

/// A lap lasts this long (s).
pub const LAP_SECS: f64 = 115.0;

/// The script of the whole test: one lap, or `SOAK_LAPS` of them.
pub fn script(soak: bool) -> Vec<(f64, Act)> {
    let laps = if soak { SOAK_LAPS } else { 1 };
    let mut v = Vec::new();
    for n in 0..laps {
        for (t, a) in lap() {
            v.push((f64::from(n) * LAP_SECS + t, a));
        }
    }
    v
}

/// The cues of the test show.
#[derive(Clone, Copy, Debug)]
pub struct Cues {
    pub flash: CueId,
    pub film: Option<CueId>,
    pub sound: Option<CueId>,
    pub timeline: CueId,
    pub flash_again: CueId,
}

/// The test show, made.
pub struct Built {
    pub show: Show,
    pub cues: Cues,
    /// Each file the test plays (as the show names it) and its size.
    pub files: Vec<(String, u64)>,
    /// What was left out, and displays added.
    pub notes: Vec<String>,
    /// The displays it shows on, by name.
    pub displays: Vec<String>,
}

fn media(c: &mut Cue) -> &mut MediaCue {
    c.kind.media_mut().expect("a cue that plays something")
}

/// Make the test show from the user's: their displays and sound outputs, a
/// display for every monitor of a computer the show gives none, the test's
/// cues, and the user's media in a list that never plays.
pub fn build(
    original: &Show,
    res: &HashMap<&'static str, Resolved>,
    flash: Option<&Path>,
    peers: &[PeerView],
) -> Built {
    let mut show = original.clone();
    show.cues.clear();
    show.lists.clear();
    show.main_list_name = None;
    show.name = SHOW_NAME.into();
    show.own_layers = true;
    let mut notes = Vec::new();
    // A display for each monitor of a computer that has none on it.
    for p in peers.iter().filter(|p| p.ours) {
        let Some(s) = &p.status else { continue };
        for (i, m) in s.monitors.iter().enumerate() {
            let used = show.outputs.iter().any(|o| {
                o.computer == Some(p.id) && o.monitor.as_ref().is_some_and(|r| r.index == i)
            });
            if used {
                continue;
            }
            let id = show.add_display();
            let twins = s
                .monitors
                .iter()
                .filter(|o| o.identity == m.identity)
                .count() as u32;
            if let Some(o) = show.outputs.iter_mut().find(|o| o.id == id) {
                o.name = format!("{} monitor {} (test)", p.name, i + 1);
                o.computer = Some(p.id);
                o.monitor = Some(m.key().reference(i));
                o.twins = twins.max(1);
                notes.push(format!(
                    "{}'s monitor {} ({}) has no display in your show; the test shows on it too",
                    p.name,
                    i + 1,
                    m.identity
                ));
            }
        }
    }
    let pictures: Vec<OutputId> = show
        .outputs
        .iter()
        .filter(|o| o.enabled && !o.is_sub_display() && o.part_of.is_none())
        .map(|o| o.id)
        .collect();
    let displays: Vec<String> = show
        .outputs
        .iter()
        .filter(|o| pictures.contains(&o.id))
        .map(|o| o.name.clone())
        .collect();
    if pictures.is_empty() {
        notes.push("Your show has no displays: the test measures timing and sound only".into());
    }
    let audio = show.audio_outputs.first().map(|a| a.id);
    let mut files: Vec<(String, u64)> = Vec::new();
    let add_file = |files: &mut Vec<(String, u64)>, p: &Path| {
        let s = p.display().to_string();
        if !files.iter().any(|(f, _)| *f == s) {
            let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
            files.push((s, size));
        }
    };
    let on_every = |m: &mut MediaCue| {
        m.output = pictures.first().copied();
        m.also_on = pictures.iter().skip(1).copied().collect();
        m.fit = Fit::Cover;
    };
    let push = |show: &mut Show,
                ty: CueType,
                file: Option<PathBuf>,
                name: &str,
                parent: Option<CueId>,
                tweak: &mut dyn FnMut(&mut Cue)|
     -> CueId {
        let mut c = show.make_cue(ty, file, show.cues.len());
        c.name = name.into();
        c.parent = parent;
        c.list = 0;
        if let Some(m) = c.kind.media_mut() {
            m.audio_output = audio;
        }
        tweak(&mut c);
        let id = c.id;
        show.cues.push(c);
        id
    };
    let film_of = |id: &str| res.get(id).filter(|r| r.path.is_file() || cfg!(test));
    // The flash (or, without it, a film in its place).
    let flash_file: Option<PathBuf> = flash
        .map(Path::to_path_buf)
        .or_else(|| film_of("bbb_60").map(|r| r.path.clone()));
    if flash.is_none() {
        notes.push("The Sync flash clip couldn't be made: a film plays in its place".into());
    }
    let flash_cue = |show: &mut Show, name: &str| {
        push(
            show,
            CueType::Video,
            flash_file.clone(),
            name,
            None,
            &mut |c| {
                let m = media(c);
                on_every(m);
                m.loop_media = true;
                m.layer = 0;
                m.transition = Transition::Cut;
                m.fade_in = 0.0;
                m.volume_db = -60.0;
            },
        )
    };
    let flash_id = flash_cue(&mut show, "Sync flash (film the displays)");
    if let Some(f) = &flash_file {
        add_file(&mut files, f);
    }
    let film = film_of("bbb_60").map(|r| r.path.clone());
    let film_id = film.as_ref().map(|f| {
        add_file(&mut files, f);
        push(
            &mut show,
            CueType::Video,
            Some(f.clone()),
            "Film",
            None,
            &mut |c| {
                let m = media(c);
                on_every(m);
                m.loop_media = true;
                m.layer = 0;
                m.transition = Transition::FadeIn;
                m.fade_in = 1.0;
                m.volume_db = -60.0;
            },
        )
    });
    if film.is_none() {
        notes.push("The film wasn't available, so the test goes without it".into());
    }
    let sound = res.get("sintel_sound").map(|r| r.path.clone());
    let sound_id = sound.as_ref().map(|f| {
        add_file(&mut files, f);
        push(
            &mut show,
            CueType::Audio,
            Some(f.clone()),
            "Sound",
            None,
            &mut |c| {
                let m = media(c);
                m.loop_media = true;
                m.volume_db = -14.0;
                m.fade_in = 1.0;
            },
        )
    });
    if sound.is_none() {
        notes.push("The sound wasn't available, so the test goes without it".into());
    }
    // The timeline: a picture, a film over it, then the flash on top.
    let tl = push(
        &mut show,
        CueType::Group,
        None,
        "Timeline",
        None,
        &mut |c| {
            if let CueKind::Group(g) = &mut c.kind {
                g.mode = GroupMode::Timeline;
                g.timeline = Some(crate::timeline::TimelineSettings::for_editor());
            }
        },
    );
    if let Some(r) = res.get("apollo11") {
        add_file(&mut files, &r.path);
        push(
            &mut show,
            CueType::Image,
            Some(r.path.clone()),
            "Apollo 11",
            Some(tl),
            &mut |c| {
                c.pre_wait = 0.0;
                let m = media(c);
                on_every(m);
                m.end = Some(30.0);
                m.layer = 0;
                m.run_out = crate::timeline::RunOut::Hold;
                m.transition = Transition::Cut;
                m.fade_in = 0.0;
                m.fade_out = 0.0;
            },
        );
    }
    if let Some(r) = res.get("sintel_24") {
        add_file(&mut files, &r.path);
        let len = 8.0f64.min(r.duration.max(1.0));
        push(
            &mut show,
            CueType::Video,
            Some(r.path.clone()),
            "Sintel",
            Some(tl),
            &mut |c| {
                c.pre_wait = 3.0;
                let m = media(c);
                on_every(m);
                m.start = 0.0;
                m.end = Some(len);
                m.layer = 1;
                m.transition = Transition::Cut;
                m.fade_in = 0.0;
                m.fade_out = 0.0;
                m.volume_db = -60.0;
            },
        );
    }
    if let Some(f) = &flash_file {
        push(
            &mut show,
            CueType::Video,
            Some(f.clone()),
            "Sync flash on the timeline",
            Some(tl),
            &mut |c| {
                c.pre_wait = 12.0;
                let m = media(c);
                on_every(m);
                m.loop_media = true;
                m.end = Some(18.0);
                m.layer = 2;
                m.run_out = crate::timeline::RunOut::Hold;
                m.transition = Transition::Cut;
                m.fade_in = 0.0;
                m.fade_out = 0.0;
                m.volume_db = -60.0;
            },
        );
    }
    let again = flash_cue(&mut show, "Sync flash, again");
    // The user's own media, never played: kept in the media manager's list.
    let kept: Vec<Cue> = original
        .cues
        .iter()
        .filter(|c| {
            c.kind
                .file_media()
                .is_some_and(|m| !m.file.as_os_str().is_empty())
        })
        .cloned()
        .collect();
    if !kept.is_empty() {
        let list = show.add_list(KEPT_LIST);
        for mut c in kept {
            c.id = CueId(show.alloc_id());
            c.list = list;
            c.parent = None;
            c.armed = false;
            c.continue_mode = ContinueMode::DoNotContinue;
            show.cues.push(c);
        }
    }
    Built {
        show,
        cues: Cues {
            flash: flash_id,
            film: film_id,
            sound: sound_id,
            timeline: tl,
            flash_again: again,
        },
        files,
        notes,
        displays,
    }
}

// ----- Media on the computers ---------------------------------------------------

/// Where a computer's copy of the test's files stands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MediaState {
    /// It hasn't the primary's list of them yet.
    Waiting,
    /// On their way: bytes still to come, of all the test's.
    Coming {
        left: u64,
        total: u64,
    },
    Here,
}

/// From a computer's media report: does it have the test's `files` of the
/// version `current` the primary serves?
pub fn media_state(
    r: Option<&MediaReport>,
    current: Option<&str>,
    files: &[(String, u64)],
) -> MediaState {
    let total: u64 = files.iter().map(|(_, s)| s).sum();
    let Some(r) = r else {
        return MediaState::Waiting;
    };
    if current.is_none() || r.progress.manifest_id.as_deref() != current {
        return MediaState::Waiting;
    }
    let keys: Vec<String> = files.iter().map(|(f, _)| path_key(f)).collect();
    let left: u64 = r
        .lacking
        .iter()
        .filter(|l| keys.contains(&path_key(&l.path)))
        .map(|l| l.total.saturating_sub(l.done).max(1))
        .sum();
    if left == 0 {
        MediaState::Here
    } else {
        MediaState::Coming { left, total }
    }
}

/// A computer's transfer, as the test watched it.
#[derive(Clone, Copy, Debug, Default)]
struct Watched {
    /// When it first had the list, and how much was then still to come.
    first: Option<(Instant, u64)>,
    done: Option<Instant>,
}

// ----- Preparing ----------------------------------------------------------------

/// The media found, and the Sync flash's file (or why it couldn't be made).
pub type Ready = (HashMap<&'static str, Resolved>, Result<PathBuf, String>);

/// The media being made ready on this computer.
pub struct NetPrep {
    pub soak: bool,
    pub fetch: Fetch,
    flash: Arc<parking_lot::Mutex<Option<Result<PathBuf, String>>>>,
    cancel: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub log_dir: PathBuf,
    pub wanted: Vec<&'static Asset>,
    pub since: Instant,
}

impl NetPrep {
    pub fn start(ff: Ffmpeg, log_dir: &Path, soak: bool, download: bool) -> NetPrep {
        let wanted: Vec<&'static Asset> = MEDIA.iter().filter_map(|id| assets::find(id)).collect();
        let dir = assets::cache_dir(log_dir);
        let fetch = assets::start(ff.clone(), dir.clone(), wanted.clone(), download);
        let flash = Arc::new(parking_lot::Mutex::new(None));
        let cancel = Arc::new(AtomicBool::new(false));
        let (slot, stop) = (flash.clone(), cancel.clone());
        let thread = std::thread::Builder::new()
            .name("test-sync-flash".into())
            .spawn(move || {
                let r = super::syncflash::make(&ff, &dir, &stop);
                *slot.lock() = Some(r);
            })
            .ok();
        if thread.is_none() {
            *flash.lock() = Some(Err("no thread to make it on".into()));
        }
        NetPrep {
            soak,
            fetch,
            flash,
            cancel,
            thread,
            log_dir: log_dir.to_path_buf(),
            wanted,
            since: Instant::now(),
        }
    }

    pub fn cancel(&self) {
        self.fetch.cancel();
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed) || self.fetch.status.lock().cancelled
    }

    /// Whether the Sync flash is made (or failed).
    pub fn flash_done(&self) -> bool {
        self.flash.lock().is_some()
    }

    /// Everything is ready: the media found, and the flash's path (None: it
    /// couldn't be made, with why).
    pub fn ready(&mut self) -> Option<Ready> {
        if !self.fetch.finished() || !self.flash_done() {
            return None;
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let res = self.fetch.status.lock().resolved.clone();
        let flash = self.flash.lock().clone().unwrap_or(Err("not made".into()));
        Some((res, flash))
    }
}

impl Drop for NetPrep {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ----- Running ------------------------------------------------------------------

/// Where the test is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stage {
    /// The test show is in place; its files are on their way to the computers.
    WaitingForMedia,
    /// Playing its script, from the shared time `start`.
    Playing,
    /// Over: the computers' last reports are awaited.
    Ending { since: f64 },
}

/// One computer taking part.
pub struct Computer {
    pub id: NodeId,
    pub notes: ComputerNotes,
    /// Shared time of the last report (None: none yet).
    last_heard: Option<f64>,
    watched: Watched,
    pub media: MediaState,
    pub log_tail: Option<String>,
}

/// The network test under way.
pub struct NetRun {
    pub soak: bool,
    pub run: u64,
    pub stage: Stage,
    pub started: Instant,
    /// The shared time the script began (Playing on).
    pub start: Option<f64>,
    pub folder: PathBuf,
    log_dir: PathBuf,
    stash: Option<TestStash>,
    pub cues: Cues,
    pub files: Vec<(String, u64)>,
    pub notes: Vec<String>,
    pub displays: Vec<String>,
    script: Vec<(f64, Act)>,
    next: usize,
    /// The media version served when the test began (the test's is the next).
    before: Option<String>,
    pub computers: BTreeMap<NodeId, Computer>,
    /// When the primary's list of the test's media was first seen.
    version_seen: Option<Instant>,
    /// The shared time of the first look.
    first_look: Option<f64>,
    /// When each computer was last told to measure (shared time).
    told: HashMap<NodeId, f64>,
    /// For the computers: messages the app sends (`TestMode::net_out`).
    pub out: Vec<(Option<NodeId>, NetTest)>,
    pub stop: bool,
    events: Option<std::io::BufWriter<std::fs::File>>,
    hitches: super::hitches::Hitches,
    /// This computer's own output windows' drawing while it played, and when
    /// it was last looked at (shared time).
    own_drawing: Vec<net_judge::MonitorNotes>,
    drawing_looked: Option<f64>,
    _awake: KeepAwake,
    wanted: Vec<&'static Asset>,
    /// How long it is meant to play (s).
    pub planned: f64,
}

impl NetRun {
    /// Put the test show in place and begin waiting for its media. The
    /// computers are those of the network now (`st.net`).
    pub fn launch(
        built: Built,
        soak: bool,
        log_dir: &Path,
        wanted: Vec<&'static Asset>,
        show: &mut Show,
        engine: &mut Engine,
        st: &mut UiState,
    ) -> Result<NetRun, String> {
        if let Some(why) = refusal(st) {
            return Err(why);
        }
        let folder = super::session::run_folder_named(
            log_dir,
            if soak { "network-soak" } else { "network-test" },
            unix_now(),
        );
        std::fs::create_dir_all(&folder).map_err(|e| {
            format!(
                "Can't make the folder for the test's files ({}): {e}. Nothing was changed.",
                folder.display()
            )
        })?;
        let events = std::fs::File::create(folder.join("events.log"))
            .ok()
            .map(std::io::BufWriter::new);
        let computers: BTreeMap<NodeId, Computer> = st
            .net
            .peers
            .iter()
            .filter(|p| p.ours && p.connected)
            .map(|p| {
                let mut notes = ComputerNotes::new(&p.name);
                if let Some(s) = &p.status {
                    notes.link = s.link;
                    notes.refresh_hz = s
                        .monitors
                        .iter()
                        .filter_map(|m| m.refresh_hz)
                        .reduce(f32::max);
                }
                (
                    p.id,
                    Computer {
                        id: p.id,
                        notes,
                        last_heard: None,
                        watched: Watched::default(),
                        media: MediaState::Waiting,
                        log_tail: None,
                    },
                )
            })
            .collect();
        let _ = std::fs::write(folder.join("computers.txt"), computers_text(&st.net));
        let awake = KeepAwake::new("NewQ is running a network test");
        let before = st.media.current.clone();
        let started = Instant::now();
        let stash = st.begin_test(show, engine, built.show);
        engine.set_log_tap(true);
        let mut run = NetRun {
            soak,
            run: crate::net::node::random_u64(),
            stage: Stage::WaitingForMedia,
            started,
            start: None,
            folder,
            log_dir: log_dir.to_path_buf(),
            stash: Some(stash),
            cues: built.cues,
            files: built.files,
            notes: built.notes,
            displays: built.displays,
            script: script(soak),
            next: 0,
            before,
            computers,
            told: HashMap::new(),
            first_look: None,
            version_seen: None,
            out: Vec::new(),
            stop: false,
            events,
            hitches: super::hitches::Hitches::new(started),
            own_drawing: Vec::new(),
            drawing_looked: None,
            _awake: awake,
            wanted,
            planned: if soak {
                f64::from(SOAK_LAPS) * LAP_SECS
            } else {
                LAP_SECS
            },
        };
        let names: Vec<&str> = run
            .computers
            .values()
            .map(|c| c.notes.name.as_str())
            .collect();
        let line = format!(
            "Network test begins with {} ({} files, {}); waiting for the media to reach them",
            names.join(", "),
            run.files.len(),
            megabytes(run.files.iter().map(|f| f.1).sum())
        );
        run.log(&line);
        for n in run.notes.clone() {
            run.log(&n);
        }
        Ok(run)
    }

    fn secs(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    fn log(&mut self, text: &str) {
        let t = self.secs();
        if let Some(f) = &mut self.events {
            let _ = writeln!(f, "[{t:>9.3}] {text}");
            let _ = f.flush();
        }
        log::info!("network test: {text}");
    }

    /// Seconds played (Playing on).
    pub fn played(&self, now: f64) -> f64 {
        self.start.map_or(0.0, |s| (now - s).max(0.0))
    }

    /// One look, at shared time `now` (the primary's own clock), with the
    /// messages that came from the computers. Returns how it ended, once it has.
    pub fn tick(
        &mut self,
        show: &Show,
        engine: &mut Engine,
        st: &UiState,
        now: f64,
        inbox: Vec<(NodeId, NetTest)>,
    ) -> Option<End> {
        for (t, line) in self.hitches.tick(Instant::now()) {
            if let Some(f) = &mut self.events {
                let _ = writeln!(f, "[{t:>9.3}] {line}");
            }
        }
        for (at, text) in engine.take_log_tap() {
            let t = at.saturating_duration_since(self.started).as_secs_f64();
            if let Some(f) = &mut self.events {
                let _ = writeln!(f, "[{t:>9.3}] {text}");
            }
        }
        for (from, m) in inbox {
            self.take(from, m, now, &st.net);
        }
        match self.stage {
            Stage::WaitingForMedia => {
                if self.stop {
                    return Some(End::Stopped);
                }
                let first = *self.first_look.get_or_insert(now);
                // (A moment for the show to reach the computers first.)
                if self.media_ready(st) && now - first >= 2.0 {
                    let line = self.transfers_line();
                    self.log(&format!("All here. {line}"));
                    self.begin(now);
                } else if self.started.elapsed() > MEDIA_LIMIT {
                    let lacking: Vec<String> = self
                        .computers
                        .values()
                        .filter(|c| c.media != MediaState::Here)
                        .map(|c| c.notes.name.clone())
                        .collect();
                    return Some(End::Failed(format!(
                        "the test's media hadn't reached {} after {} minutes",
                        lacking.join(", "),
                        MEDIA_LIMIT.as_secs() / 60
                    )));
                }
                None
            }
            Stage::Playing => {
                let played = self.played(now);
                // This computer's screens, once a second, as the others
                // report theirs.
                if self.drawing_looked.is_none_or(|t| now - t >= 1.0) {
                    self.drawing_looked = Some(now);
                    net_judge::note_drawing(&mut self.own_drawing, &st.drawing);
                }
                while self.next < self.script.len() && self.script[self.next].0 <= played {
                    let act = self.script[self.next].1;
                    self.next += 1;
                    self.act(show, engine, act, played);
                }
                // A computer that came back (or joined) is told again.
                let begin = NetTest::Begin {
                    run: self.run,
                    start: self.start.unwrap_or(now),
                };
                for p in st.net.peers.iter().filter(|p| p.ours && p.connected) {
                    let told = self.told.get(&p.id).copied();
                    let quiet = self
                        .computers
                        .get(&p.id)
                        .and_then(|c| c.last_heard)
                        .is_none_or(|h| now - h > AWAY_SECS);
                    if quiet && told.is_none_or(|t| now - t > AWAY_SECS) {
                        self.told.insert(p.id, now);
                        self.out.push((Some(p.id), begin.clone()));
                    }
                }
                if self.stop || self.next >= self.script.len() && played >= self.planned {
                    self.finish_playing(engine, now);
                }
                None
            }
            Stage::Ending { since } => {
                let all_in = self.computers.iter().all(|(id, c)| {
                    (c.notes.ended && c.log_tail.is_some())
                        || !st.net.peers.iter().any(|p| p.id == *id && p.connected)
                });
                (all_in || now - since > ENDING_LIMIT).then_some(if self.stop {
                    End::Stopped
                } else {
                    End::Finished
                })
            }
        }
    }

    /// Every computer has the test's files (and the test show).
    fn media_ready(&mut self, st: &UiState) -> bool {
        let m: &MediaView = &st.media;
        let current = m
            .current
            .as_deref()
            .filter(|c| Some(*c) != self.before.as_deref());
        let hashing = m.hashing.is_some();
        let now = Instant::now();
        if current.is_some() && !hashing && self.version_seen.is_none() {
            self.version_seen = Some(now);
        }
        let version_seen = self.version_seen.unwrap_or(now);
        let mut all = !self.computers.is_empty();
        for c in self.computers.values_mut() {
            let p = st.net.peers.iter().find(|p| p.id == c.id);
            let report = p
                .and_then(|p| p.status.as_ref())
                .and_then(|s| s.media_progress.as_deref());
            let state = if hashing {
                MediaState::Waiting
            } else {
                media_state(report, current, &self.files)
            };
            if c.watched.first.is_none() && state != MediaState::Waiting {
                let left = match state {
                    MediaState::Coming { left, .. } => left,
                    _ => 0,
                };
                c.watched.first = Some((now, left));
            }
            if state == MediaState::Here && c.watched.done.is_none() {
                c.watched.done = Some(now);
                let (t0, left) = c.watched.first.unwrap_or((now, 0));
                // All there at the first look: it came within the time since
                // the primary listed it (or had it from an earlier test).
                let total: u64 = self.files.iter().map(|f| f.1).sum();
                c.notes.transfer = Some(if left == 0 {
                    Transfer {
                        bytes: total,
                        secs: now.duration_since(version_seen).as_secs_f64().max(0.1),
                        within: true,
                    }
                } else {
                    Transfer {
                        bytes: left,
                        secs: now.duration_since(t0).as_secs_f64().max(0.1),
                        within: false,
                    }
                });
            }
            c.media = state;
            let show_in = p
                .and_then(|p| p.status.as_ref())
                .is_some_and(|s| s.in_step && p.is_some_and(|p| p.connected));
            all &= state == MediaState::Here && show_in;
        }
        all
    }

    /// The window's line while the media is on its way.
    pub fn media_lines(&self) -> Vec<String> {
        self.computers
            .values()
            .map(|c| match c.media {
                MediaState::Waiting => format!("{}: getting the list of media", c.notes.name),
                MediaState::Coming { left, total } => {
                    let done = total.saturating_sub(left);
                    format!(
                        "{}: {:.0} % ({} of {})",
                        c.notes.name,
                        done as f64 * 100.0 / total.max(1) as f64,
                        megabytes(done),
                        megabytes(total)
                    )
                }
                MediaState::Here => match &c.notes.transfer {
                    Some(t) => format!(
                        "{}: all here, {}",
                        c.notes.name,
                        net_judge::transfer_words(t)
                    ),
                    None => format!("{}: all here", c.notes.name),
                },
            })
            .collect()
    }

    fn transfers_line(&self) -> String {
        self.computers
            .values()
            .map(|c| match &c.notes.transfer {
                Some(t) => format!("{}: {}", c.notes.name, net_judge::transfer_words(t)),
                None => format!("{}: already there", c.notes.name),
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn begin(&mut self, now: f64) {
        self.stage = Stage::Playing;
        self.start = Some(now);
        self.out.push((
            None,
            NetTest::Begin {
                run: self.run,
                start: now,
            },
        ));
        for id in self.computers.keys() {
            self.told.insert(*id, now);
        }
        self.log("The computers are measuring; the test plays");
    }

    fn act(&mut self, show: &Show, engine: &mut Engine, act: Act, played: f64) {
        let lap = (played / LAP_SECS) as u32;
        let c = self.cues;
        let line = match act {
            Act::Begin if lap == 0 => {
                engine.go(show);
                "GO: the Sync flash".to_string()
            }
            Act::Begin => {
                engine.fire(show, c.flash);
                format!("Lap {}: the Sync flash", lap + 1)
            }
            Act::Fire(w) => {
                let (id, name) = match w {
                    Which::Film => (c.film, "the film"),
                    Which::Sound => (c.sound, "the sound"),
                    Which::Timeline => (Some(c.timeline), "the timeline"),
                    Which::FlashAgain => (Some(c.flash_again), "the Sync flash again"),
                };
                match id {
                    Some(id) => {
                        engine.fire(show, id);
                        format!("Fire {name}")
                    }
                    None => format!("({name} isn't in the test)"),
                }
            }
            Act::Seek(pos) => {
                engine.timeline_seek(show, TimelineKey::Group(c.timeline), pos, true);
                format!("The timeline is sought to {pos:.0} s")
            }
            Act::Pause(on) => {
                engine.set_paused(on);
                if on { "Pause" } else { "Resume" }.to_string()
            }
            Act::StopAll(fade) => {
                engine.stop_all(fade);
                format!("Stop all ({fade:.0} s fade)")
            }
        };
        self.log(&format!("{} {line}", net_judge::at(played)));
    }

    fn finish_playing(&mut self, engine: &mut Engine, now: f64) {
        if self.stage != Stage::Playing {
            return;
        }
        engine.stop_all(0.5);
        self.out.push((None, NetTest::End { run: self.run }));
        self.stage = Stage::Ending { since: now };
        self.log("The test is over; waiting for the computers' last reports");
    }

    /// A shorter script (the tests of the test).
    #[cfg(test)]
    pub fn shorten(&mut self, script: Vec<(f64, Act)>, planned: f64) {
        self.script = script;
        self.planned = planned;
    }

    /// The user asked to stop.
    pub fn request_stop(&mut self) {
        self.stop = true;
    }

    /// A message from a computer.
    fn take(&mut self, from: NodeId, m: NetTest, now: f64, net: &NetView) {
        let t = self.played(now);
        let c = self.computers.entry(from).or_insert_with(|| Computer {
            id: from,
            notes: ComputerNotes::new(&net.name_of(from)),
            last_heard: None,
            watched: Watched::default(),
            media: MediaState::Here,
            log_tail: None,
        });
        match m {
            NetTest::Report(r) if r.run == self.run => {
                if let Some(h) = c.last_heard {
                    let gap = now - h;
                    if gap > net_judge::GAP_SECS {
                        let at = self.start.map_or(0.0, |s| (h - s).max(0.0));
                        c.notes.gaps.push((at, gap));
                    }
                }
                c.last_heard = Some(now);
                let lines: Vec<String> = r
                    .commands
                    .iter()
                    .filter(|k| k.lead_ms < 0.0)
                    .map(|k| {
                        format!(
                            "{}: command {} arrived {:.1} ms late",
                            c.notes.name, k.seq, -k.lead_ms
                        )
                    })
                    .chain(
                        r.corrections
                            .iter()
                            .map(|k| format!("{}: put right: {}", c.notes.name, k.what)),
                    )
                    .chain(r.stalls.iter().map(|s| {
                        format!(
                            "{}: its main loop stalled for {} ms ({})",
                            c.notes.name, s.ms, s.part
                        )
                    }))
                    .collect();
                c.notes.take(*r);
                for l in lines {
                    if let Some(f) = &mut self.events {
                        let _ = writeln!(f, "[{t:>9.3}] {l}");
                    }
                }
            }
            NetTest::LogTail { run, text } if run == self.run => {
                c.notes.log_tail = true;
                c.log_tail = Some(text);
            }
            _ => {}
        }
    }

    /// The computers' lines while it plays: how far apart, now.
    pub fn live_lines(&self, now: f64) -> Vec<(String, bool)> {
        self.computers
            .values()
            .map(|c| {
                let away = c.last_heard.is_none_or(|h| now - h > AWAY_SECS);
                let frame = c.notes.frame_ms();
                let last = c.notes.rows.iter().rev().find(|r| r.apart_ms.is_some());
                let worst = c
                    .notes
                    .rows
                    .iter()
                    .filter_map(|r| r.apart_ms)
                    .reduce(f64::max);
                let late = c.notes.commands.iter().filter(|k| k.lead_ms < 0.0).count();
                let text = if away && self.played(now) > 3.0 {
                    format!("{}: not heard from", c.notes.name)
                } else {
                    format!(
                        "{}: {} apart now, worst {}; clock ±{}; {} commands, {} late",
                        c.notes.name,
                        last.and_then(|r| r.apart_ms)
                            .map_or("–".into(), |a| format!("{:.1} frames", a / frame)),
                        worst.map_or("–".into(), |a| format!("{:.1} frames", a / frame)),
                        c.notes
                            .rows
                            .last()
                            .and_then(|r| r.clock_error_ms)
                            .map_or("–".into(), |e| format!("{e:.1} ms")),
                        c.notes.commands.len(),
                        late
                    )
                };
                (text, away || late > 0)
            })
            .collect()
    }

    /// Put the user's show back (at once, whatever happens next).
    fn restore(&mut self, show: &mut Show, engine: &mut Engine, st: &mut UiState) {
        engine.stop_all(0.0);
        if let Some(stash) = self.stash.take() {
            st.end_test(show, engine, stash);
        }
        engine.set_log_tap(false);
    }
}

/// The computers as the primary sees them, for `computers.txt`.
fn computers_text(v: &NetView) -> String {
    let mut s = format!(
        "This computer: {} (NewQ {})\n\n",
        v.name,
        crate::net::proto::VERSION
    );
    for p in v.peers.iter().filter(|p| p.ours) {
        s.push_str(&format!(
            "{}: {}, NewQ {}, {}\n",
            p.name,
            p.role.described(),
            p.version,
            if p.connected {
                "connected"
            } else {
                "not connected"
            }
        ));
        if let Some(st) = &p.status {
            s.push_str(&format!(
                "  link {:?}, clock ±{} ms, round trip {} ms, {}\n",
                st.link,
                st.clock_error_ms.map_or("?".into(), |e| format!("{e:.2}")),
                st.rtt_ms.map_or("?".into(), |e| format!("{e:.2}")),
                st.standing.clone().unwrap_or_default()
            ));
            for (i, m) in st.monitors.iter().enumerate() {
                s.push_str(&format!(
                    "  monitor {}: {} {}x{} {}\n",
                    i + 1,
                    m.identity,
                    m.size.0,
                    m.size.1,
                    m.refresh_hz.map_or("?".into(), |r| format!("{r:.2} Hz"))
                ));
            }
        }
    }
    s
}

fn megabytes(b: u64) -> String {
    format!("{:.0} MB", b as f64 / 1_000_000.0)
}

/// What the window shows of a network test that has ended.
#[derive(Clone, Debug)]
pub struct NetDone {
    pub soak: bool,
    pub computers: Vec<net_judge::ComputerVerdict>,
}

/// End the run: the user's show back first, then the files, the verdict and
/// the zip.
pub fn finish(
    mut run: NetRun,
    end: End,
    show: &mut Show,
    engine: &mut Engine,
    st: &mut UiState,
    now: f64,
) -> Done {
    run.restore(show, engine, st);
    let elapsed = run.started.elapsed();
    let played = run.played(now);
    run.log(&format!("Ended ({end:?}); your show is back"));
    // A computer silent at the end dropped out then.
    if run.start.is_some() {
        for c in run.computers.values_mut() {
            if let Some(h) = c.last_heard
                && !c.notes.ended
                && now - h > net_judge::GAP_SECS
            {
                let at = run.start.map_or(0.0, |s| (h - s).max(0.0));
                c.notes.gaps.push((at, now - h));
            }
        }
    }
    let mut own: Vec<Finding> = Vec::new();
    if let End::Failed(why) = &end {
        own.push(Finding {
            severity: Severity::Problem,
            text: format!("The test could not go on: {why}"),
        });
    }
    let hours = elapsed.as_secs_f64() / 3600.0;
    own.extend(run.hitches.findings(hours, None).into_iter().map(|mut f| {
        f.text = format!("This computer (the primary): {}", f.text);
        f
    }));
    own.extend(net_judge::drawing_findings("This computer (the primary)", &run.own_drawing).1);
    let notes: Vec<ComputerNotes> = run.computers.values().map(|c| c.notes.clone()).collect();
    // Before playing began there is nothing of the computers to judge.
    let judged: Judged = if run.start.is_some() {
        net_judge::judge(&notes, played, own)
    } else {
        Judged {
            verdict: Verdict::of(&own),
            headline: match &end {
                End::Failed(why) => format!("The test could not go on: {why}"),
                _ => "Stopped before it played.".into(),
            },
            computers: Vec::new(),
            findings: own,
        }
    };
    for f in &judged.findings {
        run.log(&format!("Finding: {}", f.text));
    }
    let about = format!(
        "{} from {} with {}. Media: {}.",
        if run.soak {
            "Network soak"
        } else {
            "Network test"
        },
        st.net.name,
        run.computers
            .values()
            .map(|c| c.notes.name.clone())
            .collect::<Vec<_>>()
            .join(", "),
        run.transfers_line()
    );
    let mut report = net_judge::report(&judged, &notes, played, &about);
    if !run.notes.is_empty() {
        report.push_str("\nNotes\n");
        for n in &run.notes {
            report.push_str(&format!("  {n}\n"));
        }
    }
    report.push_str(&format!("\nDisplays: {}\n", run.displays.join(", ")));
    let _ = std::fs::write(run.folder.join("report.txt"), &report);
    let _ = std::fs::write(run.folder.join("network.csv"), net_judge::csv(&notes));
    let _ = std::fs::write(
        run.folder.join("media-credits.txt"),
        assets::credits(&run.wanted),
    );
    for c in run.computers.values() {
        if let Some(t) = &c.log_tail {
            let name: String = c
                .notes
                .name
                .chars()
                .map(|ch| {
                    if ch.is_ascii_alphanumeric() || ch == '-' {
                        ch
                    } else {
                        '_'
                    }
                })
                .collect();
            let _ = std::fs::write(run.folder.join(format!("computer-{name}-newq.log")), t);
        }
    }
    if let Some(f) = &mut run.events {
        let _ = f.flush();
    }
    Done {
        compare: None,
        full: None,
        network: Some(NetDone {
            soak: run.soak,
            computers: judged.computers.clone(),
        }),
        flavour: super::Flavour::Cues,
        bundle: super::bundle::Bundle::start(
            run.folder.clone(),
            run.log_dir.clone(),
            matches!(end, End::Quit),
            super::bundle::TIDY,
        ),
        level: super::Level::Short,
        verdict: judged.verdict,
        headline: judged.headline.clone(),
        steady: None,
        findings: judged.findings,
        folder: run.folder.clone(),
        report,
        laps: 0,
        elapsed,
        ended: end,
    }
}

/// What the window says while the media is made ready.
pub fn prep_line(p: &NetPrep) -> String {
    let st = p.fetch.status.lock();
    let total: u64 = st.items.iter().map(|i| i.bytes).sum();
    let done: u64 = st
        .items
        .iter()
        .map(|i| match &i.state {
            assets::State::Waiting => 0,
            assets::State::Downloading { done, .. } => *done,
            _ => i.bytes,
        })
        .sum();
    format!(
        "Getting the test's media ready on this computer: {} of {}{} ({} s)",
        megabytes(done),
        megabytes(total),
        if p.flash_done() {
            ""
        } else {
            ", and making the Sync flash"
        },
        p.since.elapsed().as_secs()
    )
}

#[cfg(test)]
#[path = "network_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "network_ui_tests.rs"]
mod ui_tests;
