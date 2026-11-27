//! Render threads. Presenting to a window can block (vsync, or for a long time
//! when the OS throttles an occluded window), so every window presents from its
//! own thread. The main thread only posts the latest frame into a mailbox and
//! never waits, so timing, input and the other outputs are unaffected.

use crate::gpu::{BlendPass, DrawGroup, DrawLayer, Geometry, Gpu, LayerTexture, Look, Slot};
use crate::show::{ColorAdjust, EdgeBlend, Fit, LayerTransform};
use parking_lot::{Condvar, Mutex};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use winit::window::{Window, WindowId};

/// A single-slot, latest-wins handoff between threads.
pub(crate) struct Mailbox<T> {
    slot: Mutex<Option<T>>,
    cv: Condvar,
}

impl<T> Mailbox<T> {
    pub(crate) fn new() -> Self {
        Mailbox {
            slot: Mutex::new(None),
            cv: Condvar::new(),
        }
    }

    /// Post a value. If one is already waiting, `merge(old, new)` decides what remains.
    pub(crate) fn put(&self, value: T, merge: impl FnOnce(T, T) -> T) {
        let mut slot = self.slot.lock();
        *slot = Some(match slot.take() {
            Some(old) => merge(old, value),
            None => value,
        });
        self.cv.notify_one();
    }

    /// Wait up to `wait` for a value.
    pub(crate) fn take_timeout(&self, wait: std::time::Duration) -> Option<T> {
        let mut slot = self.slot.lock();
        if slot.is_none() {
            self.cv.wait_for(&mut slot, wait);
        }
        slot.take()
    }

    pub(crate) fn take(&self) -> T {
        let mut slot = self.slot.lock();
        loop {
            if let Some(v) = slot.take() {
                return v;
            }
            self.cv.wait(&mut slot);
        }
    }
}

/// Whether a window's surface is configured, as far as its render thread
/// knows. A display change (Windows rearranging its displays, a driver
/// reset) can make a configure fail; wgpu then panics on the next
/// `get_current_texture` of a surface that was never configured ("Surface is
/// not configured for presentation"). So a frame is skipped, and the
/// configure tried again a little later, until it takes.
struct Ready {
    /// Names the window in the log.
    what: String,
    ok: bool,
    /// It has been said that it isn't configured (so "again" can be said).
    said: bool,
    last_try: Option<Instant>,
}

/// How often a surface that would not configure is tried again.
const RETRY_CONFIGURE: Duration = Duration::from_millis(250);

impl Ready {
    fn new(what: impl Into<String>) -> Ready {
        Ready {
            what: what.into(),
            ok: false,
            said: false,
            last_try: None,
        }
    }

    /// Note whether a configure took, logging only a change.
    fn note(&mut self, ok: bool, why: &str) {
        if ok != self.ok {
            if ok {
                if self.said {
                    log::info!("{}: its surface is configured again", self.what);
                }
            } else {
                self.said = true;
                log::warn!(
                    "{}: its surface is not configured ({why}); skipping frames until it is",
                    self.what
                );
            }
        }
        self.ok = ok;
        self.last_try = Some(Instant::now());
    }

    fn configure(
        &mut self,
        gpu: &Gpu,
        surface: &wgpu::Surface,
        config: &wgpu::SurfaceConfiguration,
        why: &str,
    ) -> bool {
        let ok = gpu.try_configure(surface, config);
        self.note(ok, why);
        ok
    }

    /// Configured, or configured now (at most every `RETRY_CONFIGURE`).
    fn ensure(
        &mut self,
        gpu: &Gpu,
        surface: &wgpu::Surface,
        config: &wgpu::SurfaceConfiguration,
    ) -> bool {
        if self.ok {
            return true;
        }
        if self.last_try.is_some_and(|t| t.elapsed() < RETRY_CONFIGURE) {
            return false;
        }
        self.configure(gpu, surface, config, "it would not configure")
    }
}

/// The surface's next picture, if there is one to draw into: skipped (None)
/// while the device is lost or the surface can't be configured, never a
/// panic of the render thread.
fn acquire(
    gpu: &Gpu,
    surface: &wgpu::Surface,
    config: &wgpu::SurfaceConfiguration,
    ready: &mut Ready,
) -> Option<wgpu::SurfaceTexture> {
    // The main thread rebuilds everything once it has seen the loss.
    if gpu.lost.why().is_some() || !ready.ensure(gpu, surface, config) {
        return None;
    }
    let get = || crate::crash::catch_expected(|| surface.get_current_texture());
    match get() {
        Ok(
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f),
        ) => Some(f),
        Ok(wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost) => {
            if !ready.configure(gpu, surface, config, "the surface was outdated or lost") {
                return None;
            }
            match get() {
                Ok(
                    wgpu::CurrentSurfaceTexture::Success(f)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(f),
                ) => Some(f),
                Ok(_) => None,
                Err(()) => {
                    ready.note(false, "getting its picture failed");
                    None
                }
            }
        }
        Ok(wgpu::CurrentSurfaceTexture::Validation) => {
            ready.note(false, "getting its picture failed validation");
            None
        }
        // Timeout, occluded: try at the next frame.
        Ok(_) => None,
        Err(()) => {
            ready.note(false, "getting its picture failed");
            None
        }
    }
}

fn resize(
    gpu: &Gpu,
    surface: &wgpu::Surface,
    config: &mut wgpu::SurfaceConfiguration,
    size: (u32, u32),
    ready: &mut Ready,
) {
    let size = (size.0.max(1), size.1.max(1));
    if (config.width, config.height) != size {
        config.width = size.0;
        config.height = size.1;
        ready.configure(gpu, surface, config, "a resize would not configure");
    }
}

// ----- How the graphics are keeping up ------------------------------------------

/// For each output window, in thousandths, how many of its last frames took
/// clearly longer than the display's own rate (the graphics card falling
/// behind).
static LATE: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];

/// How fast each output window has been drawing, in tenths of a frame a
/// second, and when it last said (milliseconds since the program began).
static FPS: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];
static FPS_AT: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];

/// When the program began, as these counters count it.
fn start() -> Instant {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *START.get_or_init(Instant::now)
}

fn now_ms() -> u32 {
    start().elapsed().as_millis() as u32
}

/// The rate the window on display number `monitor` is drawing at, while it
/// is drawing (None when nothing has been drawn there lately).
pub fn output_fps(monitor: usize) -> Option<f32> {
    let i = monitor % FPS.len();
    let age = now_ms().wrapping_sub(FPS_AT[i].load(Ordering::Relaxed));
    (FPS_AT[i].load(Ordering::Relaxed) != 0 && u128::from(age) < crate::fps::FRESH.as_millis())
        .then(|| FPS[i].load(Ordering::Relaxed) as f32 / 10.0)
}

/// The output window in render slot `slot`: how many of its last frames
/// were late, in thousandths (0 when it isn't drawing).
pub fn output_late(slot: usize) -> u32 {
    LATE[slot % LATE.len()].load(Ordering::Relaxed)
}

/// The worst of the outputs: how much of the time frames are late, in
/// thousandths, while something is playing.
pub fn late_frames_permille() -> u32 {
    LATE.iter()
        .map(|l| l.load(Ordering::Relaxed))
        .max()
        .unwrap_or(0)
}

/// For each output window (by render slot, as `LATE`), every frame this run
/// that put a new picture on its display (`Shot`: a layer's texture written
/// again, a layer moved, faded or coloured, the screen's geometry or blend
/// changed), and of those the late ones: a "seen" miss, a refresh that showed
/// the old picture where a new one was due. A late frame that showed the same
/// picture as the one before it (a 25 fps film's three refreshes in four on a
/// 100 Hz display) is not among them: nobody could see it. The previews'
/// governor judges by these (`preview_pace`).
static NEW_PICTURES: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];
static SEEN_LATE: [AtomicU32; 8] = [const { AtomicU32::new(0) }; 8];

/// Each output window's new pictures and seen misses this run (`NEW_PICTURES`).
pub fn shown_totals() -> [crate::preview_pace::Shown; 8] {
    std::array::from_fn(|i| crate::preview_pace::Shown {
        pictures: NEW_PICTURES[i].load(Ordering::Relaxed),
        late: SEEN_LATE[i].load(Ordering::Relaxed),
    })
}

/// What an output window put on its display at a frame, as plain values (no
/// texture kept alive): two frames alike showed the same picture.
#[derive(Clone, Debug, PartialEq)]
struct Shot {
    size: (u32, u32),
    blend: EdgeBlend,
    groups: Vec<GroupShot>,
    /// The mapped pictures drawn for it (`mapped::MappedFrame`): a mapped
    /// display's layer is a target drawn into, whose stamp stays the same,
    /// so what is new is told by its canvases' layers and its map.
    mapped: Vec<MappedShot>,
}

type GroupShot = (Geometry, Vec<LayerShot>);

/// A mapped frame as plain values: its map, whether shares are on, its
/// canvases' layers and its moving screens.
#[derive(Clone, Debug, PartialEq)]
struct MappedShot {
    map: u64,
    shares: bool,
    feeds: Vec<(crate::show::OutputId, (u32, u32), Vec<GroupShot>)>,
    moving: Vec<crate::mapped::MovingShot>,
    /// The lens they are seen through.
    lens: [[f32; 4]; 4],
}

#[derive(Clone, Debug, PartialEq)]
struct LayerShot {
    stamp: u64,
    opacity: f32,
    fit: Fit,
    transform: LayerTransform,
    color: ColorAdjust,
    fx: crate::fx::VideoFx,
}

