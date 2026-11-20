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

use crate::gpu::{Gpu, LayerTexture, Slot};
use crate::render::{Mailbox, OutputGroup};
use crate::render3d::{Camera, FrameStats, Options, Renderer, Scene, Target};
use crate::show::OutputId;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The format the feeds and the views' targets are drawn in: display values,
/// as the outputs' pictures are.
pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// A feed's longest side, at full size.
pub const FEED_SIDE: u32 = 1024;
/// A feed's longest side, at the least (a quarter-size preview).
pub const FEED_SIDE_LEAST: u32 = 256;
/// A feed not asked for in this long is let go.
const FEED_KEPT: Duration = Duration::from_secs(10);

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
    /// Its key for the stats (`StageThread::stats`).
    pub key: u64,
    pub target: Arc<LayerTexture>,
    pub scene: Arc<Scene>,
    pub camera: Camera,
    pub options: Options,
}

#[derive(Clone, Default)]
pub struct StageJob {
    pub feeds: Vec<FeedJob>,
    pub views: Vec<ViewJob>,
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
        let mut feeds = newer.feeds;
        for f in self.feeds {
            if !feeds.iter().any(|n| n.output == f.output) {
                feeds.push(f);
            }
        }
        StageJob {
            feeds,
            views,
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
}

pub struct StageThread {
    mailbox: Arc<Mailbox<Work>>,
    handle: JoinHandle<()>,
    done: Arc<AtomicU64>,
    stats: Arc<parking_lot::Mutex<HashMap<u64, FrameStats>>>,
}

impl StageThread {
    pub fn spawn(gpu: Gpu) -> std::io::Result<StageThread> {
        let mailbox: Arc<Mailbox<Work>> = Arc::new(Mailbox::new());
        let mb = mailbox.clone();
        let done = Arc::new(AtomicU64::new(0));
        let d = done.clone();
        let stats: Arc<parking_lot::Mutex<HashMap<u64, FrameStats>>> = Arc::default();
        let s = stats.clone();
        let handle = std::thread::Builder::new()
            .name("stage-render".into())
            .spawn(move || {
                let _said = crate::crash::OnPanic(|| {
                    log::warn!("the 3D stage's drawing thread stopped unexpectedly");
                });
                stage_loop(gpu, mb, d, s)
            })?;
        Ok(StageThread {
            mailbox,
            handle,
            done,
            stats,
        })
    }

    /// Draw `job` (merged with one not yet taken up).
    pub fn submit(&self, job: StageJob) {
        self.mailbox
            .put(Work::Job(Box::new(job)), |old, new| match (old, new) {
                (Work::Quit, _) | (_, Work::Quit) => Work::Quit,
                (Work::Job(a), Work::Job(b)) => Work::Job(Box::new(a.merged(*b))),
            });
    }

    /// The serial of the last job drawn.
    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Acquire)
    }

    /// What the last drawing of view `key` cost.
    pub fn stats(&self, key: u64) -> Option<FrameStats> {
        self.stats.lock().get(&key).copied()
    }

    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
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

/// What the thread keeps between jobs: the renderer (its pipelines, meshes
/// and pictures) and the feeds' textures.
pub struct StageDrawer {
    renderer: Renderer,
    compositor: wgpu::RenderPipeline,
    feeds: HashMap<OutputId, Feed>,
}

impl StageDrawer {
    pub fn new(gpu: &Gpu) -> StageDrawer {
        StageDrawer {
            renderer: Renderer::new(gpu),
            compositor: gpu.compositor.pipeline(&gpu.device, FORMAT),
            feeds: HashMap::new(),
        }
    }

    /// Draw the feeds, then each view; the stats of each view by its key.
    pub fn draw(&mut self, gpu: &Gpu, job: &StageJob) -> Vec<(u64, FrameStats)> {
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
        for v in &job.views {
            let Some(view) = v.target.render_view() else {
                continue;
            };
            let target = Target {
                view: &view,
                format: FORMAT,
                size: (v.target.width, v.target.height),
            };
            let feeds = &self.feeds;
            let s = self.renderer.render(
                gpu,
                &target,
                &v.scene,
                &v.camera,
                &|o| feeds.get(&o).map(|f| &f.view),
                &v.options,
            );
            v.target.mark_drawn();
            stats.push((v.key, s));
        }
        stats
    }
}

fn stage_loop(
    gpu: Gpu,
    mailbox: Arc<Mailbox<Work>>,
    done: Arc<AtomicU64>,
    stats: Arc<parking_lot::Mutex<HashMap<u64, FrameStats>>>,
) {
    // Made at the first job: a show with no stage never builds the pipelines.
    let mut drawer: Option<StageDrawer> = None;
    loop {
        let job = match mailbox.take() {
            Work::Quit => break,
            Work::Job(j) => j,
        };
        let d = drawer.get_or_insert_with(|| StageDrawer::new(&gpu));
        let drawn = d.draw(&gpu, &job);
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
