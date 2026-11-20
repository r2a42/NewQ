//! The 3D stage as the app keeps it from pass to pass (`App::stage`): the
//! venue model loaded on a thread, the scene the views draw (made again only
//! when the stage changes), the views' targets, and the jobs handed to the
//! drawing thread (`stage_render`).
//!
//! Each pass `App::update_outputs` calls `begin` (venue, scene), takes a
//! display's stage picture with `output_layer` for each display that shows
//! the stage, and ends with `send`: the views asked for (the previews that
//! the governor let draw, `StageShared::requests`, and every display showing
//! the stage), with the pictures of the displays their scene shows.

use crate::gpu::{Geometry, Gpu, LayerTexture};
use crate::preview_pace::PreviewId;
use crate::render::{OutputGroup, OutputLayer};
use crate::render3d::{self, Camera, Options, Scene};
use crate::show::{OutputId, Show};
use crate::stage_render::{FeedJob, StageJob, StageThread, ViewJob, feed_size};
use crate::stage3d::Venue;
use crate::stage3d::load::LoadedVenue;
use crate::stage3d::samples::SampleVenue;
use crate::ui::stage_view::{StageShared, VenueStatus};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// A venue's settings that need it read again are left this long without a
/// change before it is (a brightness slider being dragged).
pub const RELOAD_SETTLE: Duration = Duration::from_millis(400);
/// The drawing thread, if it stops, is started again at most this often.
const RESTART_EVERY: Duration = Duration::from_secs(2);
/// With no 3D view and no display showing the stage for this long, the
/// drawing thread goes, and with it everything it kept on the graphics
/// card (the venue's meshes and pictures, the displays' pictures, the
/// render targets); it starts again when a view is next asked for.
pub const IDLE_RELEASE: Duration = Duration::from_secs(30);
/// The largest side of a display's stage picture.
const MOST_SIDE: u32 = 4096;

/// Whose picture a target is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ViewKey {
    Preview(PreviewId),
    Output(OutputId),
}

impl ViewKey {
    /// For the drawing thread's stats.
    fn number(self) -> u64 {
        let mut h = crate::stage3d::scene::Fnv::new();
        h.bytes(format!("{self:?}").as_bytes());
        h.0
    }
}

/// A display that shows the stage, as `send` is told of it.
#[derive(Clone, Copy, Debug)]
pub struct StageOutput {
    pub id: OutputId,
    /// What it sees the stage with.
    pub camera: Camera,
    /// A window or a stream shows it: a display's picture, drawn before the
    /// previews. One only the previews show is preview work (governed).
    pub seen: bool,
    /// For one not `seen`: whether the previews' pace lets it be drawn now.
    pub due: bool,
}

/// What a display's picture is made of, for drawing it as the stage shows it.
#[derive(Clone)]
pub struct PictureLayers {
    pub layers: Vec<OutputLayer>,
    /// Its own canvas (pixels).
    pub canvas: (u32, u32),
}

#[derive(Default)]
struct VenueState {
    loaded: Option<Arc<LoadedVenue>>,
    loading: Option<(Venue, Receiver<Result<LoadedVenue, String>>)>,
    /// What last failed to load, so it isn't tried every pass.
    failed: Option<Venue>,
    /// A wanted venue, and when it may be read (once its settings have rested).
    settling: Option<(Venue, Instant)>,
}

