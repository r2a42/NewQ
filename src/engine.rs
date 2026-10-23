//! Playback engine: fires cues, runs pre-waits, continues, fades and stops,
//! and owns the decoders for everything that is playing.

use crate::media::audio::{AudioOut, Voice};
use crate::media::ffmpeg::{Ffmpeg, MediaInfo};
use crate::media::playback::Playback;
use crate::media::video::{self, RgbaImage, VideoPlayer};
use crate::media::{ProbeCache, ProbeState};
use crate::show::*;
use crossbeam_channel::Receiver;
use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::time::{Duration, Instant};

/// Cues from the playhead onward (along a continue chain) that get preloaded.
const MAX_PRELOAD: usize = 4;
/// Start a video even if its audio has not arrived after this long.
const AUDIO_START_TIMEOUT: Duration = Duration::from_secs(3);
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
        if duration <= 0.0 {
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

pub struct VideoState {
    pub player: VideoPlayer,
    pub voice: Option<Voice>,
    pub output: Option<OutputId>,
    pub layer: i32,
    pub fit: Fit,
    spawned_at: Instant,
}

pub struct ImageState {
    pub image: Option<RgbaImage>,
    loader: Option<Receiver<Result<RgbaImage, String>>>,
    pub output: Option<OutputId>,
    pub layer: i32,
    pub fit: Fit,
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
    pub opacity: Param,
    /// Volume in dB.
    pub volume: Param,
    /// Media is decoded and playing (pre-roll finished).
    pub started: bool,
    /// Fading out; removed once the fade completes.
    pub stopping: bool,
    /// Ended and is holding its last frame.
    pub held: bool,
    /// A fade cue has changed its levels, so live edits to the cue no longer apply.
    faded: bool,
    auto_faded: bool,
    followed: bool,
    panic: bool,
    entry: Option<EntryAction>,
}

impl ActiveCue {
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
        self.opacity.ramp_to(0.0, fade, curve);
        self.volume.ramp_to(MIN_DB, fade, curve);
        self.stopping = true;
    }
}

pub struct PendingCue {
    pub cue_id: CueId,
    pub remaining: f64,
    pub total: f64,
}

#[derive(Clone, PartialEq)]
struct PrepSig {
    media: MediaCue,
    size: (u32, u32),
}

enum PreparedKind {
    Video {
        player: VideoPlayer,
        voice: Option<Voice>,
    },
    Image(Receiver<Result<RgbaImage, String>>),
    Audio(Voice),
}

struct Prepared {
    cue_id: CueId,
    sig: PrepSig,
    kind: PreparedKind,
    duration: Option<f64>,
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
    pub source: LayerSource<'a>,
}

