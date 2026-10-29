//! Playback engine: fires cues, runs pre-waits, continues, fades and stops,
//! and owns the decoders for everything that is playing.

use crate::clock::{ClockFollower, TimeOfDay};
use crate::fx::VideoFx;
use crate::media::audio::{AudioOut, Voice};
use crate::media::camera;
use crate::media::ffmpeg::{ColorMatrix, Ffmpeg, MediaInfo};
use crate::media::playback::Playback;
use crate::media::video::{self, RgbaImage, VideoPlayer};
use crate::media::waveform::WaveformCache;
use crate::media::{ProbeCache, ProbeState};
use crate::output::{Outbox, Outgoing};
use crate::show::*;
use crate::timecode::{Follower, Rate, Timecode};
use crossbeam_channel::Receiver;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Cues from the playhead onward (along a continue chain) that get preloaded.
const MAX_PRELOAD: usize = 4;
/// Start a video even if its audio has not arrived after this long.
const AUDIO_START_TIMEOUT: Duration = Duration::from_secs(3);

/// What to do about a video, or a seek in one, whose decoders are starting.
#[derive(Debug, PartialEq)]
enum VideoStart {
    /// Not ready yet.
    Wait,
    /// Go, with its sound if it has any.
    Go,
    /// Go without the sound: it wasn't ready in time. The sound plays from
    /// its own start whenever it does arrive, so starting it late would leave
    /// it behind the picture for the whole cue.
    GoSilent,
}

fn video_start(
    video_ready: bool,
    has_voice: bool,
    audio_ready: bool,
    waited: Duration,
) -> VideoStart {
    if !video_ready {
        VideoStart::Wait
    } else if audio_ready || !has_voice {
        VideoStart::Go
    } else if waited > AUDIO_START_TIMEOUT {
        VideoStart::GoSilent
    } else {
        VideoStart::Wait
    }
}
/// Decode size when the target output window is not open.
const DEFAULT_OUTPUT_SIZE: (u32, u32) = (1920, 1080);
/// A dip reveals anyway if the incoming cue hasn't started after this long.
const REVEAL_TIMEOUT: f64 = 3.0;

#[derive(Clone, Debug)]
struct Ramp {
    from: f32,
    to: f32,
    duration: f64,
    t: f64,
    curve: Curve,
}

/// A value that can be faded over time (opacity 0..1, or volume in dB).
#[derive(Clone, Debug)]
pub struct Param {
    pub value: f32,
    ramp: Option<Ramp>,
}

impl Param {
    pub fn new(value: f32) -> Param {
        Param { value, ramp: None }
    }
    pub fn ramp_to(&mut self, to: f32, duration: f64, curve: Curve) {
        // A time that isn't one (a hand-edited file) would never run out,
        // and a fade that never ends is a cue that never stops.
        if !duration.is_finite() || duration <= 0.0 {
            self.value = to;
            self.ramp = None;
        } else {
            self.ramp = Some(Ramp {
                from: self.value,
                to,
                duration,
                t: 0.0,
                curve,
            });
        }
    }
    pub fn advance(&mut self, dt: f64) {
        if let Some(r) = &mut self.ramp {
            r.t += dt;
            let k = r.curve.apply(r.t / r.duration) as f32;
            self.value = r.from + (r.to - r.from) * k;
            if r.t >= r.duration {
                self.value = r.to;
                self.ramp = None;
            }
        }
    }
    pub fn ramping(&self) -> bool {
        self.ramp.is_some()
    }
}

/// A cue's position, scale and rotation once a fade cue has taken them over:
/// they ramp to where it says and stay there (as a faded level does).
pub struct LiveTransform {
    x: Param,
    y: Param,
    scale_x: Param,
    scale_y: Param,
    rotation: Param,
}

impl LiveTransform {
    fn from(t: &LayerTransform) -> LiveTransform {
        LiveTransform {
            x: Param::new(t.x),
            y: Param::new(t.y),
            scale_x: Param::new(t.scale_x),
            scale_y: Param::new(t.scale_y),
            rotation: Param::new(t.rotation),
        }
    }

    fn advance(&mut self, dt: f64) {
        for p in [
            &mut self.x,
            &mut self.y,
            &mut self.scale_x,
            &mut self.scale_y,
            &mut self.rotation,
        ] {
            p.advance(dt);
        }
    }

    /// `base` (for its crop) with these.
    fn apply(&self, base: &LayerTransform) -> LayerTransform {
        LayerTransform {
            x: self.x.value,
            y: self.y.value,
            scale_x: self.scale_x.value,
            scale_y: self.scale_y.value,
            rotation: self.rotation.value,
            crop: base.crop,
        }
    }
}

pub struct VideoState {
    pub player: VideoPlayer,
    pub voice: Option<Voice>,
    pub output: Option<OutputId>,
    pub layer: i32,
    pub fit: Fit,
    pub transform: LayerTransform,
    pub color: ColorAdjust,
    /// The picture effects now (see `fx::VideoFx`).
    pub fx: VideoFx,
    spawned_at: Instant,
}

pub struct ImageState {
    pub image: Option<RgbaImage>,
    /// Counts the pictures: the renderer uploads a new one when it changes.
    pub version: u64,
    loader: Option<Receiver<Result<RgbaImage, String>>>,
    pub output: Option<OutputId>,
    pub layer: i32,
    pub fit: Fit,
    pub transform: LayerTransform,
    pub color: ColorAdjust,
    pub fx: VideoFx,
    /// Set for a clock or timer: the picture is redrawn as the time changes.
    live: Option<LiveText>,
}

/// A clock or timer text cue being kept up to date.
struct LiveText {
    size: (u32, u32),
    /// What the picture on screen (or on its way) shows.
    drawn: (String, [u8; 4]),
    /// A newer picture being drawn.
    pending: Option<Receiver<Result<RgbaImage, String>>>,
}

/// What distinguishes one picture of a live cue from the next.
fn live_key(c: &TextCue) -> (String, [u8; 4]) {
    (c.text.clone(), c.color)
}

/// A full-screen colour layer used for dip-to-black/white transitions. It
/// fades up over the outgoing content, holds while the incoming cue starts
/// underneath, then fades away.
pub struct MatteState {
    pub output: Option<OutputId>,
    pub color: [u8; 4],
    half: f64,
    curve: Curve,
    /// The incoming cue has been started.
    fired: bool,
    fired_at: f64,
}

/// A running group. Its children are separate active cues; the group is
/// the thing that stays active until they have all finished.
pub struct GroupState {
    mode: GroupMode,
    children: Vec<CueId>,
    /// Playlist: index of the next child to fire.
    next: usize,
}

pub enum ActiveKind {
    Video(Box<VideoState>),
    Image(ImageState),
    Audio(Voice),
    Matte(MatteState),
    Fade,
    Wait,
    /// Makes MIDI Time Code while it lives (none in a test outbox).
    Timecode(Option<crate::output::MtcRunner>),
    Group(GroupState),
}

/// What a cue does to the rest of its output once its first frame is ready.
#[derive(Clone, Copy, Debug)]
enum EntryAction {
    Cut,
    Crossfade {
        time: f64,
        curve: Curve,
    },
    /// Second half of a dip: remove what is under the matte and fade the matte away.
    Reveal {
        matte: u64,
        time: f64,
        curve: Curve,
    },
}

pub struct ActiveCue {
    pub instance: u64,
    pub cue_id: CueId,
    pub kind: ActiveKind,
    /// Media/action time in seconds since the cue started.
    pub elapsed: f64,
    /// None = runs until stopped.
    pub duration: Option<f64>,
    /// The length of one pass of a loop, if it is a loop whose passes have a
    /// length (the bar shows where it is in the pass and jumps within it).
    pub pass_len: Option<f64>,
    /// Its keyframed speed, if it has one: then `pass_len` is in the file's
    /// seconds (a pass is that long in the file, however long it takes).
    pub speed: Option<std::sync::Arc<crate::speed::SpeedMap>>,
    /// Set once a fade cue has moved the cue's geometry: it then keeps what that says.
    live_transform: Option<LiveTransform>,
    pub opacity: Param,
    /// Volume in dB.
    pub volume: Param,
    /// Media is decoded and playing (pre-roll finished).
    pub started: bool,
    /// Held where it is by a control cue (a pause of this cue alone).
    pub hold: bool,
    /// Fading out; removed once the fade completes.
    pub stopping: bool,
    /// Ended and is holding its last frame.
    pub held: bool,
    /// Where in the cue the running decoders began (after a seek), so their
    /// clocks, which start at zero, line up with `elapsed`.
    seek_base: f64,
    /// Seconds its sound is held back and its picture is held back (the
    /// show's sync settings, as they were when it started).
    audio_lead: f64,
    video_lag: f64,
    /// Seconds since it started, for holding the picture back.
    age: f64,
    /// Decoders started at a new position, swapped in once they have a frame.
    seeking: Option<Seek>,
    /// A seek took the cue back out of its automatic fade-out: put its levels
    /// back at the next tick.
    relevel: bool,
    /// The loss of its audio device has been logged.
    lost_logged: bool,
    /// A fade cue has changed its levels, so live edits to the cue no longer apply.
    faded: bool,
    auto_faded: bool,
    followed: bool,
    panic: bool,
    entry: Option<EntryAction>,
}

/// Decoders for a new position in a cue that is playing. The old ones keep
/// going until these have a frame, so scrubbing doesn't flash black.
struct Seek {
    player: Option<VideoPlayer>,
    voice: Option<Voice>,
    base: f64,
    spawned_at: Instant,
}

impl ActiveCue {
    /// The length of the bar for the cue if it can be scrubbed: a file that
    /// plays once (not a camera), or a still with a time; a loop with a set
    /// number of plays, all of them; a loop that never ends, one pass.
    pub fn scrub_length(&self, show: &Show) -> Option<f64> {
        if !self.started || self.stopping || self.held {
            return None;
        }
        let media = show.cue(self.cue_id)?.kind.media()?;
        // Live sound has no file to jump about in.
        let live_sound = matches!(show.cue(self.cue_id)?.kind, CueKind::LiveAudio(_));
        match &self.kind {
            ActiveKind::Video(v) if v.player.is_live() => None,
            ActiveKind::Video(_) | ActiveKind::Audio(_) if live_sound => None,
            ActiveKind::Video(_) | ActiveKind::Audio(_) => {
                if media.loop_media {
                    let pass = self.pass_len?;
                    // Set plays: the whole of them. Forever: one pass.
                    Some(self.duration.filter(|d| *d > 0.0).unwrap_or(pass))
                } else {
                    self.duration.filter(|d| *d > 0.0)
                }
            }
            ActiveKind::Image(_) => self.duration.filter(|d| *d > 0.0),
            _ => None,
        }
    }

    /// How far through the loop's passes it is, in the units of `pass_len`: its
    /// time, or with a keyframed speed the file's time that has been played.
    fn pass_time(&self) -> f64 {
        match &self.speed {
            Some(map) => map.media_at(self.elapsed),
            None => self.elapsed,
        }
    }

    /// Whether this is a loop that never ends, whose bar shows the pass.
    pub fn loops_forever(&self) -> bool {
        self.pass_len.is_some() && self.duration.is_none()
    }

    /// Where the bar is: the time into the cue, or for a loop that never
    /// ends the time into the current pass.
    pub fn bar_position(&self) -> f64 {
        match self.pass_len.filter(|_| self.loops_forever()) {
            Some(len) => self.pass_time().rem_euclid(len),
            None => self.elapsed,
        }
    }

    /// Which pass of a loop it is in, counting from 1.
    pub fn pass_number(&self) -> Option<u32> {
        let len = self.pass_len?;
        Some((self.pass_time() / len).floor() as u32 + 1)
    }

    /// Jumping in a cue starts its automatic fade-out over: it is at full
    /// level again until it comes round to fading out once more.
    fn restart_auto_fade(&mut self) {
        self.relevel = self.auto_faded;
        self.auto_faded = false;
    }
    /// A seek has been asked for and is not on screen yet.
    pub fn seeking(&self) -> bool {
        self.seeking.is_some()
    }
    pub fn output(&self) -> Option<OutputId> {
        match &self.kind {
            ActiveKind::Video(v) => v.output,
            ActiveKind::Image(i) => i.output,
            ActiveKind::Matte(m) => m.output,
            _ => None,
        }
    }
    fn is_visual(&self) -> bool {
        matches!(self.kind, ActiveKind::Video(_) | ActiveKind::Image(_))
    }
    /// Anything drawn on an output, including dip mattes.
    fn is_layer(&self) -> bool {
        self.is_visual() || matches!(self.kind, ActiveKind::Matte(_))
    }
    pub fn is_matte(&self) -> bool {
        matches!(self.kind, ActiveKind::Matte(_))
    }
    fn voice(&self) -> Option<&Voice> {
        match &self.kind {
            ActiveKind::Video(v) => v.voice.as_ref(),
            ActiveKind::Audio(v) => Some(v),
            _ => None,
        }
    }
    fn fade_out_and_stop(&mut self, fade: f64, curve: Curve) {
        // A cue held by a control cue lets go to fade: its levels don't move while held.
        if self.hold {
            self.hold = false;
            if let Some(v) = self.voice() {
                v.set_paused(false);
            }
        }
        self.opacity.ramp_to(0.0, fade, curve);
        self.volume.ramp_to(MIN_DB, fade, curve);
        self.stopping = true;
    }
}

pub struct PendingCue {
    pub cue_id: CueId,
    pub remaining: f64,
    pub total: f64,
    /// Not waiting out a time but for the file to be read (ffprobe), which
    /// takes a moment when the cue wasn't read ahead of GO.
    pub reading: Option<Reading>,
}

/// A cue whose start waits for its file to be probed. The rest of the show
/// carries on meanwhile; the cue starts as soon as the answer is in.
pub struct Reading {
    pub file: PathBuf,
    pub since: Instant,
    /// If it is the second half of a dip, how to finish the dip.
    reveal: Option<(u64, f64)>,
}

/// How long to wait for a file to be probed before giving up on the cue.
/// How long a cue whose preparation failed is left alone before another try.
const PREP_RETRY: Duration = Duration::from_secs(5);
const PROBE_LIMIT: Duration = Duration::from_secs(30);

/// What a preload was made for. It is reused only if the cue still asks for
/// the same decoding (see `MediaCue::decodes_like`), so dragging a volume or
/// opacity slider doesn't restart the decoders on every frame.
#[derive(Clone)]
struct PrepSig {
    media: MediaCue,
    size: (u32, u32),
    /// A camera's device and mode, which a file cue doesn't have.
    camera: Option<(String, u32, u32, u32)>,
}

impl PartialEq for PrepSig {
    fn eq(&self, other: &PrepSig) -> bool {
        self.size == other.size
            && self.camera == other.camera
            && self.media.decodes_like(&other.media)
    }
}

enum PreparedKind {
    Video {
        player: VideoPlayer,
        voice: Option<Voice>,
    },
    Image(Receiver<Result<RgbaImage, String>>),
    Audio(Voice),
    /// A title being drawn on a thread (at 4K it takes tens of milliseconds).
    Text(Receiver<Result<RgbaImage, String>>),
}

struct Prepared {
    /// How long one pass of a loop is, for jumping about in it.
    pass_len: Option<f64>,
    speed: Option<std::sync::Arc<crate::speed::SpeedMap>>,
    cue_id: CueId,
    sig: PrepSig,
    kind: PreparedKind,
    duration: Option<f64>,
    /// Something the operator should be told when the cue starts (its sound
    /// couldn't be started, say), though it can play.
    warning: Option<String>,
}

pub enum CueStatus {
    Idle,
    PreWait { remaining: f64, total: f64 },
    Playing { elapsed: f64, duration: Option<f64> },
}

pub struct LayerRef<'a> {
    pub instance: u64,
    pub opacity: f32,
    pub fit: Fit,
    pub transform: LayerTransform,
    pub color: ColorAdjust,
    pub fx: VideoFx,
    pub source: LayerSource<'a>,
}

pub enum LayerSource<'a> {
    Video(&'a VideoPlayer),
    /// The picture and its version (see `ImageState::version`).
    Image(&'a RgbaImage, u64),
    Solid([u8; 4]),
}

pub struct Engine {
    pub ff: Option<Ffmpeg>,
    pub audio: AudioOut,
    pub active: Vec<ActiveCue>,
    pub pending: Vec<PendingCue>,
    /// The playhead of the cue list being worked (`active_list`).
    pub playhead: Option<CueId>,
    /// Keyframed speeds' maps, kept while the keyframes are as they were (tabulating one
    /// is work, and the cue list asks for every cue's length every frame).
    speed_maps: HashMap<CueId, (Animated, std::sync::Arc<crate::speed::SpeedMap>)>,
    /// The cue list GO, the playhead and the transport are for.
    pub active_list: u32,
    /// The playheads the other cue lists have been left at.
    list_playheads: HashMap<u32, Option<CueId>>,
    pub paused: bool,
    pub messages: VecDeque<(Instant, String)>,
    /// Current size of each open output window, used to pick decode resolution.
    pub output_sizes: HashMap<OutputId, (u32, u32)>,
    pub probe: ProbeCache,
    /// The sound of each file, drawn in the inspector for setting in and out points.
    pub waveforms: WaveformCache,
    prepared: Vec<Prepared>,
    /// Cues whose preparation failed, and when: not tried again with the same
    /// settings for a few seconds, so a bad file isn't retried every tick.
    failed_preps: HashMap<CueId, (PrepSig, Instant)>,
    next_instance: u64,
    /// Cameras found on this machine (the test picture first), and a scan under way.
    cameras: Vec<String>,
    camera_scan: Option<Receiver<Vec<String>>>,
    scanned: bool,
    /// Follows incoming MIDI Time Code and says which cues it reaches.
    pub timecode: Follower,
    /// Follows the time of day for cues with a clock time.
    clock: ClockFollower,
    /// Where send cues' messages go.
    outbox: Outbox,
    /// Cues asked to fire while one is being fired (a continue, a group's
    /// children), taken in order once it has started. Starting them from
    /// inside each other would nest once per cue, and a long chain of
    /// auto-continues (or of auto-follows that fail at once, say with their
    /// media missing) would run out of stack.
    fire_queue: VecDeque<CueId>,
    firing: bool,
    /// Cues a control cue asked to be got ready ahead of time (kept ready
    /// until they are fired or reset).
    loads: HashSet<CueId>,
    /// Changes to the show that control cues ask for (the engine only reads
    /// the show); the app makes them after each tick.
    edits: Vec<ShowEdit>,
}

/// A change to the show asked for by a control cue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShowEdit {
    /// Arm (true) or disarm (false) a cue.
    Arm(CueId, bool),
}

impl Engine {
    /// An engine with no ffmpeg and no audio device, for tests.
    #[cfg(test)]
    pub fn for_tests() -> Engine {
        Engine {
            ff: None,
            audio: AudioOut::disabled(),
            active: Vec::new(),
            pending: Vec::new(),
            playhead: None,
            speed_maps: HashMap::new(),
            active_list: 0,
            list_playheads: HashMap::new(),
            paused: false,
            messages: VecDeque::new(),
            output_sizes: HashMap::new(),
            probe: ProbeCache::default(),
            waveforms: WaveformCache::default(),
            prepared: Vec::new(),
            failed_preps: HashMap::new(),
            next_instance: 1,
            cameras: vec![TEST_PATTERN_DEVICE.to_string()],
            camera_scan: None,
            scanned: false,
            timecode: Follower::default(),
            clock: ClockFollower::default(),
            outbox: Outbox::recording(),
            fire_queue: VecDeque::new(),
            firing: false,
            loads: HashSet::new(),
            edits: Vec::new(),
        }
    }

    pub fn new() -> Engine {
        let ff = Ffmpeg::locate();
        match &ff {
            Some(f) => log::info!("using ffmpeg at {}", f.ffmpeg.display()),
            None => log::error!("ffmpeg not found"),
        }
        Engine {
            ff,
            audio: AudioOut::new(),
            active: Vec::new(),
            pending: Vec::new(),
            playhead: None,
            speed_maps: HashMap::new(),
            active_list: 0,
            list_playheads: HashMap::new(),
            paused: false,
            messages: VecDeque::new(),
            output_sizes: HashMap::new(),
            probe: ProbeCache::default(),
            waveforms: WaveformCache::default(),
            prepared: Vec::new(),
            failed_preps: HashMap::new(),
            next_instance: 1,
            cameras: vec![TEST_PATTERN_DEVICE.to_string()],
            camera_scan: None,
            scanned: false,
            timecode: Follower::default(),
            clock: ClockFollower::default(),
            outbox: Outbox::new(),
            fire_queue: VecDeque::new(),
            firing: false,
            loads: HashSet::new(),
            edits: Vec::new(),
        }
    }

    /// The cameras this machine has. The first call starts a scan on a
    /// background thread (ffmpeg takes a moment to list devices), so the
    /// list fills in a little after the picker is first opened.
    pub fn cameras(&mut self) -> &[String] {
        if let Some(rx) = &self.camera_scan
            && let Ok(list) = rx.try_recv()
        {
            self.cameras = list;
            self.camera_scan = None;
        }
        if !self.scanned {
            self.refresh_cameras();
        }
        &self.cameras
    }

    /// Look for cameras again, e.g. after plugging one in.
    pub fn refresh_cameras(&mut self) {
        self.scanned = true;
        let Some(ff) = self.ff.clone() else { return };
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.camera_scan = Some(rx);
        let _ = std::thread::Builder::new()
            .name("camera-scan".into())
            .spawn(move || {
                let _ = tx.send(camera::list_devices(&ff));
            });
    }

    /// The clock now reads `now`: fire the cues whose time of day has just passed.
    /// `gap` is the seconds since the last look, by a steady clock (see
    /// `ClockFollower::advance_after`).
    pub fn on_clock(&mut self, show: &Show, now: TimeOfDay, gap: f64) {
        let triggers: Vec<(CueId, TimeOfDay)> = show
            .cues
            .iter()
            .filter_map(|c| c.clock.map(|t| (c.id, t)))
            .collect();
        for id in self.clock.advance_after(&triggers, now, gap) {
            self.fire(show, id);
        }
    }

    /// MIDI Time Code reached `tc`: fire the cues set for the time it has just passed.
    pub fn on_timecode(&mut self, show: &Show, tc: Timecode, rate: Rate) {
        let triggers: Vec<(CueId, Timecode)> = show
            .cues
            .iter()
            .filter_map(|c| c.timecode.map(|t| (c.id, t)))
            .collect();
        for id in self.timecode.advance(&triggers, tc, rate) {
            self.fire(show, id);
        }
    }

    pub fn log(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        log::info!("{msg}");
        self.messages.push_back((Instant::now(), msg));
        while self.messages.len() > 50 {
            self.messages.pop_front();
        }
    }

    pub fn is_active(&self) -> bool {
        !self.active.is_empty() || !self.pending.is_empty()
    }

    pub fn probe_state(&mut self, path: &Path) -> &ProbeState {
        self.probe.get(self.ff.as_ref(), path)
    }

    /// `MediaCue::speed_map`, kept while the cue's keyframes don't change.
    fn speed_map_cached(
        &mut self,
        id: CueId,
        m: &MediaCue,
    ) -> Option<std::sync::Arc<crate::speed::SpeedMap>> {
        if !m.rate.is_animated() {
            self.speed_maps.remove(&id);
            return None;
        }
        if let Some((keys, map)) = self.speed_maps.get(&id)
            && *keys == m.rate
        {
            return Some(map.clone());
        }
        let map = m.speed_map()?;
        // A few cues at a time have keyframed speeds; a show of thousands doesn't keep thousands.
        if self.speed_maps.len() > 256 {
            self.speed_maps.clear();
        }
        self.speed_maps.insert(id, (m.rate.clone(), map.clone()));
        Some(map)
    }

