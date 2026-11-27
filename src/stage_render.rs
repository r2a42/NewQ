//! The 3D stage drawn off screen, on a thread of its own ("stage-render"), for
//! the Preview pane's 3D views and for the displays that show the stage
//! (`OutputConfig::source`).
//!
//! The main loop hands it a `StageJob` each pass that wants a picture (latest
//! wins, merged so no view asked for is lost): first the pictures of the
//! displays the stage shows (`FeedJob`: each display's layers composited off
//! screen, as `CaptureThread` does a stream's, into a texture of its own),
//! then each view (`ViewJob`): the scene seen by its camera, drawn by
//! `render3d::Renderer` into the view's target, a `LayerTexture` made with
//! `LayerTexture::new_target`. A display that shows the stage has that layer as
//! its only picture, so its window, keystone, edge blend, streams, previews
//! and the projectors sharing its picture all work as for any other.
//!
//! Nothing here runs by itself: a preview the governor holds back sends no job,
//! so its displays' pictures aren't drawn for it either. A display's target is
//! drawn whenever the main loop asks, never governed.
//!
//! The feeds are composited through the one `Slot::Stage`, each submitted
//! before the next is set: a layer can be on two displays (`also_on`, a picture
//! shared by projectors) and its uniforms for the slot are written per display.
//!
//! For a view with its light on, each projector's picture is drawn too
//! (`ThrowJob`): what its display's window sends, its keystone or mesh and
//! its edge blend included, into one layer of the throws' picture array, which
//! the renderer throws into the room (`render3d::lit`). Throws are drawn only
//! in a job with such a view (a preview), through the same slot, after the
//! displays' views: an output never waits for them.
//!
//! Mapped projectors' lens maps (`MapJob`, `render3d::bake`): those a window
//! or a stream draws (output work) first of all, before the feeds; those a
//! 3D view's light wants (preview work, sent only with a governed lit view)
//! after the previews. Each lands, as soon as it is made, in the slot the
//! thread was given (`StageThread::spawn_with`), which the app owns, so a
//! map outlives the thread.

use crate::gpu::{Gpu, LayerTexture, Slot};
use crate::mapped::{Drawer, LensMap, MapOwner, MappedFrame};
use crate::render::{Mailbox, OutputGroup};
use crate::render3d::{Camera, FrameStats, LightView, Line, Options, Renderer, Scene, Target};
use crate::show::{EdgeBlend, OutputId};
use crate::stage3d::{Projector, ProjectorId};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[path = "stage_counts.rs"]
mod counts;
#[allow(unused_imports)]
pub use counts::{CountTo, Counters, FRAME_MS_BANDS, StageTotals, TOTALS, totals};

/// The format the feeds and the views' targets are drawn in: display values,
/// as the outputs' pictures are.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// A feed's longest side, at full size.
pub const FEED_SIDE: u32 = 1024;
/// A feed's longest side, at the least (a quarter-size preview).
pub const FEED_SIDE_LEAST: u32 = 256;
/// A feed not asked for in this long is let go.
const FEED_KEPT: Duration = Duration::from_secs(10);
/// A projector's picture's longest side, at full size (`throw_size`).
pub const THROW_SIDE: u32 = 960;

/// One projector's picture, as its display sends it, for the light.
#[derive(Clone)]
pub struct ThrowJob {
    /// Its layer in the throws' picture array (`Throw::layer`).
    pub layer: u32,
    /// What its display's window shows: its layers through its keystone,
    /// mesh and part of a picture (not where the window is on its monitor).
    pub groups: Vec<OutputGroup>,
    /// Its edge blend.
    pub blend: EdgeBlend,
    /// A mapped projector's: its mapped picture, drawn first (by
    /// `mapped::Drawer`, through `MapOwner::Throw(layer)`'s slots) into the
    /// frame's target, which `groups` shows as the display's layer. None for
    /// any other projector: drawn as before.
    pub mapped: Option<MappedFrame>,
}

