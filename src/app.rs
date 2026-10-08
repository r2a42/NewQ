//! Window management: the control window (egui) and one window per output.
//!
//! The main thread runs the event loop, the engine and the UI. Each window
//! presents from its own render thread (see `render.rs`), so nothing here ever
//! blocks on vsync or on a window the OS is throttling.

use crate::engine::{Engine, LayerSource};
use crate::gpu::{self, Gpu, LayerTexture};
use crate::render::{ControlThread, EguiFrame, OutputLayer, OutputThread};
use crate::show::{DisplayRef, Fit, OutputConfig, OutputId, Placement, Show};
use crate::ui::{self, MonitorInfo, OutputRoute, PreviewOutput, UiState};
use std::collections::{HashMap, HashSet};
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

/// A fullscreen output window on its own display.
struct OutputWindow {
    window: Arc<Window>,
    thread: Option<OutputThread>,
    size: (u32, u32),
    monitor: usize,
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
    /// Test-pattern textures per output, with the output number they show.
    patterns: HashMap<OutputId, (Arc<LayerTexture>, usize)>,
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
            patterns: HashMap::new(),
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

    /// Decide which display each enabled output goes fullscreen on.
    fn route_outputs(&self) -> HashMap<OutputId, Option<usize>> {
        let control_monitor = self
            .control
            .as_ref()
            .and_then(|c| c.window.current_monitor())
            .and_then(|m| self.monitors.iter().position(|x| *x == m));
        let names: Vec<&str> = self.ui.monitors.iter().map(|m| m.name.as_str()).collect();
        route_outputs(&self.show.outputs, &names, control_monitor)
    }

    fn create_output(&mut self, event_loop: &ActiveEventLoop, cfg: &OutputConfig, monitor: usize) {
        let Some(gpu) = &self.gpu else { return };
        let Some(m) = self.monitors.get(monitor) else {
            return;
        };
        let attrs = Window::default_attributes()
            .with_title(format!("NewQ: {}", cfg.name))
            .with_inner_size(LogicalSize::new(960.0, 540.0))
            .with_fullscreen(Some(Fullscreen::Borderless(Some(m.clone()))));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.engine.log(format!("Could not open {}: {e}", cfg.name));
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
        self.outputs.insert(
            cfg.id,
            OutputWindow {
                window,
                thread: Some(thread),
                size,
                monitor,
                needs_redraw: true,
            },
        );
    }

    fn close_output(&mut self, id: OutputId) {
        if let Some(out) = self.outputs.remove(&id)
            && let (window, Some(handle)) = out.close()
        {
            window.set_visible(false);
            self.closing.push((window, handle));
        }
    }