impl Shot {
    fn of(
        size: (u32, u32),
        groups: &[OutputGroup],
        blend: EdgeBlend,
        mapped: &[crate::mapped::MappedFrame],
    ) -> Shot {
        Shot {
            size,
            blend,
            groups: group_shots(groups),
            mapped: mapped
                .iter()
                .map(|f| MappedShot {
                    map: f.map.stamp(),
                    shares: f.shares,
                    feeds: f
                        .feeds
                        .iter()
                        .map(|c| (c.output, c.canvas, group_shots(&c.groups)))
                        .collect(),
                    moving: crate::mapped::moving_shots(&f.moving),
                    lens: f.lens,
                })
                .collect(),
        }
    }
}

fn group_shots(groups: &[OutputGroup]) -> Vec<GroupShot> {
    groups
        .iter()
        .map(|g| {
            let layers = g
                .layers
                .iter()
                .map(|l| LayerShot {
                    stamp: l.texture.stamp(),
                    opacity: l.opacity,
                    fit: l.fit,
                    transform: l.transform,
                    color: l.color,
                    fx: l.fx,
                })
                .collect();
            (g.geometry.clone(), layers)
        })
        .collect()
}

/// Whether a frame showing `now` puts a new picture up after `last` (the last
/// one presented; none: it does, as a frame that can't be told counts).
fn is_new_picture(last: Option<&Shot>, now: &Shot) -> bool {
    last != Some(now)
}

/// Judges whether a window's frames are late: a frame takes as long as the
/// display's refresh (when presenting waits for it) or less, so one taking
/// much longer is late. The yardstick is the display's refresh interval when
/// the system says what it is; otherwise the quickest frame seen, which an
/// early frame that did not have to wait for the display can set too low (every
/// later frame then reads as late). Only runs of frames close together
/// count; a still picture, drawn now and then, says nothing.
#[derive(Default)]
struct FrameTimer {
    /// The display's time between refreshes, if known.
    refresh: Option<Duration>,
    quickest: Option<Duration>,
    last_start: Option<Instant>,
    /// How long the last frame took.
    last_took: Duration,
    frames: u32,
    late: u32,
    /// The last frame was late (only judged against a known refresh, and
    /// while drawing continuously).
    was_late: bool,
}

impl FrameTimer {
    const WINDOW: u32 = 60;

    fn with_refresh(refresh: Option<Duration>) -> FrameTimer {
        FrameTimer {
            refresh,
            ..FrameTimer::default()
        }
    }

    /// A frame that began at `start` took `took`. Returns the share of late
    /// frames, in thousandths, each time a window of frames has been seen.
    fn frame(&mut self, start: Instant, took: Duration) -> Option<u32> {
        // The time nothing was asked of the window: from the end of the last
        // frame to the start of this one. (Not from start to start: a window
        // whose every frame waits 250 ms for the system is drawing all the
        // time, four frames a second, not showing a still picture.)
        let idle = self
            .last_start
            .map(|l| start.duration_since(l).saturating_sub(self.last_took));
        self.last_start = Some(start);
        self.last_took = took;
        self.was_late = false;
        if idle.is_none_or(|g| g > Duration::from_millis(150)) {
            // Not playing continuously: start counting again.
            self.frames = 0;
            self.late = 0;
            return idle.map(|_| 0);
        }
        let quickest = self.quickest.map_or(took, |q| q.min(took));
        self.quickest = Some(quickest);
        self.frames += 1;
        let yardstick = self.refresh.unwrap_or(quickest);
        if took > yardstick.mul_f32(1.6) + Duration::from_millis(2) {
            self.late += 1;
            self.was_late = self.refresh.is_some();
        }
        if self.frames >= Self::WINDOW {
            let share = self.late * 1000 / self.frames;
            self.frames = 0;
            self.late = 0;
            return Some(share);
        }
        None
    }
}

// ----- Output windows ---------------------------------------------------------

#[derive(Clone)]
pub struct OutputLayer {
    pub texture: Arc<LayerTexture>,
    pub opacity: f32,
    pub fit: Fit,
    pub transform: LayerTransform,
    pub color: ColorAdjust,
    pub fx: crate::fx::VideoFx,
}

/// One output's layers, placed on a screen.
#[derive(Clone)]
pub struct OutputGroup {
    pub layers: Vec<OutputLayer>,
    pub geometry: Geometry,
}

impl OutputGroup {
    pub(crate) fn draw(&self) -> DrawGroup<'_> {
        DrawGroup {
            layers: self
                .layers
                .iter()
                .map(|l| DrawLayer {
                    texture: &l.texture,
                    opacity: l.opacity,
                    fit: l.fit,
                    transform: l.transform,
                    color: l.color,
                    fx: l.fx,
                })
                .collect(),
            geometry: self.geometry.clone(),
        }
    }
}

enum OutputJob {
    Frame {
        size: (u32, u32),
        groups: Vec<OutputGroup>,
        blend: EdgeBlend,
        /// The mapped pictures to draw first (`mapped::Drawer`), which
        /// `groups` show as layers: empty for every display that isn't a
        /// mapped projector's, and then the loop draws as it always has.
        mapped: Vec<crate::mapped::MappedFrame>,
    },
    Quit,
}

pub struct OutputThread {
    mailbox: Arc<Mailbox<OutputJob>>,
    handle: JoinHandle<()>,
    /// The size of the last frame presented (`width << 32 | height`), 0
    /// before the first: a window is shown only once it has drawn at the
    /// size it is to be (`presented_size`).
    presented: Arc<std::sync::atomic::AtomicU64>,
}

impl OutputThread {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn spawn(
        gpu: Gpu,
        window: Arc<Window>,
        surface: wgpu::Surface<'static>,
        config: wgpu::SurfaceConfiguration,
        slot: Slot,
    ) -> std::io::Result<OutputThread> {
        let mailbox = Arc::new(Mailbox::new());
        let mb = mailbox.clone();
        let presented = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let flag = presented.clone();
        let handle = std::thread::Builder::new()
            .name("output-render".into())
            .spawn(move || output_loop(gpu, window, surface, config, slot, mb, flag))?;
        Ok(OutputThread {
            mailbox,
            handle,
            presented,
        })
    }

    /// The size of the last frame it presented, None before the first.
    pub fn presented_size(&self) -> Option<(u32, u32)> {
        let v = self.presented.load(Ordering::Acquire);
        (v != 0).then_some(((v >> 32) as u32, v as u32))
    }

    /// Whether the thread has ended by itself (it only should when asked to).
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    /// Draw `groups` (bottom first) on the whole screen, with its edges
    /// faded as `blend` says, after drawing the mapped pictures they show
    /// (none for a window with no mapped display).
    pub fn submit_mapped(
        &self,
        size: (u32, u32),
        groups: Vec<OutputGroup>,
        blend: EdgeBlend,
        mapped: Vec<crate::mapped::MappedFrame>,
    ) {
        self.mailbox.put(
            OutputJob::Frame {
                size,
                groups,
                blend,
                mapped,
            },
            |old, new| match old {
                OutputJob::Quit => OutputJob::Quit,
                _ => new,
            },
        );
    }

    /// Ask the thread to stop. Returns its handle so the caller can keep the
    /// window alive until the surface is gone.
    pub fn quit(self) -> JoinHandle<()> {
        self.mailbox.put(OutputJob::Quit, |_, quit| quit);
        self.handle
    }
}

