//! Window management: the control window (egui) and one window per output.
//!
//! The main thread runs the event loop, the engine and the UI. Each window
//! presents from its own render thread (see `render.rs`), so nothing here ever
//! blocks on vsync or on a window the OS is throttling.

use crate::engine::{Engine, LayerSource};
use crate::gpu::{self, Geometry, Gpu, Homography, LayerTexture};
use crate::render::{ControlThread, EguiFrame, OutputGroup, OutputLayer, OutputThread};
use crate::show::{
    Corners, DisplayRef, Fit, LayerTransform, OutputConfig, OutputId, Placement, Show, SubDisplay,
};
use crate::ui::{self, MonitorInfo, OutputRoute, PreviewOutput, UiState};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, NamedKey};
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, Window, WindowId};

/// The control UI only needs to animate progress bars while cues run...
const CONTROL_FPS_WHEN_PLAYING: f64 = 20.0;
/// ...but output previews in it must play smoothly.
const CONTROL_FPS_WITH_EMBEDDED: f64 = 60.0;
/// Engine update rate while something is playing. Output threads pick up the
/// latest state at their own refresh rate.
const TICK_WHILE_PLAYING: Duration = Duration::from_millis(4);

struct ControlWindow {
    window: Arc<Window>,
    state: egui_winit::State,
    thread: Option<ControlThread>,
    next_repaint: Option<Instant>,
    last_paint: Instant,
}

/// Where an output is drawn: an area of a physical display (the whole of it
/// unless the output is on a sub display).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Target {
    monitor: usize,
    area: Corners,
}

/// A fullscreen window on one physical display. It draws every output routed
/// to that display: one filling it, or several on sub displays.
struct ScreenWindow {
    window: Arc<Window>,
    thread: Option<OutputThread>,
    size: (u32, u32),
    monitor: MonitorHandle,
    /// Outputs drawn on it, bottom first.
    outputs: Vec<OutputId>,
    needs_redraw: bool,
    /// Outputs and warps last sent to the render thread.
    sent: Vec<(OutputId, Homography)>,
}

impl ScreenWindow {
    fn close(mut self) -> (Arc<Window>, Option<JoinHandle<()>>) {
        let handle = self.thread.take().map(OutputThread::quit);
        (self.window.clone(), handle)
    }
}

impl Drop for ScreenWindow {
    fn drop(&mut self) {
        if let Some(t) = self.thread.take() {
            drop(t.quit());
        }
    }
}

/// A test-pattern texture, with the output number and size it shows.
type Pattern = (Arc<LayerTexture>, usize, (u32, u32));

pub struct App {
    show: Show,
    engine: Engine,
    ui: UiState,
    egui_ctx: egui::Context,
    gpu: Option<Gpu>,
    control: Option<ControlWindow>,
    /// Fullscreen windows by monitor index.
    screens: HashMap<usize, ScreenWindow>,
    /// Where each output with a screen is drawn.
    targets: HashMap<OutputId, Target>,
    /// Windows whose render thread is shutting down; kept alive until it exits.
    closing: Vec<(Arc<Window>, JoinHandle<()>)>,
    /// GPU textures for playing layers, with the serial of the uploaded frame.
    textures: HashMap<u64, (Arc<LayerTexture>, u64)>,
    /// Test-pattern textures per output.
    patterns: HashMap<OutputId, Pattern>,
    monitors: Vec<MonitorHandle>,
    last_monitor_scan: Instant,
    last_tick: Instant,
    /// Whether something was playing at the previous tick.
    was_active: bool,
}

impl App {
    pub fn new(initial_show: Option<PathBuf>) -> App {
        let mut engine = Engine::new();
        let mut ui = UiState::default();
        let show = match initial_show {
            Some(path) => match Show::load(&path) {
                Ok(show) => {
                    ui.show_path = Some(path);
                    show
                }
                Err(e) => {
                    engine.log(format!("Could not open {}: {e:#}", path.display()));
                    Show::default()
                }
            },
            None => Show::default(),
        };
        engine.playhead = show.cues.first().map(|c| c.id);
        ui.selected = engine.playhead;
        App {
            show,
            engine,
            ui,
            egui_ctx: egui::Context::default(),
            gpu: None,
            control: None,
            screens: HashMap::new(),
            targets: HashMap::new(),
            closing: Vec::new(),
            textures: HashMap::new(),
            patterns: HashMap::new(),
            monitors: Vec::new(),
            last_monitor_scan: Instant::now(),
            last_tick: Instant::now(),
            was_active: false,
        }
    }