#[derive(Default)]
pub struct StageLive {
    thread: Option<StageThread>,
    started: Option<Instant>,
    /// Views were due but there was no thread (it stopped, and may start
    /// again only `RESTART_EVERY` after the last start).
    restart_wanted: bool,
    /// When a view (a preview, a display showing the stage) last wanted the
    /// thread (`IDLE_RELEASE`).
    wanted_at: Option<Instant>,
    venue: VenueState,
    /// The stage as last seen, and its key: the key is worked out again only
    /// when the stage is not the same (a cheap comparison each pass).
    stage_seen: Option<crate::stage3d::Stage3d>,
    stage_key: u64,
    scene_key: u64,
    scene: Arc<Scene>,
    bounds: Option<render3d::Bounds>,
    targets: HashMap<ViewKey, Arc<LayerTexture>>,
    /// What each view was last drawn with (not drawn again while still and
    /// nothing plays).
    drawn_with: HashMap<ViewKey, u64>,
    serial: u64,
    seen_done: u64,
    seen_displays: u64,
    /// Every display showing the stage was one only the previews show, last
    /// pass: their pictures are preview work (`feeds_a_display`).
    governed_only: bool,
    /// The displays whose pictures a display showing the stage needs.
    needed_by_outputs: HashSet<OutputId>,
    /// When each preview last asked for a picture (a pane closed lets its
    /// target go after `PREVIEW_KEPT`).
    asked_at: HashMap<PreviewId, Instant>,
}

/// A preview's target is kept this long after it last asked for a picture.
const PREVIEW_KEPT: Duration = Duration::from_secs(30);

/// The settings of a venue a load depends on.
fn reload_key(v: &Venue) -> (std::path::PathBuf, Option<SampleVenue>, u32) {
    (v.path.clone(), v.sample, v.brightness().to_bits())
}

/// What the user calls a venue: its file's name, or the sample's.
fn venue_name(v: &Venue) -> String {
    match v.sample {
        Some(k) => k.label().to_string(),
        None => file_name(&v.path),
    }
}

