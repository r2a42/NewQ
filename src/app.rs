//! Window management: the control window (egui) and one window per output.
//!
//! The main thread runs the event loop, the engine and the UI. Each window
//! presents from its own render thread (see `render.rs`), so nothing here ever
//! blocks on vsync or on a window the OS is throttling.

use crate::engine::{Engine, LayerSource};
use crate::gpu::{self, Gpu, LayerTexture};
use crate::render::{ControlThread, EguiFrame, OutputLayer, OutputThread};
use crate::show::{Fit, OutputConfig, OutputId, Show};
use crate::ui::{self, MonitorInfo, UiState};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::keyboard::{Key, NamedKey};
use winit::monitor::MonitorHandle;
use winit::window::{Fullscreen, Window, WindowId};

/// The control UI only needs to animate progress bars while cues run.
const CONTROL_FPS_WHEN_PLAYING: f64 = 20.0;
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

/// What was last applied to an output window, so changes can be detected.
#[derive(PartialEq, Clone)]
struct Placement {
    monitor: Option<usize>,
    fullscreen: bool,
}

struct OutputWindow {
    window: Arc<Window>,
    thread: Option<OutputThread>,
    size: (u32, u32),
    placement: Placement,
    test_pattern: Option<(Arc<LayerTexture>, usize)>,
    needs_redraw: bool,
}

impl OutputWindow {
    fn close(mut self) -> (Arc<Window>, Option<JoinHandle<()>>) {
        let handle = self.thread.take().map(OutputThread::quit);
        (self.window.clone(), handle)
    }
}

impl Drop for OutputWindow {
    fn drop(&mut self) {
        if let Some(t) = self.thread.take() {
            drop(t.quit());
        }
    }
}

pub struct App {
    show: Show,
    engine: Engine,
    ui: UiState,
    egui_ctx: egui::Context,
    gpu: Option<Gpu>,
    control: Option<ControlWindow>,
    outputs: HashMap<OutputId, OutputWindow>,
    /// Windows whose render thread is shutting down; kept alive until it exits.
    closing: Vec<(Arc<Window>, JoinHandle<()>)>,
    /// GPU textures for playing layers, with the serial of the uploaded frame.
    textures: HashMap<u64, (Arc<LayerTexture>, u64)>,
    monitors: Vec<MonitorHandle>,
    last_monitor_scan: Instant,
    last_tick: Instant,
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
            outputs: HashMap::new(),
            closing: Vec::new(),
            textures: HashMap::new(),
            monitors: Vec::new(),
            last_monitor_scan: Instant::now(),
            last_tick: Instant::now(),
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

    /// Resolve an output's display choice to a monitor index: by name first,
    /// then by index (so shows survive monitors being re-ordered).
    fn resolve_monitor(&self, cfg: &OutputConfig) -> Option<usize> {
        let d = cfg.display.as_ref()?;
        let same_name: Vec<usize> = (0..self.ui.monitors.len())
            .filter(|&i| self.ui.monitors[i].name == d.name)
            .collect();
        match same_name.len() {
            1 => Some(same_name[0]),
            n if n > 1 && same_name.contains(&d.index) => Some(d.index),
            n if n > 1 => Some(same_name[0]),
            _ => (d.index < self.monitors.len()).then_some(d.index),
        }
    }