fn output_loop(
    gpu: Gpu,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    mut config: wgpu::SurfaceConfiguration,
    slot: Slot,
    mailbox: Arc<Mailbox<OutputJob>>,
    presented: Arc<std::sync::atomic::AtomicU64>,
) {
    let pipeline = gpu.compositor.pipeline(&gpu.device, config.format);
    let fade = BlendPass::new(&gpu.device, config.format);
    // NEWQ_TRACE_OUTPUT=1 logs every composited frame's layer opacities.
    let trace = std::env::var_os("NEWQ_TRACE_OUTPUT").is_some();
    let refresh_of = |w: &Window| {
        w.current_monitor()
            .and_then(|m| m.refresh_rate_millihertz())
            .filter(|mhz| *mhz > 0)
            .map(|mhz| Duration::from_secs_f64(1000.0 / f64::from(mhz)))
    };
    let mut timer = FrameTimer::with_refresh(refresh_of(&window));
    let late = &LATE[slot_index(slot)];
    let mut fps = crate::fps::FpsMeter::default();
    let monitor = window
        .current_monitor()
        .and_then(|m| m.name())
        .unwrap_or_else(|| "an unknown display".into());
    // What the next log needs to tell a slow window's cause: the display, how
    // it presents, and whether the window covers the display.
    let modes = surface.get_capabilities(&gpu.adapter).present_modes;
    if let Some(want) = present_override(&modes)
        && want != config.present_mode
    {
        config.present_mode = want;
        gpu.configure(&surface, &config);
    }
    log::info!(
        "output {slot:?} on {monitor}: {}x{}, presents {:?} (offered {modes:?}), frame latency {}, display refresh {}, {}",
        config.width,
        config.height,
        config.present_mode,
        config.desired_maximum_frame_latency,
        refresh_of(&window).map_or("unknown".into(), |r| format!(
            "{:.2} Hz",
            1.0 / r.as_secs_f64()
        )),
        placement(&window)
    );
    let mut waits = WaitLog::default();
    // What the last frame presented showed (`Shot`).
    let mut last_shot: Option<Shot> = None;
    let mut ready = Ready::new(format!("output {slot:?} on {monitor}"));
    ready.configure(&gpu, &surface, &config, "it would not configure at start");
    // Made at the first mapped frame (a window that shows none never makes it).
    let mut drawer: Option<crate::mapped::Drawer> = None;
    let owner = crate::mapped::MapOwner::Window(match slot {
        Slot::Window(n) => n,
        _ => 0,
    });
    loop {
        let (size, groups, blend, mapped) = match mailbox.take() {
            OutputJob::Quit => break,
            OutputJob::Frame {
                size,
                groups,
                blend,
                mapped,
            } => (size, groups, blend, mapped),
        };
        let started = Instant::now();
        crate::crash::test_panic("render");
        resize(&gpu, &surface, &mut config, size, &mut ready);
        let asked = Instant::now();
        let Some(frame) = acquire(&gpu, &surface, &config, &mut ready) else {
            continue;
        };
        // How long the graphics system made this window wait for a picture
        // to draw into (`get_current_texture`: on Direct3D 12 the swap
        // chain's frame-latency wait, which ends when the display or the
        // compositor has taken an earlier frame). The system holds this up
        // while it rearranges displays; a window held up at every frame is
        // drawing a few frames a second.
        let waited = asked.elapsed();
        let refresh = timer.refresh.unwrap_or(Duration::from_micros(16_667));
        if let Some(line) = waits.note(Instant::now(), waited, refresh) {
            log::warn!("{line} on {slot:?} ({monitor}, {:?})", config.present_mode);
        }
        // Held up at most frames, on vsync: present without waiting for the
        // display (the compositor shows the newest) and say so.
        if waits.stuck()
            && config.present_mode != wgpu::PresentMode::Mailbox
            && modes.contains(&wgpu::PresentMode::Mailbox)
            && std::env::var_os("NEWQ_PRESENT").is_none()
        {
            log::warn!(
                "output {slot:?} on {monitor} waited for most of its frames: it presents without waiting for the display (Mailbox) from now on"
            );
            config.present_mode = wgpu::PresentMode::Mailbox;
            drop(frame);
            ready.configure(&gpu, &surface, &config, "Mailbox would not configure");
            waits = WaitLog::default();
            continue;
        }
        let view = frame.texture.create_view(&Default::default());
        // The mapped pictures first (nothing, with none), so their targets
        // are filled before the frame's picture is told.
        let maps = crate::mapped::draw_mapped(&mut drawer, &gpu, owner, &mapped);
        let shot = Shot::of(size, &groups, blend, &mapped);
        let fresh = is_new_picture(last_shot.as_ref(), &shot);
        let draw: Vec<DrawGroup> = groups.iter().map(OutputGroup::draw).collect();
        if trace {
            let ops: Vec<String> = draw
                .iter()
                .map(|g| {
                    let o: Vec<String> = g
                        .layers
                        .iter()
                        .map(|l| format!("{:.2}", l.opacity))
                        .collect();
                    o.join(", ")
                })
                .collect();
            log::info!("output composite [{}]", ops.join(" | "));
        }
        let faded = (!blend.is_off()).then(|| {
            fade.set(&gpu.queue, &blend);
            &fade
        });
        let cmd = gpu.composite(&pipeline, slot, &view, &draw, faded);
        if maps.is_empty() {
            gpu.submit([cmd]);
        } else {
            gpu.submit(maps.into_iter().chain([cmd]));
        }
        window.pre_present_notify();
        gpu.queue.present(frame);
        presented.store(
            (u64::from(config.width) << 32) | u64::from(config.height.max(1)),
            Ordering::Release,
        );
        if let Some(rate) = fps.tick(Instant::now()) {
            let i = slot_index(slot);
            FPS[i].store((rate * 10.0).round() as u32, Ordering::Relaxed);
            FPS_AT[i].store(now_ms().max(1), Ordering::Relaxed);
        }
        let share = timer.frame(started, started.elapsed());
        if fresh {
            let i = slot_index(slot);
            NEW_PICTURES[i].fetch_add(1, Ordering::Relaxed);
            if timer.was_late {
                SEEN_LATE[i].fetch_add(1, Ordering::Relaxed);
            }
            last_shot = Some(shot);
        }
        if let Some(share) = share {
            late.store(share, Ordering::Relaxed);
            // The window may have been moved to another display.
            timer.refresh = refresh_of(&window);
        }
    }
    late.store(0, Ordering::Relaxed);
}

/// `NEWQ_PRESENT=fifo|mailbox|immediate`: how output windows present, for
/// trying a display that draws slowly (only a mode the window offers).
fn present_override(offered: &[wgpu::PresentMode]) -> Option<wgpu::PresentMode> {
    let want = match std::env::var("NEWQ_PRESENT")
        .ok()?
        .to_ascii_lowercase()
        .as_str()
    {
        "fifo" => wgpu::PresentMode::Fifo,
        "mailbox" => wgpu::PresentMode::Mailbox,
        "immediate" => wgpu::PresentMode::Immediate,
        _ => return None,
    };
    offered.contains(&want).then_some(want)
}

/// Where the window is against its display, for the log.
fn placement(w: &Window) -> String {
    let inner = w.inner_size();
    let pos = w.outer_position().ok();
    let on = w.current_monitor();
    let covers = match (&on, pos) {
        (Some(m), Some(p)) => {
            let (mp, ms) = (m.position(), m.size());
            p.x <= mp.x && p.y <= mp.y && inner.width >= ms.width && inner.height >= ms.height
        }
        _ => false,
    };
    format!(
        "window {}x{} at {:?}, {}{}",
        inner.width,
        inner.height,
        pos.map(|p| (p.x, p.y)),
        if covers {
            "covers its display"
        } else {
            "does not cover its display"
        },
        if w.fullscreen().is_some() {
            ", full screen"
        } else {
            ""
        }
    )
}

/// A window's waits for a picture to draw into: a line for the log at most
/// once every `WaitLog::EVERY` (how many, the worst), and whether it has been
/// held up at most of its recent frames.
#[derive(Default)]
struct WaitLog {
    since: Option<Instant>,
    count: u32,
    worst: Duration,
    /// The last frames: whether each waited past three refreshes.
    recent: std::collections::VecDeque<bool>,
}

impl WaitLog {
    const EVERY: Duration = Duration::from_secs(5);
    /// A wait this long is worth saying.
    const SAY_OVER: Duration = Duration::from_millis(100);
    /// Frames looked at for `stuck`.
    const RECENT: usize = 30;

    /// Note a wait; the line to log, if one is due.
    fn note(&mut self, now: Instant, waited: Duration, refresh: Duration) -> Option<String> {
        self.recent.push_back(waited > refresh * 3);
        if self.recent.len() > Self::RECENT {
            self.recent.pop_front();
        }
        if waited > Self::SAY_OVER {
            self.count += 1;
            self.worst = self.worst.max(waited);
        }
        let due = self
            .since
            .is_none_or(|s| now.duration_since(s) >= Self::EVERY);
        if self.count == 0 || !due {
            return None;
        }
        let line = if self.count == 1 {
            format!(
                "waiting for a frame to draw took {} ms",
                self.worst.as_millis()
            )
        } else {
            format!(
                "waiting for a frame to draw took over {} ms {} times in {} s, the worst {} ms",
                Self::SAY_OVER.as_millis(),
                self.count,
                self.since
                    .map_or(0, |s| now.duration_since(s).as_secs().max(1)),
                self.worst.as_millis()
            )
        };
        self.since = Some(now);
        self.count = 0;
        self.worst = Duration::ZERO;
        Some(line)
    }

    /// Held up past three refreshes at most of its last frames.
    fn stuck(&self) -> bool {
        self.recent.len() >= Self::RECENT
            && self.recent.iter().filter(|w| **w).count() * 3 >= Self::RECENT * 2
    }
}

/// Which of the lateness counters a window's own slot uses.
fn slot_index(slot: Slot) -> usize {
    match slot {
        Slot::Window(n) => n as usize % LATE.len(),
        _ => 0,
    }
}

// ----- Streams out ------------------------------------------------------------

enum CaptureJob {
    /// The picture's layers on its own canvas, and the mapped pictures they
    /// show (drawn first; none for a display that isn't mapped).
    Picture {
        groups: Vec<OutputGroup>,
        mapped: Vec<crate::mapped::MappedFrame>,
    },
    Quit,
}

/// A display's picture drawn off screen at the rate of a stream and handed to
/// FFmpeg (see `stream.rs`). It draws what it was last given, again and
/// again, so a still picture is still a stream.
pub struct CaptureThread {
    mailbox: Arc<Mailbox<CaptureJob>>,
    errors: crossbeam_channel::Receiver<String>,
    handle: JoinHandle<()>,
    /// Ends the stream's FFmpeg if the thread is stuck writing to it when asked to stop.
    killer: Arc<parking_lot::Mutex<Option<crate::stream::Killer>>>,
    /// How long `stop` waits before it ends FFmpeg: a little longer than the
    /// time `Sink::finish` gives it, which a stop that is going well can use.
    patience: std::time::Duration,
}

impl CaptureThread {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn spawn(
        gpu: Gpu,
        slot: Slot,
        size: (u32, u32),
        fps: u32,
        ff: crate::media::ffmpeg::Ffmpeg,
        kind: crate::show::StreamKind,
    ) -> std::io::Result<CaptureThread> {
        Self::spawn_slow(gpu, slot, size, fps, ff, kind, std::time::Duration::ZERO)
    }

    /// `spawn`, for a recording that has sound: `audio` is a copy of what is played.
    pub fn spawn_with_audio(
        gpu: Gpu,
        slot: Slot,
        size: (u32, u32),
        fps: u32,
        ff: crate::media::ffmpeg::Ffmpeg,
        kind: crate::show::StreamKind,
        audio: Option<crate::media::audio::AudioTap>,
    ) -> std::io::Result<CaptureThread> {
        Self::spawn_inner(
            gpu,
            slot,
            size,
            fps,
            ff,
            kind,
            std::time::Duration::ZERO,
            audio,
        )
    }