impl StageLive {
    /// Start of a pass: look after the venue's loading and make the scene
    /// again if the stage has changed. Says what happened in `say`.
    pub fn begin(&mut self, show: &Show, shared: &mut StageShared, say: &mut Vec<Said>) {
        // Sample venues written as .glb.
        shared.exports.retain(|rx| match rx.try_recv() {
            Ok(Ok(path)) => {
                say.push(Said::Note(format!(
                    "Saved the sample venue as {}",
                    path.display()
                )));
                false
            }
            Ok(Err(e)) => {
                say.push(Said::Problem(format!("The sample venue wasn't saved: {e}")));
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => false,
        });
        self.look_after_venue(show, shared, say);
        let stage = &show.stage3d;
        // (Comparing is cheap; a key is worked out only on a change, and the
        // copy kept only when the key moved, so a stage with a NaN in it,
        // never equal to itself, isn't copied every pass.)
        if self.stage_seen.as_ref() != Some(stage) {
            let key = if stage.is_empty() {
                0
            } else {
                stage.shape_key()
            };
            if key != self.stage_key || self.stage_seen.is_none() {
                self.stage_seen = Some(stage.clone());
            }
            self.stage_key = key;
        }
        let mut h = crate::stage3d::scene::Fnv::new();
        h.bytes(&self.stage_key.to_le_bytes());
        let loaded = self
            .venue
            .loaded
            .as_ref()
            .map_or(0, |l| Arc::as_ptr(l) as usize);
        h.bytes(&loaded.to_le_bytes());
        // Which displays exist and which show the stage changes what is fed.
        for o in &show.outputs {
            h.bytes(&o.id.0.to_le_bytes());
            h.bytes(&[u8::from(o.source.is_some()), u8::from(o.part_of.is_some())]);
            if let Some(p) = o.part_of {
                h.bytes(&p.display.0.to_le_bytes());
            }
        }
        if h.0 != self.scene_key || shared.scene_key != h.0 {
            self.scene_key = h.0;
            let scene = stage.scene(show, self.venue.loaded.as_deref());
            self.bounds = stage_bounds(stage, &scene, self.venue.loaded.as_deref());
            self.scene = Arc::new(scene);
            shared.scene = self.scene.clone();
            shared.scene_key = h.0;
            shared.bounds = self.bounds;
        }
        let shown = show
            .outputs
            .iter()
            .any(|o| show.stage_source(o.id).is_some());
        self.needed_by_outputs = if shown && !self.governed_only {
            feeds_wanted(show, &self.scene)
        } else {
            HashSet::new()
        };
    }

    fn look_after_venue(&mut self, show: &Show, shared: &mut StageShared, say: &mut Vec<Said>) {
        let v = &mut self.venue;
        // A load that has finished.
        if let Some((venue, rx)) = &v.loading {
            match rx.try_recv() {
                Ok(Ok(loaded)) => {
                    say.push(Said::Note(format!(
                        "Venue loaded: {} ({} triangles)",
                        venue_name(venue),
                        loaded.triangles
                    )));
                    for w in &loaded.warnings {
                        say.push(Said::Problem(format!("Venue: {w}")));
                    }
                    v.loaded = Some(Arc::new(loaded));
                    v.failed = None;
                    v.loading = None;
                }
                Ok(Err(why)) => {
                    say.push(Said::Problem(format!("The venue didn't load: {why}")));
                    v.failed = Some(venue.clone());
                    v.loading = None;
                    shared.venue = VenueStatus::Failed(why);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => v.loading = None,
            }
        }
        let wanted = show.stage3d.venue.as_ref();
        match wanted {
            None => {
                v.loaded = None;
                v.settling = None;
                v.failed = None;
                shared.venue = VenueStatus::None;
            }
            Some(want) => {
                let have = v.loaded.as_ref().map(|l| &l.loaded_for);
                let current = have.is_some_and(|h| !want.needs_reload(h));
                let busy = v
                    .loading
                    .as_ref()
                    .is_some_and(|(l, _)| reload_key(l) == reload_key(want));
                let failed = v
                    .failed
                    .as_ref()
                    .is_some_and(|f| reload_key(f) == reload_key(want));
                if current {
                    v.settling = None;
                } else if !busy && !failed {
                    // Wait for the settings to rest, then read it on a thread.
                    let ready_at = match &v.settling {
                        Some((s, at)) if reload_key(s) == reload_key(want) => *at,
                        _ => {
                            let now = Instant::now();
                            // A new file is read at once; a change of brightness waits.
                            let at = if have
                                .is_some_and(|h| h.path == want.path && h.sample == want.sample)
                            {
                                now + RELOAD_SETTLE
                            } else {
                                now
                            };
                            v.settling = Some((want.clone(), at));
                            at
                        }
                    };
                    if Instant::now() >= ready_at {
                        v.settling = None;
                        let (tx, rx) = std::sync::mpsc::channel();
                        let venue = want.clone();
                        let path = want.path.clone();
                        let spawned = std::thread::Builder::new().name("venue-load".into()).spawn(
                            move || {
                                let _ = tx.send(crate::stage3d::load::load_venue(&path, &venue));
                            },
                        );
                        match spawned {
                            Ok(_) => v.loading = Some((want.clone(), rx)),
                            Err(e) => {
                                say.push(Said::Problem(format!("The venue didn't load: {e}")));
                                v.failed = Some(want.clone());
                            }
                        }
                    }
                }
                shared.venue = if v.loading.is_some() || v.settling.is_some() {
                    VenueStatus::Loading(venue_name(want))
                } else if let Some(l) = v.loaded.as_ref().filter(|_| current) {
                    VenueStatus::Loaded {
                        triangles: l.triangles,
                        warnings: l.warnings.clone(),
                    }
                } else if failed {
                    match &shared.venue {
                        VenueStatus::Failed(w) => VenueStatus::Failed(w.clone()),
                        _ => VenueStatus::Failed("it couldn't be read".into()),
                    }
                } else {
                    VenueStatus::None
                };
            }
        }
    }

    /// Whether a load is under way or waiting for its settings to rest (the
    /// main loop keeps coming round meanwhile).
    pub fn venue_busy(&self) -> bool {
        self.venue.loading.is_some() || self.venue.settling.is_some()
    }

    /// Whether display `id`'s pictures are wanted by a display showing the
    /// stage (so they are uploaded as a display's, not paced as a preview's).
    pub fn feeds_a_display(&self, id: OutputId) -> bool {
        self.needed_by_outputs.contains(&id)
    }

    /// The picture of display `id`, which shows the stage, `canvas` big: its
    /// target, made (or made again at a new size) here.
    pub fn output_layer(&mut self, gpu: &Gpu, id: OutputId, canvas: (u32, u32)) -> OutputLayer {
        let size = capped(canvas);
        let tex = self.target(gpu, ViewKey::Output(id), size);
        OutputLayer {
            texture: tex,
            opacity: 1.0,
            fit: crate::show::Fit::Stretch,
            transform: Default::default(),
            color: Default::default(),
            fx: Default::default(),
        }
    }

    fn target(&mut self, gpu: &Gpu, key: ViewKey, size: (u32, u32)) -> Arc<LayerTexture> {
        match self.targets.get(&key) {
            Some(t) if (t.width, t.height) == size => t.clone(),
            _ => {
                let t = Arc::new(LayerTexture::new_target(gpu, size.0, size.1));
                self.targets.insert(key, t.clone());
                self.drawn_with.remove(&key);
                t
            }
        }
    }

    /// End of a pass: hand the drawing thread the views due. `outputs` are
    /// the displays showing the stage, `pictures` every display's layers,
    /// `fresh` whether anything moved since the last pass (something plays,
    /// a picture came in). A display's view is drawn again when what it
    /// sees changed, the pictures of the displays on the stage included (a
    /// test pattern, a still). The displays a window or stream shows come
    /// first: while the thread is behind with them, the previews (and the
    /// displays only the previews show) wait. Returns whether a job is
    /// still being drawn.
    pub fn send(
        &mut self,
        gpu: &Gpu,
        show: &Show,
        shared: &mut StageShared,
        outputs: &[StageOutput],
        pictures: &HashMap<OutputId, PictureLayers>,
        fresh: bool,
    ) -> bool {
        let now = Instant::now();
        // A thread that stopped (a panic) draws nothing: what it was given is
        // to be drawn again by the next one.
        if self.thread.as_ref().is_some_and(StageThread::is_finished) {
            log::warn!("the 3D stage's drawing thread stopped; starting another");
            self.thread = None;
            self.drawn_with.clear();
        }
        let behind = self.thread.as_ref().is_some_and(|t| t.done() < self.serial);
        let displays_behind = behind && outputs.iter().any(|o| o.seen);
        self.governed_only = !outputs.is_empty() && outputs.iter().all(|o| !o.seen);
        let asked: Vec<_> = std::mem::take(&mut shared.requests);
        let pictures_key = if outputs.is_empty() && asked.is_empty() {
            0
        } else {
            feeds_key(show, &self.scene, pictures)
        };
        let mut views = Vec::new();
        // What each view is drawn with, once it is handed over.
        let mut marks = Vec::new();
        let mut scale = 0.0f32;
        for o in outputs {
            let key = ViewKey::Output(o.id);
            let Some(target) = self.targets.get(&key).cloned() else {
                continue;
            };
            if !o.seen && (!o.due || displays_behind) {
                continue;
            }
            let options = Options::default();
            let sig = signature(self.scene_key, &o.camera, &options, &target, pictures_key);
            if fresh || self.drawn_with.get(&key) != Some(&sig) {
                marks.push((key, sig));
                scale = 1.0;
                views.push(ViewJob {
                    key: key.number(),
                    display: o.seen,
                    target,
                    scene: self.scene.clone(),
                    camera: o.camera,
                    options,
                });
            }
        }
        let mut shown: HashSet<PreviewId> = HashSet::new();
        for r in asked {
            let key = ViewKey::Preview(r.id);
            shown.insert(r.id);
            self.asked_at.insert(r.id, now);
            let target = self.target(gpu, key, capped(r.size));
            shared.targets.insert(r.id, target.clone());
            // Giving way to the displays: asked for again next frame.
            if displays_behind {
                continue;
            }
            let sig = signature(self.scene_key, &r.camera, &r.options, &target, pictures_key);
            if fresh || self.drawn_with.get(&key) != Some(&sig) {
                marks.push((key, sig));
                scale = scale.max(r.options.quality.scale.clamp(0.1, 1.0));
                views.push(ViewJob {
                    key: key.number(),
                    display: false,
                    target,
                    scene: self.scene.clone(),
                    camera: r.camera,
                    options: r.options,
                });
            }
        }
        // Stats for the views on show.
        if let Some(t) = &self.thread {
            for id in &shown {
                if let Some(s) = t.stats(ViewKey::Preview(*id).number()) {
                    shared.stats.insert(*id, s);
                }
            }
        }
        // A display that shows the stage and has gone lets its target go.
        let live: HashSet<OutputId> = outputs.iter().map(|o| o.id).collect();
        let asked_at = &mut self.asked_at;
        asked_at.retain(|_, at| now.saturating_duration_since(*at) < PREVIEW_KEPT);
        self.targets.retain(|k, _| match k {
            ViewKey::Output(o) => live.contains(o),
            ViewKey::Preview(p) => asked_at.contains_key(p),
        });
        shared.targets.retain(|p, _| asked_at.contains_key(p));
        if !outputs.is_empty() || !shown.is_empty() {
            self.wanted_at = Some(now);
        }
        if !views.is_empty() {
            self.keep_thread(gpu);
            match &self.thread {
                Some(t) => {
                    let feeds = feed_jobs(show, &self.scene, pictures, scale);
                    self.serial += 1;
                    // Displays first (`StageJob::merged` keeps them so).
                    let job = StageJob {
                        feeds,
                        views,
                        serial: self.serial,
                    }
                    .merged(StageJob::default());
                    t.submit(job);
                    self.drawn_with.extend(marks);
                    self.restart_wanted = false;
                }
                // Drawn when there is a thread again (`wake_at`).
                None => self.restart_wanted = true,
            }
        }
        // Nothing has wanted a picture for a while: everything the thread
        // keeps on the graphics card goes with it.
        if self.thread.is_some()
            && self
                .wanted_at
                .is_none_or(|t| now.saturating_duration_since(t) >= IDLE_RELEASE)
        {
            log::info!("3D stage: nothing to draw for a while; its pictures are let go");
            if let Some(t) = self.thread.take() {
                drop(t.quit());
            }
            self.drawn_with.clear();
            self.started = None;
            self.restart_wanted = false;
        }
        self.busy()
    }

    /// Whether the thread is drawing a job handed over (a thread that has
    /// stopped never is).
    pub fn busy(&self) -> bool {
        self.thread
            .as_ref()
            .is_some_and(|t| !t.is_finished() && t.done() < self.serial)
    }

    /// When the main loop should come round for the stage though nothing
    /// else happens: to start the drawing thread again once it may (a still
    /// stage is drawn then), or to let it go when idle.
    pub fn wake_at(&self) -> Option<Instant> {
        let mut at: Option<Instant> = None;
        let mut take = |t: Instant| at = Some(at.map_or(t, |a| a.min(t)));
        if self.restart_wanted && self.thread.is_none() {
            take(
                self.started
                    .map_or_else(Instant::now, |s| s + RESTART_EVERY),
            );
        }
        if self.thread.is_some()
            && let Some(w) = self.wanted_at
        {
            take(w + IDLE_RELEASE);
        }
        at
    }

    /// Whether a picture has been drawn since this was last asked: the
    /// windows showing one are to be drawn again. A job's displays land
    /// before its previews are drawn.
    pub fn landed(&mut self) -> bool {
        let (done, displays) = self
            .thread
            .as_ref()
            .map_or((0, 0), |t| (t.done(), t.displays_done()));
        let new = done != self.seen_done || displays != self.seen_displays;
        self.seen_done = done;
        self.seen_displays = displays;
        new
    }

    /// Start the drawing thread if there is none (at most every
    /// `RESTART_EVERY` after one stopped).
    fn keep_thread(&mut self, gpu: &Gpu) {
        if self.thread.is_none() && self.started.is_none_or(|t| t.elapsed() >= RESTART_EVERY) {
            self.started = Some(Instant::now());
            match StageThread::spawn(gpu.clone()) {
                Ok(t) => {
                    self.seen_done = 0;
                    self.seen_displays = 0;
                    self.serial = 0;
                    self.thread = Some(t);
                }
                Err(e) => log::warn!("could not start drawing the 3D stage: {e}"),
            }
        }
    }

    /// Whether the drawing thread is running (for a test).
    #[cfg(test)]
    pub fn has_thread(&self) -> bool {
        self.thread.is_some()
    }

    /// Stop the drawing thread as a panic in it would, and wait until it
    /// has (for a test).
    #[cfg(test)]
    pub fn panic_thread(&self) {
        if let Some(t) = &self.thread {
            t.panic();
            let end = Instant::now() + Duration::from_secs(10);
            while !t.is_finished() && Instant::now() < end {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    /// As if `IDLE_RELEASE` had passed since a view last asked (for a test).
    #[cfg(test)]
    pub fn as_if_idle(&mut self) {
        self.wanted_at = Instant::now().checked_sub(IDLE_RELEASE);
    }

    /// The device is gone: let go of everything made on it. The thread's
    /// handle, to wait for before a new device is made.
    pub fn forget_device(
        &mut self,
        shared: &mut StageShared,
    ) -> Option<std::thread::JoinHandle<()>> {
        self.targets.clear();
        self.drawn_with.clear();
        shared.targets.clear();
        self.started = None;
        self.thread.take().map(StageThread::quit)
    }

    /// Every target (for a test).
    #[cfg(test)]
    pub fn target_of(&self, key: ViewKey) -> Option<Arc<LayerTexture>> {
        self.targets.get(&key).cloned()
    }
}

/// What the stage has to say to the user (the status bar).
pub enum Said {
    Note(String),
    Problem(String),
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// A target no bigger than `MOST_SIDE`, in its shape.
fn capped(size: (u32, u32)) -> (u32, u32) {
    let (w, h) = (size.0.max(1), size.1.max(1));
    let most = w.max(h);
    if most <= MOST_SIDE {
        return (w, h);
    }
    let f = |v: u32| ((u64::from(v) * u64::from(MOST_SIDE) / u64::from(most)) as u32).max(1);
    (f(w), f(h))
}

fn signature(
    scene: u64,
    camera: &Camera,
    options: &Options,
    target: &Arc<LayerTexture>,
    pictures: u64,
) -> u64 {
    let mut h = crate::stage3d::scene::Fnv::new();
    h.bytes(&scene.to_le_bytes());
    h.bytes(&pictures.to_le_bytes());
    h.bytes(format!("{camera:?}{options:?}").as_bytes());
    h.bytes(&(Arc::as_ptr(target) as usize).to_le_bytes());
    h.0
}

/// A key for what the pictures of the displays `scene` shows are made of
/// (each layer's texture, whether it holds a picture yet, and how it is
/// placed): a display showing the stage is drawn again when it changes,
/// though nothing plays (a test pattern turned on, a still, a cue gone).
pub fn feeds_key(show: &Show, scene: &Scene, pictures: &HashMap<OutputId, PictureLayers>) -> u64 {
    let mut h = crate::stage3d::scene::Fnv::new();
    let mut ids: Vec<OutputId> = feeds_wanted(show, scene).into_iter().collect();
    ids.sort_by_key(|o| o.0);
    for id in ids {
        h.bytes(&id.0.to_le_bytes());
        let Some(p) = pictures.get(&id) else {
            continue;
        };
        h.bytes(&p.canvas.0.to_le_bytes());
        h.bytes(&p.canvas.1.to_le_bytes());
        for l in &p.layers {
            h.bytes(&(Arc::as_ptr(&l.texture) as usize).to_le_bytes());
            h.bytes(&[u8::from(l.texture.is_filled())]);
            h.bytes(&l.opacity.to_bits().to_le_bytes());
            h.bytes(format!("{:?}{:?}{:?}{:?}", l.fit, l.transform, l.color, l.fx).as_bytes());
        }
    }
    h.0
}

/// Around what `scene` shows: its screens' corners, and the venue's box
/// (`Stage3d::venue_bounds`, worked out when it was read) if it is in the
/// scene. Never every corner of a big venue on the main thread.
pub fn stage_bounds(
    stage: &crate::stage3d::Stage3d,
    scene: &Scene,
    loaded: Option<&LoadedVenue>,
) -> Option<render3d::Bounds> {
    let screens = Scene {
        parts: scene.parts.iter().filter(|p| !p.lit).cloned().collect(),
    };
    let screens = render3d::scene_bounds(&screens);
    let venue = loaded
        .filter(|_| scene.parts.iter().any(|p| p.lit))
        .and_then(|l| stage.venue_bounds(l));
    render3d::Bounds::around(
        screens
            .into_iter()
            .flat_map(|b| [b.min, b.max])
            .chain(venue.into_iter().flat_map(|b| [b.min, b.max])),
    )
}

/// The displays whose pictures `scene` shows, and the displays those are
/// drawn from (the whole picture a projector shows part of, and the sub
/// displays within it): what must be up to date for it.
pub fn feeds_wanted(show: &Show, scene: &Scene) -> HashSet<OutputId> {
    let mut out = HashSet::new();
    for o in crate::stage3d::outputs_in(scene) {
        let whole = show.whole_picture(o);
        out.insert(o);
        out.insert(whole);
        for s in show.outputs.iter().filter(|s| s.within == Some(whole)) {
            out.insert(s.id);
        }
    }
    out
}

/// The layers of display `output`'s picture as the stage shows it: the
/// whole picture it is part of (if it is), on that picture's canvas with no
/// keystone, mesh or blend, with its sub displays in their areas. None if
/// there is nothing to draw it from (a display that isn't on).
pub fn feed_groups(
    show: &Show,
    output: OutputId,
    pictures: &HashMap<OutputId, PictureLayers>,
) -> Option<(Vec<OutputGroup>, (u32, u32))> {
    let whole = show.whole_picture(output);
    let main = pictures.get(&whole)?;
    let mut groups = vec![OutputGroup {
        layers: main.layers.clone(),
        geometry: plain(main.canvas, crate::gpu::IDENTITY),
    }];
    for s in show.outputs.iter().filter(|s| s.within == Some(whole)) {
        if let Some(p) = pictures.get(&s.id) {
            let pin = crate::gpu::corner_pin(&s.area.0);
            groups.push(OutputGroup {
                layers: p.layers.clone(),
                geometry: plain(p.canvas, pin),
            });
        }
    }
    Some((groups, main.canvas))
}

fn plain(canvas: (u32, u32), warp: crate::gpu::Homography) -> Geometry {
    Geometry {
        canvas,
        warp,
        pin: warp,
        mesh: None,
        slice: Geometry::WHOLE,
    }
}

/// The pictures `scene` shows, drawn for views at `scale`.
pub fn feed_jobs(
    show: &Show,
    scene: &Scene,
    pictures: &HashMap<OutputId, PictureLayers>,
    scale: f32,
) -> Vec<FeedJob> {
    crate::stage3d::outputs_in(scene)
        .into_iter()
        .filter_map(|o| {
            let (groups, canvas) = feed_groups(show, o, pictures)?;
            Some(FeedJob {
                output: o,
                groups,
                size: feed_size(canvas, scale),
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "stage_live_tests.rs"]
mod tests;