    fn create_output(&mut self, event_loop: &ActiveEventLoop, cfg: &OutputConfig) {
        let Some(gpu) = &self.gpu else { return };
        let monitor = self.resolve_monitor(cfg);
        let placement = Placement {
            monitor,
            fullscreen: cfg.fullscreen && monitor.is_some(),
        };
        let mut attrs = Window::default_attributes()
            .with_title(format!("NewQ: {}", cfg.name))
            .with_inner_size(LogicalSize::new(960.0, 540.0));
        if let Some(m) = monitor.and_then(|i| self.monitors.get(i)) {
            if placement.fullscreen {
                attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(Some(m.clone()))));
            } else {
                let p = m.position();
                attrs = attrs.with_position(PhysicalPosition::new(p.x + 80, p.y + 80));
            }
        }
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.engine.log(format!("Could not open {}: {e}", cfg.name));
                return;
            }
        };
        window.set_cursor_visible(!placement.fullscreen);
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
        self.outputs.insert(
            cfg.id,
            OutputWindow {
                window,
                thread: Some(thread),
                size,
                placement,
                test_pattern: None,
                needs_redraw: true,
            },
        );
    }

    /// Make the set of open output windows match the show's output configs.
    fn reconcile_outputs(&mut self, event_loop: &ActiveEventLoop) {
        let wanted: Vec<OutputConfig> = self
            .show
            .outputs
            .iter()
            .filter(|o| o.enabled)
            .cloned()
            .collect();
        let wanted_ids: HashSet<OutputId> = wanted.iter().map(|o| o.id).collect();
        let gone: Vec<OutputId> = self
            .outputs
            .keys()
            .filter(|id| !wanted_ids.contains(id))
            .copied()
            .collect();
        for id in gone {
            if let Some(out) = self.outputs.remove(&id)
                && let (window, Some(handle)) = out.close()
            {
                window.set_visible(false);
                self.closing.push((window, handle));
            }
        }
        for cfg in &wanted {
            let monitor = self.resolve_monitor(cfg);
            let placement = Placement {
                monitor,
                fullscreen: cfg.fullscreen && monitor.is_some(),
            };
            match self.outputs.get_mut(&cfg.id) {
                None => self.create_output(event_loop, cfg),
                Some(out) if out.placement != placement => {
                    let m = monitor.and_then(|i| self.monitors.get(i)).cloned();
                    if placement.fullscreen {
                        out.window
                            .set_fullscreen(Some(Fullscreen::Borderless(m.clone())));
                    } else {
                        out.window.set_fullscreen(None);
                        if let Some(m) = m {
                            let p = m.position();
                            out.window
                                .set_outer_position(PhysicalPosition::new(p.x + 80, p.y + 80));
                        }
                    }
                    out.window.set_cursor_visible(!placement.fullscreen);
                    out.placement = placement;
                    out.needs_redraw = true;
                }
                Some(out) => {
                    let title = format!("NewQ: {}", cfg.name);
                    if out.window.title() != title {
                        out.window.set_title(&title);
                    }
                }
            }
        }
        self.engine.output_sizes = self.outputs.iter().map(|(id, o)| (*id, o.size)).collect();
        // Release windows once their render thread has dropped the surface.
        self.closing.retain(|(_, h)| !h.is_finished());
    }

    /// Upload new frames and hand each output its current layer stack.
    fn update_outputs(&mut self, playing: bool) {
        let Some(gpu) = &self.gpu else { return };
        let ids: Vec<OutputId> = self.outputs.keys().copied().collect();
        for id in ids {
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
                });
            }

            let number = self
                .show
                .outputs
                .iter()
                .position(|o| o.id == id)
                .map_or(1, |i| i + 1);
            let want_pattern = self.show.output(id).is_some_and(|c| c.test_pattern);
            let Some(out) = self.outputs.get_mut(&id) else {
                continue;
            };
            if want_pattern {
                if out.test_pattern.as_ref().is_none_or(|(_, n)| *n != number) {
                    let (w, h) = (1280, 720);
                    let tex = LayerTexture::new_rgba(gpu, w, h);
                    tex.upload_rgba(&gpu.queue, &gpu::test_pattern(w, h, number));
                    out.test_pattern = Some((Arc::new(tex), number));
                    out.needs_redraw = true;
                }
            } else if out.test_pattern.take().is_some() {
                out.needs_redraw = true;
            }
            if let Some((tex, _)) = &out.test_pattern {
                layers.push(OutputLayer {
                    texture: tex.clone(),
                    opacity: 1.0,
                    fit: Fit::Stretch,
                });
            }

            if playing || out.needs_redraw {
                out.needs_redraw = false;
                if let Some(t) = &out.thread {
                    t.submit(out.size, layers);
                }
            }
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

        let Some(out_id) = self
            .outputs
            .iter()
            .find(|(_, o)| o.window.id() == id)
            .map(|(k, _)| *k)
        else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                if let Some(cfg) = self.show.outputs.iter_mut().find(|o| o.id == out_id) {
                    cfg.enabled = false;
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(out) = self.outputs.get_mut(&out_id) {
                    out.size = (size.width.max(1), size.height.max(1));
                    out.needs_redraw = true;
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(out) = self.outputs.get_mut(&out_id) {
                    out.needs_redraw = true;
                }
            }
            WindowEvent::KeyboardInput { event, .. } => self.handle_output_key(&event),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64();
        self.last_tick = now;
        if dt > 0.1 && self.engine.is_active() {
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
            let mut due = ctl.next_repaint.is_some_and(|t| t <= now);
            if playing {
                let frame = Duration::from_secs_f64(1.0 / CONTROL_FPS_WHEN_PLAYING);
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
        self.outputs.clear();
    }
}