/// The size every projector's picture is drawn at for views drawn at `scale`:
/// 16:9, its longest side `THROW_SIDE` times that, never under
/// `FEED_SIDE_LEAST`. (A projector of another shape is stretched into it:
/// its own shape is in its lens's matrix, so it lands right.)
pub fn throw_size(scale: f32) -> (u32, u32) {
    let scale = if scale.is_finite() {
        scale.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let side = ((THROW_SIDE as f32 * scale).round() as u32).max(FEED_SIDE_LEAST);
    (side, (side * 9).div_ceil(16))
}

/// A display's picture, to draw before the views.
#[derive(Clone)]
pub struct FeedJob {
    pub output: OutputId,
    /// Its layers on its own canvas (no keystone, mesh or edge blend: the
    /// picture the audience sees).
    pub groups: Vec<OutputGroup>,
    pub size: (u32, u32),
}

/// One view of the stage.
#[derive(Clone)]
pub struct ViewJob {
    /// Its key for the stats (`StageThread::stats`) and the renderer's
    /// textures (`render3d::Target::owner`).
    pub key: u64,
    /// A display's picture (a window or a stream shows it): drawn before
    /// the previews, and said to have landed as soon as they are.
    pub display: bool,
    pub target: Arc<LayerTexture>,
    pub scene: Arc<Scene>,
    pub camera: Camera,
    pub options: Options,
    /// Drawn over the picture (a preview's projector cones); none on a
    /// display's picture.
    pub lines: Arc<Vec<Line>>,
}

/// A lens map to bake: projector `projector`'s at `size`, its picture
/// `aspect` wide per high, of `scene` (`lensmap::still_scene`), naming
/// `feeds` (`lensmap::feeds_for`); `key` is `lensmap::map_key`'s.
#[derive(Clone)]
pub struct MapJob {
    pub projector: Projector,
    pub aspect: f32,
    pub size: (u32, u32),
    pub key: u64,
    pub scene: Arc<Scene>,
    pub feeds: Vec<OutputId>,
    /// For a window or a stream: baked before anything else, never
    /// governed. Else for a 3D view's light, after the previews.
    pub output: bool,
    /// With its depth picture (the show has a movable screen).
    pub depth: bool,
}

/// A lens map made (or refused: None, said in the log).
pub struct Baked {
    pub projector: ProjectorId,
    pub size: (u32, u32),
    pub aspect: f32,
    pub key: u64,
    pub map: Option<Arc<LensMap>>,
}

/// Where the maps the thread makes land (`StageThread::spawn_with`).
pub type BakedSlot = Arc<parking_lot::Mutex<Vec<Baked>>>;

#[derive(Clone, Default)]
pub struct StageJob {
    pub feeds: Vec<FeedJob>,
    /// The projectors' pictures, for the views with their light on.
    pub throws: Vec<ThrowJob>,
    /// The size they are drawn at (`throw_size`).
    pub throw_size: (u32, u32),
    pub views: Vec<ViewJob>,
    /// Lens maps to bake.
    pub maps: Vec<MapJob>,
    /// The moved screens' parts and where they are (`lensmap::moved_parts`,
    /// mapping plan 4.2): every view draws its scene with these parts'
    /// transforms put in, so a screen that moves makes no new scene (and
    /// no new lit scene: its shadows follow by `shadow_key`).
    pub moved: Arc<Vec<(usize, [[f32; 4]; 4])>>,
    /// Counts up; `StageThread::done` says the last one drawn.
    pub serial: u64,
}

impl StageJob {
    /// `newer` taken over `self` when the thread hasn't drawn `self` yet: every
    /// view of either (the newer one's where both have it), every feed.
    pub fn merged(self, newer: StageJob) -> StageJob {
        let mut views = newer.views;
        for v in self.views {
            if !views.iter().any(|n| Arc::ptr_eq(&n.target, &v.target)) {
                views.push(v);
            }
        }
        // The displays first.
        views.sort_by_key(|v| !v.display);
        let mut feeds = newer.feeds;
        for f in self.feeds {
            if !feeds.iter().any(|n| n.output == f.output) {
                feeds.push(f);
            }
        }
        // The newer pictures of the projectors, and the older ones of those
        // the newer job doesn't have.
        let (throws, throw_size) = if newer.throws.is_empty() {
            (self.throws, self.throw_size)
        } else {
            let mut throws = newer.throws;
            if newer.throw_size == self.throw_size {
                for t in self.throws {
                    if !throws.iter().any(|n| n.layer == t.layer) {
                        throws.push(t);
                    }
                }
            }
            (throws, newer.throw_size)
        };
        // The newer maps, and the older ones of those the newer job hasn't.
        let mut maps = newer.maps;
        for m in self.maps {
            if !maps.iter().any(|n| {
                n.projector.id == m.projector.id && n.size == m.size && n.aspect == m.aspect
            }) {
                maps.push(m);
            }
        }
        StageJob {
            feeds,
            throws,
            throw_size,
            views,
            maps,
            // Where the screens are now.
            moved: newer.moved,
            serial: newer.serial.max(self.serial),
        }
    }
}

/// The size a feed of a picture `canvas` big is drawn at, for views drawn at
/// `scale` (1 for a display, the governor's for a preview): its longest side
/// `FEED_SIDE` times that, never under `FEED_SIDE_LEAST`, and never more than
/// the picture itself.
pub fn feed_size(canvas: (u32, u32), scale: f32) -> (u32, u32) {
    let (w, h) = (canvas.0.max(1), canvas.1.max(1));
    let scale = if scale.is_finite() {
        scale.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let side = ((FEED_SIDE as f32 * scale).round() as u32)
        .max(FEED_SIDE_LEAST)
        .min(w.max(h));
    let long = w.max(h) as f32;
    let f = |v: u32| ((v as f32 * side as f32 / long).round() as u32).max(1);
    (f(w), f(h))
}

enum Work {
    Job(Box<StageJob>),
    Quit,
    /// Stop as a panic would (a test of what follows).
    #[cfg(test)]
    Panic,
}

pub struct StageThread {
    mailbox: Arc<Mailbox<Work>>,
    handle: JoinHandle<()>,
    done: Arc<AtomicU64>,
    /// The serial of the last job whose displays are drawn (its previews
    /// may still be being drawn).
    displays_done: Arc<AtomicU64>,
    stats: Arc<parking_lot::Mutex<HashMap<u64, FrameStats>>>,
}

impl StageThread {
    /// The thread, counting what it draws into `count` (`TOTALS` by
    /// default); its lens maps land in a slot of its own.
    #[cfg(test)]
    pub fn spawn(gpu: Gpu, count: CountTo) -> std::io::Result<StageThread> {
        StageThread::spawn_with(gpu, count, BakedSlot::default())
    }

    /// The thread, its lens maps landing in `baked`.
    pub fn spawn_with(gpu: Gpu, count: CountTo, baked: BakedSlot) -> std::io::Result<StageThread> {
        let mailbox: Arc<Mailbox<Work>> = Arc::new(Mailbox::new());
        let mb = mailbox.clone();
        let done = Arc::new(AtomicU64::new(0));
        let d = done.clone();
        let displays_done = Arc::new(AtomicU64::new(0));
        let dd = displays_done.clone();
        let stats: Arc<parking_lot::Mutex<HashMap<u64, FrameStats>>> = Arc::default();
        let s = stats.clone();
        let handle = std::thread::Builder::new()
            .name("stage-render".into())
            .spawn(move || {
                let _said = crate::crash::OnPanic(|| {
                    log::warn!("the 3D stage's drawing thread stopped unexpectedly");
                });
                stage_loop(gpu, mb, d, dd, s, count, baked)
            })?;
        Ok(StageThread {
            mailbox,
            handle,
            done,
            displays_done,
            stats,
        })
    }

    /// Draw `job` (merged with one not yet taken up).
    pub fn submit(&self, job: StageJob) {
        self.mailbox
            .put(Work::Job(Box::new(job)), |old, new| match (old, new) {
                (Work::Quit, _) | (_, Work::Quit) => Work::Quit,
                #[cfg(test)]
                (Work::Panic, _) | (_, Work::Panic) => Work::Panic,
                (Work::Job(a), Work::Job(b)) => Work::Job(Box::new(a.merged(*b))),
            });
    }

    /// The serial of the last job drawn.
    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Acquire)
    }

    /// The serial of the last job whose displays' views are drawn.
    pub fn displays_done(&self) -> u64 {
        self.displays_done.load(Ordering::Acquire)
    }

    /// What the last drawing of view `key` cost.
    pub fn stats(&self, key: u64) -> Option<FrameStats> {
        self.stats.lock().get(&key).copied()
    }

    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    /// Make the thread stop as a panic would (for a test).
    #[cfg(test)]
    pub fn panic(&self) {
        self.mailbox.put(Work::Panic, |_, p| p);
    }

    pub fn quit(self) -> JoinHandle<()> {
        self.mailbox.put(Work::Quit, |_, q| q);
        self.handle
    }
}