    /// `spawn`, with each frame made to take `slow` longer to draw: for
    /// trying a slow graphics card.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn spawn_slow(
        gpu: Gpu,
        slot: Slot,
        size: (u32, u32),
        fps: u32,
        ff: crate::media::ffmpeg::Ffmpeg,
        kind: crate::show::StreamKind,
        slow: std::time::Duration,
    ) -> std::io::Result<CaptureThread> {
        Self::spawn_inner(gpu, slot, size, fps, ff, kind, slow, None)
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_inner(
        gpu: Gpu,
        slot: Slot,
        size: (u32, u32),
        fps: u32,
        ff: crate::media::ffmpeg::Ffmpeg,
        kind: crate::show::StreamKind,
        slow: std::time::Duration,
        audio: Option<crate::media::audio::AudioTap>,
    ) -> std::io::Result<CaptureThread> {
        let mailbox = Arc::new(Mailbox::new());
        let mb = mailbox.clone();
        let (err_tx, errors) = crossbeam_channel::unbounded();
        let patience = crate::stream::finish_patience(&kind) + std::time::Duration::from_secs(2);
        let killer: Arc<parking_lot::Mutex<Option<crate::stream::Killer>>> = Arc::default();
        let kill_slot = killer.clone();
        let handle = std::thread::Builder::new()
            .name("stream-render".into())
            .spawn(move || {
                // A panic in here is a stream that stopped, with a reason the
                // main loop can report and retry on (not a stream that is
                // started again every frame).
                let panicked = err_tx.clone();
                let _said = crate::crash::OnPanic(move || {
                    let _ = panicked.send("it stopped unexpectedly (a report was saved)".into());
                });
                // Started here: asking FFmpeg what it can encode and starting it
                // can take a while.
                match crate::stream::Sink::start_with_audio(&ff, size, fps, &kind, audio) {
                    Ok(sink) => {
                        *kill_slot.lock() = Some(sink.killer());
                        capture_loop(gpu, slot, size, fps, slow, sink, mb, err_tx)
                    }
                    Err(e) => {
                        let _ = err_tx.send(e);
                    }
                }
            })?;
        Ok(CaptureThread {
            mailbox,
            errors,
            handle,
            killer,
            patience,
        })
    }

    /// What to draw from now on (the picture's layers, on its own canvas).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn submit(&self, groups: Vec<OutputGroup>) {
        self.submit_mapped(groups, Vec::new());
    }

    /// `submit`, with the mapped pictures `groups` show drawn first.
    pub fn submit_mapped(&self, groups: Vec<OutputGroup>, mapped: Vec<crate::mapped::MappedFrame>) {
        self.mailbox.put(
            CaptureJob::Picture { groups, mapped },
            |old, new| match old {
                CaptureJob::Quit => CaptureJob::Quit,
                _ => new,
            },
        );
    }

    /// Why it stopped, if it has.
    pub fn failure(&self) -> Option<String> {
        self.errors.try_recv().ok()
    }

    pub fn is_running(&self) -> bool {
        !self.handle.is_finished()
    }

    /// Stop, letting FFmpeg finish what it has been given, and wait. A thread that
    /// does not stop (FFmpeg has stalled and the thread is stuck writing to it) has
    /// its FFmpeg killed after its patience, which fails the write; one that still
    /// does not stop after that is left behind rather than waited for.
    pub fn stop(self) {
        self.mailbox.put(CaptureJob::Quit, |_, quit| quit);
        let patience = self.patience;
        let wait = |limit: std::time::Duration| {
            let end = std::time::Instant::now() + limit;
            while !self.handle.is_finished() && std::time::Instant::now() < end {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            self.handle.is_finished()
        };
        if !wait(patience) {
            log::warn!("the stream did not stop in {patience:?}: ending its FFmpeg");
            if let Some(k) = self.killer.lock().as_ref() {
                k.kill();
            }
            if !wait(patience) {
                log::warn!("the stream's thread is still stuck: leaving it");
                return;
            }
        }
        let _ = self.handle.join();
    }
}

#[allow(clippy::too_many_arguments)]
fn capture_loop(
    gpu: Gpu,
    slot: Slot,
    size: (u32, u32),
    fps: u32,
    slow: std::time::Duration,
    mut sink: crate::stream::Sink,
    mailbox: Arc<Mailbox<CaptureJob>>,
    errors: crossbeam_channel::Sender<String>,
) {
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let pipeline = gpu.compositor.pipeline(&gpu.device, format);
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("stream"),
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    // Rows are copied out padded to a multiple of 256 bytes.
    let row = size.0 * 4;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("stream readback"),
        size: u64::from(padded * size.1),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let frame_time = std::time::Duration::from_secs_f64(1.0 / f64::from(fps.max(1)));
    let mut groups: Vec<OutputGroup> = Vec::new();
    let mut mapped: Vec<crate::mapped::MappedFrame> = Vec::new();
    let mut drawer: Option<crate::mapped::Drawer> = None;
    let owner = crate::mapped::MapOwner::Capture(match slot {
        Slot::Capture(n) => n,
        _ => 0,
    });
    let mut next = std::time::Instant::now();
    let mut frame = vec![0u8; (row * size.1) as usize];
    loop {
        // Pick up what was last submitted every time round, not only while
        // waiting for the next frame: when drawing takes as long as a frame,
        // there is no waiting, and a picture would never be taken up.
        match mailbox.take_timeout(std::time::Duration::ZERO) {
            Some(CaptureJob::Quit) => break,
            Some(CaptureJob::Picture {
                groups: g,
                mapped: m,
            }) => (groups, mapped) = (g, m),
            None => {}
        }
        let now = std::time::Instant::now();
        if now < next {
            match mailbox.take_timeout(next - now) {
                Some(CaptureJob::Quit) => break,
                Some(CaptureJob::Picture {
                    groups: g,
                    mapped: m,
                }) => (groups, mapped) = (g, m),
                None => {}
            }
            continue;
        }
        // Keep to the rate, but don't try to catch up after a long stall.
        next += frame_time;
        if next + frame_time * 3 < now {
            next = now + frame_time;
        }
        // A mapped display's picture first (no blend, no shares: section 4.5).
        let maps = crate::mapped::draw_mapped(&mut drawer, &gpu, owner, &mapped);
        let draw: Vec<DrawGroup> = groups.iter().map(OutputGroup::draw).collect();
        let cmd = gpu.composite(&pipeline, slot, &view, &draw, None);
        std::thread::sleep(slow);
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(size.1),
                },
            },
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
        gpu.submit(maps.into_iter().chain([cmd, encoder.finish()]));
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let polled = gpu.device.poll(wgpu::PollType::wait_indefinitely());
        if polled.is_err() || !matches!(rx.recv(), Ok(Ok(()))) {
            let _ = errors.send("the graphics device stopped".into());
            return;
        }
        {
            let Ok(data) = slice.get_mapped_range() else {
                let _ = errors.send("could not read the picture back".into());
                return;
            };
            for (y, out) in frame.chunks_mut(row as usize).enumerate() {
                let from = y * padded as usize;
                out.copy_from_slice(&data[from..from + row as usize]);
            }
        }
        readback.unmap();
        if let Err(e) = sink
            .write(&frame)
            .or_else(|e| sink.failed().map_or(Err(e), Err))
        {
            let _ = errors.send(e);
            return;
        }
    }
    sink.finish();
}

// ----- Control window (egui) ----------------------------------------------------

pub struct EguiFrame {
    pub size: (u32, u32),
    pub pixels_per_point: f32,
    pub jobs: Vec<egui::ClippedPrimitive>,
    pub textures: egui::TexturesDelta,
}

enum ControlJob {
    Frame(EguiFrame),
    Quit,
}

pub struct ControlThread {
    mailbox: Arc<Mailbox<ControlJob>>,
    handle: JoinHandle<()>,
    /// Set once the first frame has been presented.
    presented: Arc<std::sync::atomic::AtomicBool>,
}

impl ControlThread {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn spawn(
        gpu: Gpu,
        window: Arc<Window>,
        surface: wgpu::Surface<'static>,
        config: wgpu::SurfaceConfiguration,
    ) -> std::io::Result<ControlThread> {
        let mailbox = Arc::new(Mailbox::new());
        let mb = mailbox.clone();
        let presented = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = presented.clone();
        let handle = std::thread::Builder::new()
            .name("control-render".into())
            .spawn(move || control_loop(gpu, window, surface, config, mb, flag))?;
        Ok(ControlThread {
            mailbox,
            handle,
            presented,
        })
    }

    /// Whether the thread has ended by itself (it only should when asked to).
    pub fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    /// Whether a frame has reached the window yet. A window is kept hidden
    /// until then, so what the user first sees is the finished picture and
    /// not the system's blank window.
    pub fn has_presented(&self) -> bool {
        self.presented.load(std::sync::atomic::Ordering::Acquire)
    }

    pub fn submit(&self, frame: EguiFrame) {
        self.mailbox
            .put(ControlJob::Frame(frame), |old, new| match (old, new) {
                // A skipped frame's texture changes (e.g. font atlas) must still be applied.
                (ControlJob::Frame(mut old), ControlJob::Frame(mut new)) => {
                    let newer = std::mem::take(&mut new.textures);
                    old.textures.append(newer);
                    new.textures = std::mem::take(&mut old.textures);
                    ControlJob::Frame(new)
                }
                (ControlJob::Frame(mut old), quit) => {
                    old.textures.clear();
                    quit
                }
                (quit, ControlJob::Frame(mut new)) => {
                    new.textures.clear();
                    quit
                }
                (quit, _) => quit,
            });
    }