    /// Duration of a cue's action, if it can be known. `Some(None)` = infinite.
    pub fn cue_duration(&mut self, show: &Show, cue: &Cue) -> Option<Option<f64>> {
        match &cue.kind {
            CueKind::Video(m) | CueKind::Audio(m) => {
                let total = match self.probe_state(&m.file) {
                    ProbeState::Ready(info) => info.duration,
                    _ => None,
                };
                let rate = m.speed();
                // File seconds to the cue's own, at a steady speed or along
                // the keyframes.
                let map = self.speed_map_cached(cue.id, m);
                let cue_secs = |file: f64| match &map {
                    Some(map) => map.time_for(file),
                    None => file / rate,
                };
                match m.playback(total) {
                    // Without an out point or a known length there's nothing to count.
                    Playback::Once { duration: None } => {
                        Some(Some(cue_secs((total? - m.start).max(0.0))))
                    }
                    pb => Some(pb.total(None).map(cue_secs)),
                }
            }
            CueKind::Image(m) => Some(m.end.filter(|d| *d > 0.0)),
            CueKind::Text(t) => Some(t.media.end.filter(|d| *d > 0.0)),
            CueKind::Camera(c) => Some(c.media.end.filter(|d| *d > 0.0)),
            CueKind::LiveAudio(c) => Some(c.media.end.filter(|d| *d > 0.0)),
            CueKind::Fade(f) => Some(Some(f.duration)),
            CueKind::Wait(w) => Some(Some(w.duration)),
            CueKind::Timecode(t) => Some((t.duration > 0.0).then_some(t.duration)),
            CueKind::Stop(_) | CueKind::Send(_) | CueKind::Control(_) => Some(Some(0.0)),
            // Slide show: the slides are timed to end together with the last.
            CueKind::Group(g) if g.slides.is_some() => Some(Some(show.slides_length(cue.id))),
            CueKind::Group(g) => {
                // Timeline: until the last child ends. Playlist: all of them in turn.
                let mut total = 0.0_f64;
                for child in show.armed_children(cue.id) {
                    let Some(d) = self.cue_duration(show, child)? else {
                        return Some(None);
                    };
                    let t = child.pre_wait + d;
                    total = match g.mode {
                        GroupMode::Timeline => total.max(t),
                        GroupMode::Playlist => total + t,
                    };
                }
                Some(Some(total))
            }
        }
    }

    pub fn status(&self, id: CueId) -> CueStatus {
        if let Some(a) = self.active.iter().rev().find(|a| a.cue_id == id) {
            return CueStatus::Playing {
                elapsed: a.elapsed,
                duration: a.duration,
            };
        }
        if let Some(p) = self.pending.iter().find(|p| p.cue_id == id) {
            return CueStatus::PreWait {
                remaining: p.remaining,
                total: p.total,
            };
        }
        CueStatus::Idle
    }

    pub fn is_prepared(&self, id: CueId) -> bool {
        self.prepared.iter().any(|p| p.cue_id == id)
    }

    // ----- Transport -------------------------------------------------------

    /// Fire the cue at the playhead and move the playhead past its continue chain.
    pub fn go(&mut self, show: &Show) {
        let Some(here) = self
            .playhead
            .and_then(|id| show.cue_index(id))
            .map(|i| show.top_index(i))
        else {
            self.log("GO: no cue at the playhead");
            return;
        };
        // Disarmed cues are passed over, as if they weren't in the list.
        let Some(idx) = show.armed_top_from(here) else {
            self.log("GO: every cue from the playhead on is disarmed");
            return;
        };
        // Children never take the playhead: it moves over whole groups.
        let mut end = idx;
        while show.cues[end].continue_mode != ContinueMode::DoNotContinue
            && let Some(next) = show.next_armed_top(end)
        {
            end = next;
        }
        self.playhead = show.next_armed_top(end).map(|i| show.cues[i].id);
        log::info!("GO {} ({})", show.cues[idx].number, show.cues[idx].name);
        self.fire(show, show.cues[idx].id);
    }

    /// Work another cue list: the playhead of the one being left is kept
    /// where it is, and the other's is taken up again (the first cue that
    /// will run, if it hasn't been used).
    pub fn switch_list(&mut self, show: &Show, list: u32) {
        let list = if show.has_list(list) { list } else { 0 };
        if list == self.active_list {
            return;
        }
        self.list_playheads.insert(self.active_list, self.playhead);
        self.active_list = list;
        self.playhead = match self.list_playheads.remove(&list) {
            Some(p) => p.filter(|id| show.cue(*id).is_some_and(|c| c.list == list)),
            None => Self::first_in_list(show, list),
        };
    }

    fn first_in_list(show: &Show, list: u32) -> Option<CueId> {
        let r = show.list_range(list);
        (!r.is_empty())
            .then(|| show.armed_top_from(r.start))
            .flatten()
            .map(|i| show.cues[i].id)
    }

    /// Put the playheads right after the show has changed under them: a list
    /// that is gone is left, and a playhead on a cue that is gone is dropped.
    pub fn sync_lists(&mut self, show: &Show) {
        self.list_playheads
            .retain(|l, _| show.has_list(*l) && *l != self.active_list);
        if !show.has_list(self.active_list) {
            self.active_list = 0;
            self.playhead = Self::first_in_list(show, 0);
        }
    }

    /// Forget where the playheads were (another show was opened).
    pub fn forget_lists(&mut self) {
        self.active_list = 0;
        self.list_playheads.clear();
    }

    /// GO on a cue list that is not the one being worked, leaving that one as
    /// it is. Does nothing if there is no such list.
    pub fn go_list(&mut self, show: &Show, list: u32) {
        if !show.has_list(list) {
            return;
        }
        let was = self.active_list;
        self.switch_list(show, list);
        self.go(show);
        self.switch_list(show, was);
    }

    /// Fire a cue (honouring its pre-wait) without touching the playhead.
    /// Cues it leads to (continues, a group's children) are fired after it,
    /// in order, before this returns.
    pub fn fire(&mut self, show: &Show, id: CueId) {
        self.fire_queue.push_back(id);
        if self.firing {
            return;
        }
        self.firing = true;
        while let Some(id) = self.fire_queue.pop_front() {
            self.fire_one(show, id);
        }
        self.firing = false;
    }

    fn fire_one(&mut self, show: &Show, id: CueId) {
        let Some(cue) = show.cue(id) else { return };
        if show.cue_index(id).is_some_and(|i| !show.is_armed(i)) {
            self.log(format!("Cue {} ({}) is disarmed", cue.number, cue.name));
            return;
        }
        if self.paused {
            self.set_paused(false);
        }
        // Fired: it is no longer waiting to be got ready.
        self.loads.remove(&id);
        if cue.pre_wait > 0.0 {
            self.pending.push(PendingCue {
                cue_id: id,
                remaining: cue.pre_wait,
                total: cue.pre_wait,
                reading: None,
            });
        } else {
            self.start(show, id);
        }
    }