struct Feed {
    size: (u32, u32),
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    used: Instant,
}

/// The projectors' pictures: one layer each.
struct Throws {
    size: (u32, u32),
    _texture: wgpu::Texture,
    layers: Vec<wgpu::TextureView>,
    array: wgpu::TextureView,
    used: Instant,
}

impl Throws {
    /// `count` layers (`render3d::lit::layers_for`).
    fn new(gpu: &Gpu, size: (u32, u32), count: usize) -> Throws {
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("stage throws"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: count as u32,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            // (Copied out only by a test.)
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let layers = (0..count as u32)
            .map(|i| {
                texture.create_view(&wgpu::TextureViewDescriptor {
                    label: Some("stage throw"),
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: i,
                    array_layer_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let array = texture.create_view(&wgpu::TextureViewDescriptor {
            label: Some("stage throws"),
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        Throws {
            size,
            _texture: texture,
            layers,
            array,
            used: Instant::now(),
        }
    }
}

/// What the thread keeps between jobs: the renderer (its pipelines, meshes
/// and pictures) and the feeds' textures.
pub struct StageDrawer {
    renderer: Renderer,
    compositor: wgpu::RenderPipeline,
    /// The edge blend of a projector's picture.
    blend: crate::gpu::BlendPass,
    feeds: HashMap<OutputId, Feed>,
    throws: Option<Throws>,
    /// The scenes the last job drew (when they change, the renderer lets
    /// go of what only the old ones had: `Renderer::keep_only`).
    scenes: Vec<Arc<Scene>>,
    /// Where what it draws is counted.
    count: CountTo,
    /// Where the lens maps it makes land.
    baked: BakedSlot,
    /// Draws mapped projectors' pictures for the light; made at the first.
    mapped: Option<Drawer>,
}

impl StageDrawer {
    /// (Counting into `TOTALS`; for a test.)
    #[cfg(test)]
    pub fn new(gpu: &Gpu) -> StageDrawer {
        StageDrawer::counting(gpu, CountTo::default())
    }

    /// One counting what it draws into `count`.
    pub fn counting(gpu: &Gpu, count: CountTo) -> StageDrawer {
        StageDrawer {
            renderer: Renderer::new(gpu),
            compositor: gpu.compositor.pipeline(&gpu.device, FORMAT),
            blend: crate::gpu::BlendPass::new(&gpu.device, FORMAT),
            feeds: HashMap::new(),
            throws: None,
            scenes: Vec::new(),
            count,
            baked: BakedSlot::default(),
            mapped: None,
        }
    }

    /// The lens maps it makes land in `baked`.
    pub fn baked_into(&mut self, baked: BakedSlot) {
        self.baked = baked;
    }

    /// The lens maps made since this was last asked (for a test).
    #[cfg(test)]
    pub fn take_baked(&self) -> Vec<Baked> {
        std::mem::take(&mut *self.baked.lock())
    }

    /// Have the renderer refuse the bake, as a card that won't build its
    /// pipeline does (for a test).
    #[cfg(test)]
    pub fn refuse_bake(&mut self) {
        self.renderer.refuse_bake();
    }

    /// Bake `job`'s map, and put it (or None, refused) where they land.
    fn bake(&mut self, gpu: &Gpu, job: &MapJob) {
        let made = self.renderer.bake(
            gpu,
            &crate::render3d::bake::Bake {
                scene: &job.scene,
                projector: &job.projector,
                aspect: job.aspect,
                size: job.size,
                key: job.key,
                feeds: &job.feeds,
                depth: job.depth,
            },
        );
        self.count.map_baked(made.is_ok());
        self.baked.lock().push(Baked {
            projector: job.projector.id,
            size: job.size,
            aspect: job.aspect,
            key: job.key,
            map: made.ok().map(Arc::new),
        });
    }

    /// Have the renderer refuse the light, as a card that won't build its
    /// pipelines does (for a test).
    #[cfg(test)]
    pub fn refuse_light(&mut self) {
        self.renderer.refuse_light();
    }

    /// What is kept: the renderer's meshes, pictures and render targets, and
    /// the feeds.
    #[cfg(test)]
    pub fn kept(&self) -> (usize, usize, usize, usize) {
        let (m, i, t) = self.renderer.kept();
        (m, i, t, self.feeds.len())
    }

    /// The projectors' pictures as drawn (for a test): the array, its size.
    #[cfg(test)]
    pub fn throws(&self) -> Option<(&wgpu::Texture, (u32, u32))> {
        self.throws.as_ref().map(|t| (&t._texture, t.size))
    }

    /// Draw the feeds, then each view (the displays' first, then
    /// `after_displays` is called); the stats of each view by its key.
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        job: &StageJob,
        after_displays: &mut dyn FnMut(),
    ) -> Vec<(u64, FrameStats)> {
        // A new scene (the stage edited, a venue read again): what only the
        // old one had goes now.
        let mut scenes: Vec<Arc<Scene>> = Vec::new();
        let in_views = job.views.iter().map(|v| &v.scene);
        for s in in_views.chain(job.maps.iter().map(|m| &m.scene)) {
            if !scenes.iter().any(|o| Arc::ptr_eq(o, s)) {
                scenes.push(s.clone());
            }
        }
        let same = scenes.len() == self.scenes.len()
            && scenes
                .iter()
                .all(|s| self.scenes.iter().any(|o| Arc::ptr_eq(s, o)));
        // (Only with a view: a job of maps alone keeps the views' meshes.)
        if !same && !job.views.is_empty() {
            let refs: Vec<&Scene> = scenes.iter().map(|s| s.as_ref()).collect();
            self.renderer.keep_only(&refs);
            self.scenes = scenes;
        }
        // The maps a window or stream draws: output work, first of all.
        for m in job.maps.iter().filter(|m| m.output) {
            self.bake(gpu, m);
        }
        let now = Instant::now();
        for f in &job.feeds {
            let size = (f.size.0.max(1), f.size.1.max(1));
            let feed = match self.feeds.get_mut(&f.output) {
                Some(feed) if feed.size == size => feed,
                _ => {
                    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("stage feed"),
                        size: wgpu::Extent3d {
                            width: size.0,
                            height: size.1,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: FORMAT,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    });
                    let view = texture.create_view(&Default::default());
                    self.feeds.insert(
                        f.output,
                        Feed {
                            size,
                            _texture: texture,
                            view,
                            used: now,
                        },
                    );
                    self.feeds.get_mut(&f.output).expect("just put in")
                }
            };
            feed.used = now;
            let draw: Vec<_> = f.groups.iter().map(OutputGroup::draw).collect();
            let cmd = gpu.composite(&self.compositor, Slot::Stage, &feed.view, &draw, None);
            // Before the next display's uniforms are written for the slot.
            gpu.submit([cmd]);
        }
        self.feeds
            .retain(|_, f| now.saturating_duration_since(f.used) < FEED_KEPT);
        let mut stats = Vec::new();
        let mut displays = true;
        let mut thrown = 0;
        for v in &job.views {
            if displays && !v.display {
                displays = false;
                after_displays();
                // Only the previews have light: their projectors' pictures
                // are drawn after the displays' views, never ahead of them.
                thrown = self.draw_throws(gpu, job, now);
            }
            let Some(view) = v.target.render_view() else {
                continue;
            };
            let target = Target {
                view: &view,
                format: FORMAT,
                size: (v.target.width, v.target.height),
                owner: v.key,
            };
            let feeds = &self.feeds;
            let pictures = match v.options.light {
                LightView::Off => None,
                _ => self.throws.as_ref().map(|t| &t.array),
            };
            // The moved screens where they are (a copy of the parts, the
            // meshes shared; the scene the app keeps is never changed).
            let moved;
            let scene: &Scene = if job.moved.is_empty() {
                &v.scene
            } else {
                moved = crate::stage3d::lensmap::with_moved(&v.scene, &job.moved);
                &moved
            };
            let s = self.renderer.render_lit(
                gpu,
                &target,
                scene,
                &v.camera,
                &|o| feeds.get(&o).map(|f| &f.view),
                &v.options,
                &v.lines,
                pictures,
            );
            v.target.mark_drawn();
            self.count.view(v.display, &s);
            stats.push((v.key, s));
        }
        if displays {
            after_displays();
        }
        // The maps a 3D view's light wants: preview work, last.
        for m in job.maps.iter().filter(|m| !m.output) {
            self.bake(gpu, m);
        }
        self.count.job(job.feeds.len(), thrown);
        stats
    }

    /// The projectors' pictures, each as its window would send it, into
    /// their layers of the throws' array (and the array let go once unused
    /// for a while); how many were drawn.
    fn draw_throws(&mut self, gpu: &Gpu, job: &StageJob, now: Instant) -> usize {
        let mut drawn = 0;
        if !job.throws.is_empty() {
            let size = (job.throw_size.0.max(1), job.throw_size.1.max(1));
            // As many layers as the highest one asked for (in powers of two).
            let count = crate::render3d::lit::layers_for(
                job.throws
                    .iter()
                    .map(|t| t.layer as usize + 1)
                    .max()
                    .unwrap_or(1),
            );
            if self
                .throws
                .as_ref()
                .is_none_or(|t| t.size != size || t.layers.len() < count)
            {
                self.throws = Some(Throws::new(gpu, size, count));
            }
            if let Some(throws) = &mut self.throws {
                throws.used = now;
                for t in &job.throws {
                    let Some(layer) = throws.layers.get(t.layer as usize) else {
                        continue;
                    };
                    // A mapped projector's picture first, into the target
                    // its groups show (nothing at all for any other).
                    let mut cmds = match &t.mapped {
                        Some(f) => crate::mapped::draw_mapped(
                            &mut self.mapped,
                            gpu,
                            MapOwner::Throw(t.layer as u8),
                            std::slice::from_ref(f),
                        ),
                        None => Vec::new(),
                    };
                    let draw: Vec<_> = t.groups.iter().map(OutputGroup::draw).collect();
                    let blend = (!t.blend.is_off()).then(|| {
                        self.blend.set(&gpu.queue, &t.blend);
                        &self.blend
                    });
                    cmds.push(gpu.composite(&self.compositor, Slot::Stage, layer, &draw, blend));
                    // Before the next one's uniforms and blend are written.
                    gpu.submit(cmds);
                    drawn += 1;
                }
            }
        }
        if self
            .throws
            .as_ref()
            .is_some_and(|t| now.saturating_duration_since(t.used) >= FEED_KEPT)
        {
            self.throws = None;
        }
        drawn
    }
}

fn stage_loop(
    gpu: Gpu,
    mailbox: Arc<Mailbox<Work>>,
    done: Arc<AtomicU64>,
    displays_done: Arc<AtomicU64>,
    stats: Arc<parking_lot::Mutex<HashMap<u64, FrameStats>>>,
    count: CountTo,
    baked: BakedSlot,
) {
    // Made at the first job: a show with no stage never builds the pipelines.
    let mut drawer: Option<StageDrawer> = None;
    loop {
        let job = match mailbox.take() {
            Work::Quit => break,
            Work::Job(j) => j,
            #[cfg(test)]
            Work::Panic => panic!("a test stops the 3D stage's drawing thread"),
        };
        let d = drawer.get_or_insert_with(|| {
            let mut d = StageDrawer::counting(&gpu, count);
            d.baked_into(baked.clone());
            d
        });
        let drawn = d.draw(&gpu, &job, &mut || {
            displays_done.store(job.serial, Ordering::Release);
        });
        {
            let mut s = stats.lock();
            for (k, v) in drawn {
                s.insert(k, v);
            }
        }
        done.store(job.serial, Ordering::Release);
    }
}

#[cfg(test)]
#[path = "stage_render_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "stage_render_map_tests.rs"]
mod map_tests;

#[cfg(test)]
#[path = "stage_render_mapped_tests.rs"]
mod mapped_tests;