    pub fn quit(self) -> JoinHandle<()> {
        self.mailbox.put(ControlJob::Quit, |old, quit| {
            if let ControlJob::Frame(mut f) = old {
                f.textures.clear();
            }
            quit
        });
        self.handle
    }
}

fn control_loop(
    gpu: Gpu,
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    mut config: wgpu::SurfaceConfiguration,
    mailbox: Arc<Mailbox<ControlJob>>,
    presented: Arc<std::sync::atomic::AtomicBool>,
) {
    let mut renderer = egui_wgpu::Renderer::new(
        &gpu.device,
        config.format,
        egui_wgpu::RendererOptions::default(),
    );
    let compositor = gpu.compositor.pipeline(&gpu.device, config.format);
    let mut previews = PreviewCache::new(&gpu.device, config.format, compositor.clone());
    previews.window = Some(window.id());
    renderer.callback_resources.insert(previews);
    renderer
        .callback_resources
        .insert(PreviewPipeline(compositor));
    // The textures this renderer has been given in full. A partial update to
    // one it doesn't have (egui believes an earlier renderer holds it) would
    // panic; skipping it just leaves a stale patch until egui sends it whole.
    let mut known: std::collections::HashSet<egui::TextureId> = Default::default();
    let mut ready = Ready::new("a control window");
    ready.configure(&gpu, &surface, &config, "it would not configure at start");
    loop {
        let mut f = match mailbox.take() {
            ControlJob::Quit => break,
            ControlJob::Frame(f) => f,
        };
        for (id, deltas) in f.textures.set.drain() {
            for delta in &deltas {
                if delta.pos.is_some() && !known.contains(&id) {
                    log::warn!("skipping a partial update to a texture the renderer lacks");
                    continue;
                }
                renderer.update_texture(&gpu.device, &gpu.queue, id, delta);
                known.insert(id);
            }
        }
        let freed: Vec<egui::TextureId> = f.textures.free.drain().collect();
        resize(&gpu, &surface, &mut config, f.size, &mut ready);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [config.width, config.height],
            pixels_per_point: f.pixels_per_point,
        };
        if let Some(frame) = acquire(&gpu, &surface, &config, &mut ready) {
            let view = frame.texture.create_view(&Default::default());
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("egui"),
                });
            let mut cmds =
                renderer.update_buffers(&gpu.device, &gpu.queue, &mut encoder, &f.jobs, &screen);
            {
                let mut pass = encoder
                    .begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("egui"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    })
                    .forget_lifetime();
                renderer.render(&mut pass, &f.jobs, &screen);
            }
            cmds.push(encoder.finish());
            gpu.submit(cmds);
            window.pre_present_notify();
            gpu.queue.present(frame);
            presented.store(true, std::sync::atomic::Ordering::Release);
        }
        for id in &freed {
            renderer.free_texture(id);
            known.remove(id);
        }
    }
}

// ----- Outputs shown inside the main window ------------------------------------

/// The compositor pipeline for the control window's format, kept in the egui
/// renderer's callback resources.
struct PreviewPipeline(wgpu::RenderPipeline);

/// How a preview in the main window is drawn while it gives way to the outputs
/// (`preview_pace`): into a picture of its own, no more often than `every` and
/// at `scale` of the size it is shown at; between times the last picture is
/// shown again, so it never flickers or goes black. A preview without one draws
/// its layers straight into the window, every frame, as it always did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pace {
    pub every: Duration,
    pub scale: f32,
    /// Paused: keep showing the last picture (one with none yet draws one).
    pub hold: bool,
    /// What the preview shows (an output's id, a monitor's number): the
    /// last picture is kept only for the same, so a slot that now shows
    /// something else (another output chosen, multiview tiles renumbered)
    /// draws a fresh picture at once, paused or not.
    pub shows: u64,
}

impl Pace {
    /// How a compositor-drawn preview of `shows` follows the governor's
    /// budget: None at full rate (straight into the window, as always). The
    /// cadence is kept here on the render thread by `every`, so a frame of
    /// the main window that never reaches it (latest wins) loses no picture.
    pub fn of(budget: crate::preview_pace::PreviewBudget, shows: u64) -> Option<Pace> {
        (!budget.is_full()).then_some(Pace {
            every: budget.every,
            scale: budget.scale,
            hold: budget.paused,
            shows,
        })
    }

    /// Pictures are made in steps of this many pixels across, so a pane being
    /// resized doesn't make a new one every frame.
    const STEP: u32 = 16;
    /// The largest made, either way.
    const MOST: u32 = 4096;

    /// The size of the picture made for a preview `px` pixels big: `scale` of
    /// it, rounded up to `STEP` across, in the preview's shape.
    pub fn size_for(self, px: (f32, f32)) -> (u32, u32) {
        let (w, h) = (px.0.max(1.0), px.1.max(1.0));
        let want = (w * self.scale.clamp(0.05, 1.0)).ceil() as u32;
        let across = want.div_ceil(Self::STEP).max(1) * Self::STEP;
        let across = across.min(Self::MOST);
        let down = ((across as f32 * h / w).round() as u32).clamp(1, Self::MOST);
        (across, down)
    }
}

/// When a paced preview that was not drawn this frame is next due, by the
/// window it is in (None: not known). The app draws that window again then,
/// so the last picture of a run isn't left stale.
static PREVIEW_DUE: std::sync::Mutex<Vec<(Option<WindowId>, Instant)>> =
    std::sync::Mutex::new(Vec::new());

/// Note that a paced preview in `window` wants drawing at `at` (the earliest
/// wins).
fn note_preview_due(window: Option<WindowId>, at: Instant) {
    let mut due = PREVIEW_DUE.lock().unwrap_or_else(|e| e.into_inner());
    match due.iter_mut().find(|(w, _)| *w == window) {
        Some((_, t)) => *t = (*t).min(at),
        None => due.push((window, at)),
    }
}