    /// Fire the cue after `after` in the list. A group's children don't chain:
    /// their group decides when each one starts.
    fn fire_next(&mut self, show: &Show, after: CueId) {
        let Some(i) = show.cue_index(after) else {
            return;
        };
        if show.cues[i].parent.is_some() {
            return;
        }
        if let Some(next) = show.next_armed_top(i) {
            self.fire(show, show.cues[next].id);
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        for a in &self.active {
            if a.started
                && let Some(v) = a.voice()
            {
                v.set_paused(paused || a.hold);
            }
        }
    }

    /// The changes to the show that control cues have asked for since the last
    /// call (arming and disarming), to be made by whoever owns the show.
    pub fn take_show_edits(&mut self) -> Vec<ShowEdit> {
        std::mem::take(&mut self.edits)
    }

    /// Hold the cue `id` (a group's children too), or let it go on.
    fn hold_cue(&mut self, show: &Show, id: CueId, hold: bool) -> bool {
        let targets = show.with_children(id);
        let paused = self.paused;
        let mut hit = false;
        for a in &mut self.active {
            if !targets.contains(&a.cue_id) || a.is_matte() {
                continue;
            }
            hit = true;
            a.hold = hold;
            if a.started
                && let Some(v) = a.voice()
            {
                v.set_paused(paused || hold);
            }
        }
        hit
    }

    /// Let loops that are playing in `id` (a group's children too) finish
    /// the pass they are on and go on from there.
    fn devamp(&mut self, show: &Show, id: CueId) -> bool {
        let targets = show.with_children(id);
        let mut hit = false;
        for a in &mut self.active {
            if !targets.contains(&a.cue_id) || a.stopping {
                continue;
            }
            let Some(len) = a.pass_len else { continue };
            // The end of the pass that is playing, in the cue's time.
            let end = match &a.speed {
                Some(map) => map.time_for(((map.media_at(a.elapsed) / len).floor() + 1.0) * len),
                None => ((a.elapsed / len).floor() + 1.0) * len,
            };
            // A loop that plays on goes on to the rest of the file instead.
            if let ActiveKind::Audio(vo) = &a.kind
                && vo.release()
            {
                a.duration = Some(end + vo.tail_len());
                a.pass_len = None;
                hit = true;
                continue;
            }
            a.duration = Some(a.duration.map_or(end, |d| d.min(end)));
            hit = true;
        }
        hit
    }

    /// Stop everything, forget what was asked to be ready and put the
    /// playhead back at the top of the list.
    pub fn reset_all(&mut self, show: &Show) {
        self.stop_all(0.0);
        self.loads.clear();
        self.list_playheads.clear();
        for l in show.list_ids() {
            let first = Self::first_in_list(show, l);
            if l == self.active_list {
                self.playhead = first;
            } else {
                self.list_playheads.insert(l, first);
            }
        }
    }

    /// Do what a control cue says.
    fn control(&mut self, show: &Show, cue: &Cue, c: &ControlCue) {
        if !c.action.needs_target() {
            return;
        }
        let name = format!("{} ({})", cue.number, cue.name);
        let Some(target) = c.target.filter(|t| show.cue(*t).is_some()) else {
            self.log(format!("Cue {name}: it has no cue to act on"));
            return;
        };
        self.control_target(show, c.action, target, &name);
    }

    /// Do `action` to the cue `target`; `from` is who asked, for the log.
    pub fn control_target(
        &mut self,
        show: &Show,
        action: ControlAction,
        target: CueId,
        from: &str,
    ) {
        let name = from;
        let tname = show
            .cue(target)
            .map(|t| format!("{} ({})", t.number, t.name))
            .unwrap_or_default();
        match action {
            ControlAction::Start => self.fire(show, target),
            ControlAction::Pause => {
                if !self.hold_cue(show, target, true) {
                    self.log(format!("Cue {name}: {tname} is not playing"));
                }
            }
            ControlAction::Resume => {
                if !self.hold_cue(show, target, false) {
                    self.log(format!("Cue {name}: {tname} is not playing"));
                }
            }
            ControlAction::Load => {
                for id in show.with_children(target) {
                    self.loads.insert(id);
                }
            }
            ControlAction::Reset => {
                for id in show.with_children(target) {
                    self.loads.remove(&id);
                    self.prepared.retain(|p| p.cue_id != id);
                    self.stop_cue(id, 0.0);
                }
            }
            ControlAction::GoTo => {
                let top = show.top_id(target);
                let list = show.cue(top).map_or(0, |c| c.list);
                if list == self.active_list {
                    self.playhead = Some(top);
                } else {
                    self.list_playheads.insert(list, Some(top));
                }
            }
            ControlAction::Arm => self.edits.push(ShowEdit::Arm(target, true)),
            ControlAction::Disarm => self.edits.push(ShowEdit::Arm(target, false)),
            ControlAction::Devamp => {
                if !self.devamp(show, target) {
                    self.log(format!("Cue {name}: {tname} is not a loop that is playing"));
                }
            }
            ControlAction::Memo => {}
        }
    }

    /// Stop everything with the given fade time.
    pub fn stop_all(&mut self, fade: f64) {
        self.pending.clear();
        if fade <= 0.0 {
            self.active.clear();
            return;
        }
        self.active.retain_mut(|a| {
            if a.is_layer() || a.voice().is_some() {
                a.fade_out_and_stop(fade, Curve::SCurve);
                true
            } else {
                false
            }
        });
    }

    /// Esc: fade everything out; pressing again during the fade stops instantly.
    pub fn panic(&mut self, show: &Show) {
        if self.paused {
            self.set_paused(false);
        }
        if self.active.iter().any(|a| a.panic) || self.active.is_empty() {
            self.stop_all(0.0);
            self.log("Stopped all");
        } else {
            self.stop_all(show.panic_fade);
            for a in &mut self.active {
                a.panic = true;
            }
            self.log(format!(
                "Fading out all ({:.1}s), press again to stop now",
                show.panic_fade
            ));
        }
    }

    /// The children of a running group instance.
    fn group_children(&self, instance: u64) -> Option<Vec<CueId>> {
        self.active.iter().find_map(|a| match &a.kind {
            ActiveKind::Group(g) if a.instance == instance => Some(g.children.clone()),
            _ => None,
        })
    }

    pub fn stop_instance(&mut self, instance: u64, fade: f64) {
        // Stopping a group stops its children; the group itself just ends.
        if let Some(kids) = self.group_children(instance) {
            self.active.retain(|a| a.instance != instance);
            for k in kids {
                self.stop_cue(k, fade);
            }
            return;
        }
        if fade <= 0.0 {
            self.active.retain(|a| a.instance != instance);
        } else if let Some(a) = self.active.iter_mut().find(|a| a.instance == instance) {
            a.fade_out_and_stop(fade, Curve::SCurve);
        }
    }

    /// Fade a playing instance out using its cue's own fade-out time.
    pub fn fade_out_instance(&mut self, show: &Show, instance: u64) {
        if let Some(kids) = self.group_children(instance) {
            self.active.retain(|a| a.instance != instance);
            for k in kids {
                self.fade_out_cue(show, k);
            }
            return;
        }
        let fade = self
            .active
            .iter()
            .find(|a| a.instance == instance)
            .and_then(|a| show.cue(a.cue_id))
            .and_then(|c| c.kind.media())
            .map_or(DEFAULT_FADE_OUT, MediaCue::manual_fade_out);
        self.stop_instance(instance, fade);
    }

    /// Jump a playing cue to `to` seconds from its start. The decoders for the
    /// new position start now and take over when they have a frame; asking
    /// again meanwhile replaces them.
    pub fn seek(&mut self, show: &Show, instance: u64, to: f64) -> Result<(), String> {
        let a = self
            .active
            .iter()
            .find(|a| a.instance == instance)
            .ok_or("that cue is no longer playing")?;
        let length = a.scrub_length(show).ok_or("that cue can't be scrubbed")?;
        let to = to.clamp(0.0, (length - 0.1).max(0.0));
        // A loop with a keyframed speed that never ends has its bar in the
        // file's time: jump to where in this pass of the file `to` is, which
        // is a time in the cue that keeps the speed where the keyframes put it.
        let to = match (&a.speed, a.loops_forever()) {
            (Some(map), true) => {
                let len = a.pass_len.unwrap_or(1.0).max(1e-6);
                let pass = (map.media_at(a.elapsed) / len).floor();
                map.time_for(pass * len + to)
            }
            _ => to,
        };
        let cue = show.cue(a.cue_id).ok_or("that cue was deleted")?.clone();
        let is_image = matches!(a.kind, ActiveKind::Image(_));
        let (player, voice) = if is_image {
            (None, None)
        } else {
            let file = cue.kind.file_media().ok_or("not a file cue")?.file.clone();
            // It is playing, so it has been read.
            let info = match self.probe.get(self.ff.as_ref(), &file) {
                ProbeState::Ready(info) => info.clone(),
                ProbeState::Failed(e) => return Err(e.clone()),
                ProbeState::Pending => return Err("still reading the file".into()),
            };
            match self.prepare_from(&cue, &info, to)?.kind {
                PreparedKind::Video { player, voice } => (Some(player), voice),
                PreparedKind::Audio(voice) => (None, Some(voice)),
                _ => return Err("that cue can't be scrubbed".into()),
            }
        };
        let a = self
            .active
            .iter_mut()
            .find(|a| a.instance == instance)
            .ok_or("that cue is no longer playing")?;
        if is_image {
            a.elapsed = to;
            a.restart_auto_fade();
        } else {
            a.seeking = Some(Seek {
                player,
                voice,
                base: to,
                spawned_at: Instant::now(),
            });
        }
        Ok(())
    }

    /// Fade every instance of a cue out using its own fade-out time.
    pub fn fade_out_cue(&mut self, show: &Show, id: CueId) {
        self.pending.retain(|p| p.cue_id != id);
        let instances: Vec<u64> = self
            .active
            .iter()
            .filter(|a| a.cue_id == id)
            .map(|a| a.instance)
            .collect();
        for i in instances {
            self.fade_out_instance(show, i);
        }
    }

    pub fn stop_cue(&mut self, id: CueId, fade: f64) {
        self.pending.retain(|p| p.cue_id != id);
        let instances: Vec<u64> = self
            .active
            .iter()
            .filter(|a| a.cue_id == id)
            .map(|a| a.instance)
            .collect();
        for i in instances {
            self.stop_instance(i, fade);
        }
    }

    // ----- Starting cues ---------------------------------------------------

    fn output_size(&self, output: Option<OutputId>) -> (u32, u32) {
        output
            .and_then(|o| self.output_sizes.get(&o).copied())
            .unwrap_or(DEFAULT_OUTPUT_SIZE)
    }

    fn signature(&self, cue: &Cue) -> Option<PrepSig> {
        if let CueKind::Camera(c) = &cue.kind {
            return Some(PrepSig {
                media: c.media.clone(),
                size: self.output_size(c.media.output),
                camera: Some((c.device.clone(), c.width, c.height, c.fps)),
            });
        }
        let m = cue.kind.file_media()?;
        Some(PrepSig {
            media: m.clone(),
            size: self.output_size(m.output),
            camera: None,
        })
    }

    /// Open a camera. It starts delivering frames at once and keeps only the
    /// newest, so it can be opened ahead of GO.
    fn prepare_camera(&self, cue: &Cue, c: &CameraCue) -> Result<Prepared, String> {
        let sig = self.signature(cue).ok_or("not a camera cue")?;
        if c.device.is_empty() {
            return Err("no camera chosen".into());
        }
        let size = video::decode_size((c.width, c.height), sig.size, c.media.fit == Fit::Cover);
        if let Some(source) = crate::ndi::source_of(&c.device) {
            let matrix = if c.height >= 720 {
                ColorMatrix::Bt709
            } else {
                ColorMatrix::Bt601
            };
            let player = VideoPlayer::spawn_ndi(source, matrix, c.fps, size)
                .map_err(|e| format!("{e:#}"))?;
            return Ok(Prepared {
                cue_id: cue.id,
                pass_len: None,
                speed: None,
                sig,
                kind: PreparedKind::Video {
                    player,
                    voice: None,
                },
                duration: c.media.end.filter(|d| *d > 0.0),
                warning: None,
            });
        }
        let ff = self.ff.as_ref().ok_or("ffmpeg not found")?;
        let input = camera::input_args(
            camera::Platform::current(),
            &c.device,
            c.width,
            c.height,
            c.fps,
        );
        let matrix = if c.height >= 720 {
            ColorMatrix::Bt709
        } else {
            ColorMatrix::Bt601
        };
        let player = VideoPlayer::spawn_live(ff, input, matrix, c.fps, size)
            .map_err(|e| format!("{e:#}"))?;
        Ok(Prepared {
            cue_id: cue.id,
            pass_len: None,
            speed: None,
            sig,
            kind: PreparedKind::Video {
                player,
                voice: None,
            },
            duration: c.media.end.filter(|d| *d > 0.0),
            warning: None,
        })
    }

    /// Open an audio input as a voice. It is opened when the cue starts, not
    /// ahead of GO.
    fn prepare_live_audio(&self, cue: &Cue, c: &LiveAudioCue) -> Result<Prepared, String> {
        let m = &c.media;
        let initial_gain = if m.fade_in > 0.0 {
            0.0
        } else {
            db_to_gain(m.volume_db)
        };
        let voice = self
            .audio
            .spawn_input_voice(
                c.device.as_deref(),
                c.input_channel,
                initial_gain,
                (m.audio_output, m.audio_channel),
                &m.sound_chain(),
            )
            .map_err(|e| format!("{e:#}"))?;
        Ok(Prepared {
            cue_id: cue.id,
            pass_len: None,
            speed: None,
            sig: PrepSig {
                media: m.clone(),
                size: (1, 1),
                camera: None,
            },
            kind: PreparedKind::Audio(voice),
            duration: m.end.filter(|d| *d > 0.0),
            warning: None,
        })
    }

    /// The cue's crosspoints as the mixer wants them, for a file with
    /// `file_channels` channels; None when the cue routes a stereo pair.
    fn matrix_for(
        m: &crate::show::MediaCue,
        file_channels: u16,
    ) -> Option<crate::media::audio::Matrix> {
        if m.matrix.is_empty() {
            return None;
        }
        let have = usize::from(file_channels.max(1));
        // Silent crosspoints stay, so that a fade cue can bring them up.
        let kept: Vec<&crate::show::Crosspoint> = m
            .matrix
            .iter()
            .filter(|p| {
                usize::from(p.from) < have.min(crate::media::audio::MAX_SOURCE_CHANNELS)
                    && p.db.is_finite()
            })
            .collect();
        let points: Vec<(usize, usize, f32)> = kept
            .iter()
            .map(|p| {
                let g = if p.db > -120.0 { db_to_gain(p.db) } else { 0.0 };
                (usize::from(p.from), usize::from(p.to), g)
            })
            .collect();
        let keys: Vec<(u8, u16)> = kept.iter().map(|p| (p.from, p.to)).collect();
        let max = crate::media::audio::MAX_SOURCE_CHANNELS;
        Some(crate::media::audio::Matrix {
            points,
            keys,
            source_channels: have.min(max),
            crop: have > max,
        })
    }

    /// Spawn the decoders for a media cue. They start paused/buffering.
    fn prepare(&self, cue: &Cue, info: &MediaInfo) -> Result<Prepared, String> {
        self.prepare_from(cue, info, 0.0)
    }

    /// Like `prepare`, but starting `shift` seconds into the cue (a seek).
    fn prepare_from(&self, cue: &Cue, info: &MediaInfo, shift: f64) -> Result<Prepared, String> {
        let ff = self.ff.as_ref().ok_or("ffmpeg not found")?;
        let sig = self.signature(cue).ok_or("not a media cue")?;
        let mut shifted = sig.media.clone();
        // A keyframed speed is followed from where the cue is started (a
        // seek); otherwise `shift`, in the cue's time, is `rate` times as much
        // of the file.
        let speed = shifted
            .speed_map()
            .map(|map| crate::speed::SpeedPlan { map, t0: shift });
        let (rate, shift_file) = match &speed {
            Some(p) => (1.0, p.map.media_at(shift)),
            None => (shifted.speed(), shift * shifted.speed()),
        };
        let looping = shifted.loop_media && shifted.playback(info.duration).pass_len().is_some();
        // A loop is jumped about in without moving its in point: the first
        // pass starts part way in and the ones after it are whole.
        if !looping {
            shifted.start += shift_file;
        }
        let m = &shifted;
        let mut playback = m.playback(info.duration);
        let pass_len = playback.pass_len().map(|l| l / rate);
        if looping && shift > 0.0 {
            playback = playback.started_at(shift_file);
        }
        let duration = match playback {
            Playback::Once { duration } => {
                duration.or(info.duration.map(|d| (d - m.start).max(0.0)))
            }
            pb => pb.total(None),
        }
        .map(|d| match &speed {
            Some(p) => p.time_after(d),
            None => d / rate,
        });
        let initial_gain = if m.fade_in > 0.0 {
            0.0
        } else {
            db_to_gain(m.volume_db)
        };
        let mut warning = None;
        let matrix = Self::matrix_for(m, info.audio_channels);
        // A loop that never ends can go on past its out point once devamped.
        // (Not with a keyframed speed: where its pass ends in the cue's time varies.)
        let play_on = match (m.play_on && speed.is_none(), playback, info.duration) {
            (
                true,
                Playback::Slice { len, passes: None }
                | Playback::SliceFrom {
                    len, passes: None, ..
                },
                Some(total),
            ) if total - (m.start + len) > 0.05 => Some(crate::media::audio::PlayOn {
                from: m.start + len,
                len: (total - (m.start + len)) / rate,
            }),
            _ => None,
        };
        let kind = match &cue.kind {
            CueKind::Video(_) => {
                let vi = info.video.as_ref().ok_or("file has no video stream")?;
                let size = video::decode_size((vi.width, vi.height), sig.size, m.fit == Fit::Cover);
                // Hap goes to the graphics card as it is; anything else is decoded and scaled.
                let player = if video::plays_as_hap(vi) {
                    VideoPlayer::spawn_hap(ff, &m.file, vi, m.start, playback, rate, speed.clone())
                } else {
                    VideoPlayer::spawn_at_rate(
                        ff,
                        &m.file,
                        vi,
                        m.start,
                        playback,
                        size,
                        rate,
                        speed.clone(),
                    )
                }
                .map_err(|e| format!("{e:#}"))?;
                let voice = if info.has_audio && self.audio.available() {
                    match self.audio.spawn_voice_routed(
                        ff,
                        &m.file,
                        m.start,
                        playback,
                        initial_gain,
                        (m.audio_output, m.audio_channel),
                        &m.sound_chain(),
                        rate,
                        matrix.as_ref(),
                        None,
                        speed.clone(),
                    ) {
                        Ok(voice) => Some(voice),
                        Err(e) => {
                            warning = Some(format!("playing without sound: {e:#}"));
                            None
                        }
                    }
                } else {
                    None
                };
                PreparedKind::Video { player, voice }
            }
            CueKind::Image(_) => {
                let vi = info.video.as_ref().ok_or("file is not an image")?;
                let size = video::decode_size((vi.width, vi.height), sig.size, m.fit == Fit::Cover);
                PreparedKind::Image(video::load_image(ff, &m.file, size))
            }
            CueKind::Audio(_) => {
                if !info.has_audio {
                    return Err("file has no audio stream".into());
                }
                let voice = self
                    .audio
                    .spawn_voice_routed(
                        ff,
                        &m.file,
                        m.start,
                        playback,
                        initial_gain,
                        (m.audio_output, m.audio_channel),
                        &m.sound_chain(),
                        rate,
                        matrix.as_ref(),
                        play_on,
                        speed.clone(),
                    )
                    .map_err(|e| format!("{e:#}"))?;
                PreparedKind::Audio(voice)
            }
            _ => return Err("not a media cue".into()),
        };
        let duration = match &cue.kind {
            CueKind::Image(_) => m.end.filter(|d| *d > 0.0),
            _ => duration,
        };
        Ok(Prepared {
            cue_id: cue.id,
            pass_len,
            speed: speed.as_ref().map(|p| p.map.clone()),
            sig,
            kind,
            duration,
            warning,
        })
    }

    /// Draw a text cue at its output's size, on a thread.
    fn prepare_text(&self, cue: &Cue, t: &TextCue) -> Prepared {
        let size = self.output_size(t.media.output);
        Prepared {
            pass_len: None,
            speed: None,
            cue_id: cue.id,
            sig: PrepSig {
                media: t.media.clone(),
                size,
                camera: None,
            },
            kind: PreparedKind::Text(draw_text_on_thread(
                crate::live_text::resolve(t, 0.0, crate::live_text::now_hms())
                    .unwrap_or_else(|| t.clone()),
                size,
            )),
            duration: t.media.end.filter(|d| *d > 0.0),
            warning: None,
        }
    }

    fn take_prepared(&mut self, cue: &Cue) -> Option<Prepared> {
        let sig = self.signature(cue)?;
        let idx = self
            .prepared
            .iter()
            .position(|p| p.cue_id == cue.id && p.sig == sig)?;
        Some(self.prepared.remove(idx))
    }

    fn alloc_instance(&mut self) -> u64 {
        let i = self.next_instance;
        self.next_instance += 1;
        i
    }

    fn start(&mut self, show: &Show, id: CueId) {
        self.start_with(show, id, None);
    }

    /// Begin a dip: fade a colour matte up over the output. The cue itself is
    /// started at the midpoint (see `tick`).
    fn begin_dip(&mut self, show: &Show, cue: &Cue, m: &MediaCue, color: [u8; 4]) {
        let half = (m.fade_in / 2.0).max(0.0);
        // Outgoing content stays visible under the matte; its sound fades
        // out, and it is removed once the matte is opaque.
        for other in &mut self.active {
            if other.is_layer() && other.output() == m.output && !other.stopping {
                other.volume.ramp_to(MIN_DB, half, m.curve);
                let v = other.opacity.value;
                other.opacity.ramp_to(v, half, m.curve);
                other.stopping = true;
            }
        }
        let mut opacity = Param::new(0.0);
        opacity.ramp_to(1.0, half, m.curve);
        let instance = self.alloc_instance();
        self.active.push(ActiveCue {
            instance,
            cue_id: cue.id,
            kind: ActiveKind::Matte(MatteState {
                output: m.output,
                color,
                half,
                curve: m.curve,
                fired: false,
                fired_at: 0.0,
            }),
            elapsed: 0.0,
            duration: Some(m.fade_in),
            pass_len: None,
            speed: None,
            live_transform: None,
            opacity,
            volume: Param::new(0.0),
            started: true,
            hold: false,
            stopping: false,
            held: false,
            seek_base: 0.0,
            audio_lead: 0.0,
            video_lag: 0.0,
            age: 0.0,
            seeking: None,
            relevel: false,
            lost_logged: false,
            faded: false,
            auto_faded: false,
            followed: true,
            panic: false,
            entry: None,
        });
        if cue.continue_mode == ContinueMode::AutoContinue {
            self.fire_next(show, cue.id);
        }
    }

    /// Start a cue. `reveal` is set when this is the second half of a dip:
    /// (matte instance, time to fade the matte away).
    fn start_with(&mut self, show: &Show, id: CueId, reveal: Option<(u64, f64)>) {
        let Some(cue) = show.cue(id).cloned() else {
            return;
        };
        let dips = match &cue.kind {
            CueKind::Video(m) | CueKind::Image(m) => Some(m),
            CueKind::Text(t) => Some(&t.media),
            CueKind::Camera(c) => Some(&c.media),
            _ => None,
        };
        if reveal.is_none()
            && let Some(m) = dips
            && let Some(color) = m.transition.dip_color()
        {
            self.begin_dip(show, &cue, m, color);
            return;
        }
        let instance = self.alloc_instance();
        let mut active = ActiveCue {
            instance,
            cue_id: id,
            kind: ActiveKind::Wait,
            elapsed: 0.0,
            duration: None,
            pass_len: None,
            speed: None,
            live_transform: None,
            opacity: Param::new(1.0),
            volume: Param::new(0.0),
            started: true,
            hold: false,
            stopping: false,
            held: false,
            seek_base: 0.0,
            audio_lead: 0.0,
            video_lag: 0.0,
            age: 0.0,
            seeking: None,
            relevel: false,
            lost_logged: false,
            faded: false,
            auto_faded: false,
            followed: false,
            panic: false,
            entry: None,
        };
        let mut ok = true;
        match &cue.kind {
            CueKind::Video(_)
            | CueKind::Image(_)
            | CueKind::Audio(_)
            | CueKind::Text(_)
            | CueKind::Camera(_)
            | CueKind::LiveAudio(_) => {
                let m = cue.kind.media().expect("these cues have playback settings");
                let prepared = match self.take_prepared(&cue) {
                    Some(p) => Ok(p),
                    None => match (&cue.kind, &self.ff) {
                        (CueKind::Text(t), _) => Ok(self.prepare_text(&cue, t)),
                        (CueKind::LiveAudio(c), _) => self.prepare_live_audio(&cue, c),
                        (_, None) => Err("ffmpeg not found".to_string()),
                        (CueKind::Camera(c), Some(_)) => self.prepare_camera(&cue, c),
                        (_, Some(ff)) => {
                            let ff = ff.clone();
                            // Not read yet: don't make the whole show wait for
                            // ffprobe. The cue starts when the answer is in.
                            let known = match self.probe.get_urgent(Some(&ff), &m.file) {
                                ProbeState::Ready(info) => Ok(info.clone()),
                                ProbeState::Failed(e) => Err(e.clone()),
                                ProbeState::Pending => {
                                    self.pending.push(PendingCue {
                                        cue_id: id,
                                        remaining: 0.0,
                                        total: 1.0,
                                        reading: Some(Reading {
                                            file: m.file.clone(),
                                            since: Instant::now(),
                                            reveal,
                                        }),
                                    });
                                    return;
                                }
                            };
                            known.and_then(|info| self.prepare(&cue, &info))
                        }
                    },
                };
                match prepared {
                    Err(e) => {
                        self.log(format!("Cue {} ({}): {e}", cue.number, cue.name));
                        ok = false;
                    }
                    Ok(p) => {
                        if let Some(w) = &p.warning {
                            self.log(format!("Cue {} ({}): {w}", cue.number, cue.name));
                        }
                        active.duration = p.duration;
                        active.pass_len = p.pass_len;
                        active.speed = p.speed.clone();
                        active.started = false;
                        active.audio_lead = show.audio_delay_ms(m.audio_output) as f64 / 1000.0;
                        active.video_lag = show.video_delay_ms(m.output) as f64 / 1000.0;
                        active.kind = match p.kind {
                            PreparedKind::Video { player, voice } => {
                                ActiveKind::Video(Box::new(VideoState {
                                    player,
                                    voice,
                                    output: m.output,
                                    layer: m.layer,
                                    fit: m.fit,
                                    transform: m.transform,
                                    color: m.color,
                                    fx: VideoFx::at(&m.effects, 0.0),
                                    spawned_at: Instant::now(),
                                }))
                            }
                            PreparedKind::Image(loader) => ActiveKind::Image(ImageState {
                                image: None,
                                version: 0,
                                loader: Some(loader),
                                output: m.output,
                                layer: m.layer,
                                fit: m.fit,
                                transform: m.transform,
                                color: m.color,
                                fx: VideoFx::at(&m.effects, 0.0),
                                live: None,
                            }),
                            PreparedKind::Audio(voice) => ActiveKind::Audio(voice),
                            // Text is drawn at the output's size, so it is never fitted.
                            PreparedKind::Text(loader) => ActiveKind::Image(ImageState {
                                image: None,
                                version: 0,
                                loader: Some(loader),
                                output: m.output,
                                layer: m.layer,
                                fit: Fit::Stretch,
                                transform: m.transform,
                                color: m.color,
                                fx: VideoFx::at(&m.effects, 0.0),
                                live: match &cue.kind {
                                    CueKind::Text(t) if t.source.is_live() => {
                                        crate::live_text::resolve(
                                            t,
                                            0.0,
                                            crate::live_text::now_hms(),
                                        )
                                        .map(|r| {
                                            LiveText {
                                                size: self.output_size(m.output),
                                                drawn: live_key(&r),
                                                pending: None,
                                            }
                                        })
                                    }
                                    _ => None,
                                },
                            }),
                        };
                        let transition = if active.is_visual() {
                            m.transition
                        } else {
                            Transition::FadeIn
                        };
                        // (fade-in time for opacity, for volume)
                        let (fade_op, fade_vol) = match (reveal, transition) {
                            (Some((matte, time)), _) => {
                                active.entry = Some(EntryAction::Reveal {
                                    matte,
                                    time,
                                    curve: m.curve,
                                });
                                (0.0, time)
                            }
                            (None, Transition::Cut) => {
                                active.entry = Some(EntryAction::Cut);
                                (0.0, 0.0)
                            }
                            (None, Transition::Crossfade) => {
                                active.entry = Some(EntryAction::Crossfade {
                                    time: m.fade_in,
                                    curve: m.curve,
                                });
                                (m.fade_in, m.fade_in)
                            }
                            (None, _) => (m.fade_in, m.fade_in),
                        };
                        active.opacity = Param::new(if fade_op > 0.0 { 0.0 } else { m.opacity });
                        active.opacity.ramp_to(m.opacity, fade_op, m.curve);
                        active.volume =
                            Param::new(if fade_vol > 0.0 { MIN_DB } else { m.volume_db });
                        active.volume.ramp_to(m.volume_db, fade_vol, m.curve);
                    }
                }
            }
            CueKind::Fade(f) => {
                active.kind = ActiveKind::Fade;
                active.duration = Some(f.duration);
                let mut hit = false;
                // A group as target means its children.
                let targets = f.target.map(|t| show.with_children(t)).unwrap_or_default();
                for t in &mut self.active {
                    if !targets.contains(&t.cue_id) || t.stopping {
                        continue;
                    }
                    hit = true;
                    t.faded = true;
                    if let Some(o) = f.opacity {
                        t.opacity.ramp_to(o, f.duration, f.curve);
                    }
                    if let Some(v) = f.volume_db {
                        t.volume.ramp_to(v, f.duration, f.curve);
                    }
                    if !f.geometry.is_empty()
                        && let Some(m) = show.cue(t.cue_id).and_then(|c| c.kind.media())
                    {
                        // From where the picture is now.
                        let now = match &t.live_transform {
                            Some(lt) => lt.apply(&m.transform),
                            None => m.motion.transform_at(&m.transform, t.elapsed),
                        };
                        let lt = t
                            .live_transform
                            .get_or_insert_with(|| LiveTransform::from(&now));
                        let g = f.geometry;
                        for (param, to) in [
                            (&mut lt.x, g.x),
                            (&mut lt.y, g.y),
                            (&mut lt.scale_x, g.scale_x),
                            (&mut lt.scale_y, g.scale_y),
                            (&mut lt.rotation, g.rotation),
                        ] {
                            if let Some(to) = to.filter(|v| v.is_finite()) {
                                param.ramp_to(to, f.duration, f.curve);
                            }
                        }
                    }
                    if let Some(v) = t.voice() {
                        for c in &f.matrix {
                            if c.db.is_finite() {
                                v.fade_crosspoint(c.from, c.to, c.db, f.duration);
                            }
                        }
                    }
                    if f.stop_target {
                        // Ensure the stop happens after `duration` even if no level changes.
                        if f.opacity.is_none() {
                            t.opacity.ramp_to(t.opacity.value, f.duration, f.curve);
                        }
                        t.stopping = true;
                    }
                }
                if f.stop_target {
                    // Cues of the target still waiting to start won't start.
                    let waiting = self.pending.len();
                    self.pending.retain(|p| !targets.contains(&p.cue_id));
                    hit |= self.pending.len() != waiting;
                }
                if !hit {
                    let target = f
                        .target
                        .and_then(|t| show.cue(t))
                        .map(|c| format!("{} ({})", c.number, c.name))
                        .unwrap_or_else(|| "none".into());
                    self.log(format!(
                        "Fade cue {}: target {target} is not playing",
                        cue.number
                    ));
                }
            }
            CueKind::Stop(s) => {
                ok = false; // instantaneous; nothing stays active
                match (s.target, s.use_own_fade) {
                    (None, false) => self.stop_all(s.fade),
                    (None, true) => {
                        self.pending.clear();
                        let all: Vec<u64> = self.active.iter().map(|a| a.instance).collect();
                        for i in all {
                            self.fade_out_instance(show, i);
                        }
                    }
                    (Some(t), false) => self.stop_cue(t, s.fade),
                    (Some(t), true) => self.fade_out_cue(show, t),
                }
            }
            CueKind::Wait(w) => {
                active.kind = ActiveKind::Wait;
                active.duration = Some(w.duration);
            }
            CueKind::Timecode(t) => {
                active.duration = (t.duration > 0.0).then_some(t.duration);
                active.kind = ActiveKind::Timecode(self.outbox.start_timecode(
                    t.port.clone(),
                    t.start,
                    t.rate,
                ));
            }
            CueKind::Control(c) => {
                ok = false; // instantaneous; nothing stays active
                self.control(show, &cue, c);
            }
            CueKind::Send(s) => {
                ok = false; // instantaneous; nothing stays active
                self.outbox.send(Outgoing::from_cue(s));
            }
            CueKind::Group(g) => {
                let children: Vec<CueId> = show.armed_children(id).iter().map(|c| c.id).collect();
                if children.is_empty() {
                    ok = false; // nothing to wait for
                } else {
                    active.duration = self.cue_duration(show, &cue).flatten();
                    let first = match g.mode {
                        GroupMode::Timeline => children.len(),
                        GroupMode::Playlist => 1,
                    };
                    for &child in &children[..first] {
                        self.fire(show, child);
                    }
                    active.kind = ActiveKind::Group(GroupState {
                        mode: g.mode,
                        children,
                        next: first,
                    });
                }
            }
        }
        if ok {
            self.active.push(active);
        } else {
            // A failed dip still has to fade its matte away.
            if let Some((matte, time)) = reveal {
                self.apply_entry(
                    0,
                    None,
                    EntryAction::Reveal {
                        matte,
                        time,
                        curve: Curve::SCurve,
                    },
                );
            }
            if cue.continue_mode == ContinueMode::AutoFollow {
                self.fire_next(show, id);
            }
        }
        // For a dip, continues were handled when the dip began.
        if cue.continue_mode == ContinueMode::AutoContinue && reveal.is_none() {
            self.fire_next(show, id);
        }
    }

    /// Run a cue's entry action once its first frame is on screen.
    fn apply_entry(&mut self, instance: u64, output: Option<OutputId>, e: EntryAction) {
        match e {
            EntryAction::Cut => self
                .active
                .retain(|o| o.instance == instance || !(o.is_layer() && o.output() == output)),
            EntryAction::Crossfade { time, curve } => {
                for o in &mut self.active {
                    if o.instance != instance && o.is_layer() && o.output() == output && !o.stopping
                    {
                        o.fade_out_and_stop(time, curve);
                    }
                }
            }
            EntryAction::Reveal { matte, time, curve } => {
                let out = self
                    .active
                    .iter()
                    .find(|a| a.instance == matte)
                    .and_then(|a| a.output());
                self.active.retain(|o| {
                    o.instance == instance
                        || o.instance == matte
                        || !(o.is_layer() && o.output() == out)
                });
                if let Some(m) = self.active.iter_mut().find(|a| a.instance == matte) {
                    m.opacity.ramp_to(0.0, time, curve);
                    m.stopping = true;
                }
            }
        }
    }

    // ----- Per-frame update --------------------------------------------------

    pub fn tick(&mut self, show: &Show, dt: f64) {
        self.probe.poll();
        self.waveforms.poll();
        self.timecode.tick(dt);
        for problem in self.outbox.errors() {
            self.log(problem);
        }

        if !self.paused {
            let mut due: Vec<(CueId, Option<(u64, f64)>)> = Vec::new();
            let mut gave_up: Vec<CueId> = Vec::new();
            let probe = &self.probe;
            self.pending.retain_mut(|p| {
                if let Some(r) = &p.reading {
                    // Waiting for the file to be read, not for time to pass.
                    return match probe.peek(&r.file) {
                        Some(ProbeState::Pending) if r.since.elapsed() < PROBE_LIMIT => true,
                        Some(ProbeState::Pending) => {
                            gave_up.push(p.cue_id);
                            false
                        }
                        // Answered (or forgotten meanwhile: it asks again).
                        _ => {
                            due.push((p.cue_id, r.reveal));
                            false
                        }
                    };
                }
                p.remaining -= dt;
                if p.remaining <= 0.0 {
                    due.push((p.cue_id, None));
                    false
                } else {
                    true
                }
            });
            for id in gave_up {
                let name = show
                    .cue(id)
                    .map(|c| format!("{} ({})", c.number, c.name))
                    .unwrap_or_default();
                self.log(format!("Cue {name}: reading its file took too long"));
            }
            for (id, reveal) in due {
                self.start_with(show, id, reveal);
            }
        }

        let paused = self.paused;
        let done_groups = if paused {
            Vec::new()
        } else {
            self.advance_groups(show)
        };
        let mut follow = Vec::new();
        let mut messages = Vec::new();
        let mut entries = Vec::new();
        let mut midpoints = Vec::new();
        self.active.retain_mut(|a| {
            // A cue held by a control cue stands still while the rest go on.
            let paused = paused || a.hold;
            let cue = show.cue(a.cue_id);
            let media = cue.and_then(|c| c.kind.media()).filter(|_| !a.is_matte());
            let mut completed = false;
            let was_started = a.started;
            let (audio_lead, video_lag) = (a.audio_lead, a.video_lag);
            if a.started {
                a.age += dt;
            }
            if let Some(note) = apply_seek(a, paused) {
                messages.push(format!(
                    "Cue {}: {note}",
                    cue.map(|c| c.number.as_str()).unwrap_or("?")
                ));
            }

            match &mut a.kind {
                ActiveKind::Video(v) => {
                    if !a.started {
                        let video_ready = v.player.ready();
                        let audio_ready = v.voice.as_ref().is_none_or(|vo| vo.ready());
                        let start = video_start(
                            video_ready,
                            v.voice.is_some(),
                            audio_ready,
                            v.spawned_at.elapsed(),
                        );
                        if start != VideoStart::Wait {
                            a.started = true;
                            if start == VideoStart::GoSilent {
                                v.voice = None;
                                messages.push(format!(
                                    "Cue {}: its sound wasn't ready in time, so it plays without it",
                                    cue.map(|c| c.number.as_str()).unwrap_or("?")
                                ));
                            }
                            if let Some(vo) = &v.voice {
                                vo.set_lead(audio_lead);
                                vo.set_paused(paused);
                            }
                        } else if v.player.finished() {
                            let err = v.player.error().unwrap_or_else(|| "no frames".into());
                            messages.push(format!(
                                "Cue {}: video failed: {err}",
                                cue.map(|c| c.number.as_str()).unwrap_or("?")
                            ));
                            return false;
                        }
                    }
                    if a.started {
                        match &v.voice {
                            Some(vo) if !vo.finished() => {
                                a.elapsed = a.elapsed.max(a.seek_base + vo.clock());
                            }
                            _ if !paused => a.elapsed += dt,
                            _ => {}
                        }
                        // The picture follows the sound's clock, lead and all,
                        // less its own delay; with no sound, its own time.
                        let on_the_clock = match &v.voice {
                            Some(vo) if !vo.finished() => Some(vo.clock_with_lead()),
                            _ => None,
                        };
                        let t = on_the_clock.unwrap_or_else(|| {
                            let lead = if v.voice.is_some() { audio_lead } else { 0.0 };
                            a.elapsed - a.seek_base + lead
                        });
                        v.player.update((t - video_lag).max(0.0));
                        let audio_done = v.voice.as_ref().is_none_or(|vo| vo.finished());
                        // A camera never ends by itself; "run for" ends it.
                        if v.player.is_live() && a.duration.is_some_and(|d| a.elapsed >= d) {
                            completed = true;
                        } else if v.player.finished() && audio_done && !a.held {
                            if v.player.is_live() {
                                let err = v.player.error().unwrap_or_else(|| "it stopped".into());
                                messages.push(format!(
                                    "Cue {}: camera lost: {err}",
                                    cue.map(|c| c.number.as_str()).unwrap_or("?")
                                ));
                            }
                            if media.is_some_and(|m| m.hold_last_frame) && !a.stopping {
                                a.held = true;
                                completed_follow(a, cue, &mut follow);
                            } else {
                                completed = true;
                            }
                        }
                    }
                }
                ActiveKind::Image(img) => {
                    if let Some(rx) = &img.loader
                        && let Ok(result) = rx.try_recv()
                    {
                        img.loader = None;
                        match result {
                            Ok(i) => {
                                img.image = Some(i);
                                img.version += 1;
                            }
                            Err(e) => {
                                messages.push(format!("Image cue failed: {e}"));
                                return false;
                            }
                        }
                    }
                    a.started = img.image.is_some();
                    if a.started && !paused {
                        a.elapsed += dt;
                    }
                    if let (Some(live), Some(CueKind::Text(t))) =
                        (&mut img.live, cue.map(|c| &c.kind))
                    {
                        if let Some(rx) = &live.pending
                            && let Ok(result) = rx.try_recv()
                        {
                            live.pending = None;
                            if let Ok(i) = result {
                                img.image = Some(i);
                                img.version += 1;
                            }
                        }
                        if live.pending.is_none()
                            && let Some(now) = crate::live_text::resolve(
                                t,
                                a.elapsed,
                                crate::live_text::now_hms(),
                            )
                            && live_key(&now) != live.drawn
                        {
                            live.drawn = live_key(&now);
                            live.pending = Some(draw_text_on_thread(now, live.size));
                        }
                    }
                    if a.duration.is_some_and(|d| a.elapsed >= d) {
                        completed = true;
                    }
                }
                ActiveKind::Audio(vo) => {
                    if !a.started && vo.finished() {
                        let err = vo.error().unwrap_or_else(|| "no audio decoded".into());
                        messages.push(format!(
                            "Cue {}: audio failed: {err}",
                            cue.map(|c| c.number.as_str()).unwrap_or("?")
                        ));
                        return false;
                    }
                    if !a.started && vo.ready() {
                        a.started = true;
                        vo.set_lead(audio_lead);
                        vo.set_paused(paused);
                    }
                    if a.started {
                        a.elapsed = a.seek_base + vo.clock();
                        // Once the released loop has gone on, its end is known.
                        if vo.released()
                            && let Some(at) = vo.tail_begins()
                        {
                            a.duration = Some(a.seek_base + at + vo.tail_len());
                        }
                        if vo.finished() {
                            completed = true;
                        }
                    }
                }
                ActiveKind::Matte(mt) => {
                    if !paused {
                        a.elapsed += dt;
                    }
                    if !mt.fired && !a.stopping && !a.opacity.ramping() {
                        mt.fired = true;
                        mt.fired_at = a.elapsed;
                        midpoints.push((a.cue_id, a.instance, mt.half));
                    }
                    if mt.fired && !a.stopping && a.elapsed - mt.fired_at > REVEAL_TIMEOUT {
                        a.opacity.ramp_to(0.0, mt.half, mt.curve);
                        a.stopping = true;
                    }
                }
                ActiveKind::Fade | ActiveKind::Wait | ActiveKind::Timecode(_) => {
                    if let ActiveKind::Timecode(Some(clock)) = &a.kind {
                        clock.set_paused(paused);
                        if let Some(e) = clock.take_error() {
                            messages.push(format!(
                                "Cue {}: {e}",
                                cue.map(|c| c.number.as_str()).unwrap_or("?")
                            ));
                        }
                    }
                    if !paused {
                        a.elapsed += dt;
                    }
                    if a.duration.is_some_and(|d| a.elapsed >= d) {
                        completed = true;
                    }
                }
                ActiveKind::Group(_) => {
                    if !paused {
                        a.elapsed += dt;
                    }
                    completed = done_groups.contains(&a.instance);
                }
            }

            if a.started && !paused {
                a.opacity.advance(dt);
                a.volume.advance(dt);
                if let Some(lt) = &mut a.live_transform {
                    lt.advance(dt);
                }
            }
            if !was_started
                && a.started
                && let Some(e) = a.entry.take()
            {
                entries.push((a.instance, a.output(), e));
            }

            if let Some(m) = media {
                // Live level edits apply until a fade cue takes over.
                // So does position, scale, rotation and crop, so a cue can be
                // lined up while it plays.
                let geometry = match &a.live_transform {
                    Some(lt) => lt.apply(&m.transform),
                    None => m.motion.transform_at(&m.transform, a.elapsed),
                };
                match &mut a.kind {
                    ActiveKind::Video(v) => {
                        v.transform = geometry;
                        v.color = m.color;
                        v.fx = VideoFx::at(&m.effects, a.elapsed);
                        if let Some(voice) = &v.voice {
                            voice.set_effects(&m.effects, a.elapsed);
                        }
                    }
                    ActiveKind::Image(i) => {
                        i.transform = geometry;
                        i.color = m.color;
                        i.fx = VideoFx::at(&m.effects, a.elapsed);
                    }
                    ActiveKind::Audio(voice) => voice.set_effects(&m.effects, a.elapsed),
                    _ => {}
                }
                if a.relevel {
                    a.relevel = false;
                    if !a.faded && !a.stopping {
                        a.opacity = Param::new(m.opacity);
                        a.volume = Param::new(m.volume_db);
                    }
                }
                if !a.faded && !a.stopping && !a.auto_faded {
                    if !a.opacity.ramping() {
                        a.opacity.value = m.motion.opacity_at(m.opacity, a.elapsed);
                    }
                    if !a.volume.ramping() {
                        a.volume.value = m.volume_db;
                    }
                }
                // Automatic fade-out before the media ends.
                if let Some(d) = a.duration
                    && m.fade_out > 0.0
                    && !a.auto_faded
                    && !a.stopping
                    && a.started
                {
                    let remaining = d - a.elapsed;
                    if remaining <= m.fade_out {
                        a.auto_faded = true;
                        a.opacity.ramp_to(0.0, remaining.max(0.0), m.curve);
                        a.volume.ramp_to(MIN_DB, remaining.max(0.0), m.curve);
                    }
                }
            }

            let device_lost = a.voice().and_then(|vo| {
                vo.set_gain(db_to_gain(a.volume.value));
                vo.device_lost()
            });
            if let Some(why) = device_lost
                && !a.lost_logged
            {
                a.lost_logged = true;
                messages.push(format!(
                    "Cue {}: the audio device was lost ({why})",
                    cue.map(|c| c.number.as_str()).unwrap_or("?")
                ));
            }

            // A loop that has been given an end (a devamp) is over when it gets there: its
            // decoders would go round for ever.
            if a.pass_len.is_some() && a.started && a.duration.is_some_and(|d| a.elapsed >= d) {
                completed = true;
            }
            if a.stopping && !a.opacity.ramping() && !a.volume.ramping() {
                return false;
            }
            if a.auto_faded && a.held && !a.opacity.ramping() {
                return false;
            }
            if completed {
                completed_follow(a, cue, &mut follow);
                return false;
            }
            true
        });
        for m in messages {
            self.log(m);
        }
        for (instance, output, e) in entries {
            self.apply_entry(instance, output, e);
        }
        for (id, matte, half) in midpoints {
            self.start_with(show, id, Some((matte, half)));
        }
        for id in follow {
            self.fire_next(show, id);
        }

        self.update_preload(show);
    }

    /// A cue that is waiting out a pre-wait or still playing. A video that is
    /// holding its last frame has finished.
    fn is_running(&self, id: CueId) -> bool {
        self.pending.iter().any(|p| p.cue_id == id)
            || self.active.iter().any(|a| a.cue_id == id && !a.held)
    }

    /// Fire the next child of every playlist whose current child has finished.
    /// Returns the groups (instances) whose children have all finished.
    fn advance_groups(&mut self, show: &Show) -> Vec<u64> {
        let mut done = Vec::new();
        let mut to_fire = Vec::new();
        for a in &self.active {
            let ActiveKind::Group(g) = &a.kind else {
                continue;
            };
            if a.stopping || g.children.iter().any(|c| self.is_running(*c)) {
                continue;
            }
            match g.children.get(g.next) {
                Some(&child) if g.mode == GroupMode::Playlist => to_fire.push((a.instance, child)),
                _ => done.push(a.instance),
            }
        }
        for (instance, child) in to_fire {
            if let Some(ActiveKind::Group(g)) = self
                .active
                .iter_mut()
                .find(|a| a.instance == instance)
                .map(|a| &mut a.kind)
            {
                g.next += 1;
            }
            self.fire(show, child);
        }
        done
    }

    /// Keep the next cue(s) at the playhead decoded and ready, so GO is instant.
    fn update_preload(&mut self, show: &Show) {
        // Cues that have already been fired but not started yet: in a
        // pre-wait, or waiting for a dip's midpoint.
        let mut wanted: Vec<CueId> = self.pending.iter().map(|p| p.cue_id).collect();
        // Cues a control cue asked to have ready (the ones that exist and can be).
        self.loads
            .retain(|id| show.cue(*id).is_some_and(|c| c.kind.preloads()));
        wanted.extend(self.loads.iter().copied());
        for a in &self.active {
            if let ActiveKind::Matte(m) = &a.kind
                && !m.fired
            {
                wanted.push(a.cue_id);
            }
            // The cue an auto-follow will fire when this one ends.
            if !a.followed
                && let Some(i) = show.cue_index(a.cue_id)
                && show.cues[i].parent.is_none()
                && show.cues[i].continue_mode == ContinueMode::AutoFollow
                && let Some(next) = show.next_armed_top(i)
                && show.cues[next].kind.preloads()
            {
                wanted.push(show.cues[next].id);
            }
            // The child a playlist will play next.
            if let ActiveKind::Group(g) = &a.kind
                && g.mode == GroupMode::Playlist
                && let Some(&next) = g.children.get(g.next)
                && show.cue(next).is_some_and(|c| c.kind.preloads())
            {
                wanted.push(next);
            }
        }
        if let Some(mut idx) = self
            .playhead
            .and_then(|p| show.cue_index(p))
            .and_then(|i| show.armed_top_from(i))
        {
            loop {
                let cue = &show.cues[idx];
                if cue.kind.preloads() {
                    wanted.push(cue.id);
                }
                // A group at the playhead: the children that start with it.
                if let CueKind::Group(g) = &cue.kind {
                    let mode = g.mode;
                    for (n, child) in show.armed_children(cue.id).into_iter().enumerate() {
                        if mode == GroupMode::Playlist && n > 0 {
                            break;
                        }
                        if child.kind.preloads() && wanted.len() < MAX_PRELOAD {
                            wanted.push(child.id);
                        }
                    }
                }
                let next = show.next_armed_top(idx);
                match next {
                    Some(n)
                        if cue.continue_mode != ContinueMode::DoNotContinue
                            && wanted.len() < MAX_PRELOAD =>
                    {
                        idx = n
                    }
                    _ => break,
                }
            }
        }
        let sigs: Vec<(CueId, Option<PrepSig>)> = wanted
            .iter()
            .map(|id| (*id, show.cue(*id).and_then(|c| self.signature(c))))
            .collect();
        self.prepared.retain(|p| {
            sigs.iter()
                .any(|(id, sig)| *id == p.cue_id && sig.as_ref() == Some(&p.sig))
        });
        if self.ff.is_none() {
            return;
        }
        self.failed_preps
            .retain(|id, (_, at)| wanted.contains(id) && at.elapsed() < PREP_RETRY);
        for id in wanted {
            if self.is_prepared(id) {
                continue;
            }
            let Some(cue) = show.cue(id) else { continue };
            let sig = self.signature(cue);
            if let (Some(sig), Some((failed, _))) = (&sig, self.failed_preps.get(&id))
                && sig == failed
            {
                continue;
            }
            if let CueKind::Camera(c) = &cue.kind {
                // Two cues can't have one camera open at once on every system.
                if self.camera_in_use(show, &c.device) {
                    continue;
                }
                match self.prepare_camera(cue, c) {
                    Ok(p) => self.prepared.push(p),
                    Err(e) => {
                        log::debug!("preload of cue {} failed: {e}", cue.number);
                        if let Some(sig) = sig {
                            self.failed_preps.insert(id, (sig, Instant::now()));
                        }
                    }
                }
                continue;
            }
            let Some(m) = cue.kind.file_media() else {
                continue;
            };
            let info = match self.probe.get_urgent(self.ff.as_ref(), &m.file) {
                ProbeState::Ready(info) => info.clone(),
                _ => continue,
            };
            match self.prepare(cue, &info) {
                Ok(p) => self.prepared.push(p),
                Err(e) => {
                    log::debug!("preload of cue {} failed: {e}", cue.number);
                    if let Some(sig) = sig {
                        self.failed_preps.insert(id, (sig, Instant::now()));
                    }
                }
            }
        }
    }

    /// A playing camera cue has this device open.
    fn camera_in_use(&self, show: &Show, device: &str) -> bool {
        device != TEST_PATTERN_DEVICE
            && self.active.iter().any(|a| {
                matches!(
                    show.cue(a.cue_id).map(|c| &c.kind),
                    Some(CueKind::Camera(c)) if c.device == device
                )
            })
    }

    /// How many pictures are being drawn, and how many reads of them each
    /// pixel of those layers takes in all (a plain picture is one): when the
    /// second is well above the first, effects are costing the graphics card.
    /// How many cues have been started since the program began.
    pub fn started_total(&self) -> u64 {
        self.next_instance - 1
    }

    pub fn picture_layers(&self) -> u32 {
        self.active
            .iter()
            .filter(|a| matches!(a.kind, ActiveKind::Video(_) | ActiveKind::Image(_)))
            .count() as u32
    }

    pub fn picture_reads(&self) -> u32 {
        self.active
            .iter()
            .map(|a| match &a.kind {
                ActiveKind::Video(v) => v.fx.taps_per_pixel(),
                ActiveKind::Image(i) => i.fx.taps_per_pixel(),
                _ => 0,
            })
            .sum()
    }

    /// Visual layers on an output, bottom to top.
    pub fn layers(&self, output: OutputId) -> Vec<LayerRef<'_>> {
        let mut layers: Vec<(i32, LayerRef)> = Vec::new();
        for a in &self.active {
            // Held back until its delay has passed.
            if !a.started || a.age < a.video_lag {
                continue;
            }
            let (layer, fit, transform, color, fx, source) = match &a.kind {
                ActiveKind::Video(v) if v.output == Some(output) => (
                    v.layer,
                    v.fit,
                    v.transform,
                    v.color,
                    v.fx,
                    LayerSource::Video(&v.player),
                ),
                ActiveKind::Image(i) if i.output == Some(output) => match &i.image {
                    Some(img) => (
                        i.layer,
                        i.fit,
                        i.transform,
                        i.color,
                        i.fx,
                        LayerSource::Image(img, i.version),
                    ),
                    None => continue,
                },
                // Mattes cover everything on the output.
                ActiveKind::Matte(m) if m.output == Some(output) => (
                    i32::MAX,
                    Fit::Stretch,
                    LayerTransform::default(),
                    ColorAdjust::default(),
                    VideoFx::default(),
                    LayerSource::Solid(m.color),
                ),
                _ => continue,
            };
            layers.push((
                layer,
                LayerRef {
                    instance: a.instance,
                    opacity: a.opacity.value.clamp(0.0, 1.0),
                    fit,
                    transform,
                    color,
                    fx,
                    source,
                },
            ));
        }
        layers.sort_by_key(|(layer, l)| (*layer, l.instance));
        layers.into_iter().map(|(_, l)| l).collect()
    }
}