    /// Open, move or close output windows to match where each output should be.
    fn reconcile_outputs(&mut self, event_loop: &ActiveEventLoop) {
        let routes = self.route_outputs();
        let stale: Vec<OutputId> = self
            .outputs
            .iter()
            .filter(|(id, w)| routes.get(id).copied().flatten() != Some(w.monitor))
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            self.close_output(id);
        }
        self.ui.routes.clear();
        let configs: Vec<OutputConfig> = self.show.outputs.clone();
        for cfg in &configs {
            let route = routes.get(&cfg.id).copied().flatten();
            match route {
                Some(m) => {
                    if !self.outputs.contains_key(&cfg.id) {
                        self.create_output(event_loop, cfg, m);
                    }
                    if let Some(out) = self.outputs.get(&cfg.id) {
                        let title = format!("NewQ: {}", cfg.name);
                        if out.window.title() != title {
                            out.window.set_title(&title);
                        }
                    }
                    let name = self
                        .ui
                        .monitors
                        .get(m)
                        .map(|x| x.name.clone())
                        .unwrap_or_default();
                    self.ui.routes.insert(cfg.id, OutputRoute::Display(name));
                }
                None if cfg.enabled => {
                    self.ui.routes.insert(cfg.id, OutputRoute::MainWindow);
                }
                None => {}
            }
        }
        // Outputs in the main window decode at the default (1080p) size.
        self.engine.output_sizes = self.outputs.iter().map(|(id, o)| (*id, o.size)).collect();
        // Release windows once their render thread has dropped the surface.
        self.closing.retain(|(_, h)| !h.is_finished());
    }

    /// Upload new frames and hand each output its current layer stack: to its
    /// render thread if it has a window, and to the UI for the live preview.
    fn update_outputs(&mut self, playing: bool) {
        let Some(gpu) = &self.gpu else { return };
        self.ui.previews.clear();
        let configs: Vec<(OutputId, String, bool, usize)> = self
            .show
            .outputs
            .iter()
            .enumerate()
            .filter(|(_, o)| o.enabled)
            .map(|(i, o)| (o.id, o.name.clone(), o.test_pattern, i + 1))
            .collect();
        for (id, name, want_pattern, number) in configs {
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

            let mut pattern_changed = false;
            if want_pattern {
                if self.patterns.get(&id).is_none_or(|(_, n)| *n != number) {
                    let (w, h) = (1280, 720);
                    let tex = LayerTexture::new_rgba(gpu, w, h);
                    tex.upload_rgba(&gpu.queue, &gpu::test_pattern(w, h, number));
                    self.patterns.insert(id, (Arc::new(tex), number));
                    pattern_changed = true;
                }
            } else if self.patterns.remove(&id).is_some() {
                pattern_changed = true;
            }
            if let Some((tex, _)) = self.patterns.get(&id) {
                layers.push(OutputLayer {
                    texture: tex.clone(),
                    opacity: 1.0,
                    fit: Fit::Stretch,
                });
            }

            if let Some(out) = self.outputs.get_mut(&id)
                && (playing || out.needs_redraw || pattern_changed)
            {
                out.needs_redraw = false;
                if let Some(t) = &out.thread {
                    t.submit(out.size, layers.clone());
                }
            }
            if pattern_changed && let Some(ctl) = &self.control {
                ctl.window.request_redraw();
            }
            self.ui.previews.push(PreviewOutput { id, name, layers });
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
        self.outputs.clear();
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

/// Decide which display each enabled output goes fullscreen on (`None` = it is
/// only shown in the main window): explicit displays first, then automatic
/// outputs take displays other than the main window's, one each.
fn route_outputs(
    outputs: &[OutputConfig],
    monitors: &[&str],
    control: Option<usize>,
) -> HashMap<OutputId, Option<usize>> {
    let enabled: Vec<&OutputConfig> = outputs.iter().filter(|o| o.enabled).collect();
    let mut routes = HashMap::new();
    let mut claimed = HashSet::new();
    for o in &enabled {
        if let Placement::Display(d) = &o.placement {
            let m = resolve_display(d, monitors, control);
            claimed.extend(m);
            routes.insert(o.id, m);
        }
    }
    for o in &enabled {
        match &o.placement {
            Placement::Display(_) => {}
            Placement::MainWindow => {
                routes.insert(o.id, None);
            }
            Placement::Auto => {
                let m = (0..monitors.len()).find(|i| Some(*i) != control && !claimed.contains(i));
                claimed.extend(m);
                routes.insert(o.id, m);
            }
        }
    }
    routes
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn auto_uses_main_window_with_one_display() {
        let r = route_outputs(&[output(1, Placement::Auto)], &["Laptop"], Some(0));
        assert_eq!(r[&OutputId(1)], None);
    }

    #[test]
    fn auto_goes_to_the_other_display() {
        let r = route_outputs(
            &[output(1, Placement::Auto)],
            &["Laptop", "Projector"],
            Some(0),
        );
        assert_eq!(r[&OutputId(1)], Some(1));
        // Main window on the second display: auto takes the first.
        let r = route_outputs(
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
        let r = route_outputs(&outs, &["Laptop", "Projector A", "Projector B"], Some(0));
        assert_eq!(r[&OutputId(2)], Some(1));
        assert_eq!(r[&OutputId(1)], Some(2));
        assert_eq!(r[&OutputId(3)], None);
        // Not enough displays for both: the auto output falls back to the main window.
        let r = route_outputs(&outs, &["Laptop", "Projector A"], Some(0));
        assert_eq!((r[&OutputId(2)], r[&OutputId(1)]), (Some(1), None));
    }

    #[test]
    fn unplugged_display_falls_back_to_main_window() {
        let r = route_outputs(&[output(1, display("Projector", 1))], &["Laptop"], Some(0));
        assert_eq!(r[&OutputId(1)], None);
    }

    #[test]
    fn renamed_display_matches_by_position_but_never_covers_the_main_window() {
        let outs = [output(1, display("Old projector", 1))];
        let r = route_outputs(&outs, &["Laptop", "Venue projector"], Some(0));
        assert_eq!(r[&OutputId(1)], Some(1));
        let r = route_outputs(&outs, &["Venue projector", "Laptop"], Some(1));
        assert_eq!(r[&OutputId(1)], None);
    }

    #[test]
    fn disabled_outputs_are_not_routed() {
        let mut o = output(1, Placement::Auto);
        o.enabled = false;
        let r = route_outputs(&[o], &["Laptop", "Projector"], Some(0));
        assert!(!r.contains_key(&OutputId(1)));
    }
}