/// When each window should be drawn again for a paced preview's next
/// picture, if one was held back (None: whichever has previews). Taken once.
pub fn take_preview_due() -> Vec<(Option<WindowId>, Instant)> {
    std::mem::take(&mut *PREVIEW_DUE.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The pictures paced previews are drawn into, by slot, with what draws them
/// and shows them; kept in the egui renderer's callback resources.
struct PreviewCache {
    /// The compositor, for the window's format.
    compositor: wgpu::RenderPipeline,
    /// Shows a picture over the preview's rect.
    show: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    format: wgpu::TextureFormat,
    pictures: HashMap<usize, Cached>,
    /// The window it draws in (for `note_preview_due`).
    window: Option<WindowId>,
}

struct Cached {
    size: (u32, u32),
    view: wgpu::TextureView,
    bind: wgpu::BindGroup,
    /// When it was last drawn.
    drawn: Instant,
    /// When it was last shown (one unused for a while is let go).
    used: Instant,
    /// What it is a picture of (`Pace::shows`), and its shape (across over
    /// down, of the preview it was drawn for).
    shows: u64,
    aspect: f32,
}

const SHOW_PICTURE_SHADER: &str = r#"
@group(0) @binding(0) var pic: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// One triangle over the viewport.
@vertex
fn vs(@builtin(vertex_index) i: u32) -> VOut {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    var o: VOut;
    o.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    o.uv = vec2<f32>(x, y);
    return o;
}

@fragment
fn fs(v: VOut) -> @location(0) vec4<f32> {
    return textureSample(pic, smp, v.uv);
}
"#;

impl PreviewCache {
    /// Let go of a picture not shown for this long.
    const KEEP_UNUSED: Duration = Duration::from_secs(10);

    fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        compositor: wgpu::RenderPipeline,
    ) -> PreviewCache {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("preview picture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("preview picture"),
            source: wgpu::ShaderSource::Wgsl(SHOW_PICTURE_SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("preview picture"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let show = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("preview picture"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("preview picture"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        PreviewCache {
            compositor,
            show,
            layout,
            sampler,
            format,
            pictures: HashMap::new(),
            window: None,
        }
    }
}

/// Draws outputs into a rectangle of the egui UI, using the same compositor
/// as a real output window: one output on its own, or a whole screen.
pub struct OutputPreview {
    pub groups: Vec<OutputGroup>,
    /// Which uniforms to use; each kind of preview needs its own.
    pub slot: Slot,
    /// Drawn less often and smaller while the outputs need the room (`Pace`);
    /// None: straight into the window every frame.
    pub pace: Option<Pace>,
}

impl OutputPreview {
    fn write_uniforms(&self, queue: &wgpu::Queue) {
        for g in &self.groups {
            for l in &g.layers {
                l.texture.write_uniform(
                    queue,
                    self.slot,
                    &g.geometry,
                    l.opacity,
                    &Look {
                        fit: l.fit,
                        transform: l.transform,
                        color: l.color,
                        fx: l.fx,
                    },
                );
            }
        }
    }

    fn draw_layers(&self, pass: &mut wgpu::RenderPass) {
        for l in self.groups.iter().flat_map(|g| &g.layers) {
            if l.opacity > 0.0 {
                l.texture.draw(self.slot, pass);
            }
        }
    }

    /// A paced preview `px` pixels big: draw its picture if it is due (or has
    /// none yet, or a new size), else note when it will be. Returns whether a
    /// picture is there to show.
    pub(crate) fn prepare_paced(
        &self,
        pace: Pace,
        px: (f32, f32),
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> bool {
        let Some(cache) = resources.get_mut::<PreviewCache>() else {
            return false;
        };
        let now = Instant::now();
        cache
            .pictures
            .retain(|_, c| now.saturating_duration_since(c.used) < PreviewCache::KEEP_UNUSED);
        let size = pace.size_for(px);
        let aspect = px.0.max(1.0) / px.1.max(1.0);
        let window = cache.window;
        let key = self.slot.index();
        if let Some(c) = cache.pictures.get_mut(&key) {
            c.used = now;
            // The last picture is only kept for the same thing in the same
            // shape: another output in this slot gets its own at once.
            if c.shows == pace.shows && (c.aspect / aspect - 1.0).abs() < 0.02 {
                // Paused: the last picture, whatever its size, and nothing new.
                if pace.hold {
                    return true;
                }
                // A little early counts: a window drawn at the preview's rate
                // would otherwise miss every other picture by a hair.
                let next = c.drawn + pace.every.saturating_sub(Duration::from_millis(2));
                if c.size == size && now < next {
                    note_preview_due(window, next);
                    return true;
                }
            }
        }
        let PreviewCache {
            compositor,
            layout,
            sampler,
            format,
            pictures,
            ..
        } = cache;
        let c = match pictures.get_mut(&key) {
            Some(c) if c.size == size => c,
            _ => {
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("preview picture"),
                    size: wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: *format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let view = texture.create_view(&Default::default());
                let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("preview picture"),
                    layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(sampler),
                        },
                    ],
                });
                pictures.insert(
                    key,
                    Cached {
                        size,
                        view,
                        bind,
                        drawn: now,
                        used: now,
                        shows: pace.shows,
                        aspect,
                    },
                );
                pictures.get_mut(&key).expect("just put in")
            }
        };
        self.write_uniforms(queue);
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("preview picture"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &c.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(compositor);
            self.draw_layers(&mut pass);
        }
        c.drawn = now;
        c.used = now;
        c.shows = pace.shows;
        c.aspect = aspect;
        true
    }

    /// Show a paced preview's last picture over the viewport set on `pass`;
    /// false when there is none (it was drawn straight, as unpaced).
    pub(crate) fn paint_paced(
        &self,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) -> bool {
        let Some(cache) = resources.get::<PreviewCache>() else {
            return false;
        };
        let Some(c) = cache.pictures.get(&self.slot.index()) else {
            return false;
        };
        pass.set_pipeline(&cache.show);
        pass.set_bind_group(0, &c.bind, &[]);
        pass.draw(0..3, 0..1);
        true
    }
}

impl egui_wgpu::CallbackTrait for OutputPreview {
    fn prepare(
        &self,
        _device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        _resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        self.write_uniforms(queue);
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(PreviewPipeline(pipeline)) = resources.get::<PreviewPipeline>() else {
            return;
        };
        pass.set_pipeline(pipeline);
        self.draw_layers(pass);
    }
}
#[cfg(test)]
mod tests {
    use super::Mailbox;

    #[test]
    fn mailbox_keeps_latest_and_merges() {
        let mb = Mailbox::new();
        mb.put(1, |_, n| n);
        mb.put(2, |o, n| o + n);
        assert_eq!(mb.take(), 3);
        mb.put(5, |_, n| n);
        assert_eq!(mb.take(), 5);
    }
}

#[cfg(test)]
mod late_tests {
    use super::*;

    fn run(timer: &mut FrameTimer, frames: &[(u64, u64)]) -> Option<u32> {
        // (ms between frame starts, ms the frame took)
        let mut t = Instant::now();
        let mut last = None;
        for &(gap, took) in frames {
            t += Duration::from_millis(gap);
            if let Some(share) = timer.frame(t, Duration::from_millis(took)) {
                last = Some(share);
            }
        }
        last
    }

    #[test]
    fn frames_at_the_display_rate_are_not_late_and_slow_ones_are() {
        let mut t = FrameTimer::default();
        // 60 fps, each taking a frame time to present: all on time.
        let steady: Vec<(u64, u64)> = vec![(17, 16); 130];
        assert_eq!(run(&mut t, &steady), Some(0));
        // Then every other frame takes twice as long: half are late.
        let mut slow = Vec::new();
        for _ in 0..70 {
            slow.push((17, 16));
            slow.push((34, 33));
        }
        let share = run(&mut t, &slow).unwrap();
        assert!((450..=550).contains(&share), "{share}");
    }

    #[test]
    fn a_window_that_waits_a_quarter_second_for_every_frame_is_all_late() {
        // 0.24.1 on a laptop's built-in screen: each frame waited 250 to 290 ms
        // for the system, back to back. That is four frames a second, every
        // one late, not a still picture.
        let mut t = FrameTimer::with_refresh(Some(Duration::from_micros(15_625)));
        let slow: Vec<(u64, u64)> = (0..130).map(|i| (270 + i % 20, 268 + i % 20)).collect();
        assert_eq!(run(&mut t, &slow), Some(1000));
    }

    #[test]
    fn waits_are_said_once_every_five_seconds_with_a_count_and_the_worst() {
        let mut w = WaitLog::default();
        let t = Instant::now();
        let refresh = Duration::from_micros(15_625);
        let mut lines = Vec::new();
        // Four frames a second for 12 s, each waiting about 260 ms.
        for i in 0..48u64 {
            let at = t + Duration::from_millis(i * 250);
            let waited = Duration::from_millis(250 + i % 40);
            if let Some(l) = w.note(at, waited, refresh) {
                lines.push(l);
            }
        }
        assert_eq!(lines.len(), 3, "{lines:?}");
        assert_eq!(lines[0], "waiting for a frame to draw took 250 ms");
        assert!(
            lines[1].starts_with("waiting for a frame to draw took over 100 ms 20 times in 5 s"),
            "{}",
            lines[1]
        );
        assert!(w.stuck(), "held up at every frame");
        // Quick frames say nothing and are not stuck.
        let mut q = WaitLog::default();
        for i in 0..100u64 {
            let at = t + Duration::from_millis(i * 16);
            assert!(q.note(at, Duration::from_millis(15), refresh).is_none());
        }
        assert!(!q.stuck());
    }

    #[test]
    fn each_late_frame_is_told_at_once_against_a_known_refresh() {
        let mut t = FrameTimer::with_refresh(Some(Duration::from_micros(16_667)));
        let start = Instant::now();
        let mut late = Vec::new();
        for (i, took) in [16, 16, 16, 40, 16, 16].into_iter().enumerate() {
            t.frame(
                start + Duration::from_millis(17 * i as u64 + if i > 3 { 24 } else { 0 }),
                Duration::from_millis(took),
            );
            late.push(t.was_late);
        }
        // The first frame starts the count; the slow one is told on its own frame.
        assert_eq!(late, [false, false, false, true, false, false]);
        // With no refresh known, single frames aren't told (the share still is).
        let mut t = FrameTimer::default();
        for i in 0..5u64 {
            t.frame(
                start + Duration::from_millis(17 * i),
                Duration::from_millis(if i == 3 { 40 } else { 16 }),
            );
            assert!(!t.was_late);
        }
    }

    #[test]
    fn a_still_picture_redrawn_now_and_then_says_nothing() {
        let mut t = FrameTimer::default();
        // Frames seconds apart never fill a window, and reset it.
        let rare: Vec<(u64, u64)> = vec![(2000, 30); 100];
        assert_eq!(run(&mut t, &rare), Some(0));
        assert_eq!(t.frames, 0);
    }

    #[test]
    fn a_quick_first_frame_does_not_make_every_later_frame_late() {
        // A frame of 1 ms that did not have to wait for the display (the very first
        // frame is not counted), then
        // 60 Hz frames of 16 ms for ever: with no refresh known this reads as
        // late all the time; knowing the refresh, none is late.
        let mut frames: Vec<(u64, u64)> = vec![(17, 16), (17, 1)];
        frames.extend(vec![(17, 16); 200]);
        assert_eq!(
            run(&mut FrameTimer::default(), &frames),
            Some(1000),
            "the old yardstick"
        );
        let mut t = FrameTimer::with_refresh(Some(Duration::from_micros(16_667)));
        assert_eq!(run(&mut t, &frames), Some(0));
        // And a graphics card that really can't keep up is still caught.
        let slow: Vec<(u64, u64)> = vec![(34, 33); 130];
        let mut t = FrameTimer::with_refresh(Some(Duration::from_micros(16_667)));
        assert_eq!(run(&mut t, &slow), Some(1000));
    }
}

#[cfg(test)]
mod pace_tests {
    use super::*;
    use crate::fx::VideoFx;

    #[test]
    fn a_paced_picture_is_made_in_steps_in_the_previews_shape() {
        let half = Pace {
            every: Duration::from_millis(66),
            scale: 0.5,
            hold: false,
            shows: 1,
        };
        assert_eq!(half.size_for((800.0, 450.0)), (400, 225));
        // A pane resized by a few pixels keeps the same picture.
        assert_eq!(half.size_for((790.0, 444.0)), (400, 225));
        assert_eq!(half.size_for((801.0, 450.0)).0, 416);
        let whole = Pace {
            every: Duration::from_millis(33),
            scale: 1.0,
            hold: false,
            shows: 1,
        };
        assert_eq!(whole.size_for((1920.0, 1080.0)), (1920, 1080));
        // Nothing to draw in, or more than a picture may be: still a picture.
        assert_eq!(whole.size_for((0.0, -5.0)), (16, 16));
        let (w, h) = whole.size_for((20_000.0, 100.0));
        assert!(w <= 4096 && h >= 1, "{w}x{h}");
    }

    fn gpu() -> Option<Gpu> {
        let gpu = Gpu::headless();
        if gpu.is_none() && std::env::var_os("NEWQ_REQUIRE_GPU").is_some() {
            panic!("NEWQ_REQUIRE_GPU is set but there is no GPU adapter");
        }
        gpu
    }

    const SIZE: (u32, u32) = (64, 36);
    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

    fn solid(gpu: &Gpu, px: [u8; 4]) -> Arc<LayerTexture> {
        let t = LayerTexture::new_rgba(gpu, 2, 2);
        t.upload_rgba(&gpu.queue, &px.repeat(4));
        Arc::new(t)
    }

    /// One layer of `tex` filling a picture of `SIZE`, placed by `warp`.
    fn group(tex: &Arc<LayerTexture>, warp: crate::gpu::Homography) -> OutputGroup {
        OutputGroup {
            layers: vec![OutputLayer {
                texture: tex.clone(),
                opacity: 1.0,
                fit: Fit::Stretch,
                transform: LayerTransform::default(),
                color: ColorAdjust::default(),
                fx: VideoFx::default(),
            }],
            geometry: Geometry {
                canvas: SIZE,
                warp,
                pin: warp,
                mesh: None,
                slice: Geometry::WHOLE,
            },
        }
    }

    #[test]
    fn a_frame_is_a_new_picture_only_when_something_in_it_changed() {
        let Some(gpu) = gpu() else { return };
        let tex = solid(&gpu, [255, 0, 0, 255]);
        let blend = EdgeBlend::default();
        let shot = |groups: &[OutputGroup]| Shot::of(SIZE, groups, blend, &[]);
        let first = shot(&[group(&tex, crate::gpu::IDENTITY)]);
        // The first frame can't be told: it counts.
        assert!(is_new_picture(None, &first));
        // The same layers, nothing written: the same picture (a missed refresh
        // here shows nothing different).
        let again = shot(&[group(&tex, crate::gpu::IDENTITY)]);
        assert!(!is_new_picture(Some(&first), &again));
        // A new frame written into the texture: new.
        tex.upload_rgba(&gpu.queue, &[0, 255, 0, 255].repeat(4));
        let written = shot(&[group(&tex, crate::gpu::IDENTITY)]);
        assert!(is_new_picture(Some(&again), &written));
        // A fade, a move, an effect: new, though the texture is the same.
        let mut faded = group(&tex, crate::gpu::IDENTITY);
        faded.layers[0].opacity = 0.5;
        assert!(is_new_picture(Some(&written), &shot(&[faded])));
        let mut moved = group(&tex, crate::gpu::IDENTITY);
        moved.layers[0].transform.x = 0.1;
        assert!(is_new_picture(Some(&written), &shot(&[moved])));
        let mut blurred = group(&tex, crate::gpu::IDENTITY);
        blurred.layers[0].fx.blur = 1.0;
        assert!(is_new_picture(Some(&written), &shot(&[blurred])));
        // Another texture with the same picture is another picture: each write
        // has its own stamp.
        let other = solid(&gpu, [0, 255, 0, 255]);
        assert_ne!(other.stamp(), tex.stamp());
        assert!(is_new_picture(
            Some(&written),
            &shot(&[group(&other, crate::gpu::IDENTITY)])
        ));
        // A layer gone: new.
        assert!(is_new_picture(Some(&written), &shot(&[])));
    }

    /// A target of `SIZE`, and what reading it back needs.
    fn target(gpu: &Gpu) -> (wgpu::Texture, wgpu::TextureView) {
        let t = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("target"),
            size: wgpu::Extent3d {
                width: SIZE.0,
                height: SIZE.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let v = t.create_view(&Default::default());
        (t, v)
    }

    /// Submit `cmds`, then read `tex` back (RGBA, row by row).
    fn read(gpu: &Gpu, mut enc: wgpu::CommandEncoder, tex: &wgpu::Texture) -> Vec<u8> {
        let row = SIZE.0 * 4; // 256: already aligned
        let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(row * SIZE.1),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_texture_to_buffer(
            tex.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(SIZE.1),
                },
            },
            wgpu::Extent3d {
                width: SIZE.0,
                height: SIZE.1,
                depth_or_array_layers: 1,
            },
        );
        gpu.submit([enc.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map"));
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        slice.get_mapped_range().expect("mapped").to_vec()
    }

    fn encoder(gpu: &Gpu) -> wgpu::CommandEncoder {
        gpu.device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None })
    }

    /// A frame of the main window with `p` in it, as the control window draws
    /// one (every callback's prepare on the frame's encoder, then the pass);
    /// the middle pixel of what it showed.
    fn preview_frame(
        gpu: &Gpu,
        res: &mut egui_wgpu::CallbackResources,
        p: &OutputPreview,
    ) -> [u8; 4] {
        let (tex, view) = target(gpu);
        let mut enc = encoder(gpu);
        let px = (SIZE.0 as f32, SIZE.1 as f32);
        let paced = p
            .pace
            .is_some_and(|pace| p.prepare_paced(pace, px, &gpu.device, &gpu.queue, &mut enc, res));
        if !paced {
            p.write_uniforms(&gpu.queue);
        }
        {
            let mut pass = enc
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("window"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            if !(paced && p.paint_paced(&mut pass, res)) {
                let PreviewPipeline(pipeline) = res.get::<PreviewPipeline>().unwrap();
                pass.set_pipeline(pipeline);
                p.draw_layers(&mut pass);
            }
        }
        let px = read(gpu, enc, &tex);
        let i = ((SIZE.1 / 2 * SIZE.0 + SIZE.0 / 2) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    /// What an output window of `SIZE` shows of `groups`, the whole picture.
    fn output_frame(gpu: &Gpu, groups: &[OutputGroup]) -> Vec<u8> {
        let (tex, view) = target(gpu);
        let pipeline = gpu.compositor.pipeline(&gpu.device, FORMAT);
        let draw: Vec<DrawGroup> = groups.iter().map(OutputGroup::draw).collect();
        let cmd = gpu.composite(&pipeline, Slot::Window(0), &view, &draw, None);
        gpu.submit([cmd]);
        read(gpu, encoder(gpu), &tex)
    }

    #[test]
    fn a_paced_preview_shows_its_last_picture_between_times_and_never_black() {
        let Some(gpu) = gpu() else {
            eprintln!("no GPU adapter here; skipping the paced preview test");
            return;
        };
        let compositor = gpu.compositor.pipeline(&gpu.device, FORMAT);
        let mut res = egui_wgpu::CallbackResources::default();
        res.insert(PreviewCache::new(&gpu.device, FORMAT, compositor.clone()));
        res.insert(PreviewPipeline(compositor));
        let red = solid(&gpu, [255, 0, 0, 255]);
        let blue = solid(&gpu, [0, 0, 255, 255]);
        let id = crate::gpu::IDENTITY;
        let slow = Some(Pace {
            every: Duration::from_secs(30),
            scale: 0.5,
            hold: false,
            shows: 1,
        });
        let preview = |tex: &Arc<LayerTexture>, pace| OutputPreview {
            groups: vec![group(tex, id)],
            slot: Slot::Multi(3),
            pace,
        };
        // Unpaced, as before: what is there now, every frame.
        assert_eq!(
            preview_frame(&gpu, &mut res, &preview(&red, None)),
            [255, 0, 0, 255]
        );
        // Paced: the first frame draws (it has no picture yet), at half size.
        let _ = take_preview_due();
        assert_eq!(
            preview_frame(&gpu, &mut res, &preview(&red, slow)),
            [255, 0, 0, 255]
        );
        let made = res.get::<PreviewCache>().unwrap().pictures[&Slot::Multi(3).index()].size;
        assert_eq!(made, (32, 18));
        // The picture changes before it is due: the last one is shown, not black,
        // and the window is asked to draw again when it is due.
        assert_eq!(
            preview_frame(&gpu, &mut res, &preview(&blue, slow)),
            [255, 0, 0, 255]
        );
        let due = take_preview_due()
            .into_iter()
            .map(|(_, t)| t)
            .min()
            .expect("a picture held back says when");
        assert!(due > Instant::now() + Duration::from_secs(25), "{due:?}");
        // Once due (here: a pace with no wait), the new picture.
        let quick = Some(Pace {
            every: Duration::ZERO,
            scale: 0.5,
            hold: false,
            shows: 1,
        });
        assert_eq!(
            preview_frame(&gpu, &mut res, &preview(&blue, quick)),
            [0, 0, 255, 255]
        );
        // A new size is drawn at once, whatever the wait.
        let bigger = Some(Pace {
            every: Duration::from_secs(30),
            scale: 1.0,
            hold: false,
            shows: 1,
        });
        assert_eq!(
            preview_frame(&gpu, &mut res, &preview(&red, bigger)),
            [255, 0, 0, 255]
        );
        // Paused: the last picture of the same output stays, whatever comes
        // and whatever its size.
        let paused = crate::preview_pace::Level::Paused;
        let hold = Some(Pace {
            every: paused.every(),
            scale: paused.scale(),
            hold: true,
            shows: 1,
        });
        assert_eq!(
            preview_frame(&gpu, &mut res, &preview(&blue, hold)),
            [255, 0, 0, 255]
        );
        // And a preview with no picture yet draws one, so none is black.
        let fresh = OutputPreview {
            groups: vec![group(&blue, id)],
            slot: Slot::Multi(9),
            pace: hold,
        };
        assert_eq!(preview_frame(&gpu, &mut res, &fresh), [0, 0, 255, 255]);
        // The slot shows another output now (another chosen while paused, the
        // tiles renumbered): never the old one's picture under its name, but
        // one fresh picture, which is then held.
        let other = |pace: Option<Pace>| pace.map(|p| Pace { shows: 2, ..p });
        assert_eq!(
            preview_frame(&gpu, &mut res, &preview(&blue, other(hold))),
            [0, 0, 255, 255]
        );
        assert_eq!(
            preview_frame(&gpu, &mut res, &preview(&red, other(hold))),
            [0, 0, 255, 255],
            "held again"
        );
        // Not paused, the same: no waiting out the last output's time.
        assert_eq!(
            preview_frame(&gpu, &mut res, &preview(&red, slow)),
            [255, 0, 0, 255]
        );
        assert_eq!(
            preview_frame(&gpu, &mut res, &preview(&blue, slow)),
            [255, 0, 0, 255],
            "paced again"
        );
    }

    #[test]
    fn an_outputs_own_picture_is_the_same_whatever_the_preview_does() {
        let Some(gpu) = gpu() else {
            eprintln!("no GPU adapter here; skipping");
            return;
        };
        let compositor = gpu.compositor.pipeline(&gpu.device, FORMAT);
        let mut res = egui_wgpu::CallbackResources::default();
        res.insert(PreviewCache::new(&gpu.device, FORMAT, compositor.clone()));
        res.insert(PreviewPipeline(compositor));
        // A half-red, half-green picture, so a wrong placement would show.
        let tex = LayerTexture::new_rgba(&gpu, 2, 1);
        tex.upload_rgba(&gpu.queue, &[255, 0, 0, 255, 0, 255, 0, 255]);
        let tex = Arc::new(tex);
        let out = [group(&tex, crate::gpu::IDENTITY)];
        let before = output_frame(&gpu, &out);
        assert_eq!(&before[..4], &[255, 0, 0, 255]);
        // The same layer previewed at every step (other placements, smaller,
        // held), between the output's frames.
        let squeezed = crate::gpu::corner_pin(&[[0.2, 0.1], [0.7, 0.0], [0.9, 0.8], [0.0, 1.0]]);
        for level in [
            crate::preview_pace::Level::Full,
            crate::preview_pace::Level::Fps30,
            crate::preview_pace::Level::Half,
            crate::preview_pace::Level::Quarter,
            crate::preview_pace::Level::Paused,
        ] {
            let pace = level.fps().map(|_| Pace {
                every: level.every(),
                scale: level.scale(),
                hold: level == crate::preview_pace::Level::Paused,
                shows: 1,
            });
            for slot in [Slot::Preview, Slot::Multi(0), Slot::Multi(15)] {
                let p = OutputPreview {
                    groups: vec![group(&tex, squeezed)],
                    slot,
                    pace,
                };
                preview_frame(&gpu, &mut res, &p);
            }
            assert_eq!(output_frame(&gpu, &out), before, "{level:?}");
        }
    }

    /// A display that isn't mapped draws exactly what it drew before mapping:
    /// the window's frame with no mapped pictures is today's one composite,
    /// byte for byte, makes no drawer, and stays so while another window
    /// maps a canvas made of the very same layers in the same submit.
    #[test]
    fn an_unmapped_display_draws_exactly_what_it_drew_before() {
        use crate::mapped::gpu_tests as m;
        let Some(gpu) = gpu() else {
            eprintln!("no GPU adapter here; skipping");
            return;
        };
        let a = m::layer(&gpu, &m::picture(30));
        let b = m::layer(&gpu, &m::picture(220));
        // Two layers, one faded and moved, through a keystone and a mesh,
        // and a sub display over them: what a busy display sends.
        let pin = crate::gpu::corner_pin(&[[0.05, 0.0], [0.95, 0.1], [1.0, 1.0], [0.0, 0.9]]);
        let mut mesh = crate::mesh::Mesh::grid(3, 3);
        *mesh.point_mut(1, 1) = [0.55, 0.45];
        let mut top = OutputLayer {
            opacity: 0.6,
            ..m::stretched(b.clone())
        };
        top.transform.x = 0.1;
        top.transform.rotation = 12.0;
        let groups = vec![
            OutputGroup {
                layers: vec![m::stretched(a.clone()), top],
                geometry: Geometry {
                    canvas: SIZE,
                    warp: crate::gpu::IDENTITY,
                    pin: crate::gpu::IDENTITY,
                    mesh: Some(Arc::new(mesh)),
                    slice: Geometry::WHOLE,
                },
            },
            group(&b, pin),
        ];
        let before = output_frame(&gpu, &groups);
        // Through the window's new path, with nothing mapped.
        let mut drawer = None;
        let now = m::window(&gpu, &mut drawer, 0, &groups, &[]);
        assert!(drawer.is_none(), "no drawer for a window that maps nothing");
        assert_eq!(now, before);
        // Another window maps a canvas of the same layers in the same submit.
        let identity = m::made_up(1, |x, y| Some((0, (x, y), 1.0, 1.0)));
        let mut canvas = m::feed(100, &a);
        canvas.groups[0].layers.push(OutputLayer {
            opacity: 0.3,
            ..m::stretched(b.clone())
        });
        canvas.groups[0].layers[0].transform.scale_x = -1.0;
        let f = m::frame(&gpu, &identity, vec![canvas]);
        let (t0, v0) = m::window_target(&gpu);
        let (t1, v1) = m::window_target(&gpu);
        let pipeline = gpu.compositor.pipeline(&gpu.device, FORMAT);
        let mut other = None;
        let mut cmds = crate::mapped::draw_mapped(
            &mut other,
            &gpu,
            crate::mapped::MapOwner::Window(1),
            std::slice::from_ref(&f),
        );
        let plain: Vec<DrawGroup> = groups.iter().map(OutputGroup::draw).collect();
        cmds.push(gpu.composite(&pipeline, Slot::Window(0), &v0, &plain, None));
        let mapped = [m::shown(&f, m::plain(SIZE))];
        let mapped: Vec<DrawGroup> = mapped.iter().map(OutputGroup::draw).collect();
        cmds.push(gpu.composite(&pipeline, Slot::Window(1), &v1, &mapped, None));
        gpu.submit(cmds);
        assert_eq!(m::read(&gpu, &t0, Vec::new()), before);
        let shown = m::read(&gpu, &t1, Vec::new());
        assert_ne!(shown, before, "the mapped window drew its own picture");
    }

    /// A mapped window's frames are new pictures when a canvas's layer is
    /// written, faded or moved, or the map changes, and not when nothing did:
    /// its own layer is a target whose stamp stays the same however often it
    /// is drawn into.
    #[test]
    fn a_mapped_frame_is_a_new_picture_when_its_canvas_or_map_changes() {
        use crate::mapped::gpu_tests as m;
        let Some(gpu) = gpu() else {
            eprintln!("no GPU adapter here; skipping");
            return;
        };
        let content = m::picture(10);
        let tex = m::layer(&gpu, &content);
        let identity = m::made_up(1, |x, y| Some((0, (x, y), 1.0, 1.0)));
        let f = m::frame(&gpu, &identity, vec![m::feed(100, &tex)]);
        let groups = [m::shown(&f, m::plain(SIZE))];
        let blend = EdgeBlend::default();
        let mut drawer = None;
        // Drawn as a window draws it: the mapped pictures, then the shot.
        let mut frame = |f: &crate::mapped::MappedFrame| {
            let _ = m::window(&gpu, &mut drawer, 0, &groups, std::slice::from_ref(f));
            Shot::of(SIZE, &groups, blend, std::slice::from_ref(f))
        };
        let first = frame(&f);
        let again = frame(&f);
        assert!(!is_new_picture(Some(&first), &again), "nothing changed");
        assert!(f.target.is_filled());
        // A new frame of the canvas's video: new, at every one.
        let mut last = again;
        for _ in 0..5 {
            tex.upload_rgba(&gpu.queue, &content);
            let now = frame(&f);
            assert!(is_new_picture(Some(&last), &now));
            last = now;
        }
        // A fade on the canvas: new.
        let mut faded = f.clone();
        faded.feeds[0].groups[0].layers[0].opacity = 0.5;
        assert!(is_new_picture(Some(&last), &frame(&faded)));
        // Another map (a re-bake): new.
        let mut remapped = f.clone();
        remapped.map = Arc::new(crate::mapped::LensMap::from_cpu(&gpu, &identity, 2));
        assert!(is_new_picture(Some(&last), &frame(&remapped)));
        // A frame without its mapped picture is another picture.
        assert!(is_new_picture(
            Some(&last),
            &Shot::of(SIZE, &groups, blend, &[])
        ));
        // A moving screen: the same place twice is not new; moved, it is.
        let mesh = crate::stage3d::scene::Mesh {
            positions: vec![[-1.0, -1.0, 0.0], [1.0, -1.0, 0.0], [1.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0]],
            indices: vec![0, 1, 2],
        };
        let mut moving = f.clone();
        moving.moving = vec![crate::mapped::MovingScreen {
            surface: crate::stage3d::SurfaceId(1),
            mesh_key: mesh.content_key(),
            mesh: Arc::new(mesh),
            matrix: crate::stage3d::scene::IDENTITY,
            feed: Some(0),
            region: [0.0, 0.0, 1.0, 1.0],
            front_only: true,
        }];
        let still = frame(&moving);
        assert!(
            !is_new_picture(Some(&still), &frame(&moving)),
            "standing still"
        );
        let mut on = moving.clone();
        on.moving[0].matrix[3][0] = 0.25;
        assert!(is_new_picture(Some(&still), &frame(&on)), "moved");
    }
}