    fn scan_monitors(&mut self, event_loop: &ActiveEventLoop) {
        self.monitors = event_loop.available_monitors().collect();
        let primary = event_loop.primary_monitor();
        self.ui.monitors = self
            .monitors
            .iter()
            .enumerate()
            .map(|(i, m)| MonitorInfo {
                name: m.name().unwrap_or_else(|| format!("Display {}", i + 1)),
                size: (m.size().width, m.size().height),
                refresh_hz: m.refresh_rate_millihertz().map(|r| r as f32 / 1000.0),
                primary: primary.as_ref() == Some(m),
            })
            .collect();
        self.last_monitor_scan = Instant::now();
    }

    fn control_monitor(&self) -> Option<usize> {
        self.control
            .as_ref()
            .and_then(|c| c.window.current_monitor())
            .and_then(|m| self.monitors.iter().position(|x| *x == m))
    }

    /// Decide where each enabled output is drawn.
    fn route_outputs(&self) -> HashMap<OutputId, Option<Target>> {
        let names: Vec<&str> = self.ui.monitors.iter().map(|m| m.name.as_str()).collect();
        route_outputs(
            &self.show.outputs,
            &self.show.sub_displays,
            &names,
            self.control_monitor(),
        )
    }

    fn create_screen(&mut self, event_loop: &ActiveEventLoop, monitor: usize) {
        let Some(gpu) = &self.gpu else { return };
        let Some(m) = self.monitors.get(monitor) else {
            return;
        };
        let attrs = Window::default_attributes()
            .with_title("NewQ output")
            .with_inner_size(LogicalSize::new(960.0, 540.0))
            .with_fullscreen(Some(Fullscreen::Borderless(Some(m.clone()))));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.engine
                    .log(format!("Could not open an output window: {e}"));
                return;
            }
        };
        window.set_cursor_visible(false);
        let surface = match gpu.instance.create_surface(window.clone()) {
            Ok(s) => s,
            Err(e) => {
                self.engine.log(format!("Could not create surface: {e}"));
                return;
            }
        };
        let size = window.inner_size();
        let size = (size.width, size.height);
        let config = gpu.surface_config(&surface, size);
        surface.configure(&gpu.device, &config);
        let thread = OutputThread::spawn(gpu.clone(), window.clone(), surface, config);
        self.screens.insert(
            monitor,
            ScreenWindow {
                window,
                thread: Some(thread),
                size,
                monitor: m.clone(),
                outputs: Vec::new(),
                needs_redraw: true,
                sent: Vec::new(),
            },
        );
    }

    fn close_screen(&mut self, monitor: usize) {
        if let Some(screen) = self.screens.remove(&monitor)
            && let (window, Some(handle)) = screen.close()
        {
            window.set_visible(false);
            self.closing.push((window, handle));
        }
    }

    /// Open, update or close fullscreen windows to match where outputs should be.
    fn reconcile_outputs(&mut self, event_loop: &ActiveEventLoop) {
        let routes = self.route_outputs();
        // Outputs per display, bottom first: those filling the display, then
        // sub displays in list order.
        let mut per_screen = BTreeMap::<usize, Vec<_>>::new();
        self.targets.clear();
        for (oi, o) in self.show.outputs.iter().enumerate() {
            let Some(Some(t)) = routes.get(&o.id) else {
                continue;
            };
            let order = match o.placement {
                Placement::SubDisplay(v) => {
                    1 + self
                        .show
                        .sub_displays
                        .iter()
                        .position(|x| x.id == v)
                        .unwrap_or(0)
                }
                _ => 0,
            };
            per_screen
                .entry(t.monitor)
                .or_default()
                .push(((order, oi), o.id));
            self.targets.insert(o.id, *t);
        }
        // Close windows on displays with nothing left to show, or that changed.
        let stale: Vec<usize> = self
            .screens
            .iter()
            .filter(|(m, w)| {
                !per_screen.contains_key(m) || self.monitors.get(**m) != Some(&w.monitor)
            })
            .map(|(m, _)| *m)
            .collect();
        for m in stale {
            self.close_screen(m);
        }
        for (m, mut list) in per_screen {
            list.sort_by_key(|(k, _)| *k);
            let ids: Vec<OutputId> = list.into_iter().map(|(_, id)| id).collect();
            if !self.screens.contains_key(&m) {
                self.create_screen(event_loop, m);
            }
            if let Some(screen) = self.screens.get_mut(&m) {
                let names: Vec<&str> = ids
                    .iter()
                    .filter_map(|id| self.show.output(*id))
                    .map(|o| o.name.as_str())
                    .collect();
                let title = format!("NewQ: {}", names.join(", "));
                if screen.window.title() != title {
                    screen.window.set_title(&title);
                }
                if screen.outputs != ids {
                    screen.outputs = ids;
                    screen.needs_redraw = true;
                }
            }
        }

        self.ui.routes.clear();
        for cfg in self.show.outputs.iter().filter(|o| o.enabled) {
            let route = match self.targets.get(&cfg.id) {
                None => OutputRoute::MainWindow,
                Some(t) => {
                    let display = self
                        .ui
                        .monitors
                        .get(t.monitor)
                        .map(|x| x.name.clone())
                        .unwrap_or_default();
                    match cfg.placement {
                        Placement::SubDisplay(v) => OutputRoute::SubDisplay {
                            name: self
                                .show
                                .sub_display(v)
                                .map(|x| x.name.clone())
                                .unwrap_or_default(),
                            display,
                        },
                        _ => OutputRoute::Display(display),
                    }
                }
            };
            self.ui.routes.insert(cfg.id, route);
        }
        let names: Vec<&str> = self.ui.monitors.iter().map(|m| m.name.as_str()).collect();
        let control = self.control_monitor();
        self.ui.sub_monitors = self
            .show
            .sub_displays
            .iter()
            .filter_map(|v| Some((v.id, resolve_display(&v.parent, &names, control)?)))
            .collect();

        // Decode at the size each output is shown at. Outputs in the main
        // window decode at the default (1080p) size.
        self.engine.output_sizes = self
            .targets
            .iter()
            .filter_map(|(id, t)| Some((*id, t.area.size_on(self.screens.get(&t.monitor)?.size))))
            .collect();
        // Release windows once their render thread has dropped the surface.
        self.closing.retain(|(_, h)| !h.is_finished());
    }

    /// Upload new frames and hand each output's current layer stack to the
    /// window of the display it is on, and to the UI for the live previews.
    fn update_outputs(&mut self, playing: bool) {
        let Some(gpu) = &self.gpu else { return };
        self.ui.previews.clear();
        let configs: Vec<(OutputId, String, bool, usize, Corners)> = self
            .show
            .outputs
            .iter()
            .enumerate()
            .filter(|(_, o)| o.enabled)
            .map(|(i, o)| (o.id, o.name.clone(), o.test_pattern, i + 1, o.corners))
            .collect();
        let mut groups: HashMap<OutputId, OutputGroup> = HashMap::new();
        for (id, name, want_pattern, number, corners) in configs {
            // The output's canvas: its area of a display, or 1080p in the main window.
            let pin = gpu::corner_pin(&corners.0);
            let target = self.targets.get(&id).copied();
            let screen = target.and_then(|t| Some((t, self.screens.get(&t.monitor)?.size)));
            let (warp, canvas) = match screen {
                Some((t, size)) => (
                    gpu::compose(&gpu::corner_pin(&t.area.0), &pin),
                    t.area.size_on(size),
                ),
                None => (pin, (1920, 1080)),
            };

            let mut layers = Vec::new();
            for l in self.engine.layers(id) {
                let entry = match &l.source {
                    LayerSource::Video(p) => {
                        let Some(frame) = p.current() else { continue };
                        let entry = self.textures.entry(l.instance).or_insert_with(|| {
                            (
                                Arc::new(LayerTexture::new_yuv(gpu, p.width, p.height, p.matrix)),
                                0,
                            )
                        });
                        if entry.1 != p.serial {
                            entry.0.upload_yuv(&gpu.queue, &frame.data);
                            entry.1 = p.serial;
                        }
                        entry
                    }
                    LayerSource::Image(img) => {
                        self.textures.entry(l.instance).or_insert_with(|| {
                            let tex = LayerTexture::new_rgba(gpu, img.width, img.height);
                            tex.upload_rgba(&gpu.queue, &img.data);
                            (Arc::new(tex), 1)
                        })
                    }
                    LayerSource::Solid(color) => {
                        self.textures.entry(l.instance).or_insert_with(|| {
                            let tex = LayerTexture::new_rgba(gpu, 2, 2);
                            tex.upload_rgba(&gpu.queue, &color.repeat(4));
                            (Arc::new(tex), 1)
                        })
                    }
                };
                layers.push(OutputLayer {
                    texture: entry.0.clone(),
                    opacity: l.opacity,
                    fit: l.fit,
                    transform: l.transform,
                });
            }

            // The test pattern is drawn at the canvas's shape, so its circles
            // and grid stay true on a sub display of any size.
            let mut pattern_changed = false;
            if want_pattern {
                let scale = (1280.0 / canvas.0.max(canvas.1) as f32).min(1.0);
                // In 32 px steps, so dragging an area doesn't redraw it every frame.
                let step = |v: u32| ((v as f32 * scale / 32.0).round() as u32).max(1) * 32;
                let size = (step(canvas.0), step(canvas.1));
                if self
                    .patterns
                    .get(&id)
                    .is_none_or(|(_, n, s)| *n != number || *s != size)
                {
                    let tex = LayerTexture::new_rgba(gpu, size.0, size.1);
                    tex.upload_rgba(&gpu.queue, &gpu::test_pattern(size.0, size.1, number));
                    self.patterns.insert(id, (Arc::new(tex), number, size));
                    pattern_changed = true;
                }
            } else if self.patterns.remove(&id).is_some() {
                pattern_changed = true;
            }
            if let Some((tex, _, _)) = self.patterns.get(&id) {
                layers.push(OutputLayer {
                    texture: tex.clone(),
                    opacity: 1.0,
                    fit: Fit::Stretch,
                    transform: LayerTransform::default(),
                });
            }
            if pattern_changed {
                if let Some(t) = target
                    && let Some(s) = self.screens.get_mut(&t.monitor)
                {
                    s.needs_redraw = true;
                }
                if let Some(ctl) = &self.control {
                    ctl.window.request_redraw();
                }
            }

            groups.insert(
                id,
                OutputGroup {
                    layers: layers.clone(),
                    geometry: Geometry { canvas, warp, pin },
                },
            );
            self.ui.previews.push(PreviewOutput {
                id,
                name,
                layers,
                warp: pin,
                canvas,
            });
        }

        self.ui.screens.clear();
        for (m, screen) in &mut self.screens {
            let list: Vec<OutputGroup> = screen
                .outputs
                .iter()
                .filter_map(|id| groups.get(id).cloned())
                .collect();
            let sent: Vec<(OutputId, Homography)> = screen
                .outputs
                .iter()
                .filter_map(|id| Some((*id, groups.get(id)?.geometry.warp)))
                .collect();
            if playing || screen.needs_redraw || sent != screen.sent {
                screen.needs_redraw = false;
                screen.sent = sent;
                if let Some(t) = &screen.thread {
                    t.submit(screen.size, list.clone());
                }
            }
            self.ui.screens.insert(*m, (screen.size, list));
        }
    }

    fn render_control(&mut self, event_loop: &ActiveEventLoop) {
        let Some(ctl) = &mut self.control else { return };
        ctl.last_paint = Instant::now();

        let raw = ctl.state.take_egui_input(&ctl.window);
        let (show, engine, ui_state) = (&mut self.show, &mut self.engine, &mut self.ui);
        let mut full = self
            .egui_ctx
            .run_ui(raw, |ui| ui::draw(ui, show, engine, ui_state));
        ctl.state
            .handle_platform_output(&ctl.window, full.platform_output);
        ctl.next_repaint = full
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| Instant::now() + v.repaint_delay.min(Duration::from_secs(3600)));

        let jobs = self.egui_ctx.tessellate(full.shapes, full.pixels_per_point);
        let size = ctl.window.inner_size();
        let mut textures = std::mem::take(&mut full.textures_delta);
        match &ctl.thread {
            Some(t) => t.submit(EguiFrame {
                size: (size.width, size.height),
                pixels_per_point: full.pixels_per_point,
                jobs,
                textures,
            }),
            None => textures.clear(),
        }

        if self.ui.quit_requested {
            self.ui.quit_requested = false;
            if ui::confirm_discard(&self.show, &mut self.ui) {
                self.engine.stop_all(0.0);
                event_loop.exit();
            }
        }
    }

    fn handle_output_key(&mut self, event: &KeyEvent) {
        if event.state != ElementState::Pressed || event.repeat {
            return;
        }
        match &event.logical_key {
            Key::Named(NamedKey::Space) => self.engine.go(&self.show),
            Key::Named(NamedKey::Escape) => self.engine.panic(&self.show),
            _ => return,
        }
        self.ui.selected = self.engine.playhead;
        if let Some(ctl) = &self.control {
            ctl.window.request_redraw();
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.control.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(
                    Window::default_attributes()
                        .with_title("NewQ")
                        .with_inner_size(LogicalSize::new(1280.0, 760.0))
                        .with_min_inner_size(LogicalSize::new(800.0, 500.0))
                        .with_maximized(true),
                )
                .expect("create control window"),
        );
        // DX12 on Windows (Vulkan drivers there can stall acquiring frames for
        // occluded windows). `WGPU_BACKEND=vulkan` etc. overrides this.
        let mut desc = wgpu::InstanceDescriptor::new_with_display_handle(Box::new(
            event_loop.owned_display_handle(),
        ));
        if cfg!(windows) {
            desc.backends = wgpu::Backends::DX12;
        }
        let instance = wgpu::Instance::new(desc.with_env());
        let (gpu, surface) = Gpu::new(instance, window.clone()).expect("initialise GPU");
        let size = window.inner_size();
        let config = gpu.surface_config(&surface, (size.width, size.height));
        surface.configure(&gpu.device, &config);
        ui::setup_style(&self.egui_ctx);
        let state = egui_winit::State::new(
            self.egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(gpu.device.limits().max_texture_dimension_2d as usize),
        );
        let thread = ControlThread::spawn(gpu.clone(), window.clone(), surface, config);
        self.gpu = Some(gpu);
        self.control = Some(ControlWindow {
            window,
            state,
            thread: Some(thread),
            next_repaint: Some(Instant::now()),
            last_paint: Instant::now(),
        });
        self.scan_monitors(event_loop);
        self.reconcile_outputs(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let is_control = self.control.as_ref().is_some_and(|c| c.window.id() == id);
        if is_control {
            let ctl = self.control.as_mut().unwrap();
            let response = ctl.state.on_window_event(&ctl.window, &event);
            // egui-winit asks for a repaint after every RedrawRequested; honouring
            // that would redraw in a tight loop.
            if response.repaint && !matches!(event, WindowEvent::RedrawRequested) {
                ctl.window.request_redraw();
            }
            match event {
                WindowEvent::CloseRequested => {
                    self.ui.quit_requested = true;
                    ctl.window.request_redraw();
                }
                WindowEvent::Resized(_) => ctl.window.request_redraw(),
                WindowEvent::RedrawRequested => self.render_control(event_loop),
                _ => {}
            }
            return;
        }

        let Some(m) = self
            .screens
            .iter()
            .find(|(_, s)| s.window.id() == id)
            .map(|(m, _)| *m)
        else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                // Closing a display's window (e.g. Alt+F4) turns off what's on it.
                let on_it = self.screens[&m].outputs.clone();
                for cfg in &mut self.show.outputs {
                    if on_it.contains(&cfg.id) {
                        cfg.enabled = false;
                    }
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(s) = self.screens.get_mut(&m) {
                    s.size = (size.width.max(1), size.height.max(1));
                    s.needs_redraw = true;
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(s) = self.screens.get_mut(&m) {
                    s.needs_redraw = true;
                }
            }
            WindowEvent::KeyboardInput { event, .. } => self.handle_output_key(&event),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let mut dt = now.duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;
        // While nothing is playing the loop sleeps for up to 500 ms, so the
        // gap before the first tick of a new cue is idle time, not a stall,
        // and must not count towards the cue's pre-wait or fade.
        if !self.was_active {
            dt = 0.0;
        }
        self.was_active = self.engine.is_active();
        if dt > 0.1 && self.was_active {
            log::warn!("event loop stalled for {:.0} ms", dt * 1000.0);
        }

        if now.duration_since(self.last_monitor_scan) > Duration::from_secs(2) {
            self.scan_monitors(event_loop);
        }
        self.reconcile_outputs(event_loop);
        self.engine.tick(&self.show, dt);

        // Drop GPU textures of cues that are no longer playing.
        let live: HashSet<u64> = self.engine.active.iter().map(|a| a.instance).collect();
        let before = self.textures.len();
        self.textures.retain(|k, _| live.contains(k));
        let removed_layer = self.textures.len() != before;

        let playing = self.engine.is_active();
        // One extra frame after the last layer stops, so outputs go to black.
        self.update_outputs(playing || removed_layer);

        let mut next = now + Duration::from_millis(500);
        if let Some(ctl) = &mut self.control {
            let previews = !self.ui.previews.is_empty();
            let mut due = ctl.next_repaint.is_some_and(|t| t <= now) || (previews && removed_layer);
            if playing {
                let fps = if previews {
                    CONTROL_FPS_WITH_EMBEDDED
                } else {
                    CONTROL_FPS_WHEN_PLAYING
                };
                let frame = Duration::from_secs_f64(1.0 / fps);
                due |= now.duration_since(ctl.last_paint) >= frame;
                next = next.min(ctl.last_paint + frame);
            }
            if due {
                ctl.next_repaint = None;
                ctl.window.request_redraw();
            }
            if let Some(t) = ctl.next_repaint {
                next = next.min(t);
            }
        }
        if playing {
            next = next.min(now + TICK_WHILE_PLAYING);
        }
        event_loop.set_control_flow(ControlFlow::WaitUntil(next));
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.engine.stop_all(0.0);
        if let Some(t) = self.control.as_mut().and_then(|c| c.thread.take()) {
            drop(t.quit());
        }
        self.screens.clear();
    }
}

/// Resolve a saved display to a monitor index: by name first, then by position
/// (so a show moved to a venue with differently named projectors still works),
/// but never by position onto the main window's own display.
fn resolve_display(d: &DisplayRef, monitors: &[&str], control: Option<usize>) -> Option<usize> {
    let same_name: Vec<usize> = (0..monitors.len())
        .filter(|&i| monitors[i] == d.name)
        .collect();
    match same_name.len() {
        1 => Some(same_name[0]),
        n if n > 1 && same_name.contains(&d.index) => Some(d.index),
        n if n > 1 => Some(same_name[0]),
        _ => (d.index < monitors.len() && Some(d.index) != control).then_some(d.index),
    }
}

/// Decide where each enabled output is drawn (`None` = only in the main
/// window): explicit displays and sub displays first, then automatic
/// outputs take displays other than the main window's that nothing else is on.
fn route_outputs(
    outputs: &[OutputConfig],
    subs: &[SubDisplay],
    monitors: &[&str],
    control: Option<usize>,
) -> HashMap<OutputId, Option<Target>> {
    let enabled: Vec<&OutputConfig> = outputs.iter().filter(|o| o.enabled).collect();
    let mut routes = HashMap::new();
    let mut claimed = HashSet::new();
    for o in &enabled {
        let target = match &o.placement {
            Placement::Display(d) => resolve_display(d, monitors, control).map(|monitor| Target {
                monitor,
                area: Corners::default(),
            }),
            Placement::SubDisplay(v) => subs.iter().find(|x| x.id == *v).and_then(|v| {
                Some(Target {
                    monitor: resolve_display(&v.parent, monitors, control)?,
                    area: v.area,
                })
            }),
            _ => continue,
        };
        claimed.extend(target.map(|t| t.monitor));
        routes.insert(o.id, target);
    }
    for o in &enabled {
        match &o.placement {
            Placement::Display(_) | Placement::SubDisplay(_) => {}
            Placement::MainWindow => {
                routes.insert(o.id, None);
            }
            Placement::Auto => {
                let m = (0..monitors.len()).find(|i| Some(*i) != control && !claimed.contains(i));
                claimed.extend(m);
                routes.insert(
                    o.id,
                    m.map(|monitor| Target {
                        monitor,
                        area: Corners::default(),
                    }),
                );
            }
        }
    }
    routes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::SubDisplayId;

    fn output(id: u64, placement: Placement) -> OutputConfig {
        let mut o = OutputConfig::new(OutputId(id), format!("Output {id}"));
        o.placement = placement;
        o
    }

    fn display(name: &str, index: usize) -> Placement {
        Placement::Display(DisplayRef {
            name: name.into(),
            index,
        })
    }

    /// Routes as monitor indices.
    fn route(
        outputs: &[OutputConfig],
        monitors: &[&str],
        control: Option<usize>,
    ) -> HashMap<OutputId, Option<usize>> {
        route_outputs(outputs, &[], monitors, control)
            .into_iter()
            .map(|(id, t)| (id, t.map(|t| t.monitor)))
            .collect()
    }

    #[test]
    fn auto_uses_main_window_with_one_display() {
        let r = route(&[output(1, Placement::Auto)], &["Laptop"], Some(0));
        assert_eq!(r[&OutputId(1)], None);
    }

    #[test]
    fn auto_goes_to_the_other_display() {
        let r = route(
            &[output(1, Placement::Auto)],
            &["Laptop", "Projector"],
            Some(0),
        );
        assert_eq!(r[&OutputId(1)], Some(1));
        // Main window on the second display: auto takes the first.
        let r = route(
            &[output(1, Placement::Auto)],
            &["Laptop", "Projector"],
            Some(1),
        );
        assert_eq!(r[&OutputId(1)], Some(0));
    }

    #[test]
    fn explicit_displays_are_claimed_before_auto() {
        let outs = [
            output(1, Placement::Auto),
            output(2, display("Projector A", 1)),
            output(3, Placement::MainWindow),
        ];
        let r = route(&outs, &["Laptop", "Projector A", "Projector B"], Some(0));
        assert_eq!(r[&OutputId(2)], Some(1));
        assert_eq!(r[&OutputId(1)], Some(2));
        assert_eq!(r[&OutputId(3)], None);
        // Not enough displays for both: the auto output falls back to the main window.
        let r = route(&outs, &["Laptop", "Projector A"], Some(0));
        assert_eq!((r[&OutputId(2)], r[&OutputId(1)]), (Some(1), None));
    }

    #[test]
    fn unplugged_display_falls_back_to_main_window() {
        let r = route(&[output(1, display("Projector", 1))], &["Laptop"], Some(0));
        assert_eq!(r[&OutputId(1)], None);
    }

    #[test]
    fn renamed_display_matches_by_position_but_never_covers_the_main_window() {
        let outs = [output(1, display("Old projector", 1))];
        let r = route(&outs, &["Laptop", "Venue projector"], Some(0));
        assert_eq!(r[&OutputId(1)], Some(1));
        let r = route(&outs, &["Venue projector", "Laptop"], Some(1));
        assert_eq!(r[&OutputId(1)], None);
    }

    #[test]
    fn disabled_outputs_are_not_routed() {
        let mut o = output(1, Placement::Auto);
        o.enabled = false;
        let r = route(&[o], &["Laptop", "Projector"], Some(0));
        assert!(!r.contains_key(&OutputId(1)));
    }

    #[test]
    fn sub_displays_share_a_display_and_keep_auto_off_it() {
        let parent = DisplayRef {
            name: "Projector".into(),
            index: 1,
        };
        let left = SubDisplay {
            id: SubDisplayId(10),
            name: "Left".into(),
            parent: parent.clone(),
            area: Corners::rect(0.0, 0.0, 0.5, 1.0),
        };
        let right = SubDisplay {
            id: SubDisplayId(11),
            area: Corners::rect(0.5, 0.0, 0.5, 1.0),
            name: "Right".into(),
            ..left.clone()
        };
        let outs = [
            output(1, Placement::SubDisplay(SubDisplayId(10))),
            output(2, Placement::SubDisplay(SubDisplayId(11))),
            output(3, Placement::Auto),
            output(4, Placement::SubDisplay(SubDisplayId(99))),
        ];
        let subs = [left.clone(), right.clone()];
        let r = route_outputs(&outs, &subs, &["Laptop", "Projector"], Some(0));
        assert_eq!(
            r[&OutputId(1)],
            Some(Target {
                monitor: 1,
                area: left.area
            })
        );
        assert_eq!(
            r[&OutputId(2)],
            Some(Target {
                monitor: 1,
                area: right.area
            })
        );
        // The projector is taken by the sub displays, so automatic stays in the main window.
        assert_eq!(r[&OutputId(3)], None);
        // A deleted sub display falls back to the main window.
        assert_eq!(r[&OutputId(4)], None);
        // Projector unplugged: everything in the main window.
        let r = route_outputs(&outs, &subs, &["Laptop"], Some(0));
        assert!(r.values().all(|t| t.is_none()));
        // A sub display may be on the main window's own display when chosen by name.
        let on_laptop = SubDisplay {
            parent: DisplayRef {
                name: "Laptop".into(),
                index: 0,
            },
            ..left
        };
        let r = route_outputs(&outs[..1], &[on_laptop], &["Laptop"], Some(0));
        assert_eq!(r[&OutputId(1)].map(|t| t.monitor), Some(0));
    }
}
