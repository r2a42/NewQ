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
    venue: VenueState,
    scene_key: u64,
    scene: Arc<Scene>,
    bounds: Option<render3d::Bounds>,
    targets: HashMap<ViewKey, Arc<LayerTexture>>,
    /// What each view was last drawn with (not drawn again while still and
    /// nothing plays).
    drawn_with: HashMap<ViewKey, u64>,
    serial: u64,
    seen_done: u64,
    /// The displays whose pictures a display showing the stage needs.
    needed_by_outputs: HashSet<OutputId>,
}

/// The settings of a venue a load depends on.
fn reload_key(v: &Venue) -> (std::path::PathBuf, u32) {
    (v.path.clone(), v.brightness().to_bits())
}

impl StageLive {
    /// Start of a pass: look after the venue's loading and make the scene
    /// again if the stage has changed. Says what happened in `say`.
    pub fn begin(&mut self, show: &Show, shared: &mut StageShared, say: &mut Vec<Said>) {
        self.look_after_venue(show, shared, say);
        let stage = &show.stage3d;
        let mut h = crate::stage3d::scene::Fnv::new();
        if !stage.is_empty() {
            h.bytes(&stage.shape_key().to_le_bytes());
        }
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
            self.bounds = render3d::scene_bounds(&scene);
            self.scene = Arc::new(scene);
            shared.scene = self.scene.clone();
            shared.scene_key = h.0;
            shared.bounds = self.bounds;
        }
        self.needed_by_outputs = if show.outputs.iter().any(|o| o.source.is_some()) {
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
                        file_name(&venue.path),
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
                            let at = if have.is_some_and(|h| h.path == want.path) {
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
                    VenueStatus::Loading(file_name(&want.path))
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
    /// the displays showing the stage (with the camera each sees it by),
    /// `pictures` every display's layers, `fresh` whether anything moved
    /// since the last pass (something plays, a picture came in). Returns
    /// whether a job is still being drawn.
    pub fn send(
        &mut self,
        gpu: &Gpu,
        show: &Show,
        shared: &mut StageShared,
        outputs: &[(OutputId, Camera)],
        pictures: &HashMap<OutputId, PictureLayers>,
        fresh: bool,
    ) -> bool {
        let mut views = Vec::new();
        let mut scale = 0.0f32;
        for (id, camera) in outputs {
            let key = ViewKey::Output(*id);
            let Some(target) = self.targets.get(&key).cloned() else {
                continue;
            };
            let options = Options::default();
            let sig = signature(self.scene_key, camera, &options, &target);
            if fresh || self.drawn_with.get(&key) != Some(&sig) {
                self.drawn_with.insert(key, sig);
                scale = 1.0;
                views.push(ViewJob {
                    key: key.number(),
                    target,
                    scene: self.scene.clone(),
                    camera: *camera,
                    options,
                });
            }
        }
        let asked: Vec<_> = std::mem::take(&mut shared.requests);
        let mut shown: HashSet<PreviewId> = HashSet::new();
        for r in asked {
            let key = ViewKey::Preview(r.id);
            shown.insert(r.id);
            let target = self.target(gpu, key, capped(r.size));
            shared.targets.insert(r.id, target.clone());
            let sig = signature(self.scene_key, &r.camera, &r.options, &target);
            if fresh || self.drawn_with.get(&key) != Some(&sig) {
                self.drawn_with.insert(key, sig);
                scale = scale.max(r.options.quality.scale.clamp(0.1, 1.0));
                views.push(ViewJob {
                    key: key.number(),
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
        let live: HashSet<OutputId> = outputs.iter().map(|(o, _)| *o).collect();
        self.targets.retain(|k, _| match k {
            ViewKey::Output(o) => live.contains(o),
            ViewKey::Preview(_) => true,
        });
        if !views.is_empty() {
            self.keep_thread(gpu);
            if self.thread.is_none() {
                // Drawn when there is a thread again.
                self.drawn_with.clear();
            }
            let feeds = feed_jobs(show, &self.scene, pictures, scale);
            self.serial += 1;
            if let Some(t) = &self.thread {
                t.submit(StageJob {
                    feeds,
                    views,
                    serial: self.serial,
                });
            }
        }
        self.thread.as_ref().is_some_and(|t| t.done() < self.serial)
    }

    /// Whether a picture has been drawn since this was last asked: the
    /// windows showing one are to be drawn again.
    pub fn landed(&mut self) -> bool {
        let done = self.thread.as_ref().map_or(0, StageThread::done);
        let new = done != self.seen_done;
        self.seen_done = done;
        new
    }

    /// Start the drawing thread, or again if it stopped.
    fn keep_thread(&mut self, gpu: &Gpu) {
        if self.thread.as_ref().is_some_and(StageThread::is_finished) {
            log::warn!("the 3D stage's drawing thread stopped; starting another");
            self.thread = None;
            self.drawn_with.clear();
        }
        if self.thread.is_none() && self.started.is_none_or(|t| t.elapsed() >= RESTART_EVERY) {
            self.started = Some(Instant::now());
            match StageThread::spawn(gpu.clone()) {
                Ok(t) => {
                    self.seen_done = 0;
                    self.serial = 0;
                    self.thread = Some(t);
                }
                Err(e) => log::warn!("could not start drawing the 3D stage: {e}"),
            }
        }
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

fn signature(scene: u64, camera: &Camera, options: &Options, target: &Arc<LayerTexture>) -> u64 {
    let mut h = crate::stage3d::scene::Fnv::new();
    h.bytes(&scene.to_le_bytes());
    h.bytes(format!("{camera:?}{options:?}").as_bytes());
    h.bytes(&(Arc::as_ptr(target) as usize).to_le_bytes());
    h.0
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
