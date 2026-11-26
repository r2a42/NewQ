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
use crate::stage_render::{
    CountTo, FeedJob, StageJob, StageThread, ThrowJob, ViewJob, feed_size, throw_size,
};
use crate::stage3d::ProjectorId;
use crate::stage3d::Venue;
use crate::stage3d::light::{self, MAX_THROWS};
use crate::stage3d::load::{Found, LoadedVenue, Unread, load_venue_with};
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
/// With no 3D view in sight and no display showing the stage for this long,
/// the drawing thread goes, and with it everything it kept on the graphics
/// card (the venue's meshes and pictures, the displays' pictures, the
/// render targets); it starts again when a view is next asked for. A view
/// on screen keeps it however still it is (`StageShared::in_sight`).
pub const IDLE_RELEASE: Duration = Duration::from_secs(30);
/// A 3D view on screen has its window drawn at least this often, though
/// nothing changes, so it is still counted as in sight (`IDLE_RELEASE`).
pub const IN_SIGHT_EVERY: Duration = Duration::from_secs(10);
/// The venue model's files (the model and those it reads beside it) are
/// looked at this often for a change on disk (a model exported again over
/// itself), on a thread; read again once two looks in a row agree.
pub const WATCH_EVERY: Duration = Duration::from_secs(2);
/// A venue still arriving from a show network's primary is looked at again
/// this often.
const ARRIVING_LOOK: Duration = Duration::from_secs(1);
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

/// A projector, as the views with their light on throw its light: its shape
/// (as the 3D views' lines have it, `stage_projectors::shape_of`) and what
/// its display sends.
#[derive(Clone)]
pub struct ProjectorThrow {
    pub id: ProjectorId,
    pub pixels: (u32, u32),
    pub aspect: f32,
    /// None: it has no display here (or none that can be drawn): it throws
    /// plain white.
    pub picture: Option<ThrowPicture>,
}

/// What a projector's display sends: what its window shows (its layers
/// through its keystone or mesh and its part of a picture, its sub displays),
/// and its edge blend.
#[derive(Clone)]
pub struct ThrowPicture {
    pub groups: Vec<OutputGroup>,
    pub blend: crate::show::EdgeBlend,
}

/// What a display's picture is made of, for drawing it as the stage shows it.
#[derive(Clone)]
pub struct PictureLayers {
    pub layers: Vec<OutputLayer>,
    /// Its own canvas (pixels).
    pub canvas: (u32, u32),
}

/// How each of a venue model's files stood when looked at: where `locate`
/// found it, its length and when it was last written (None: not here).
type Stamp = Vec<Option<(std::path::PathBuf, u64, Option<std::time::SystemTime>)>>;

/// What a venue load reads, and how its files stood just before.
struct Read {
    key: (std::path::PathBuf, Option<SampleVenue>, u32),
    files: Arc<Vec<std::path::PathBuf>>,
    stamp: Stamp,
}

/// What a load thread hands back: its files, how they stood, the venue.
type LoadDone = (
    Arc<Vec<std::path::PathBuf>>,
    Stamp,
    Result<LoadedVenue, Unread>,
);

struct VenueState {
    /// Where its files are (`locate`; a test's stand-in for a show network).
    locate: fn(&std::path::Path) -> Found,
    loaded: Option<Arc<LoadedVenue>>,
    loading: Option<(Venue, Receiver<LoadDone>)>,
    /// The files of the venue last read (loaded or failed) and how they
    /// stood: looked at every `WATCH_EVERY` for a change on disk.
    read: Option<Read>,
    /// A look at them under way, and when the next may start.
    look: Option<Receiver<Stamp>>,
    next_look: Option<Instant>,
    /// What the last look found, when not as read: read again once the next
    /// look finds the same (a file still being written keeps changing).
    changed: Option<Stamp>,
    /// The files changed on disk and have rested: read again (as a change of
    /// brightness is; the old model stays until the new one is in).
    stale: bool,
    /// What last failed to load, so it isn't tried every pass (until the
    /// media change: `media_generation`).
    failed: Option<(Venue, u64)>,
    /// A venue whose files are on their way from a show network's primary,
    /// and when to look again.
    arriving: Option<(Venue, Instant)>,
    /// A wanted venue, and when it may be read (once its settings have rested).
    settling: Option<(Venue, Instant)>,
}