fn draw_text_on_thread(cue: TextCue, size: (u32, u32)) -> Receiver<Result<RgbaImage, String>> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("text-render".into())
        .spawn(move || {
            let _ = tx.send(Ok(crate::text::render(&cue, size)));
        });
    if let Err(e) = spawned {
        // No thread to draw on: the cue reports it rather than hanging.
        let (tx, rx) = crossbeam_channel::bounded(1);
        let _ = tx.send(Err(format!("could not start drawing the text: {e}")));
        return rx;
    }
    rx
}

/// Switch a cue to its new decoders once they have something to show. Says
/// so if it had to leave the sound out.
fn apply_seek(a: &mut ActiveCue, paused: bool) -> Option<&'static str> {
    let s = a.seeking.as_mut()?;
    let video_ready = s.player.as_mut().is_none_or(|p| p.ready());
    let audio_ready = s.voice.as_ref().is_none_or(|v| v.ready());
    if s.player
        .as_mut()
        .is_some_and(|p| !p.ready() && p.finished())
        || (s.player.is_none() && s.voice.as_ref().is_some_and(|v| v.finished() && !v.ready()))
    {
        // Nothing to show there: stay where we were.
        a.seeking = None;
        return None;
    }
    let mut note = None;
    match video_start(
        video_ready,
        s.voice.is_some(),
        audio_ready,
        s.spawned_at.elapsed(),
    ) {
        VideoStart::Wait => return None,
        VideoStart::Go => {}
        // Only a picture has a clock of its own to go on with; a sound alone
        // is just waited for.
        VideoStart::GoSilent if s.player.is_some() => {
            s.voice = None;
            note = Some("its sound wasn't ready in time after the jump, so it plays without it");
        }
        VideoStart::GoSilent => {}
    }
    let s = a.seeking.take()?;
    let lead = a.audio_lead;
    match &mut a.kind {
        ActiveKind::Video(v) => {
            if let Some(p) = s.player {
                v.player = p;
            }
            v.voice = s.voice;
            v.spawned_at = s.spawned_at;
            if let Some(vo) = &v.voice {
                vo.set_lead(lead);
                vo.set_paused(paused);
            }
        }
        ActiveKind::Audio(vo) => {
            if let Some(new) = s.voice {
                new.set_lead(lead);
                new.set_paused(paused);
                *vo = new;
            }
        }
        _ => return None,
    }
    a.seek_base = s.base;
    a.elapsed = s.base;
    a.restart_auto_fade();
    note
}