pub enum LayerSource<'a> {
    Video(&'a VideoPlayer),
    Image(&'a RgbaImage),
    Solid([u8; 4]),
}

pub struct Engine {
    pub ff: Option<Ffmpeg>,
    pub audio: AudioOut,
    pub active: Vec<ActiveCue>,
    pub pending: Vec<PendingCue>,
    pub playhead: Option<CueId>,
    pub paused: bool,
    pub messages: VecDeque<(Instant, String)>,
    /// Current size of each open output window, used to pick decode resolution.
    pub output_sizes: HashMap<OutputId, (u32, u32)>,
    pub probe: ProbeCache,
    prepared: Vec<Prepared>,
    next_instance: u64,
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
            paused: false,
            messages: VecDeque::new(),
            output_sizes: HashMap::new(),
            probe: ProbeCache::default(),
            prepared: Vec::new(),
            next_instance: 1,
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
            paused: false,
            messages: VecDeque::new(),
            output_sizes: HashMap::new(),
            probe: ProbeCache::default(),
            prepared: Vec::new(),
            next_instance: 1,
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

    /// Duration of a cue's action, if it can be known. `Some(None)` = infinite.
    pub fn cue_duration(&mut self, show: &Show, cue: &Cue) -> Option<Option<f64>> {
        match &cue.kind {
            CueKind::Video(m) | CueKind::Audio(m) => {
                let total = match self.probe_state(&m.file) {
                    ProbeState::Ready(info) => info.duration,
                    _ => None,
                };
                match m.playback(total) {
                    // Without an out point or a known length there's nothing to count.
                    Playback::Once { duration: None } => Some(Some((total? - m.start).max(0.0))),
                    pb => Some(pb.total(None)),
                }
            }
            CueKind::Image(m) => Some(m.end.filter(|d| *d > 0.0)),
            CueKind::Fade(f) => Some(Some(f.duration)),
            CueKind::Wait(w) => Some(Some(w.duration)),
            CueKind::Stop(_) => Some(Some(0.0)),
            CueKind::Group(g) => {
                // Timeline: until the last child ends. Playlist: all of them in turn.
                let mut total = 0.0_f64;
                for child in show.children(cue.id) {
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
        let Some(idx) = self
            .playhead
            .and_then(|id| show.cue_index(id))
            .map(|i| show.top_index(i))
        else {
            self.log("GO: no cue at the playhead");
            return;
        };
        // Children never take the playhead: it moves over whole groups.
        let mut end = idx;
        while show.cues[end].continue_mode != ContinueMode::DoNotContinue
            && let Some(next) = show.next_top(end)
        {
            end = next;
        }
        self.playhead = show.next_top(end).map(|i| show.cues[i].id);
        log::info!("GO {} ({})", show.cues[idx].number, show.cues[idx].name);
        self.fire(show, show.cues[idx].id);
    }

    /// Fire a cue (honouring its pre-wait) without touching the playhead.
    pub fn fire(&mut self, show: &Show, id: CueId) {
        let Some(cue) = show.cue(id) else { return };
        if self.paused {
            self.set_paused(false);
        }
        if cue.pre_wait > 0.0 {
            self.pending.push(PendingCue {
                cue_id: id,
                remaining: cue.pre_wait,
                total: cue.pre_wait,
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
        if let Some(next) = show.next_top(i) {
            self.fire(show, show.cues[next].id);
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        for a in &self.active {
            if a.started
                && let Some(v) = a.voice()
            {
                v.set_paused(paused);
            }
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
        let m = cue.kind.media()?;
        Some(PrepSig {
            media: m.clone(),
            size: self.output_size(m.output),
        })
    }

    /// Spawn the decoders for a media cue. They start paused/buffering.
    fn prepare(&self, cue: &Cue, info: &MediaInfo) -> Result<Prepared, String> {
        let ff = self.ff.as_ref().ok_or("ffmpeg not found")?;
        let sig = self.signature(cue).ok_or("not a media cue")?;
        let m = &sig.media;
        let playback = m.playback(info.duration);
        let duration = match playback {
            Playback::Once { duration } => {
                duration.or(info.duration.map(|d| (d - m.start).max(0.0)))
            }
            pb => pb.total(None),
        };
        let initial_gain = if m.fade_in > 0.0 {
            0.0
        } else {
            db_to_gain(m.volume_db)
        };
        let kind = match &cue.kind {
            CueKind::Video(_) => {
                let vi = info.video.as_ref().ok_or("file has no video stream")?;
                let size = video::decode_size((vi.width, vi.height), sig.size, m.fit == Fit::Cover);
                let player = VideoPlayer::spawn(ff, &m.file, vi, m.start, playback, size)
                    .map_err(|e| format!("{e:#}"))?;
                let voice = if info.has_audio && self.audio.available() {
                    self.audio
                        .spawn_voice(ff, &m.file, m.start, playback, initial_gain)
                        .ok()
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
                    .spawn_voice(ff, &m.file, m.start, playback, initial_gain)
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
            sig,
            kind,
            duration,
        })
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
            opacity,
            volume: Param::new(0.0),
            started: true,
            stopping: false,
            held: false,
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
        if reveal.is_none()
            && let CueKind::Video(m) | CueKind::Image(m) = &cue.kind
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
            opacity: Param::new(1.0),
            volume: Param::new(0.0),
            started: true,
            stopping: false,
            held: false,
            faded: false,
            auto_faded: false,
            followed: false,
            panic: false,
            entry: None,
        };
        let mut ok = true;
        match &cue.kind {
            CueKind::Video(m) | CueKind::Image(m) | CueKind::Audio(m) => {
                let prepared = match self.take_prepared(&cue) {
                    Some(p) => Ok(p),
                    None => match &self.ff {
                        None => Err("ffmpeg not found".to_string()),
                        Some(ff) => {
                            let ff = ff.clone();
                            self.probe
                                .get_blocking(&ff, &m.file)
                                .and_then(|info| self.prepare(&cue, &info))
                        }
                    },
                };
                match prepared {
                    Err(e) => {
                        self.log(format!("Cue {} ({}): {e}", cue.number, cue.name));
                        ok = false;
                    }
                    Ok(p) => {
                        active.duration = p.duration;
                        active.started = false;
                        active.kind = match p.kind {
                            PreparedKind::Video { player, voice } => {
                                ActiveKind::Video(Box::new(VideoState {
                                    player,
                                    voice,
                                    output: m.output,
                                    layer: m.layer,
                                    fit: m.fit,
                                    spawned_at: Instant::now(),
                                }))
                            }
                            PreparedKind::Image(loader) => ActiveKind::Image(ImageState {
                                image: None,
                                loader: Some(loader),
                                output: m.output,
                                layer: m.layer,
                                fit: m.fit,
                            }),
                            PreparedKind::Audio(voice) => ActiveKind::Audio(voice),
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
                    if f.stop_target {
                        // Ensure the stop happens after `duration` even if no level changes.
                        if f.opacity.is_none() {
                            t.opacity.ramp_to(t.opacity.value, f.duration, f.curve);
                        }
                        t.stopping = true;
                    }
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
            CueKind::Group(g) => {
                let children: Vec<CueId> = show.children(id).iter().map(|c| c.id).collect();
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

        if !self.paused {
            let mut due = Vec::new();
            self.pending.retain_mut(|p| {
                p.remaining -= dt;
                if p.remaining <= 0.0 {
                    due.push(p.cue_id);
                    false
                } else {
                    true
                }
            });
            for id in due {
                self.start(show, id);
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
            let cue = show.cue(a.cue_id);
            let media = cue.and_then(|c| c.kind.media()).filter(|_| !a.is_matte());
            let mut completed = false;
            let was_started = a.started;

            match &mut a.kind {
                ActiveKind::Video(v) => {
                    if !a.started {
                        let video_ready = v.player.ready();
                        let audio_ready = v.voice.as_ref().is_none_or(|vo| vo.ready());
                        if video_ready
                            && (audio_ready || v.spawned_at.elapsed() > AUDIO_START_TIMEOUT)
                        {
                            a.started = true;
                            if let Some(vo) = &v.voice {
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
                            Some(vo) if !vo.finished() => a.elapsed = a.elapsed.max(vo.clock()),
                            _ if !paused => a.elapsed += dt,
                            _ => {}
                        }
                        v.player.update(a.elapsed);
                        let audio_done = v.voice.as_ref().is_none_or(|vo| vo.finished());
                        if v.player.finished() && audio_done && !a.held {
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
                            Ok(i) => img.image = Some(i),
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
                        vo.set_paused(paused);
                    }
                    if a.started {
                        a.elapsed = vo.clock();
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
                ActiveKind::Fade | ActiveKind::Wait => {
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
            }
            if !was_started
                && a.started
                && let Some(e) = a.entry.take()
            {
                entries.push((a.instance, a.output(), e));
            }

            if let Some(m) = media {
                // Live level edits apply until a fade cue takes over.
                if !a.faded && !a.stopping && !a.auto_faded {
                    if !a.opacity.ramping() {
                        a.opacity.value = m.opacity;
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

            if let Some(vo) = a.voice() {
                vo.set_gain(db_to_gain(a.volume.value));
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
                && let Some(next) = show.next_top(i)
                && show.cues[next].kind.media().is_some()
            {
                wanted.push(show.cues[next].id);
            }
            // The child a playlist will play next.
            if let ActiveKind::Group(g) = &a.kind
                && g.mode == GroupMode::Playlist
                && let Some(&next) = g.children.get(g.next)
                && show.cue(next).is_some_and(|c| c.kind.media().is_some())
            {
                wanted.push(next);
            }
        }
        if let Some(mut idx) = self
            .playhead
            .and_then(|p| show.cue_index(p))
            .map(|i| show.top_index(i))
        {
            loop {
                let cue = &show.cues[idx];
                if cue.kind.media().is_some() {
                    wanted.push(cue.id);
                }
                // A group at the playhead: the children that start with it.
                if let CueKind::Group(g) = &cue.kind {
                    let mode = g.mode;
                    for (n, child) in show.children(cue.id).iter().enumerate() {
                        if mode == GroupMode::Playlist && n > 0 {
                            break;
                        }
                        if child.kind.media().is_some() && wanted.len() < MAX_PRELOAD {
                            wanted.push(child.id);
                        }
                    }
                }
                let next = show.next_top(idx);
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
        for id in wanted {
            if self.is_prepared(id) {
                continue;
            }
            let Some(cue) = show.cue(id) else { continue };
            let Some(m) = cue.kind.media() else { continue };
            let info = match self.probe.get(self.ff.as_ref(), &m.file) {
                ProbeState::Ready(info) => info.clone(),
                _ => continue,
            };
            match self.prepare(cue, &info) {
                Ok(p) => self.prepared.push(p),
                Err(e) => log::debug!("preload of cue {} failed: {e}", cue.number),
            }
        }
    }

    /// Visual layers on an output, bottom to top.
    pub fn layers(&self, output: OutputId) -> Vec<LayerRef<'_>> {
        let mut layers: Vec<(i32, LayerRef)> = Vec::new();
        for a in &self.active {
            if !a.started {
                continue;
            }
            let (layer, fit, source) = match &a.kind {
                ActiveKind::Video(v) if v.output == Some(output) => {
                    (v.layer, v.fit, LayerSource::Video(&v.player))
                }
                ActiveKind::Image(i) if i.output == Some(output) => match &i.image {
                    Some(img) => (i.layer, i.fit, LayerSource::Image(img)),
                    None => continue,
                },
                // Mattes cover everything on the output.
                ActiveKind::Matte(m) if m.output == Some(output) => {
                    (i32::MAX, Fit::Stretch, LayerSource::Solid(m.color))
                }
                _ => continue,
            };
            layers.push((
                layer,
                LayerRef {
                    instance: a.instance,
                    opacity: a.opacity.value.clamp(0.0, 1.0),
                    fit,
                    source,
                },
            ));
        }
        layers.sort_by_key(|(layer, l)| (*layer, l.instance));
        layers.into_iter().map(|(_, l)| l).collect()
    }
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
    fn run_until(e: &mut Engine, show: &Show, what: &str, mut cond: impl FnMut(&Engine) -> bool) {
        let t0 = Instant::now();
        let mut last = Instant::now();
        while !cond(e) {
            assert!(
                t0.elapsed() < Duration::from_secs(10),
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
}