impl VenueState {
    /// Look after the watch on `want`'s files: take a look's answer (two in
    /// a row the same and not as read: `stale`), and start the next every
    /// `WATCH_EVERY` while nothing is being read.
    fn watch(&mut self, want: &Venue, now: Instant) {
        let key = reload_key(want);
        let watched = self
            .read
            .as_ref()
            .filter(|r| r.key == key && want.sample.is_none());
        if let Some(rx) = &self.look {
            match rx.try_recv() {
                Ok(stamp) => {
                    self.look = None;
                    match watched {
                        Some(r) if r.stamp != stamp => {
                            if self.changed.as_ref() == Some(&stamp) {
                                self.stale = true;
                                self.changed = None;
                            } else {
                                self.changed = Some(stamp);
                            }
                        }
                        _ => self.changed = None,
                    }
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.look = None,
            }
        }
        // Nothing to look at, or it is being read: no look is due (and
        // `wake_at` asks for none).
        let Some(r) =
            watched.filter(|_| !self.stale && self.loading.is_none() && self.settling.is_none())
        else {
            self.next_look = None;
            return;
        };
        if self.look.is_some() || self.next_look.is_some_and(|t| now < t) {
            return;
        }
        self.next_look = Some(now + WATCH_EVERY);
        let (tx, rx) = std::sync::mpsc::channel();
        let files = r.files.clone();
        let locate = self.locate;
        let spawned = std::thread::Builder::new()
            .name("venue-watch".into())
            .spawn(move || {
                let _ = tx.send(stamp_of(&files, &locate));
            });
        if spawned.is_ok() {
            self.look = Some(rx);
        }
    }
}

impl Default for VenueState {
    fn default() -> Self {
        VenueState {
            locate,
            loaded: None,
            loading: None,
            read: None,
            look: None,
            next_look: None,
            changed: None,
            stale: false,
            failed: None,
            arriving: None,
            settling: None,
        }
    }
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
    /// As if the thread were behind with the displays (set by a test).
    force_behind: bool,
    /// The displays whose pictures a display showing the stage needs.
    needed_by_outputs: HashSet<OutputId>,
    /// When each preview last asked for a picture (a pane closed lets its
    /// target go after `PREVIEW_KEPT`).
    asked_at: HashMap<PreviewId, Instant>,
    /// The scene with the projectors' light in it, for the views with their
    /// light on, and its key (made again only when that changes).
    lit_scene: Arc<Scene>,
    lit_key: u64,
    /// Where what the stage did is counted (`stage_render::totals`).
    count: CountTo,
    /// The thread was found stopped and hasn't been started again (the
    /// next start is a restart).
    died: bool,
    /// Lens maps handed in by a test (`give_lens_map`), by projector: what
    /// `lens_map` returns until the bake makes them.
    given_maps: HashMap<ProjectorId, Arc<crate::mapped::LensMap>>,
}

/// A preview's target is kept this long after it last asked for a picture.
const PREVIEW_KEPT: Duration = Duration::from_secs(30);

/// The settings of a venue a load depends on.
fn reload_key(v: &Venue) -> (std::path::PathBuf, Option<SampleVenue>, u32) {
    (v.path.clone(), v.sample, v.brightness().to_bits())
}

/// Where a file a venue model reads is on this computer: the file itself, or
/// on a show network's computer its copy in the store, whole and checked
/// (never a part still coming: `net::media::local_path`).
fn locate(p: &std::path::Path) -> Found {
    match crate::net::media::local_path(p) {
        crate::net::media::Local::Here(x) => Found::Here(x),
        crate::net::media::Local::Transferring(_) => Found::Arriving,
        crate::net::media::Local::NotFound => Found::Missing,
    }
}

/// The files the venue model at `path` reads: itself first, then those
/// beside it (`load::venue_files`; only for a model here at that path: on a
/// show network's computer a new version comes as a new store object).
fn venue_file_list(
    path: &std::path::Path,
    locate: &dyn Fn(&std::path::Path) -> Found,
) -> Vec<std::path::PathBuf> {
    let mut files = vec![path.to_path_buf()];
    if matches!(locate(path), Found::Here(p) if p == path) {
        files.extend(crate::stage3d::load::venue_files(path));
    }
    files
}

/// How `files` stand now (a few `stat`s; on a thread).
fn stamp_of(files: &[std::path::PathBuf], locate: &dyn Fn(&std::path::Path) -> Found) -> Stamp {
    files
        .iter()
        .map(|f| match locate(f) {
            Found::Here(p) => {
                let m = std::fs::metadata(&p).ok().filter(|m| m.is_file())?;
                Some((p, m.len(), m.modified().ok()))
            }
            Found::Arriving | Found::Missing => None,
        })
        .collect()
}

/// Changes when the files `locate` finds may have (0 with no show network).
fn media_generation() -> u64 {
    crate::net::media::finder().map_or(0, |f| f.generation())
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
        let now = Instant::now();
        // A load that has finished.
        if let Some((venue, rx)) = &v.loading {
            let done = rx.try_recv().map(|(files, stamp, result)| {
                v.read = Some(Read {
                    key: reload_key(venue),
                    files,
                    stamp,
                });
                v.changed = None;
                result
            });
            match done {
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
                    v.arriving = None;
                    v.loading = None;
                }
                Ok(Err(Unread::Failed(why))) => {
                    say.push(Said::Problem(format!("The venue didn't load: {why}")));
                    v.failed = Some((venue.clone(), media_generation()));
                    v.loading = None;
                    shared.venue = VenueStatus::Failed(why);
                }
                // A file it needs is still coming from the primary: looked
                // at again in a moment (never a failure).
                Ok(Err(Unread::Arriving)) => {
                    v.arriving = Some((venue.clone(), now + ARRIVING_LOOK));
                    v.loading = None;
                    v.read = None;
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
                v.arriving = None;
                v.read = None;
                v.look = None;
                v.changed = None;
                v.stale = false;
                shared.venue = VenueStatus::None;
            }
            Some(want) => {
                v.watch(want, now);
                let have = v.loaded.as_ref().map(|l| &l.loaded_for);
                let current = have.is_some_and(|h| !want.needs_reload(h)) && !v.stale;
                let busy = v
                    .loading
                    .as_ref()
                    .is_some_and(|(l, _)| reload_key(l) == reload_key(want));
                // A failure is tried again when the media change (a show
                // network's primary sent a file it lacked).
                // And when its files change on disk (`stale`).
                let failed = !v.stale
                    && v.failed.as_ref().is_some_and(|(f, generation)| {
                        reload_key(f) == reload_key(want) && *generation == media_generation()
                    });
                // (One for a venue no longer wanted is forgotten, so it
                // brings the loop round for nothing.)
                if v.arriving
                    .as_ref()
                    .is_some_and(|(a, _)| reload_key(a) != reload_key(want))
                {
                    v.arriving = None;
                }
                let arriving = v.arriving.is_some();
                let look_again = v.arriving.as_ref().is_none_or(|(_, at)| now >= *at);
                if current {
                    v.settling = None;
                    v.arriving = None;
                } else if arriving && !look_again {
                    // Still on its way: looked at again at `ARRIVING_LOOK`.
                } else if want.sample.is_none()
                    && !busy
                    && (v.locate)(&want.path) == Found::Arriving
                {
                    // The model itself isn't here yet: not read until it is.
                    v.arriving = Some((want.clone(), now + ARRIVING_LOOK));
                } else if !busy && !failed {
                    // Wait for the settings to rest, then read it on a thread.
                    let ready_at = match &v.settling {
                        Some((s, at)) if reload_key(s) == reload_key(want) => *at,
                        _ => {
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
                    if now >= ready_at {
                        v.settling = None;
                        let (tx, rx) = std::sync::mpsc::channel();
                        let venue = want.clone();
                        let path = want.path.clone();
                        let locate = v.locate;
                        let count = self.count;
                        let spawned = std::thread::Builder::new().name("venue-load".into()).spawn(
                            move || {
                                // How its files stand just before they are
                                // read: a change while reading is seen later.
                                let files = if venue.sample.is_some() {
                                    Vec::new()
                                } else {
                                    venue_file_list(&path, &locate)
                                };
                                let stamp = stamp_of(&files, &locate);
                                let began = Instant::now();
                                let loaded = load_venue_with(&path, &venue, &locate);
                                match &loaded {
                                    Ok(l) => count.venue_read(
                                        began.elapsed(),
                                        Some((l.parts.len(), l.triangles)),
                                    ),
                                    Err(Unread::Failed(_)) => {
                                        count.venue_read(began.elapsed(), None)
                                    }
                                    Err(Unread::Arriving) => {}
                                }
                                let _ = tx.send((Arc::new(files), stamp, loaded));
                            },
                        );
                        match spawned {
                            Ok(_) => {
                                v.loading = Some((want.clone(), rx));
                                v.arriving = None;
                                v.stale = false;
                                v.look = None;
                                v.changed = None;
                            }
                            Err(e) => {
                                say.push(Said::Problem(format!("The venue didn't load: {e}")));
                                v.failed = Some((want.clone(), media_generation()));
                                v.stale = false;
                            }
                        }
                    }
                }
                let arriving = v
                    .arriving
                    .as_ref()
                    .is_some_and(|(a, _)| reload_key(a) == reload_key(want));
                shared.venue = if v.loading.is_some() || v.settling.is_some() {
                    VenueStatus::Loading(venue_name(want))
                } else if let Some(l) = v.loaded.as_ref().filter(|_| current) {
                    VenueStatus::Loaded {
                        triangles: l.triangles,
                        warnings: l.warnings.clone(),
                    }
                } else if arriving {
                    VenueStatus::Arriving(venue_name(want))
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

    /// The lens map of mapped projector `projector` at `size`, for `use_`:
    /// None until one is made (its display is black meanwhile, never its
    /// flat picture). The app calls it every pass, before `send`, for each
    /// map it draws (`mapped::wanted`, and the 3D views' light): a call is
    /// the ask, and a map not asked for a while may be let go. A map being
    /// replaced is returned until the new one is ready.
    #[allow(dead_code)]
    pub fn lens_map(
        &mut self,
        projector: ProjectorId,
        size: (u32, u32),
        use_: crate::mapped::MapUse,
    ) -> Option<Arc<crate::mapped::LensMap>> {
        let _ = use_;
        self.given_maps
            .get(&projector)
            .filter(|m| m.size == size)
            .cloned()
    }

    /// A lens map for `projector` made by a test (`lens_map` returns it at
    /// its size, whatever the bake would make).
    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) fn give_lens_map(
        &mut self,
        projector: ProjectorId,
        map: Arc<crate::mapped::LensMap>,
    ) {
        self.given_maps.insert(projector, map);
    }

    /// Whether display `id` is a canvas that a mapped projector this
    /// computer draws (for a window or a stream) uses: its pictures are then
    /// output work, never at the previews' pace.
    #[allow(dead_code)]
    pub fn feeds_output(&self, id: OutputId) -> bool {
        let _ = id;
        false
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
    #[allow(clippy::too_many_arguments)]
    pub fn send(
        &mut self,
        gpu: &Gpu,
        show: &Show,
        shared: &mut StageShared,
        outputs: &[StageOutput],
        pictures: &HashMap<OutputId, PictureLayers>,
        projectors: &[ProjectorThrow],
        fresh: bool,
    ) -> bool {
        let now = Instant::now();
        // A thread that stopped (a panic) draws nothing: what it was given is
        // to be drawn again by the next one.
        if self.thread.as_ref().is_some_and(StageThread::is_finished) {
            log::warn!("the 3D stage's drawing thread stopped; starting another");
            self.count.thread_stopped();
            self.died = true;
            self.thread = None;
            self.drawn_with.clear();
        }
        // Behind with the displays: a job is waiting while another's
        // displays are still being drawn (one job under way is normal).
        let behind = self.force_behind
            || self
                .thread
                .as_ref()
                .is_some_and(|t| t.displays_done().saturating_add(1) < self.serial);
        let displays_behind = behind && outputs.iter().any(|o| o.seen);
        self.governed_only = !outputs.is_empty() && outputs.iter().all(|o| !o.seen);
        let asked: Vec<_> = std::mem::take(&mut shared.requests);
        // The views on screen, asking or not (a still pane, a paused one):
        // they keep the thread and their targets.
        let in_sight: Vec<PreviewId> = std::mem::take(&mut shared.in_sight);
        for id in &in_sight {
            self.asked_at.insert(*id, now);
        }
        let pictures_key = if outputs.is_empty() && asked.is_empty() {
            0
        } else {
            feeds_key(show, &self.scene, pictures)
        };
        // The projectors' light, for the views with their light on.
        let lit_asked = asked
            .iter()
            .any(|r| r.options.light != render3d::LightView::Off);
        let (throws_key, throw_jobs) = if lit_asked {
            self.light_up(show, projectors)
        } else {
            (0, Vec::new())
        };
        let mut views = Vec::new();
        // What each view is drawn with, once it is handed over.
        let mut marks = Vec::new();
        let mut scale = 0.0f32;
        let mut lit_scale = 0.0f32;
        for o in outputs {
            let key = ViewKey::Output(o.id);
            let Some(target) = self.targets.get(&key).cloned() else {
                continue;
            };
            if !o.seen && (!o.due || displays_behind) {
                continue;
            }
            let options = Options::default();
            let sig = signature(
                self.scene_key,
                &o.camera,
                &options,
                &target,
                pictures_key,
                &[],
            );
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
                    lines: Arc::default(),
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
            let lit = r.options.light != render3d::LightView::Off;
            let sig = signature(
                if lit { self.lit_key } else { self.scene_key },
                &r.camera,
                &r.options,
                &target,
                pictures_key ^ if lit { throws_key } else { 0 },
                &r.lines,
            );
            if fresh || self.drawn_with.get(&key) != Some(&sig) {
                marks.push((key, sig));
                let s = r.options.quality.scale.clamp(0.1, 1.0);
                scale = scale.max(s);
                if lit {
                    lit_scale = lit_scale.max(s);
                }
                views.push(ViewJob {
                    key: key.number(),
                    display: false,
                    target,
                    scene: if lit {
                        self.lit_scene.clone()
                    } else {
                        self.scene.clone()
                    },
                    camera: r.camera,
                    options: r.options,
                    lines: r.lines,
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
        if !outputs.is_empty() || !shown.is_empty() || !in_sight.is_empty() {
            self.wanted_at = Some(now);
        }
        if !views.is_empty() {
            self.keep_thread(gpu);
            match &self.thread {
                Some(t) => {
                    let feeds = feed_jobs(show, &self.scene, pictures, scale);
                    self.serial += 1;
                    // Displays first (`StageJob::merged` keeps them so).
                    // The projectors' pictures, only for a view with its light on.
                    let (throws, throw_size) = if lit_scale > 0.0 {
                        (throw_jobs, throw_size(lit_scale))
                    } else {
                        (Vec::new(), (0, 0))
                    };
                    let job = StageJob {
                        feeds,
                        throws,
                        throw_size,
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
        // No view in sight and no display showing the stage for a while:
        // everything the thread keeps on the graphics card goes with it.
        if self.thread.is_some()
            && self
                .wanted_at
                .is_none_or(|t| now.saturating_duration_since(t) >= IDLE_RELEASE)
        {
            log::info!("3D stage: nothing to draw for a while; its pictures are let go");
            if let Some(t) = self.thread.take() {
                drop(t.quit());
            }
            self.count.idle_release();
            self.drawn_with.clear();
            self.started = None;
            self.restart_wanted = false;
        }
        self.busy()
    }

    /// The scene with the projectors' light in it (`lit_scene`, made again
    /// when it changes), a key for what their displays send (a view with its
    /// light on is drawn again when it changes: a blend's width, a still),
    /// and the jobs that draw their pictures.
    fn light_up(&mut self, show: &Show, projectors: &[ProjectorThrow]) -> (u64, Vec<ThrowJob>) {
        let mut throws = Vec::new();
        let mut jobs = Vec::new();
        let mut h = crate::stage3d::scene::Fnv::new();
        let mut left_out = 0;
        for pt in projectors {
            let Some(p) = show.projector(pt.id).filter(|p| p.visible) else {
                continue;
            };
            if throws.len() >= MAX_THROWS {
                left_out += 1;
                continue;
            }
            let layer = throws.len() as u32;
            throws.push(light::throw_of(
                p,
                pt.aspect,
                pt.pixels,
                layer,
                pt.picture.is_some(),
            ));
            if let Some(pic) = &pt.picture {
                h.bytes(&layer.to_le_bytes());
                for g in &pic.groups {
                    groups_key(&mut h, g);
                }
                h.bytes(format!("{:?}", pic.blend).as_bytes());
                jobs.push(ThrowJob {
                    layer,
                    groups: pic.groups.clone(),
                    blend: pic.blend,
                });
            }
        }
        self.count.throws_left_out(left_out);
        let mut k = crate::stage3d::scene::Fnv::new();
        k.bytes(&self.scene_key.to_le_bytes());
        k.bytes(format!("{throws:?}").as_bytes());
        if k.0 != self.lit_key || self.lit_scene.parts.len() != self.scene.parts.len() {
            self.lit_key = k.0;
            self.lit_scene = Arc::new(Scene {
                parts: self.scene.parts.clone(),
                throws,
            });
        }
        (h.0, jobs)
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
        // A venue on its way from the primary is looked for again.
        if let Some((_, a)) = &self.venue.arriving {
            take(*a);
        }
        // While something shows the stage, its venue's files are looked at
        // for a change (and a look's answer taken soon after).
        if self.thread.is_some() && self.venue.read.is_some() {
            if self.venue.look.is_some() {
                take(Instant::now() + Duration::from_millis(50));
            } else if let Some(t) = self.venue.next_look {
                take(t);
            }
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
            match StageThread::spawn(gpu.clone(), self.count) {
                Ok(t) => {
                    self.count.thread_started(self.died);
                    self.died = false;
                    self.seen_done = 0;
                    self.seen_displays = 0;
                    self.serial = 0;
                    self.thread = Some(t);
                }
                Err(e) => log::warn!("could not start drawing the 3D stage: {e}"),
            }
        }
    }

    /// Find the venue's files with `f` (a test's stand-in for a show
    /// network's store).
    #[cfg(test)]
    pub fn locate_with(&mut self, f: fn(&std::path::Path) -> Found) {
        self.venue.locate = f;
    }

    /// Look at the venue's files at the next pass, not `WATCH_EVERY` after the
    /// last look (for a test).
    #[cfg(test)]
    pub fn look_now(&mut self) {
        self.venue.next_look = None;
    }

    /// Count what the stage does into `count` (for a test).
    #[cfg(test)]
    pub fn count_into(&mut self, count: CountTo) {
        self.count = count;
    }

    /// Behave as if the thread were behind with the displays (for a test).
    #[cfg(test)]
    pub fn set_behind(&mut self, behind: bool) {
        self.force_behind = behind;
    }

    /// Whether the drawing thread is running (for a test).
    #[cfg(test)]
    pub fn has_thread(&self) -> bool {
        self.thread.is_some()
    }

    /// Stop the drawing thread as a panic in it would, and wait until it
    /// has (for a test).
    #[cfg(test)]
    /// The thread is taken as just started, so the restart waits its
    /// `RESTART_EVERY` however slowly the first picture was drawn.
    pub fn panic_thread(&mut self) {
        self.started = Some(Instant::now());
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
        let long_ago = Instant::now().checked_sub(PREVIEW_KEPT);
        for at in self.asked_at.values_mut() {
            if let Some(t) = long_ago {
                *at = t;
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

fn signature(
    scene: u64,
    camera: &Camera,
    options: &Options,
    target: &Arc<LayerTexture>,
    pictures: u64,
    lines: &[crate::render3d::Line],
) -> u64 {
    let mut h = crate::stage3d::scene::Fnv::new();
    h.bytes(&scene.to_le_bytes());
    h.bytes(&pictures.to_le_bytes());
    h.bytes(format!("{camera:?}{options:?}").as_bytes());
    // Picking another projector changes its lines, not the scene.
    for l in lines {
        l.a.iter()
            .chain(&l.b)
            .chain(&l.colour)
            .for_each(|v| h.f32(*v));
    }
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

/// Into `h`: what a group's picture is made of and how it is placed (each
/// layer's texture, whether it holds a picture yet, its look; the keystone,
/// mesh and part).
fn groups_key(h: &mut crate::stage3d::scene::Fnv, g: &OutputGroup) {
    let geo = &g.geometry;
    h.bytes(
        format!(
            "{:?}{:?}{:?}{:?}{:?}",
            geo.canvas,
            geo.warp,
            geo.pin,
            geo.slice,
            geo.mesh.as_deref()
        )
        .as_bytes(),
    );
    for l in &g.layers {
        h.bytes(&(Arc::as_ptr(&l.texture) as usize).to_le_bytes());
        h.bytes(&[u8::from(l.texture.is_filled())]);
        h.bytes(&l.opacity.to_bits().to_le_bytes());
        h.bytes(format!("{:?}{:?}{:?}{:?}", l.fit, l.transform, l.color, l.fx).as_bytes());
    }
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
        throws: Vec::new(),
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