fn completed_follow(a: &mut ActiveCue, cue: Option<&Cue>, follow: &mut Vec<CueId>) {
    if a.followed || a.stopping {
        return;
    }
    a.followed = true;
    if cue.is_some_and(|c| c.continue_mode == ContinueMode::AutoFollow) {
        follow.push(a.cue_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine_without_media() -> Engine {
        Engine::for_tests()
    }

    fn wait_cue(show: &mut Show, secs: f64, mode: ContinueMode) -> CueId {
        let mut c = show.make_cue(CueType::Wait, None, show.cues.len());
        if let CueKind::Wait(w) = &mut c.kind {
            w.duration = secs;
        }
        c.continue_mode = mode;
        let id = c.id;
        show.cues.push(c);
        id
    }

    /// Cues 1 and 2 in the main list, 3 and 4 in a second list.
    fn two_lists() -> (Show, u32, [CueId; 4]) {
        let mut show = Show::default();
        let other = show.add_list("Second");
        let mut ids = [CueId(0); 4];
        for (n, list) in [0, 0, other, other].into_iter().enumerate() {
            let id = wait_cue(&mut show, 30.0, ContinueMode::DoNotContinue);
            let i = show.cue_index(id).unwrap();
            show.cues[i].list = list;
            ids[n] = id;
        }
        show.normalize_lists();
        (show, other, ids)
    }

    #[test]
    fn a_cue_held_by_a_control_cue_can_still_be_faded_out() {
        let mut show = Show::default();
        let id = wait_cue(&mut show, 30.0, ContinueMode::DoNotContinue);
        let mut e = Engine::for_tests();
        e.fire(&show, id);
        e.control_target(&show, ControlAction::Pause, id, "test");
        assert!(e.active[0].hold);
        e.stop_cue(id, 0.5);
        assert!(!e.active[0].hold, "a fade lets it go");
        let t0 = Instant::now();
        // (A wait has no levels of its own; the stop is what is being checked.)
        e.stop_instance(e.active[0].instance, 0.0);
        assert!(t0.elapsed().as_secs() < 1 && e.active.is_empty());
    }

    #[test]
    fn keyframed_speed_lengths_are_kept_and_follow_the_keyframes() {
        let mut show = Show::default();
        let mut c = show.make_cue(CueType::Video, Some("x.mp4".into()), 0);
        c.kind.media_mut().unwrap().rate.set_key(0.0, 2.0);
        let mut e = Engine::for_tests();
        let m = c.kind.media().unwrap().clone();
        let a = e.speed_map_cached(c.id, &m).unwrap();
        let b = e.speed_map_cached(c.id, &m).unwrap();
        assert!(
            std::sync::Arc::ptr_eq(&a, &b),
            "the same table while the keys are"
        );
        let mut m2 = m.clone();
        m2.rate.set_key(10.0, 3.0);
        let c2 = e.speed_map_cached(c.id, &m2).unwrap();
        assert!(!std::sync::Arc::ptr_eq(&a, &c2), "new keys, a new table");
        // A loop follows its keyframes too; with none the map is let go.
        m2.loop_media = true;
        assert!(e.speed_map_cached(c.id, &m2).is_some());
        m2.rate.keys.clear();
        assert!(e.speed_map_cached(c.id, &m2).is_none() && e.speed_maps.is_empty());
    }

    #[test]
    fn each_cue_list_has_a_playhead_of_its_own() {
        let (show, other, ids) = two_lists();
        let mut e = Engine::for_tests();
        e.playhead = Some(ids[0]);
        e.go(&show);
        assert_eq!(e.playhead, Some(ids[1]));
        e.go(&show);
        assert_eq!(
            e.playhead, None,
            "the end of the main list: GO doesn't run into the next"
        );
        assert!(matches!(e.status(ids[2]), CueStatus::Idle));
        // Another list starts at its first cue and keeps its place when left.
        e.switch_list(&show, other);
        assert_eq!((e.active_list, e.playhead), (other, Some(ids[2])));
        e.go(&show);
        assert_eq!(e.playhead, Some(ids[3]));
        e.switch_list(&show, 0);
        assert_eq!(e.playhead, None, "the main list is where it was left");
        e.switch_list(&show, other);
        assert_eq!(e.playhead, Some(ids[3]));
        // A list that isn't there is the main list.
        e.switch_list(&show, 42);
        assert_eq!(e.active_list, 0);
    }

    #[test]
    fn go_on_another_list_leaves_the_one_being_worked_alone() {
        let (show, other, ids) = two_lists();
        let mut e = Engine::for_tests();
        e.playhead = Some(ids[0]);
        e.go_list(&show, other);
        assert!(matches!(e.status(ids[2]), CueStatus::Playing { .. }));
        assert!(matches!(e.status(ids[0]), CueStatus::Idle));
        assert_eq!((e.active_list, e.playhead), (0, Some(ids[0])));
        // And its playhead has moved on for next time.
        e.switch_list(&show, other);
        assert_eq!(e.playhead, Some(ids[3]));
    }

    #[test]
    fn reset_and_go_to_know_about_every_list() {
        let (show, other, ids) = two_lists();
        let mut e = Engine::for_tests();
        e.playhead = Some(ids[1]);
        e.switch_list(&show, other);
        e.go(&show);
        e.reset_all(&show);
        assert_eq!(e.playhead, Some(ids[2]));
        e.switch_list(&show, 0);
        assert_eq!(
            e.playhead,
            Some(ids[0]),
            "every list was put back to its top"
        );
        // Go to a cue in the other list sets that list's playhead, not this one's.
        e.control_target(&show, ControlAction::GoTo, ids[3], "test");
        assert_eq!(e.playhead, Some(ids[0]));
        e.switch_list(&show, other);
        assert_eq!(e.playhead, Some(ids[3]));
        // Opening another show forgets all of it.
        e.forget_lists();
        assert_eq!(e.active_list, 0);
    }

    #[test]
    fn param_ramps_and_finishes() {
        let mut p = Param::new(0.0);
        p.ramp_to(1.0, 2.0, Curve::Linear);
        p.advance(1.0);
        assert!((p.value - 0.5).abs() < 1e-6);
        p.advance(1.5);
        assert_eq!(p.value, 1.0);
        assert!(!p.ramping());
    }

    #[test]
    fn go_skips_continue_chain_and_auto_follow_fires() {
        let mut show = Show::default();
        let a = wait_cue(&mut show, 1.0, ContinueMode::AutoFollow);
        let b = wait_cue(&mut show, 1.0, ContinueMode::DoNotContinue);
        let c = wait_cue(&mut show, 1.0, ContinueMode::DoNotContinue);
        let mut e = engine_without_media();
        e.playhead = Some(a);
        e.go(&show);
        assert_eq!(e.playhead, Some(c));
        assert!(matches!(e.status(a), CueStatus::Playing { .. }));
        assert!(matches!(e.status(b), CueStatus::Idle));
        e.tick(&show, 1.1);
        assert!(matches!(e.status(a), CueStatus::Idle));
        assert!(matches!(e.status(b), CueStatus::Playing { .. }));
    }

    #[test]
    fn auto_continue_and_pre_wait() {
        let mut show = Show::default();
        let a = wait_cue(&mut show, 5.0, ContinueMode::AutoContinue);
        let b = wait_cue(&mut show, 5.0, ContinueMode::DoNotContinue);
        show.cues[1].pre_wait = 2.0;
        let mut e = engine_without_media();
        e.playhead = Some(a);
        e.go(&show);
        assert!(matches!(e.status(b), CueStatus::PreWait { .. }));
        e.tick(&show, 1.0);
        assert!(matches!(e.status(b), CueStatus::PreWait { .. }));
        e.tick(&show, 1.5);
        assert!(matches!(e.status(b), CueStatus::Playing { .. }));
        assert_eq!(e.playhead, None);
    }

    #[test]
    fn pause_freezes_time_and_stop_all_clears() {
        let mut show = Show::default();
        let a = wait_cue(&mut show, 2.0, ContinueMode::DoNotContinue);
        let mut e = engine_without_media();
        e.playhead = Some(a);
        e.go(&show);
        e.set_paused(true);
        e.tick(&show, 5.0);
        assert!(matches!(e.status(a), CueStatus::Playing { elapsed, .. } if elapsed == 0.0));
        e.panic(&show); // unpauses; wait cues stop immediately
        assert!(!e.is_active());
    }

    // ----- Groups ---------------------------------------------------------------

    /// A group of wait cues lasting `secs` each. Returns the group and its children.
    fn group(show: &mut Show, mode: GroupMode, secs: &[f64]) -> (CueId, Vec<CueId>) {
        let mut g = show.make_cue(CueType::Group, None, show.cues.len());
        if let CueKind::Group(gc) = &mut g.kind {
            gc.mode = mode;
        }
        let gid = g.id;
        show.cues.push(g);
        let kids = secs
            .iter()
            .map(|s| {
                let id = wait_cue(show, *s, ContinueMode::DoNotContinue);
                let i = show.cue_index(id).unwrap();
                show.cues[i].parent = Some(gid);
                id
            })
            .collect();
        (gid, kids)
    }

    impl Engine {
        fn go_again(&mut self, show: &Show, id: CueId) {
            self.playhead = Some(id);
            self.go(show);
        }
    }

    fn playing(e: &Engine, id: CueId) -> bool {
        matches!(e.status(id), CueStatus::Playing { .. })
    }

    fn idle(e: &Engine, id: CueId) -> bool {
        matches!(e.status(id), CueStatus::Idle)
    }

    #[test]
    fn timeline_group_starts_every_child_and_ends_with_the_last() {
        let mut show = Show::default();
        let (g, kids) = group(&mut show, GroupMode::Timeline, &[1.0, 3.0]);
        show.cues[0].continue_mode = ContinueMode::AutoFollow;
        let after = wait_cue(&mut show, 5.0, ContinueMode::DoNotContinue);
        let mut e = engine_without_media();
        e.playhead = Some(g);
        e.go(&show);
        assert!(playing(&e, g) && playing(&e, kids[0]) && playing(&e, kids[1]));
        // The group is auto-follow, so GO chains on to the cue after it, not into its children.
        assert_eq!(e.playhead, None);
        assert!(idle(&e, after));

        e.tick(&show, 1.1);
        assert!(idle(&e, kids[0]) && playing(&e, kids[1]) && playing(&e, g));
        e.tick(&show, 2.0);
        assert!(idle(&e, kids[1]));
        assert!(playing(&e, g), "the group notices on the next tick");
        e.tick(&show, 0.01);
        assert!(idle(&e, g));
        assert!(
            playing(&e, after),
            "auto-follow fires once the whole group is done"
        );
    }

    #[test]
    fn timeline_children_pre_wait_is_their_offset() {
        let mut show = Show::default();
        let (g, kids) = group(&mut show, GroupMode::Timeline, &[1.0, 3.0]);
        show.cues[2].pre_wait = 2.0;
        let mut e = engine_without_media();
        e.playhead = Some(g);
        e.go(&show);
        assert!(playing(&e, kids[0]));
        assert!(matches!(e.status(kids[1]), CueStatus::PreWait { .. }));
        e.tick(&show, 1.1);
        assert!(
            idle(&e, kids[0]) && playing(&e, g),
            "still waiting for the second child"
        );
        e.tick(&show, 1.0);
        assert!(playing(&e, kids[1]));
        e.tick(&show, 3.1);
        e.tick(&show, 0.01);
        assert!(!e.is_active());
    }

    #[test]
    fn playlist_group_plays_children_in_order() {
        let mut show = Show::default();
        let (g, kids) = group(&mut show, GroupMode::Playlist, &[1.0, 1.0, 1.0]);
        let mut e = engine_without_media();
        e.playhead = Some(g);
        e.go(&show);
        assert!(playing(&e, kids[0]) && idle(&e, kids[1]) && idle(&e, kids[2]));
        for n in 1..3 {
            e.tick(&show, 1.1); // child n-1 ends
            e.tick(&show, 0.01); // the group starts the next
            assert!(idle(&e, kids[n - 1]));
            assert!(playing(&e, kids[n]), "child {n} should be playing");
        }
        e.tick(&show, 1.1);
        e.tick(&show, 0.01);
        assert!(!e.is_active(), "the group ends with its last child");
    }

    #[test]
    fn group_durations_add_up_by_mode() {
        let mut show = Show::default();
        let (t, _) = group(&mut show, GroupMode::Timeline, &[1.0, 3.0]);
        let (p, _) = group(&mut show, GroupMode::Playlist, &[1.0, 3.0]);
        let first_child = show.cue_index(p).unwrap() + 1;
        show.cues[first_child].pre_wait = 0.5;
        let (empty, _) = group(&mut show, GroupMode::Timeline, &[]);
        let mut e = engine_without_media();
        let dur = |e: &mut Engine, id: CueId| e.cue_duration(&show, show.cue(id).unwrap());
        assert_eq!(dur(&mut e, t), Some(Some(3.0)));
        assert_eq!(dur(&mut e, p), Some(Some(4.5)));
        assert_eq!(dur(&mut e, empty), Some(Some(0.0)));
    }

    #[test]
    fn stopping_a_group_stops_its_children() {
        let mut show = Show::default();
        let (g, kids) = group(&mut show, GroupMode::Timeline, &[5.0, 5.0]);
        show.cues[2].pre_wait = 2.0;
        let mut e = engine_without_media();
        e.playhead = Some(g);
        e.go(&show);
        e.stop_cue(g, 0.0);
        assert!(idle(&e, g) && idle(&e, kids[0]) && idle(&e, kids[1]));
        assert!(!e.is_active(), "the pending child is gone too");

        // The same from the Active panel's Stop button.
        e.go_again(&show, g);
        let instance = e.active.iter().find(|a| a.cue_id == g).unwrap().instance;
        e.stop_instance(instance, 0.0);
        assert!(!e.is_active());
    }

    #[test]
    fn playhead_on_a_child_fires_the_whole_group() {
        let mut show = Show::default();
        let (g, kids) = group(&mut show, GroupMode::Timeline, &[1.0, 1.0]);
        let after = wait_cue(&mut show, 1.0, ContinueMode::DoNotContinue);
        let mut e = engine_without_media();
        e.playhead = Some(kids[1]);
        e.go(&show);
        assert!(playing(&e, g) && playing(&e, kids[0]) && playing(&e, kids[1]));
        assert_eq!(e.playhead, Some(after));
    }

    #[test]
    fn an_empty_group_still_continues() {
        let mut show = Show::default();
        let (g, _) = group(&mut show, GroupMode::Playlist, &[]);
        show.cues[0].continue_mode = ContinueMode::AutoFollow;
        let after = wait_cue(&mut show, 1.0, ContinueMode::DoNotContinue);
        let mut e = engine_without_media();
        e.playhead = Some(g);
        e.go(&show);
        assert!(playing(&e, after));
    }

    /// A chain this long once nested a call per cue and ran out of stack
    /// (in a test's 2 MB at about 500 cues; Windows gives the main thread 1 MB).
    const LONG_CHAIN: usize = 3000;

    #[test]
    fn a_long_chain_of_auto_continues_starts_every_cue_in_order() {
        let mut show = Show::default();
        let ids: Vec<CueId> = (0..LONG_CHAIN)
            .map(|_| wait_cue(&mut show, 5.0, ContinueMode::AutoContinue))
            .collect();
        let mut e = engine_without_media();
        e.playhead = Some(ids[0]);
        e.go(&show);
        let started: Vec<CueId> = e.active.iter().map(|a| a.cue_id).collect();
        assert_eq!(started, ids);
        assert_eq!(e.playhead, None);
    }

    #[test]
    fn a_long_chain_of_auto_follows_that_fail_at_once_is_walked_to_its_end() {
        // Video cues with no ffmpeg fail as they start, and each follows on
        // to the next: a show whose media drive has gone, say.
        let mut show = Show::default();
        for _ in 0..LONG_CHAIN {
            let mut c = show.make_cue(CueType::Video, None, show.cues.len());
            c.continue_mode = ContinueMode::AutoFollow;
            show.cues.push(c);
        }
        let last = wait_cue(&mut show, 5.0, ContinueMode::DoNotContinue);
        let mut e = engine_without_media();
        e.playhead = Some(show.cues[0].id);
        e.go(&show);
        assert!(playing(&e, last), "the wait at the end of the chain runs");
        assert_eq!(e.active.len(), 1);
    }

    #[test]
    fn a_fade_with_a_nonsense_time_is_instant_not_endless() {
        for bad in [f64::NAN, f64::INFINITY, -1.0] {
            let mut p = Param::new(0.0);
            p.ramp_to(1.0, bad, Curve::Linear);
            assert_eq!(p.value, 1.0, "{bad}");
            assert!(!p.ramping(), "{bad}");
        }
    }

    #[test]
    fn children_do_not_chain_on_their_own() {
        let mut show = Show::default();
        let (g, kids) = group(&mut show, GroupMode::Timeline, &[1.0, 1.0]);
        // Stale continue modes on children (e.g. a cue dragged in) are ignored.
        show.cues[1].continue_mode = ContinueMode::AutoFollow;
        show.cues[2].continue_mode = ContinueMode::AutoContinue;
        let mut e = engine_without_media();
        e.playhead = Some(g);
        e.go(&show);
        e.tick(&show, 0.01);
        assert!(playing(&e, kids[0]) && playing(&e, kids[1]));
        assert_eq!(e.active.len(), 3, "group and two children, nothing extra");
        e.tick(&show, 1.1);
        assert!(
            idle(&e, kids[0]) && idle(&e, kids[1]),
            "no child fired another"
        );
    }

    // ----- Arming ---------------------------------------------------------------

    fn disarm(show: &mut Show, id: CueId) {
        let i = show.cue_index(id).unwrap();
        show.cues[i].armed = false;
    }

    #[test]
    fn go_passes_over_a_disarmed_cue() {
        let mut show = Show::default();
        let a = wait_cue(&mut show, 5.0, ContinueMode::DoNotContinue);
        let b = wait_cue(&mut show, 5.0, ContinueMode::DoNotContinue);
        let c = wait_cue(&mut show, 5.0, ContinueMode::DoNotContinue);
        disarm(&mut show, b);
        let mut e = engine_without_media();
        e.playhead = Some(a);
        e.go(&show);
        assert!(playing(&e, a) && idle(&e, b));
        assert_eq!(
            e.playhead,
            Some(c),
            "the playhead lands on the next armed cue"
        );
        e.go(&show);
        assert!(playing(&e, c) && idle(&e, b));
        assert_eq!(e.playhead, None);
    }

    #[test]
    fn go_on_a_disarmed_cue_fires_the_next_armed_one() {
        let mut show = Show::default();
        let a = wait_cue(&mut show, 5.0, ContinueMode::DoNotContinue);
        let b = wait_cue(&mut show, 5.0, ContinueMode::DoNotContinue);
        disarm(&mut show, a);
        let mut e = engine_without_media();
        e.playhead = Some(a);
        e.go(&show);
        assert!(idle(&e, a) && playing(&e, b));
    }

    #[test]
    fn go_with_nothing_armed_explains_and_stays_put() {
        let mut show = Show::default();
        let a = wait_cue(&mut show, 5.0, ContinueMode::DoNotContinue);
        let b = wait_cue(&mut show, 5.0, ContinueMode::DoNotContinue);
        disarm(&mut show, a);
        disarm(&mut show, b);
        let mut e = engine_without_media();
        e.playhead = Some(a);
        e.go(&show);
        assert!(!e.is_active());
        assert_eq!(e.playhead, Some(a));
        assert!(e.messages.back().unwrap().1.contains("disarmed"));
    }

    #[test]
    fn continues_pass_over_disarmed_cues() {
        // Auto-continue: GO fires a and c together; b is not fired.
        let mut show = Show::default();
        let a = wait_cue(&mut show, 5.0, ContinueMode::AutoContinue);
        let b = wait_cue(&mut show, 5.0, ContinueMode::DoNotContinue);
        let c = wait_cue(&mut show, 5.0, ContinueMode::DoNotContinue);
        disarm(&mut show, b);
        let mut e = engine_without_media();
        e.playhead = Some(a);
        e.go(&show);
        assert!(playing(&e, a) && idle(&e, b) && playing(&e, c));
        assert_eq!(e.playhead, None);

        // Auto-follow: c starts when a ends, skipping b.
        let mut show = Show::default();
        let a = wait_cue(&mut show, 1.0, ContinueMode::AutoFollow);
        let b = wait_cue(&mut show, 5.0, ContinueMode::DoNotContinue);
        let c = wait_cue(&mut show, 5.0, ContinueMode::DoNotContinue);
        disarm(&mut show, b);
        let mut e = engine_without_media();
        e.playhead = Some(a);
        e.go(&show);
        e.tick(&show, 1.1);
        assert!(idle(&e, a) && idle(&e, b) && playing(&e, c));
    }

    #[test]
    fn a_disarmed_cue_will_not_fire_even_when_asked() {
        let mut show = Show::default();
        let a = wait_cue(&mut show, 5.0, ContinueMode::DoNotContinue);
        disarm(&mut show, a);
        let mut e = engine_without_media();
        e.fire(&show, a);
        assert!(!e.is_active());
        let msg = &e.messages.back().unwrap().1;
        assert!(msg.contains("disarmed"), "{msg}");
        // Arming it again lets it go.
        show.cues[0].armed = true;
        e.fire(&show, a);
        assert!(playing(&e, a));
    }

    #[test]
    fn groups_skip_disarmed_children_and_disarmed_groups_do_nothing() {
        // Timeline: only the armed children start.
        let mut show = Show::default();
        let (g, kids) = group(&mut show, GroupMode::Timeline, &[1.0, 5.0, 3.0]);
        disarm(&mut show, kids[1]);
        let mut e = engine_without_media();
        assert_eq!(
            e.cue_duration(&show, show.cue(g).unwrap()),
            Some(Some(3.0)),
            "a disarmed child doesn't lengthen the group"
        );
        e.playhead = Some(g);
        e.go(&show);
        assert!(playing(&e, kids[0]) && idle(&e, kids[1]) && playing(&e, kids[2]));

        // Playlist: the disarmed child is stepped over.
        let mut show = Show::default();
        let (g, kids) = group(&mut show, GroupMode::Playlist, &[1.0, 5.0, 3.0]);
        disarm(&mut show, kids[1]);
        let mut e = engine_without_media();
        e.playhead = Some(g);
        e.go(&show);
        e.tick(&show, 1.1);
        e.tick(&show, 0.01);
        assert!(idle(&e, kids[0]) && idle(&e, kids[1]) && playing(&e, kids[2]));

        // A disarmed group skips its children too, and GO moves past it.
        let mut show = Show::default();
        let (g, kids) = group(&mut show, GroupMode::Timeline, &[5.0, 5.0]);
        let after = wait_cue(&mut show, 5.0, ContinueMode::DoNotContinue);
        disarm(&mut show, g);
        let mut e = engine_without_media();
        e.playhead = Some(g);
        e.go(&show);
        assert!(!playing(&e, g) && idle(&e, kids[0]) && idle(&e, kids[1]));
        assert!(playing(&e, after));
        // Firing one of its children directly is refused as well.
        e.fire(&show, kids[0]);
        assert!(idle(&e, kids[0]));
    }

    #[test]
    fn a_group_with_every_child_disarmed_still_continues() {
        let mut show = Show::default();
        let (g, kids) = group(&mut show, GroupMode::Playlist, &[5.0]);
        show.cues[0].continue_mode = ContinueMode::AutoFollow;
        let after = wait_cue(&mut show, 5.0, ContinueMode::DoNotContinue);
        disarm(&mut show, kids[0]);
        let mut e = engine_without_media();
        e.playhead = Some(g);
        e.go(&show);
        assert!(idle(&e, kids[0]));
        assert!(playing(&e, after));
    }

    // ----- Text cues ---------------------------------------------------------------

    fn text_cue(show: &mut Show, text: &str, transition: Transition) -> CueId {
        let mut c = show.make_cue(CueType::Text, None, show.cues.len());
        if let CueKind::Text(t) = &mut c.kind {
            t.text = text.into();
            t.media.transition = transition;
            t.media.fade_in = 0.0;
            t.media.fade_out = 0.0;
        }
        let id = c.id;
        show.cues.push(c);
        id
    }

    fn first_output(show: &Show) -> OutputId {
        show.outputs[0].id
    }

    /// Titles are drawn on a thread: wait until everything playing has its picture.
    fn drawn(e: &mut Engine, show: &Show) {
        run_until(e, show, "the text to be drawn", |e| {
            !e.active.is_empty() && e.active.iter().all(|a| a.started)
        });
    }

    #[test]
    fn a_text_cue_plays_as_a_full_output_layer_without_ffmpeg() {
        let mut show = Show::default();
        let t = text_cue(&mut show, "Hello", Transition::FadeIn);
        let out = first_output(&show);
        let mut e = engine_without_media();
        e.playhead = Some(t);
        e.go(&show);
        drawn(&mut e, &show);
        let layers = e.layers(out);
        assert_eq!(layers.len(), 1, "text should be on the output");
        let LayerSource::Image(img, _) = &layers[0].source else {
            panic!("text should present as an image");
        };
        assert_eq!((img.width, img.height), DEFAULT_OUTPUT_SIZE);
        assert!(img.data.chunks(4).any(|p| p[3] > 200), "nothing was drawn");
        assert_eq!(layers[0].fit, Fit::Stretch);
    }

    fn first_picture(e: &Engine, show: &Show) -> Vec<u8> {
        let layers = e.layers(first_output(show));
        let LayerSource::Image(img, _) = &layers[0].source else {
            panic!("text should present as an image");
        };
        img.data.to_vec()
    }

    #[test]
    fn a_timer_redraws_as_it_counts_and_a_plain_text_cue_does_not() {
        let mut show = Show::default();
        let t = text_cue(&mut show, "{time}", Transition::FadeIn);
        if let CueKind::Text(c) = &mut show.cues[0].kind {
            c.source = TextSource::Timer {
                total: 600.0,
                up: false,
                warn: 60.0,
            };
        }
        let mut e = engine_without_media();
        e.playhead = Some(t);
        e.go(&show);
        drawn(&mut e, &show);
        let before = first_picture(&e, &show);
        // A tenth of a second is still 10:00: nothing is redrawn.
        e.tick(&show, 0.1);
        run_for(&mut e, &show, 0.2);
        assert_eq!(first_picture(&e, &show), before);
        // Ninety seconds in it reads 8:30 (and the minute has changed).
        e.tick(&show, 90.0);
        run_until(&mut e, &show, "the new time to be drawn", |e| {
            first_picture(e, &show) != before
        });
        // The picture stays up while the next one is drawn: still one layer.
        assert_eq!(e.layers(first_output(&show)).len(), 1);

        let mut show = Show::default();
        let t = text_cue(&mut show, "static", Transition::FadeIn);
        let mut e = engine_without_media();
        e.playhead = Some(t);
        e.go(&show);
        drawn(&mut e, &show);
        let before = first_picture(&e, &show);
        e.tick(&show, 90.0);
        run_for(&mut e, &show, 0.2);
        assert_eq!(first_picture(&e, &show), before);
    }

    #[test]
    fn a_paused_timer_stands_still() {
        let mut show = Show::default();
        let t = text_cue(&mut show, "{time}", Transition::FadeIn);
        if let CueKind::Text(c) = &mut show.cues[0].kind {
            c.source = TextSource::Timer {
                total: 600.0,
                up: false,
                warn: 60.0,
            };
        }
        let mut e = engine_without_media();
        e.playhead = Some(t);
        e.go(&show);
        drawn(&mut e, &show);
        let before = first_picture(&e, &show);
        e.paused = true;
        e.tick(&show, 90.0);
        run_for(&mut e, &show, 0.2);
        assert_eq!(first_picture(&e, &show), before);
    }

    #[test]
    fn text_holds_for_its_time_then_ends() {
        let mut show = Show::default();
        let t = text_cue(&mut show, "Hi", Transition::FadeIn);
        if let CueKind::Text(c) = &mut show.cues[0].kind {
            c.media.end = Some(2.0);
        }
        let mut e = engine_without_media();
        assert_eq!(
            e.cue_duration(&show, &show.cues[0].clone()),
            Some(Some(2.0))
        );
        e.playhead = Some(t);
        e.go(&show);
        drawn(&mut e, &show);
        e.tick(&show, 1.0);
        assert!(playing(&e, t));
        e.tick(&show, 1.5);
        assert!(idle(&e, t));
        // With no hold time it stays until stopped.
        if let CueKind::Text(c) = &mut show.cues[0].kind {
            c.media.end = None;
        }
        assert_eq!(e.cue_duration(&show, &show.cues[0].clone()), Some(None));
        e.go_again(&show, t);
        drawn(&mut e, &show);
        e.tick(&show, 30.0);
        assert!(playing(&e, t));
    }

    #[test]
    fn text_layers_or_replaces_by_its_transition() {
        let out_layers = |show: &Show, e: &Engine| e.layers(first_output(show)).len();
        // A fade in leaves what is there showing; a cut replaces it.
        let mut show = Show::default();
        let a = text_cue(&mut show, "one", Transition::FadeIn);
        let b = text_cue(&mut show, "two", Transition::FadeIn);
        let mut e = engine_without_media();
        e.playhead = Some(a);
        e.go(&show);
        e.go(&show);
        drawn(&mut e, &show);
        assert!(playing(&e, a) && playing(&e, b));
        assert_eq!(out_layers(&show, &e), 2);

        let mut show = Show::default();
        let a = text_cue(&mut show, "one", Transition::FadeIn);
        let b = text_cue(&mut show, "two", Transition::Cut);
        let mut e = engine_without_media();
        e.playhead = Some(a);
        e.go(&show);
        drawn(&mut e, &show);
        e.go(&show);
        drawn(&mut e, &show);
        e.tick(&show, 0.1);
        assert!(idle(&e, a) && playing(&e, b));
        assert_eq!(out_layers(&show, &e), 1);
    }

    #[test]
    fn text_fades_with_its_own_fade_in() {
        let mut show = Show::default();
        let t = text_cue(&mut show, "Hi", Transition::FadeIn);
        if let CueKind::Text(c) = &mut show.cues[0].kind {
            c.media.fade_in = 2.0;
        }
        let out = first_output(&show);
        let mut e = engine_without_media();
        e.playhead = Some(t);
        e.go(&show);
        drawn(&mut e, &show);
        let early = e.layers(out)[0].opacity;
        e.tick(&show, 1.0);
        let mid = e.layers(out)[0].opacity;
        e.tick(&show, 2.0);
        let late = e.layers(out)[0].opacity;
        assert!(
            early < mid && mid < late && late > 0.99,
            "{early} {mid} {late}"
        );
    }

    #[test]
    fn a_preparation_that_failed_is_not_tried_again_every_tick() {
        let mut show = Show::default();
        // A camera cue with no camera chosen cannot be prepared.
        let c = show.make_cue(CueType::Camera, None, 0);
        let id = c.id;
        show.cues.push(c);
        let mut e = engine_without_media();
        e.ff = Some(Ffmpeg {
            ffmpeg: "no-such-ffmpeg".into(),
            ffprobe: "no-such-ffprobe".into(),
        });
        e.playhead = Some(id);
        e.tick(&show, 0.1);
        let (_, first) = e.failed_preps[&id].clone();
        std::thread::sleep(Duration::from_millis(20));
        e.tick(&show, 0.1);
        assert_eq!(e.failed_preps[&id].1, first, "it was not tried again");
        // Change what it would try and it is tried at once.
        if let CueKind::Camera(c) = &mut show.cues[0].kind {
            c.fps += 1;
        }
        e.tick(&show, 0.1);
        assert_ne!(e.failed_preps[&id].1, first);
        // Once the playhead moves on it is forgotten.
        e.playhead = None;
        e.tick(&show, 0.1);
        assert!(e.failed_preps.is_empty());
    }

    #[test]
    fn text_is_not_preloaded_as_a_file() {
        let mut show = Show::default();
        let t = text_cue(&mut show, "Hi", Transition::FadeIn);
        let mut e = engine_without_media();
        e.playhead = Some(t);
        e.tick(&show, 0.1);
        assert!(!e.is_prepared(t));
        assert!(e.messages.is_empty());
    }

    // ----- Transitions, driven with real ffmpeg on a still image -------------

    /// An engine with ffmpeg and the test image, or None to skip.
    fn engine_with_image() -> Option<(Engine, std::path::PathBuf)> {
        let (ff, img) = crate::media::test_media("color_red.png")?;
        let mut e = engine_without_media();
        e.ff = Some(ff);
        Some((e, img))
    }

    fn image_cue(show: &mut Show, img: &Path, t: Transition, time: f64) -> CueId {
        let mut c = show.make_cue(CueType::Image, Some(img.to_path_buf()), show.cues.len());
        let m = c.kind.media_mut().unwrap();
        m.transition = t;
        m.fade_in = time;
        m.fade_out = 1.0;
        let id = c.id;
        show.cues.push(c);
        id
    }

    /// (cue id or None for a matte, matte colour, opacity) for each layer, bottom to top.
    fn stack(e: &Engine, out: OutputId) -> Vec<(Option<CueId>, Option<[u8; 4]>, f32)> {
        e.layers(out)
            .iter()
            .map(|l| {
                let a = e.active.iter().find(|a| a.instance == l.instance).unwrap();
                match l.source {
                    LayerSource::Solid(c) => (None, Some(c), l.opacity),
                    _ => (Some(a.cue_id), None, l.opacity),
                }
            })
            .collect()
    }

    /// Tick in real time (decoders run on threads) until `cond` holds.
    fn run_until(e: &mut Engine, show: &Show, what: &str, cond: impl FnMut(&Engine) -> bool) {
        run_until_within(e, show, what, 10, cond);
    }

    /// `run_until`, for something that takes real time (a sound that plays
    /// for several seconds) on a slow machine.
    fn run_until_within(
        e: &mut Engine,
        show: &Show,
        what: &str,
        secs: u64,
        mut cond: impl FnMut(&Engine) -> bool,
    ) {
        let t0 = Instant::now();
        let mut last = Instant::now();
        while !cond(e) {
            assert!(
                t0.elapsed() < Duration::from_secs(secs),
                "timed out waiting for {what}"
            );
            std::thread::sleep(Duration::from_millis(5));
            let now = Instant::now();
            e.tick(show, now.duration_since(last).as_secs_f64());
            last = now;
        }
    }

    fn run_for(e: &mut Engine, show: &Show, secs: f64) {
        let t0 = Instant::now();
        run_until(e, show, "time", |_| t0.elapsed().as_secs_f64() >= secs);
    }

    #[test]
    fn built_in_transitions() {
        let Some((mut e, img)) = engine_with_image() else {
            return;
        };
        let mut show = Show::default();
        let out = show.outputs[0].id;
        let a = image_cue(&mut show, &img, Transition::Cut, 0.0);
        let b = image_cue(&mut show, &img, Transition::DipToWhite, 1.0);
        let c = image_cue(&mut show, &img, Transition::Crossfade, 0.6);
        let d = image_cue(&mut show, &img, Transition::Cut, 0.0);
        e.playhead = Some(a);

        // Cut: straight to full opacity.
        e.go(&show);
        run_until(&mut e, &show, "A on screen", |e| stack(e, out).len() == 1);
        assert_eq!(stack(&e, out), vec![(Some(a), None, 1.0)]);

        // Dip to white: the white matte fades up over A...
        e.go(&show);
        run_for(&mut e, &show, 0.25);
        let s = stack(&e, out);
        assert_eq!(s.len(), 2, "{s:?}");
        assert_eq!(s[0].0, Some(a));
        assert_eq!(s[1].1, Some([255, 255, 255, 255]));
        assert!(s[1].2 > 0.05 && s[1].2 < 0.95, "matte mid-fade: {s:?}");
        // ...then B replaces A underneath and the matte fades away.
        run_until(&mut e, &show, "B under matte", |e| {
            stack(e, out).first().map(|l| l.0) == Some(Some(b))
        });
        let s = stack(&e, out);
        assert!(s.iter().all(|l| l.0 != Some(a)), "A gone: {s:?}");
        run_until(&mut e, &show, "dip finished", |e| stack(e, out).len() == 1);
        assert_eq!(stack(&e, out), vec![(Some(b), None, 1.0)]);

        // Crossfade: C fades in over B while B fades out, then B is gone.
        e.go(&show);
        run_until(&mut e, &show, "C on screen", |e| stack(e, out).len() == 2);
        run_for(&mut e, &show, 0.3);
        let s = stack(&e, out);
        assert_eq!((s[0].0, s[1].0), (Some(b), Some(c)), "{s:?}");
        assert!(
            s[0].2 < 1.0 && s[1].2 > 0.0 && s[1].2 < 1.0,
            "mid crossfade: {s:?}"
        );
        run_until(&mut e, &show, "crossfade finished", |e| {
            stack(e, out).len() == 1
        });
        assert_eq!(stack(&e, out), vec![(Some(c), None, 1.0)]);

        // Cut replaces instantly once D's frame is ready.
        e.go(&show);
        run_until(&mut e, &show, "D on screen", |e| {
            stack(e, out).iter().any(|l| l.0 == Some(d))
        });
        assert_eq!(stack(&e, out), vec![(Some(d), None, 1.0)]);

        // Built-in fade-out (1 s) on demand.
        e.fade_out_cue(&show, d);
        run_for(&mut e, &show, 0.5);
        let s = stack(&e, out);
        assert!(
            s.len() == 1 && s[0].2 > 0.1 && s[0].2 < 0.9,
            "fading: {s:?}"
        );
        run_until(&mut e, &show, "D faded out", |e| stack(e, out).is_empty());
    }

    #[test]
    fn dip_to_black_from_empty_output() {
        let Some((mut e, img)) = engine_with_image() else {
            return;
        };
        let mut show = Show::default();
        let out = show.outputs[0].id;
        let a = image_cue(&mut show, &img, Transition::DipToBlack, 0.4);
        e.playhead = Some(a);
        e.go(&show);
        let s = stack(&e, out);
        assert_eq!(s, vec![(None, Some([0, 0, 0, 255]), 0.0)]);
        run_until(&mut e, &show, "dip finished", |e| {
            stack(e, out) == vec![(Some(a), None, 1.0)]
        });
    }

    #[test]
    fn stop_cue_uses_each_cues_own_fade() {
        let Some((mut e, img)) = engine_with_image() else {
            return;
        };
        let mut show = Show::default();
        let out = show.outputs[0].id;
        let a = image_cue(&mut show, &img, Transition::Cut, 0.0);
        let stop = show.make_cue(CueType::Stop, None, 1);
        show.cues.push(stop);
        e.playhead = Some(a);
        e.go(&show);
        run_until(&mut e, &show, "A on screen", |e| stack(e, out).len() == 1);
        e.go(&show); // Stop all, own fades (A fades over 1 s)
        run_for(&mut e, &show, 0.4);
        let s = stack(&e, out);
        assert!(s.len() == 1 && s[0].2 < 1.0 && s[0].2 > 0.0, "{s:?}");
        run_until(&mut e, &show, "stopped", |e| !e.is_active());
    }

    #[test]
    fn fired_cues_stay_preloaded_through_pre_wait_and_dip() {
        let Some((mut e, img)) = engine_with_image() else {
            return;
        };
        let mut show = Show::default();
        let a = image_cue(&mut show, &img, Transition::DipToBlack, 1.0);
        let b = image_cue(&mut show, &img, Transition::Cut, 0.0);
        show.cues[1].pre_wait = 1.0;
        e.playhead = Some(a);
        run_until(&mut e, &show, "A preloaded", |e| e.is_prepared(a));
        e.go(&show);
        run_for(&mut e, &show, 0.2);
        assert!(
            e.is_prepared(a),
            "dip target must stay preloaded until its midpoint"
        );
        run_until(&mut e, &show, "B preloaded", |e| e.is_prepared(b));
        e.go(&show);
        run_for(&mut e, &show, 0.3);
        assert!(e.is_prepared(b), "pre-wait cue must stay preloaded");
    }

    // ----- Camera cues, using the built-in test picture as the "camera" ------------

    fn camera_cue(show: &mut Show, device: &str) -> CueId {
        let mut c = show.make_cue(CueType::Camera, None, show.cues.len());
        if let CueKind::Camera(cam) = &mut c.kind {
            cam.device = device.into();
            cam.width = 320;
            cam.height = 180;
            cam.media.transition = Transition::Cut;
            cam.media.fade_in = 0.0;
            cam.media.fade_out = 0.0;
        }
        let id = c.id;
        show.cues.push(c);
        id
    }

    fn shown_frames(e: &Engine, id: CueId) -> u64 {
        e.active
            .iter()
            .find(|a| a.cue_id == id)
            .and_then(|a| match &a.kind {
                ActiveKind::Video(v) => Some(v.player.serial),
                _ => None,
            })
            .unwrap_or(0)
    }

    #[test]
    fn a_camera_cue_shows_a_live_picture_and_stops_when_asked() {
        let Some((mut e, _)) = engine_with_image() else {
            return;
        };
        let mut show = Show::default();
        let out = show.outputs[0].id;
        let cam = camera_cue(&mut show, TEST_PATTERN_DEVICE);
        e.playhead = Some(cam);
        e.go(&show);
        run_until(&mut e, &show, "camera on screen", |e| {
            stack(e, out).len() == 1
        });
        assert_eq!(stack(&e, out), vec![(Some(cam), None, 1.0)]);
        // Live: no length, and new pictures keep arriving.
        assert_eq!(e.active[0].duration, None);
        let first = shown_frames(&e, cam);
        run_for(&mut e, &show, 0.6);
        assert!(shown_frames(&e, cam) >= first + 8, "picture is not moving");
        assert!(stack(&e, out).len() == 1);
        e.stop_all(0.0);
        e.tick(&show, 0.01);
        assert!(stack(&e, out).is_empty());
    }

    #[test]
    fn a_camera_cue_can_run_for_a_set_time() {
        let Some((mut e, _)) = engine_with_image() else {
            return;
        };
        let mut show = Show::default();
        let cam = camera_cue(&mut show, TEST_PATTERN_DEVICE);
        if let CueKind::Camera(c) = &mut show.cues[0].kind {
            c.media.end = Some(0.5);
        }
        e.playhead = Some(cam);
        e.go(&show);
        run_until(&mut e, &show, "camera running", |e| {
            shown_frames(e, cam) > 0
        });
        run_until(&mut e, &show, "camera cue to end", |e| e.active.is_empty());
    }

    #[test]
    fn a_camera_is_opened_ahead_of_go() {
        let Some((mut e, _)) = engine_with_image() else {
            return;
        };
        let mut show = Show::default();
        let cam = camera_cue(&mut show, TEST_PATTERN_DEVICE);
        e.playhead = Some(cam);
        e.tick(&show, 0.1);
        assert!(e.is_prepared(cam), "the camera should be open before GO");
        e.go(&show);
        // GO takes the open camera rather than opening a second one.
        assert!(!e.is_prepared(cam));
        run_until(&mut e, &show, "camera on screen", |e| {
            shown_frames(e, cam) > 0
        });
        // Changing the mode closes the preloaded camera and opens it again.
        let mut show2 = Show::default();
        let next = camera_cue(&mut show2, TEST_PATTERN_DEVICE);
        let mut e2 = engine_without_media();
        e2.ff = e.ff.clone();
        e2.playhead = Some(next);
        e2.tick(&show2, 0.1);
        assert!(e2.is_prepared(next));
        if let CueKind::Camera(c) = &mut show2.cues[0].kind {
            c.fps = 15;
        }
        e2.tick(&show2, 0.1);
        assert!(e2.is_prepared(next));
        assert_eq!(e2.prepared.len(), 1);
        assert_eq!(e2.prepared[0].sig.camera.as_ref().map(|c| c.3), Some(15));
    }

    #[test]
    fn a_camera_cue_without_a_camera_says_so() {
        let Some((mut e, _)) = engine_with_image() else {
            return;
        };
        let mut show = Show::default();
        let cam = camera_cue(&mut show, "");
        e.playhead = Some(cam);
        e.go(&show);
        assert!(e.active.is_empty());
        let (_, msg) = e.messages.back().expect("a message");
        assert!(msg.contains("no camera chosen"), "{msg}");
    }

    #[test]
    fn a_camera_that_cannot_be_opened_ends_the_cue_with_the_reason() {
        let Some((mut e, _)) = engine_with_image() else {
            return;
        };
        let mut show = Show::default();
        // Not a device on any system: ffmpeg fails to open it.
        let cam = camera_cue(&mut show, "/dev/no-such-camera");
        e.playhead = Some(cam);
        e.go(&show);
        run_until(&mut e, &show, "the failure", |e| e.active.is_empty());
        let (_, msg) = e.messages.back().expect("a message");
        assert!(msg.contains("video failed"), "{msg}");
    }

    // ----- Timecode triggers -----------------------------------------------------

    fn at(h: u8, m: u8, sec: u8, f: u8) -> Timecode {
        Timecode::new(h, m, sec, f)
    }

    fn tc_cue(show: &mut Show, tc: Timecode) -> CueId {
        let id = wait_cue(show, 30.0, ContinueMode::DoNotContinue);
        show.cue_mut(id).unwrap().timecode = Some(tc);
        id
    }

    /// Run the clock from `from` for `frames`, two frames at a time.
    fn run_clock(e: &mut Engine, show: &Show, from: Timecode, frames: u32) {
        let mut n = from.frame_number(25);
        for _ in 0..frames / 2 {
            n += 2;
            let t = at(
                (n / (25 * 3600)) as u8,
                (n / (25 * 60) % 60) as u8,
                (n / 25 % 60) as u8,
                (n % 25) as u8,
            );
            e.on_timecode(show, t, Rate::Fps25);
        }
    }

    #[test]
    fn cues_fire_as_the_timecode_reaches_them() {
        let mut show = Show::default();
        let a = tc_cue(&mut show, at(1, 0, 1, 0));
        let b = tc_cue(&mut show, at(1, 0, 3, 0));
        let untimed = wait_cue(&mut show, 30.0, ContinueMode::DoNotContinue);
        let mut e = engine_without_media();
        e.playhead = Some(untimed);
        e.timecode.locate(at(1, 0, 0, 0), Rate::Fps25);

        run_clock(&mut e, &show, at(1, 0, 0, 0), 2 * 25);
        assert!(playing(&e, a) && idle(&e, b), "a fired, b not yet");
        run_clock(&mut e, &show, at(1, 0, 2, 0), 2 * 25);
        assert!(playing(&e, b));
        assert!(idle(&e, untimed), "cues with no timecode are left alone");
        assert_eq!(e.playhead, Some(untimed), "the playhead doesn't move");
    }

    #[test]
    fn a_timecode_cue_that_is_disarmed_is_skipped_and_a_group_fires_whole() {
        let mut show = Show::default();
        let off = tc_cue(&mut show, at(0, 0, 1, 0));
        disarm(&mut show, off);
        let (g, kids) = group(&mut show, GroupMode::Timeline, &[5.0, 5.0]);
        show.cue_mut(g).unwrap().timecode = Some(at(0, 0, 2, 0));
        let mut e = engine_without_media();
        e.timecode.locate(at(0, 0, 0, 0), Rate::Fps25);
        run_clock(&mut e, &show, at(0, 0, 0, 0), 3 * 25);
        assert!(idle(&e, off), "a disarmed cue isn't fired");
        assert!(playing(&e, g) && playing(&e, kids[0]) && playing(&e, kids[1]));
    }

    #[test]
    fn jumping_the_clock_does_not_fire_the_cues_it_skips() {
        let mut show = Show::default();
        let early = tc_cue(&mut show, at(0, 0, 10, 0));
        let late = tc_cue(&mut show, at(0, 1, 0, 0));
        let mut e = engine_without_media();
        e.timecode.locate(at(0, 0, 0, 0), Rate::Fps25);
        run_clock(&mut e, &show, at(0, 0, 0, 0), 2 * 25);
        // The source jumps to 00:00:59:00, past the first cue and just short of the second.
        e.on_timecode(&show, at(0, 0, 59, 0), Rate::Fps25);
        assert!(idle(&e, early) && idle(&e, late));
        run_clock(&mut e, &show, at(0, 0, 59, 0), 2 * 25);
        assert!(playing(&e, late) && idle(&e, early));
    }

    #[test]
    fn the_follower_reports_whether_timecode_is_arriving() {
        let show = Show::default();
        let mut e = engine_without_media();
        assert!(!e.timecode.running() && e.timecode.position().is_none());
        e.on_timecode(&show, at(0, 0, 5, 0), Rate::Fps30);
        assert!(e.timecode.running());
        assert_eq!(e.timecode.position(), Some((at(0, 0, 5, 0), Rate::Fps30)));
        e.tick(&show, 0.6);
        assert!(!e.timecode.running(), "half a second of silence is a stop");
    }

    // ----- Send cues -----------------------------------------------------------

    fn send_cue(show: &mut Show, address: &str, mode: ContinueMode) -> CueId {
        let mut c = show.make_cue(CueType::Send, None, show.cues.len());
        if let CueKind::Send(s) = &mut c.kind {
            s.target = SendTarget::Osc(OscOut {
                address: address.into(),
                ..OscOut::default()
            });
        }
        c.continue_mode = mode;
        let id = c.id;
        show.cues.push(c);
        id
    }

    fn sent_addresses(e: &Engine) -> Vec<String> {
        e.outbox
            .sent()
            .iter()
            .filter_map(|m| match m {
                Outgoing::Osc { packet, .. } => {
                    let end = packet.iter().position(|&b| b == 0)?;
                    Some(String::from_utf8_lossy(&packet[..end]).into_owned())
                }
                Outgoing::Midi { .. }
                | Outgoing::Projector(_)
                | Outgoing::Tcp { .. }
                | Outgoing::Script { .. } => None,
            })
            .collect()
    }

    #[test]
    fn a_send_cue_sends_when_fired_and_takes_no_time() {
        let mut show = Show::default();
        let a = send_cue(&mut show, "/one", ContinueMode::DoNotContinue);
        let mut e = engine_without_media();
        e.playhead = Some(a);
        e.go(&show);
        assert_eq!(sent_addresses(&e), ["/one"]);
        assert!(e.active.is_empty(), "nothing is left running");
        assert_eq!(e.cue_duration(&show, show.cue(a).unwrap()), Some(Some(0.0)));
    }

    #[test]
    fn a_timecode_cue_starts_its_clock_and_ends_after_its_time() {
        use crate::timecode::{Rate, Timecode, parse_full_frame};
        let mut show = Show::default();
        let mut c = show.make_cue(CueType::Timecode, None, 0);
        if let CueKind::Timecode(t) = &mut c.kind {
            t.port = Some("Desk".into());
            t.start = Timecode::new(1, 0, 0, 0);
            t.rate = Rate::Fps30;
            t.duration = 2.0;
        }
        let id = c.id;
        show.cues.push(c);
        let mut forever = show.make_cue(CueType::Timecode, None, 1);
        if let CueKind::Timecode(t) = &mut forever.kind {
            t.duration = 0.0;
        }
        let forever_id = forever.id;
        show.cues.push(forever);
        let mut e = engine_without_media();
        e.playhead = Some(id);
        e.go(&show);
        let sent = e.outbox.sent();
        let Some(Outgoing::Midi { port, bytes }) = sent.first() else {
            panic!("no clock was started: {sent:?}")
        };
        assert_eq!(port.as_deref(), Some("Desk"));
        assert_eq!(
            parse_full_frame(bytes),
            Some((Timecode::new(1, 0, 0, 0), Rate::Fps30))
        );
        e.tick(&show, 1.0);
        assert!(playing(&e, id));
        e.tick(&show, 1.1);
        assert!(idle(&e, id), "it ends when its time is up");
        // With no time given it runs until it is stopped.
        e.playhead = Some(forever_id);
        e.go(&show);
        e.tick(&show, 100.0);
        assert!(playing(&e, forever_id));
        e.stop_all(0.0);
        assert!(idle(&e, forever_id));
    }

    #[test]
    fn send_cues_chain_like_stop_cues_and_respect_arming() {
        let mut show = Show::default();
        let a = send_cue(&mut show, "/a", ContinueMode::AutoContinue);
        send_cue(&mut show, "/b", ContinueMode::AutoFollow);
        let c = send_cue(&mut show, "/c", ContinueMode::AutoContinue);
        let d = send_cue(&mut show, "/d", ContinueMode::DoNotContinue);
        disarm(&mut show, c);
        let mut e = engine_without_media();
        e.playhead = Some(a);
        e.go(&show);
        // a continues into b, which follows into the next armed cue, passing c.
        assert_eq!(sent_addresses(&e), ["/a", "/b", "/d"]);
        assert!(e.active.is_empty());
        let _ = d;
        // A disarmed send cue is not sent by hand either.
        e.fire(&show, c);
        assert_eq!(sent_addresses(&e).len(), 3);
    }

    #[test]
    fn a_send_cue_can_follow_a_wait() {
        let mut show = Show::default();
        let w = wait_cue(&mut show, 2.0, ContinueMode::AutoFollow);
        let s = send_cue(&mut show, "/after", ContinueMode::DoNotContinue);
        let mut e = engine_without_media();
        e.playhead = Some(w);
        e.go(&show);
        e.tick(&show, 1.0);
        assert!(sent_addresses(&e).is_empty());
        e.tick(&show, 1.1);
        assert_eq!(sent_addresses(&e), ["/after"]);
        let _ = s;
    }

    #[test]
    fn colour_edits_reach_a_playing_layer_live() {
        let Some((mut e, img)) = engine_with_image() else {
            return;
        };
        let mut show = Show::default();
        let out = show.outputs[0].id;
        let a = image_cue(&mut show, &img, Transition::Cut, 0.0);
        e.playhead = Some(a);
        e.go(&show);
        run_until(&mut e, &show, "on screen", |e| e.layers(out).len() == 1);
        assert!(e.layers(out)[0].color.is_default());
        // Edit the cue while it plays: the layer follows on the next tick.
        let i = show.cue_index(a).unwrap();
        let m = show.cues[i].kind.media_mut().unwrap();
        m.color.brightness = 0.3;
        m.color.key = Some(ChromaKey::default());
        e.tick(&show, 0.01);
        let c = e.layers(out)[0].color;
        assert_eq!(c.brightness, 0.3);
        assert!(c.key.is_some());
    }

    // ----- Clock triggers --------------------------------------------------------

    fn clock_cue(show: &mut Show, at: TimeOfDay) -> CueId {
        let id = wait_cue(show, 30.0, ContinueMode::DoNotContinue);
        show.cue_mut(id).unwrap().clock = Some(at);
        id
    }

    fn tod(h: u32, m: u32, s: u32) -> TimeOfDay {
        TimeOfDay::new(h, m, s).unwrap()
    }

    #[test]
    fn a_clock_cue_fires_when_the_time_of_day_reaches_it() {
        let mut show = Show::default();
        let house = clock_cue(&mut show, tod(18, 30, 0));
        let untimed = wait_cue(&mut show, 30.0, ContinueMode::DoNotContinue);
        let mut e = engine_without_media();
        e.playhead = Some(untimed);
        e.on_clock(&show, tod(18, 29, 58), 0.0);
        e.on_clock(&show, tod(18, 29, 59), 0.0);
        assert!(idle(&e, house), "not yet");
        e.on_clock(&show, tod(18, 30, 0), 0.0);
        assert!(playing(&e, house));
        assert!(idle(&e, untimed), "cues with no time are left alone");
        assert_eq!(e.playhead, Some(untimed), "the playhead doesn't move");
        // Looking again in the same second, or later, doesn't fire it again.
        e.stop_all(0.0);
        e.on_clock(&show, tod(18, 30, 0), 0.0);
        e.on_clock(&show, tod(18, 30, 1), 0.0);
        assert!(idle(&e, house));
    }

    #[test]
    fn clock_cues_skip_disarmed_cues_start_groups_and_ignore_jumps() {
        let mut show = Show::default();
        let off = clock_cue(&mut show, tod(9, 0, 1));
        disarm(&mut show, off);
        let (g, kids) = group(&mut show, GroupMode::Timeline, &[5.0, 5.0]);
        show.cue_mut(g).unwrap().clock = Some(tod(9, 0, 2));
        let late = clock_cue(&mut show, tod(9, 30, 0));
        let mut e = engine_without_media();
        e.on_clock(&show, tod(9, 0, 0), 0.0);
        e.on_clock(&show, tod(9, 0, 1), 0.0);
        e.on_clock(&show, tod(9, 0, 2), 0.0);
        assert!(idle(&e, off), "a disarmed cue isn't fired");
        assert!(playing(&e, g) && playing(&e, kids[0]) && playing(&e, kids[1]));
        // The machine sleeps for twenty minutes: the 9:30 cue is not started late.
        e.on_clock(&show, tod(9, 22, 0), 0.0);
        assert!(idle(&e, late));
        e.on_clock(&show, tod(9, 29, 59), 0.0);
        e.on_clock(&show, tod(9, 30, 0), 0.0);
        assert!(
            playing(&e, late),
            "and it still fires on time when it plays through"
        );
    }

    // ----- Scrubbing ---------------------------------------------------------

    /// A playing video cue on the default output, or None to skip.
    fn playing_video(looping: bool) -> Option<(Engine, Show, OutputId, u64)> {
        let (ff, clip) = crate::media::test_media("testsrc_10s.mp4")?;
        let mut e = engine_without_media();
        e.ff = Some(ff);
        let mut show = Show::default();
        let out = show.outputs[0].id;
        let mut c = show.make_cue(CueType::Video, Some(clip), 0);
        c.kind.media_mut().unwrap().loop_media = looping;
        let id = c.id;
        show.cues.push(c);
        e.playhead = Some(id);
        e.go(&show);
        run_until(&mut e, &show, "the clip to start", |e| {
            e.active.first().is_some_and(|a| a.started)
        });
        let instance = e.active[0].instance;
        Some((e, show, out, instance))
    }

    fn elapsed(e: &Engine, instance: u64) -> f64 {
        e.active
            .iter()
            .find(|a| a.instance == instance)
            .unwrap()
            .elapsed
    }

    #[test]
    fn a_matrix_keeps_the_channels_the_file_has() {
        use crate::show::Crosspoint;
        let mut m = crate::show::MediaCue::new("a.wav".into(), None);
        assert!(
            Engine::matrix_for(&m, 6).is_none(),
            "no matrix, the stereo pair"
        );
        m.matrix = vec![
            Crosspoint {
                from: 0,
                to: 1,
                db: 0.0,
            },
            Crosspoint {
                from: 5,
                to: 2,
                db: -6.0,
            },
            // Not in a stereo file, and so dropped; and a level that is silence
            // (kept, as a gain of nothing, for a fade cue to bring up).
            Crosspoint {
                from: 3,
                to: 0,
                db: 0.0,
            },
            Crosspoint {
                from: 1,
                to: 0,
                db: -200.0,
            },
        ];
        let x = Engine::matrix_for(&m, 6).unwrap();
        assert_eq!(x.source_channels, 6);
        assert!(!x.crop);
        assert_eq!(x.points.len(), 4);
        assert_eq!(x.points[3], (1, 0, 0.0));
        assert_eq!(x.keys[1], (5, 2));
        let stereo = Engine::matrix_for(&m, 2).unwrap();
        assert_eq!((stereo.source_channels, stereo.points.len()), (2, 2));
        assert!(Engine::matrix_for(&m, 12).unwrap().crop);
    }

    #[test]
    fn a_fade_cue_moves_a_picture_and_an_animated_track_is_followed() {
        let Some((ff, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let mut e = engine_without_media();
        e.ff = Some(ff);
        let mut show = Show::default();
        let out = show.outputs[0].id;
        let mut c = show.make_cue(CueType::Video, Some(clip), 0);
        let m = c.kind.media_mut().unwrap();
        m.transform.x = 0.1;
        // Slides across from 0 to 0.5 over the first second of the cue.
        m.motion.x = Some(Animated::new(0.0));
        m.motion.x.as_mut().unwrap().set_key(0.0, 0.0);
        m.motion.x.as_mut().unwrap().set_key(1.0, 0.5);
        let id = c.id;
        show.cues.push(c);
        let mut fade = show.make_cue(CueType::Fade, None, 0);
        if let CueKind::Fade(f) = &mut fade.kind {
            f.target = Some(id);
            f.duration = 1.0;
            f.curve = Curve::Linear;
            f.opacity = None;
            f.volume_db = None;
            f.stop_target = false;
            f.geometry.x = Some(1.0);
            f.geometry.scale_x = Some(2.0);
        }
        let fade_id = fade.id;
        show.cues.push(fade);
        e.playhead = Some(id);
        e.go(&show);
        run_until(&mut e, &show, "the clip to start", |e| {
            e.active.first().is_some_and(|a| a.started)
        });
        let x_at = |e: &Engine| e.layers(out)[0].transform;
        let t = x_at(&e);
        assert!(t.x <= 0.5 && (t.scale_x - 1.0).abs() < 1e-6, "{t:?}");
        // The fade starts from wherever the track has got to.
        e.playhead = Some(fade_id);
        e.go(&show);
        e.tick(&show, 0.0);
        let start = x_at(&e).x;
        e.tick(&show, 0.5);
        let mid = x_at(&e);
        assert!(mid.x > start && mid.x < 1.0, "{start} -> {}", mid.x);
        assert!(mid.scale_x > 1.0 && mid.scale_x < 2.0, "{}", mid.scale_x);
        e.tick(&show, 0.6);
        let end = x_at(&e);
        assert_eq!((end.x, end.scale_x), (1.0, 2.0));
        assert_eq!(end.y, 0.0, "what the fade did not name is left");
    }

    #[test]
    fn a_fade_cue_reaches_a_looping_cue_whose_speed_and_position_are_keyframed() {
        let Some((ff, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let mut e = engine_without_media();
        e.ff = Some(ff);
        let mut show = Show::default();
        let out = show.outputs[0].id;
        let mut c = show.make_cue(CueType::Video, Some(clip), 0);
        let m = c.kind.media_mut().unwrap();
        m.end = Some(3.0);
        m.loop_media = true;
        m.loop_count = 3;
        m.rate.set_key(0.0, 0.8);
        m.rate.set_key(4.0, 1.25);
        m.rate.set_key(8.0, 0.8);
        let mut x = Animated::new(0.0);
        x.set_key(0.0, -0.3);
        x.set_key(8.0, 0.3);
        m.motion.x = Some(x);
        let id = c.id;
        show.cues.push(c);
        let mut fade = show.make_cue(CueType::Fade, None, 1);
        if let CueKind::Fade(f) = &mut fade.kind {
            f.target = Some(id);
            f.duration = 2.0;
            f.opacity = Some(0.4);
            f.volume_db = None;
            f.stop_target = false;
            f.geometry.y = Some(0.3);
        }
        let fade_id = fade.id;
        show.cues.push(fade);
        e.playhead = Some(id);
        e.go(&show);
        run_until(&mut e, &show, "the loop to start", |e| {
            e.active.first().is_some_and(|a| a.started)
        });
        let started = std::time::Instant::now();
        while started.elapsed() < Duration::from_secs(2) {
            e.tick(&show, 1.0 / 60.0);
            std::thread::sleep(Duration::from_millis(16));
        }
        assert!(playing(&e, id), "the loop is still playing after 2 s");
        e.playhead = Some(fade_id);
        e.go(&show);
        e.tick(&show, 0.0);
        assert!(
            e.messages.iter().all(|m| !m.1.contains("is not playing")),
            "{:?}",
            e.messages
        );
        e.tick(&show, 2.1);
        let l = &e.layers(out)[0];
        assert!((l.transform.y - 0.3).abs() < 1e-4, "{:?}", l.transform);
        assert!((l.opacity - 0.4).abs() < 1e-4, "{}", l.opacity);
    }

    #[test]
    fn a_ramped_speed_sets_the_length_and_the_cue_plays_to_its_end() {
        let Some((ff, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let mut e = engine_without_media();
        e.ff = Some(ff);
        let mut show = Show::default();
        let mut c = show.make_cue(CueType::Video, Some(clip), 0);
        let m = c.kind.media_mut().unwrap();
        // 1x for the first second, rising to 3x at 3 s, then 3x: 5 s of the
        // file by 3 s, and the other 5 in 5/3 s more.
        m.rate.set_key(0.0, 1.0);
        m.rate.set_key(1.0, 1.0);
        m.rate.set_key(3.0, 3.0);
        let id = c.id;
        show.cues.push(c.clone());
        let expect = (m_speed_time(&c, 10.0) * 1000.0).round() / 1000.0;
        assert!((expect - (3.0 + 5.0 / 3.0)).abs() < 0.05, "{expect}");
        e.playhead = Some(id);
        e.go(&show);
        run_until(&mut e, &show, "the clip to start", |e| {
            e.active.first().is_some_and(|a| a.started)
        });
        let d = e.active[0].duration.expect("a known length");
        assert!((d - expect).abs() < 0.05, "{d}");
        assert_eq!(
            e.cue_duration(&show, &c).flatten().map(|d| d.round()),
            Some(5.0)
        );
        run_until(&mut e, &show, "the cue to end", |e| {
            matches!(e.status(id), CueStatus::Idle)
        });
    }

    #[test]
    fn a_loop_with_a_keyframed_speed_knows_where_it_is_and_how_to_end() {
        let Some((ff, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let mut e = engine_without_media();
        e.ff = Some(ff);
        let mut show = Show::default();
        let mut c = show.make_cue(CueType::Video, Some(clip), 0);
        let m = c.kind.media_mut().unwrap();
        m.loop_media = true;
        m.end = Some(2.0);
        // 2x all the way: a 2 s pass takes 1 s of the cue.
        m.rate.set_key(0.0, 2.0);
        let id = c.id;
        show.cues.push(c.clone());
        let devamp = control_cue(&mut show, ControlAction::Devamp, id);
        e.playhead = Some(id);
        e.go(&show);
        run_until(&mut e, &show, "the loop to start", |e| {
            e.active
                .first()
                .is_some_and(|a| a.started && a.elapsed > 0.3)
        });
        let a = &e.active[0];
        assert!(a.loops_forever() && a.speed.is_some());
        // The bar is in the file's seconds, one pass of them, and moves with the speed.
        assert_eq!(a.pass_len, Some(2.0));
        let (pos, el) = (a.bar_position(), a.elapsed);
        assert!((pos - (el * 2.0) % 2.0).abs() < 1e-6, "{pos} at {el}");
        assert_eq!(a.scrub_length(&show), Some(2.0));
        // A jump to 0.5 s into the pass stays in this pass of the cue's time.
        let instance = a.instance;
        e.seek(&show, instance, 0.5).unwrap();
        run_until(&mut e, &show, "the seek to land", |e| {
            e.active.iter().all(|a| !a.seeking())
        });
        let a = &e.active[0];
        assert!((a.bar_position() - 0.5).abs() < 0.4, "{}", a.bar_position());
        // Devamp ends it at the end of the pass, in the cue's time.
        e.fire(&show, devamp);
        let a = &e.active[0];
        assert!(!a.loops_forever());
        let end = a.duration.unwrap();
        assert!(end > a.elapsed && end <= a.elapsed + 1.0 + 1e-6, "{end}");
        run_until(&mut e, &show, "the cue to end", |e| {
            matches!(e.status(id), CueStatus::Idle)
        });
    }

    #[test]
    fn a_loop_with_a_set_number_of_plays_and_a_ramped_speed_has_the_length_the_map_says() {
        let mut show = Show::default();
        let mut c = show.make_cue(CueType::Video, Some("x.mp4".into()), 0);
        let m = c.kind.media_mut().unwrap();
        m.loop_media = true;
        m.loop_count = 3;
        m.end = Some(2.0);
        m.rate.set_key(0.0, 1.0);
        m.rate.set_key(4.0, 2.0);
        let want = m.speed_map().unwrap().time_for(6.0);
        assert!(want > 3.0 && want < 6.0, "{want}");
        let total = Some(10.0);
        let map = m.speed_map().unwrap();
        assert_eq!(
            m.playback(total).total(None).map(|d| map.time_for(d)),
            Some(want)
        );
    }

    fn m_speed_time(c: &Cue, file: f64) -> f64 {
        c.kind.media().unwrap().speed_map().unwrap().time_for(file)
    }

    #[test]
    fn a_hap_cue_plays_as_hap_and_has_the_length_of_the_file() {
        let Some((ff, src)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let dir = std::env::temp_dir().join("newq_test_hap_engine");
        std::fs::create_dir_all(&dir).unwrap();
        let clip = dir.join("clip.mov");
        let ok = ff
            .command()
            .args(["-y", "-i"])
            .arg(&src)
            .args(["-t", "2", "-vf", "scale=320:180", "-c:v", "hap", "-an"])
            .arg(&clip)
            .status()
            .unwrap()
            .success();
        assert!(ok);
        video::set_bc_supported(true);
        let mut e = engine_without_media();
        e.ff = Some(ff);
        let mut show = Show::default();
        let mut c = show.make_cue(CueType::Video, Some(clip.clone()), 0);
        c.kind.media_mut().unwrap().rate.value = 2.0;
        let id = c.id;
        show.cues.push(c.clone());
        e.playhead = Some(id);
        e.go(&show);
        run_until(&mut e, &show, "the clip to start", |e| {
            e.active.first().is_some_and(|a| a.started)
        });
        // Two seconds of file at double speed.
        let d = e.active[0].duration.expect("a known length");
        assert!((d - 1.0).abs() < 0.1, "{d}");
        match &e.active[0].kind {
            ActiveKind::Video(v) => {
                assert_eq!((v.player.width, v.player.height), (320, 180));
                assert!(
                    v.player.current().is_some_and(|f| f.hap.is_some()),
                    "its frames are Hap's"
                );
            }
            _ => panic!("a video cue"),
        }
        run_until(&mut e, &show, "the cue to end", |e| {
            matches!(e.status(id), CueStatus::Idle)
        });
        video::set_bc_supported(false);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_faster_cue_is_shorter_and_ends_when_its_picture_does() {
        let Some((ff, clip)) = crate::media::test_media("testsrc_10s.mp4") else {
            return;
        };
        let mut e = engine_without_media();
        e.ff = Some(ff);
        let mut show = Show::default();
        let mut c = show.make_cue(CueType::Video, Some(clip), 0);
        let m = c.kind.media_mut().unwrap();
        m.end = Some(4.0);
        m.rate.value = 2.0;
        let id = c.id;
        show.cues.push(c.clone());
        e.playhead = Some(id);
        e.go(&show);
        run_until(&mut e, &show, "the clip to start", |e| {
            e.active.first().is_some_and(|a| a.started)
        });
        let d = e.active[0].duration.expect("a known length");
        assert!(
            (d - 2.0).abs() < 0.1,
            "4 s of file at double speed is 2 s, not {d}"
        );
        assert_eq!(e.cue_duration(&show, &c), Some(Some(2.0)));
        run_until(&mut e, &show, "the cue to end", |e| {
            matches!(e.status(id), CueStatus::Idle)
        });
    }

    #[test]
    fn seeking_jumps_a_playing_video() {
        let Some((mut e, show, out, instance)) = playing_video(false) else {
            return;
        };
        let a = e.active.iter().find(|a| a.instance == instance).unwrap();
        assert!((a.scrub_length(&show).expect("a video can be scrubbed") - 10.0).abs() < 0.5);
        assert!(elapsed(&e, instance) < 2.0);

        e.seek(&show, instance, 6.0).unwrap();
        run_until(&mut e, &show, "the seek to land", |e| {
            e.active.iter().all(|a| !a.seeking()) && elapsed(e, instance) >= 6.0
        });
        let now = elapsed(&e, instance);
        assert!((6.0..7.5).contains(&now), "elapsed {now}");
        // The picture is the one for the new position, not the old one.
        let layers = e.layers(out);
        let LayerSource::Video(player) = &layers[0].source else {
            panic!("a video layer");
        };
        assert!(player.current().is_some());
        drop(layers);

        // It keeps playing from there, and a seek past the end lands at the end.
        run_for(&mut e, &show, 0.3);
        assert!(elapsed(&e, instance) > now);
        e.seek(&show, instance, 100.0).unwrap();
        run_until(&mut e, &show, "the clip to end", |e| e.active.is_empty());
    }

    #[test]
    fn a_video_output_with_a_delay_shows_its_pictures_late() {
        let Some((mut e, mut show, out, _)) = playing_video(false) else {
            return;
        };
        // One that is already playing keeps the delay it started with.
        assert!(!e.layers(out).is_empty());
        e.stop_all(0.0);
        run_until(&mut e, &show, "the clip to stop", |e| e.active.is_empty());

        show.sync.video_ms = 600;
        let id = show.cues[0].id;
        e.playhead = Some(id);
        e.go(&show);
        run_until(&mut e, &show, "the clip to start", |e| {
            e.active.first().is_some_and(|a| a.started)
        });
        assert!(e.layers(out).is_empty(), "held back at first");
        assert_eq!(e.active[0].video_lag, 0.6);
        run_until(&mut e, &show, "the picture to show", |e| {
            !e.layers(out).is_empty()
        });
        assert!(e.active[0].age >= 0.6, "{}", e.active[0].age);
    }

    #[test]
    fn a_later_seek_replaces_an_earlier_one() {
        let Some((mut e, show, _, instance)) = playing_video(false) else {
            return;
        };
        e.seek(&show, instance, 8.0).unwrap();
        e.seek(&show, instance, 3.0).unwrap();
        run_until(&mut e, &show, "the seek to land", |e| {
            e.active.iter().all(|a| !a.seeking())
        });
        let now = elapsed(&e, instance);
        assert!((3.0..4.5).contains(&now), "elapsed {now}");
    }

    #[test]
    fn seeking_works_while_paused() {
        let Some((mut e, show, _, instance)) = playing_video(false) else {
            return;
        };
        e.set_paused(true);
        e.seek(&show, instance, 5.0).unwrap();
        run_until(&mut e, &show, "the seek to land", |e| {
            e.active.iter().all(|a| !a.seeking())
        });
        let now = elapsed(&e, instance);
        assert!((5.0..5.5).contains(&now), "elapsed {now}");
        run_for(&mut e, &show, 0.3);
        assert_eq!(elapsed(&e, instance), now, "still paused");
    }

    #[test]
    fn a_loop_that_never_ends_shows_and_jumps_within_its_pass() {
        let Some((mut e, show, out, instance)) = playing_video(true) else {
            return;
        };
        let cue = show.cues[0].id;
        // The bar is one pass long (the whole 10 s clip) and moves as it plays.
        let a = &e.active[0];
        assert!(a.loops_forever());
        let length = a.scrub_length(&show).expect("a loop can be scrubbed");
        assert!((length - 10.0).abs() < 0.5, "{length}");
        let before = e.active[0].bar_position();
        run_for(&mut e, &show, 0.4);
        assert!(e.active[0].bar_position() > before, "the bar advances");
        assert_eq!(e.active[0].pass_number(), Some(1));
        // Jump to 8 s in: the bar is there, and the cue is still playing.
        e.seek(&show, instance, 8.0).unwrap();
        run_until(&mut e, &show, "the jump", |e| {
            e.active[0].bar_position() >= 8.0 && !e.active[0].seeking()
        });
        let at = e.active[0].bar_position();
        assert!((8.0..9.0).contains(&at), "{at}");
        // The pass runs out at the end of the clip and the loop begins again:
        // the bar wraps to the start instead of the cue ending.
        run_until(&mut e, &show, "the loop to come round", |e| {
            e.active
                .first()
                .is_some_and(|a| a.bar_position() < 3.0 && a.elapsed > 8.5)
        });
        assert_eq!(e.active.len(), 1, "it is still playing");
        assert!(e.active[0].started && e.active[0].loops_forever());
        let _ = (cue, out);
    }

    #[test]
    fn a_loop_with_a_set_number_of_plays_is_scrubbed_over_all_of_them() {
        let Some((mut e, mut show, _, _)) = playing_video(false) else {
            return;
        };
        e.stop_all(0.0);
        let id = show.cues[0].id;
        {
            let m = show.cue_mut(id).unwrap().kind.media_mut().unwrap();
            m.loop_media = true;
            m.loop_count = 3;
            m.end = Some(2.0);
        }
        e.playhead = Some(id);
        e.go(&show);
        run_until(&mut e, &show, "the clip to start", |e| {
            e.active.first().is_some_and(|a| a.started)
        });
        let a = &e.active[0];
        assert!(!a.loops_forever());
        let length = a.scrub_length(&show).unwrap();
        assert!((length - 6.0).abs() < 0.1, "three 2 s plays: {length}");
        // 5 s in is 1 s into the third play.
        let instance = a.instance;
        e.seek(&show, instance, 5.0).unwrap();
        run_until(&mut e, &show, "the jump", |e| !e.active[0].seeking());
        assert!((5.0..5.6).contains(&elapsed(&e, instance)));
        // Only a second of it is left to play.
        run_until(&mut e, &show, "the cue to end", |e| e.active.is_empty());
    }

    #[test]
    fn a_finished_cue_is_not_scrubbable() {
        let Some((mut e, show, _, instance)) = playing_video(false) else {
            return;
        };
        assert!(e.seek(&show, instance + 99, 2.0).is_err());
        e.stop_all(0.0);
        assert!(e.seek(&show, instance, 2.0).is_err());
    }

    #[test]
    fn a_timed_still_can_be_scrubbed() {
        let Some((mut e, img)) = engine_with_image() else {
            return;
        };
        let mut show = Show::default();
        let out = show.outputs[0].id;
        let id = image_cue(&mut show, &img, Transition::Cut, 0.0);
        show.cue_mut(id).unwrap().kind.media_mut().unwrap().end = Some(10.0);
        e.playhead = Some(id);
        e.go(&show);
        run_until(&mut e, &show, "the image", |e| stack(e, out).len() == 1);
        let instance = e.active[0].instance;
        e.seek(&show, instance, 4.0).unwrap();
        assert!((4.0..4.5).contains(&elapsed(&e, instance)));
        // A still with no time has nothing to scrub.
        let open = image_cue(&mut show, &img, Transition::Cut, 0.0);
        e.fire(&show, open);
        run_until(&mut e, &show, "the second image", |e| e.active.len() == 2);
        let second = e.active.iter().find(|a| a.cue_id == open).unwrap();
        assert!(second.scrub_length(&show).is_none());
    }

    // ----- Fixes from the review -------------------------------------------

    #[test]
    fn editing_levels_keeps_a_preload_but_editing_the_slice_does_not() {
        let mut show = Show::default();
        let c = show.make_cue(
            CueType::Video,
            Some(std::path::PathBuf::from("clip.mp4")),
            0,
        );
        let e = engine_without_media();
        let base = e.signature(&c).unwrap();
        let sig_after = |edit: &dyn Fn(&mut MediaCue)| {
            let mut c = c.clone();
            edit(c.kind.media_mut().unwrap());
            e.signature(&c).unwrap()
        };
        // None of these change what is decoded.
        assert!(base == sig_after(&|m| m.opacity = 0.3));
        assert!(base == sig_after(&|m| m.volume_db = -20.0));
        assert!(base == sig_after(&|m| m.fade_in = 2.0));
        assert!(base == sig_after(&|m| m.fade_out = 2.0));
        assert!(base == sig_after(&|m| m.hold_last_frame = true));
        // These do.
        assert!(base != sig_after(&|m| m.start = 1.0));
        assert!(base != sig_after(&|m| m.end = Some(4.0)));
        assert!(base != sig_after(&|m| m.loop_media = true));
        assert!(base != sig_after(&|m| m.file = std::path::PathBuf::from("other.mp4")));
        assert!(base != sig_after(&|m| m.audio_channel = 3));
    }

    #[test]
    fn scrubbing_back_out_of_the_automatic_fade_restores_the_levels() {
        let Some((mut e, img)) = engine_with_image() else {
            return;
        };
        let mut show = Show::default();
        let out = show.outputs[0].id;
        let id = image_cue(&mut show, &img, Transition::Cut, 0.0);
        show.cue_mut(id).unwrap().kind.media_mut().unwrap().end = Some(10.0);
        e.playhead = Some(id);
        e.go(&show);
        run_until(&mut e, &show, "the image", |e| stack(e, out).len() == 1);
        let instance = e.active[0].instance;
        let level = |e: &Engine| {
            e.active
                .iter()
                .find(|a| a.instance == instance)
                .unwrap()
                .opacity
                .value
        };

        // Into the last second (the cue fades out over it), and let it fade.
        e.seek(&show, instance, 9.4).unwrap();
        run_for(&mut e, &show, 0.3);
        assert!(level(&e) < 0.9, "fading out: {}", level(&e));

        // Back to the start: full level again, and it stays there.
        e.seek(&show, instance, 1.0).unwrap();
        run_for(&mut e, &show, 0.4);
        assert!(level(&e) > 0.99, "restored: {}", level(&e));
    }

    #[test]
    fn fading_out_a_group_cancels_children_still_waiting() {
        let mut show = Show::default();
        let (gid, kids) = group(&mut show, GroupMode::Timeline, &[5.0, 5.0]);
        show.cue_mut(kids[1]).unwrap().pre_wait = 10.0;
        let mut f = show.make_cue(CueType::Fade, None, show.cues.len());
        if let CueKind::Fade(c) = &mut f.kind {
            c.target = Some(gid);
            c.duration = 0.5;
            c.opacity = None;
            c.volume_db = None;
            c.stop_target = true;
        }
        let fade = f.id;
        show.cues.push(f);
        let mut e = engine_without_media();
        e.playhead = Some(gid);
        e.go(&show);
        assert!(
            e.pending.iter().any(|p| p.cue_id == kids[1]),
            "second child waits"
        );
        e.fire(&show, fade);
        e.tick(&show, 1.0);
        assert!(e.pending.is_empty(), "nothing left waiting to start");
        e.tick(&show, 11.0);
        assert!(
            matches!(e.status(kids[1]), CueStatus::Idle),
            "the second child never starts"
        );
    }

    #[test]
    fn a_video_whose_sound_cannot_start_says_so() {
        let Some((ff, clip)) = crate::media::test_media("smpte_8s.mp4") else {
            return;
        };
        assert!(ff.probe(&clip).unwrap().has_audio, "the clip has sound");
        let broken = AudioOutputId(7);
        let mut show = Show::default();
        let mut c = show.make_cue(CueType::Video, Some(clip), 0);
        c.kind.media_mut().unwrap().audio_output = Some(broken);
        let id = c.id;
        show.cues.push(c);
        let mut e = engine_without_media();
        e.ff = Some(ff);
        // Another device is open, so sound is expected; this cue's is not.
        e.audio = AudioOut::with_broken_output(broken);
        e.playhead = Some(id);
        e.go(&show);
        run_until(&mut e, &show, "the video", |e| {
            e.active.first().is_some_and(|a| a.started)
        });
        assert!(
            e.messages.iter().any(|(_, m)| m.contains("without sound")),
            "{:?}",
            e.messages
        );
    }

    #[test]
    fn an_audio_cue_whose_device_is_lost_ends_and_the_show_goes_on() {
        let Some((ff, wav)) = crate::media::test_media("tone_5s.wav") else {
            return;
        };
        let mut show = Show::default();
        let a = show.make_cue(CueType::Audio, Some(wav), 0);
        let a_id = a.id;
        show.cues.push(a);
        let (mut e, lost) = {
            let mut e = engine_without_media();
            let (audio, lost) = AudioOut::with_working_device();
            e.audio = audio;
            (e, lost)
        };
        e.ff = Some(ff);
        show.cues[0].continue_mode = ContinueMode::AutoFollow;
        let b_id = wait_cue(&mut show, 30.0, ContinueMode::DoNotContinue);
        e.playhead = Some(a_id);
        e.go(&show);
        run_until(&mut e, &show, "the sound to be ready", |e| {
            e.active.first().is_some_and(|a| a.started)
        });
        // The interface is unplugged mid-cue: nothing will play again, and the
        // cue must not hang the show waiting for it.
        lost.set("The audio device is not available".into());
        run_until(&mut e, &show, "the cue to end", |e| {
            matches!(e.status(a_id), CueStatus::Idle)
        });
        assert!(
            matches!(e.status(b_id), CueStatus::Playing { .. }),
            "the cue that follows it started"
        );
        assert!(
            e.messages
                .iter()
                .any(|(_, m)| m.contains("audio device was lost")),
            "{:?}",
            e.messages
        );
    }

    #[test]
    fn a_video_waits_for_its_sound_but_not_for_ever() {
        use VideoStart::*;
        let s = Duration::from_secs;
        // No picture yet: wait, whatever the sound is doing.
        assert_eq!(video_start(false, true, true, s(0)), Wait);
        assert_eq!(video_start(false, true, false, s(60)), Wait);
        // A picture and its sound: go. A picture with no sound at all: go.
        assert_eq!(video_start(true, true, true, s(0)), Go);
        assert_eq!(video_start(true, false, false, s(0)), Go);
        // A picture and a sound still coming: wait a while...
        assert_eq!(video_start(true, true, false, s(1)), Wait);
        // ...then go without it, rather than start it late and let it lag.
        assert_eq!(video_start(true, true, false, s(4)), GoSilent);
    }

    // ----- Starting a cue whose file isn't read yet ----------------------------

    /// An engine with a video cue that has not been read ahead, the probe held
    /// up so the cue is waiting on it. None to skip.
    fn waiting_on_the_probe() -> Option<(Engine, Show, CueId)> {
        let (ff, clip) = crate::media::test_media("smpte_8s.mp4")?;
        let mut e = engine_without_media();
        e.ff = Some(ff);
        let mut show = Show::default();
        let c = show.make_cue(CueType::Video, Some(clip), 0);
        let id = c.id;
        show.cues.push(c);
        e.probe.hold(true);
        e.playhead = Some(id);
        e.go(&show);
        Some((e, show, id))
    }

    #[test]
    fn going_to_a_cue_that_is_not_read_yet_does_not_wait_for_ffprobe() {
        let Some((mut e, show, id)) = waiting_on_the_probe() else {
            return;
        };
        // GO has returned, with the cue waiting for its file and nothing playing.
        assert!(e.active.is_empty());
        assert!(
            e.pending
                .iter()
                .any(|p| p.cue_id == id && p.reading.is_some())
        );
        assert!(e.is_active(), "the loop keeps running for it");
        assert!(matches!(e.status(id), CueStatus::PreWait { .. }));
        // Time passes meanwhile, and nothing else is held up.
        for _ in 0..5 {
            e.tick(&show, 0.1);
        }
        assert!(e.active.is_empty(), "still waiting for the file");
        // The answer comes in and the cue starts.
        e.probe.hold(false);
        run_until(&mut e, &show, "the cue to start", |e| {
            e.active.first().is_some_and(|a| a.started)
        });
        assert!(e.pending.is_empty(), "no longer waiting");
    }

    #[test]
    fn a_cue_waiting_for_its_file_can_be_stopped() {
        let Some((mut e, show, id)) = waiting_on_the_probe() else {
            return;
        };
        e.stop_cue(id, 0.0);
        assert!(e.pending.is_empty());
        e.probe.hold(false);
        run_for(&mut e, &show, 0.6);
        assert!(e.active.is_empty(), "it never starts once stopped");
        // Stop All does the same.
        let Some((mut e, show, _)) = waiting_on_the_probe() else {
            return;
        };
        e.stop_all(0.0);
        e.probe.hold(false);
        run_for(&mut e, &show, 0.6);
        assert!(e.active.is_empty() && !e.is_active());
    }

    #[test]
    fn a_file_that_cannot_be_read_says_so_instead_of_waiting_for_ever() {
        let mut e = engine_without_media();
        let Some((ff, _)) = crate::media::test_media("smpte_8s.mp4") else {
            return;
        };
        e.ff = Some(ff);
        let mut show = Show::default();
        let c = show.make_cue(CueType::Video, Some(PathBuf::from("/nowhere/gone.mp4")), 0);
        let id = c.id;
        show.cues.push(c);
        e.playhead = Some(id);
        e.go(&show);
        // A missing file is answered at once, so nothing waits.
        assert!(e.pending.is_empty() && e.active.is_empty());
        assert!(
            e.messages.iter().any(|(_, m)| m.contains("file not found")),
            "{:?}",
            e.messages
        );
    }

    #[test]
    fn a_slide_show_crossfades_through_its_slides_and_ends() {
        let Some((mut e, img)) = engine_with_image() else {
            return;
        };
        let mut show = Show::default();
        let out = show.outputs[0].id;
        let slot = show.insert_slot(None, true, 0);
        let mut g = show.make_cue(CueType::Group, None, slot.at);
        if let CueKind::Group(gc) = &mut g.kind {
            gc.slides = Some(SlideShow {
                fade: 0.2,
                duration: 0.3,
            });
        }
        let gid = g.id;
        show.insert_cue(slot, g);
        show.add_slides(gid, vec![img.clone(), img.clone(), img]);
        let slides: Vec<CueId> = show.children(gid).iter().map(|c| c.id).collect();
        e.playhead = Some(gid);
        e.go(&show);
        // The first is up, alone.
        run_until(&mut e, &show, "slide 1 on screen", |e| {
            stack(e, out).iter().any(|l| l.0 == Some(slides[0]))
        });
        // The second fades in over it, and then the first is gone.
        run_until(&mut e, &show, "slide 2 on screen", |e| {
            stack(e, out).iter().any(|l| l.0 == Some(slides[1]))
        });
        run_until(&mut e, &show, "slide 1 gone", |e| {
            stack(e, out).iter().all(|l| l.0 != Some(slides[0]))
        });
        run_until(&mut e, &show, "slide 3 on screen", |e| {
            stack(e, out).iter().any(|l| l.0 == Some(slides[2]))
        });
        // The last is shown, fades out, and the show ends with nothing on screen.
        run_until(&mut e, &show, "the slide show to end", |e| {
            stack(e, out).is_empty() && !e.is_active()
        });
    }

    #[test]
    fn a_live_audio_cue_with_no_input_says_so_and_ends() {
        let mut show = Show::default();
        let mut cue = show.make_cue(CueType::LiveAudio, None, 0);
        if let CueKind::LiveAudio(c) = &mut cue.kind {
            // An input that no computer has.
            c.device = Some("no such input".into());
        }
        let id = cue.id;
        show.cues.push(cue);
        let (audio, _lost) = AudioOut::with_working_device();
        let mut e = Engine::for_tests();
        e.audio = audio;
        e.playhead = Some(id);
        e.go(&show);
        e.tick(&show, 0.01);
        assert!(!e.is_active(), "it can't play, so it is not left running");
        assert!(
            e.messages.iter().any(|(_, l)| l.contains("no such input")),
            "{:?}",
            e.messages
        );
    }

    #[test]
    fn picture_effects_follow_their_keyframes_as_the_cue_plays() {
        use crate::show::{Effect, EffectKind};
        let Some((mut e, img)) = engine_with_image() else {
            return;
        };
        let mut show = Show::default();
        let out = show.outputs[0].id;
        let a = image_cue(&mut show, &img, Transition::Cut, 0.0);
        let i = show.cue_index(a).unwrap();
        let mut blur = Effect::new(EffectKind::Blur);
        blur.p[0].set_key(0.0, 0.0);
        blur.p[0].set_key(1.0, 40.0);
        show.cues[i]
            .kind
            .media_mut()
            .unwrap()
            .effects
            .push(blur.clone());
        e.playhead = Some(a);
        e.go(&show);
        run_until(&mut e, &show, "the picture on screen", |e| {
            !e.layers(out).is_empty()
        });
        let early = e.layers(out)[0].fx.blur;
        run_for(&mut e, &show, 0.5);
        let mid = e.layers(out)[0].fx.blur;
        run_for(&mut e, &show, 0.8);
        let late = e.layers(out)[0].fx.blur;
        assert!(
            early < 15.0 && mid > early && late >= mid,
            "{early} {mid} {late}"
        );
        assert!((late - 40.0).abs() < 0.5, "held at the last key: {late}");
        // Editing an effect while it plays reaches the picture at once.
        blur.p[0].keys.clear();
        blur.p[0].value = 3.0;
        show.cues[i].kind.media_mut().unwrap().effects = vec![blur];
        run_for(&mut e, &show, 0.1);
        assert_eq!(e.layers(out)[0].fx.blur, 3.0);
        show.cues[i].kind.media_mut().unwrap().effects.clear();
        run_for(&mut e, &show, 0.1);
        assert!(e.layers(out)[0].fx.is_none());
    }

    /// A control cue acting on `target`, appended to the list.
    fn control_cue(show: &mut Show, action: ControlAction, target: CueId) -> CueId {
        let mut c = show.make_cue(CueType::Control, None, show.cues.len());
        c.kind = CueKind::Control(ControlCue {
            action,
            target: Some(target),
        });
        let id = c.id;
        show.cues.push(c);
        id
    }

    #[test]
    fn a_start_cue_fires_its_target_and_a_memo_does_nothing() {
        let mut show = Show::default();
        let w = wait_cue(&mut show, 30.0, ContinueMode::DoNotContinue);
        let start = control_cue(&mut show, ControlAction::Start, w);
        let mut memo = show.make_cue(CueType::Control, None, show.cues.len());
        memo.kind = CueKind::Control(ControlCue {
            action: ControlAction::Memo,
            target: None,
        });
        let memo_id = memo.id;
        show.cues.push(memo);
        let mut e = engine_without_media();
        // The playhead is on the memo; the start cue is fired from elsewhere.
        e.playhead = Some(memo_id);
        e.go(&show);
        assert!(e.active.is_empty(), "a memo does nothing");
        e.fire(&show, start);
        assert!(playing(&e, w));
        // It took no time itself and left nothing running.
        assert!(!playing(&e, start));
    }

    #[test]
    fn pause_holds_one_cue_while_the_rest_go_on_and_resume_lets_it_go() {
        let mut show = Show::default();
        let a = wait_cue(&mut show, 30.0, ContinueMode::DoNotContinue);
        let b = wait_cue(&mut show, 30.0, ContinueMode::DoNotContinue);
        let pause = control_cue(&mut show, ControlAction::Pause, a);
        let resume = control_cue(&mut show, ControlAction::Resume, a);
        let mut e = engine_without_media();
        e.fire(&show, a);
        e.fire(&show, b);
        e.tick(&show, 1.0);
        e.fire(&show, pause);
        e.tick(&show, 1.0);
        e.tick(&show, 1.0);
        let at = |e: &Engine, id| e.active.iter().find(|x| x.cue_id == id).unwrap().elapsed;
        assert_eq!(at(&e, a), 1.0, "held");
        assert_eq!(at(&e, b), 3.0, "the other goes on");
        // A global pause and resume leaves the held one held.
        e.set_paused(true);
        e.set_paused(false);
        e.tick(&show, 1.0);
        assert_eq!(at(&e, a), 1.0);
        e.fire(&show, resume);
        e.tick(&show, 1.0);
        assert_eq!(at(&e, a), 2.0);
        // Pausing what isn't playing says so and nothing breaks.
        e.stop_all(0.0);
        e.fire(&show, pause);
        assert!(e.messages.iter().any(|(_, m)| m.contains("is not playing")));
    }

    #[test]
    fn goto_moves_the_playhead_and_arm_and_disarm_ask_the_show_for_the_change() {
        let mut show = Show::default();
        let a = wait_cue(&mut show, 1.0, ContinueMode::DoNotContinue);
        let b = wait_cue(&mut show, 1.0, ContinueMode::DoNotContinue);
        let goto = control_cue(&mut show, ControlAction::GoTo, b);
        let disarm = control_cue(&mut show, ControlAction::Disarm, a);
        let arm = control_cue(&mut show, ControlAction::Arm, a);
        let mut e = engine_without_media();
        e.playhead = Some(a);
        e.fire(&show, goto);
        assert_eq!(e.playhead, Some(b));
        assert!(!playing(&e, b), "go to does not fire it");
        e.fire(&show, disarm);
        e.fire(&show, arm);
        assert_eq!(
            e.take_show_edits(),
            vec![ShowEdit::Arm(a, false), ShowEdit::Arm(a, true)]
        );
        assert!(e.take_show_edits().is_empty());
        // A control cue with no target says so.
        let mut none = show.make_cue(CueType::Control, None, show.cues.len());
        none.kind = CueKind::Control(ControlCue::default());
        let none_id = none.id;
        show.cues.push(none);
        e.fire(&show, none_id);
        assert!(
            e.messages
                .iter()
                .any(|(_, m)| m.contains("no cue to act on"))
        );
    }

    #[test]
    fn load_gets_a_cue_ready_and_reset_forgets_it_and_stops_it() {
        let Some((mut e, img)) = engine_with_image() else {
            return;
        };
        let mut show = Show::default();
        let out = show.outputs[0].id;
        // Far down the list, so the playhead doesn't prepare it by itself.
        let first = wait_cue(&mut show, 1.0, ContinueMode::DoNotContinue);
        let target = image_cue(&mut show, &img, Transition::Cut, 0.0);
        let load = control_cue(&mut show, ControlAction::Load, target);
        let reset = control_cue(&mut show, ControlAction::Reset, target);
        e.playhead = Some(first);
        e.update_preload(&show);
        assert!(!e.is_prepared(target));
        e.fire(&show, load);
        run_until(&mut e, &show, "the load", |e| e.is_prepared(target));
        // Fired, it uses what was ready; it is on screen.
        e.fire(&show, target);
        run_until(&mut e, &show, "on screen", |e| stack(e, out).len() == 1);
        // Reset stops it and forgets any load.
        e.fire(&show, load);
        e.fire(&show, reset);
        run_for(&mut e, &show, 0.1);
        assert!(stack(&e, out).is_empty() && !e.is_prepared(target));
        e.update_preload(&show);
        assert!(!e.is_prepared(target), "the reset cancelled the load");
    }

    #[test]
    fn devamp_plays_a_looping_sound_on_past_its_out_point() {
        let Some((ff, wav)) = crate::media::test_media("tone_5s.wav") else {
            return;
        };
        let mut e = engine_without_media();
        e.audio = AudioOut::with_playing_device();
        e.ff = Some(ff);
        let mut show = Show::default();
        let mut c = show.make_cue(CueType::Audio, Some(wav), 0);
        let m = c.kind.media_mut().unwrap();
        m.loop_media = true;
        m.end = Some(1.0);
        m.play_on = true;
        let id = c.id;
        show.cues.push(c);
        let devamp = control_cue(&mut show, ControlAction::Devamp, id);
        e.playhead = Some(id);
        e.go(&show);
        run_until(&mut e, &show, "the loop to start", |e| {
            e.active.first().is_some_and(|a| a.started)
        });
        assert!(e.active[0].loops_forever());
        e.fire(&show, devamp);
        let a = &e.active[0];
        assert!(!a.loops_forever(), "it has an end now");
        // The end of this pass and then the 4 s that are left of the file.
        let end = a.duration.unwrap();
        assert!((5.0 - 1e-6..7.0).contains(&end), "{end}");
        run_until_within(&mut e, &show, "it to play to the end", 40, |e| {
            matches!(e.status(id), CueStatus::Idle)
        });
    }

    #[test]
    fn devamp_lets_a_loop_finish_its_pass_and_end() {
        let Some((mut e, mut show, _, _)) = playing_video(true) else {
            return;
        };
        let id = show.cues[0].id;
        let devamp = control_cue(&mut show, ControlAction::Devamp, id);
        run_for(&mut e, &show, 0.3);
        assert!(e.active[0].loops_forever());
        e.fire(&show, devamp);
        assert!(!e.active[0].loops_forever(), "it now has an end");
        let end = e.active[0].duration.unwrap();
        assert!((end - 10.0).abs() < 0.5, "the end of the pass: {end}");
        // ...and it does end there, rather than going round unseen.
        e.active[0].elapsed = end;
        run_for(&mut e, &show, 1.5);
        assert!(e.active.is_empty(), "the devamped loop ended");
        // Devamping what is not a loop says so.
        e.stop_all(0.0);
        e.fire(&show, devamp);
        assert!(e.messages.iter().any(|(_, m)| m.contains("not a loop")));
    }
}
