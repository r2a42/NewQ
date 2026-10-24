//! Window management: the control window (egui) and one window per output.
//!
//! The main thread runs the event loop, the engine and the UI. Each window
//! presents from its own render thread (see `render.rs`), so nothing here ever
//! blocks on vsync or on a window the OS is throttling.

use crate::engine::{Engine, LayerSource};
use crate::gpu::{self, Geometry, Gpu, Homography, LayerTexture};
use crate::media::audio;
use crate::recovery;
use crate::remote::{self, Remote};
use crate::render::{ControlThread, EguiFrame, OutputGroup, OutputLayer, OutputThread};
use crate::show::{
    ColorAdjust, Corners, DisplayRef, Fit, LayerTransform, OutputConfig, OutputId, Placement,
    RemoteSettings, Show, SubDisplay,
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
    remote: Remote,
    /// The remote settings last seen, and when they last changed, so a port
    /// being dragged in the UI isn't rebound on every pixel.
    remote_seen: (RemoteSettings, Instant),
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
    /// The index in `monitors` of the primary display.
    primary: Option<usize>,
    /// Which display the main window was last logged as being on.
    control_logged: Option<Option<usize>>,
    /// When the app started, for how long to wait for the main window's display.
    started: Instant,
    /// The display the main window lives on, remembered while the displays
    /// are steady, so one being plugged in can't take the window away.
    control_home: Option<MonitorHandle>,
    /// When the set of displays last changed (not counting the first look).
    monitors_changed: Option<Instant>,
    /// When the main window was last put back on its display.
    last_homing: Instant,
    last_monitor_scan: Instant,
    last_tick: Instant,
    /// When the unsaved show was last copied to its recovery file.
    last_autosave: Instant,
    /// When the web remote's page was last given the show's state.
    last_web_state: Instant,
    /// A recovery file from a session that didn't end properly, to offer once.
    pending_recovery: Option<PathBuf>,
    /// Whether something was playing at the previous tick.
    was_active: bool,
    /// Keep every output in the main window, opening no output windows (a
    /// way in when the displays are what stops NewQ starting).
    main_window_only: bool,
}

impl App {
    /// `wake` is called from remote-control threads to rouse the event loop.
    pub fn new(
        initial_show: Option<PathBuf>,
        main_window_only: bool,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> App {
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
        let pending_recovery = recovery::pending(ui.show_path.as_deref());
        ui.check_media(&show);
        App {
            remote: Remote::new(wake),
            remote_seen: (RemoteSettings::default(), Instant::now()),
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
            primary: None,
            control_logged: None,
            started: Instant::now(),
            control_home: None,
            monitors_changed: None,
            last_homing: Instant::now(),
            last_monitor_scan: Instant::now(),
            last_tick: Instant::now(),
            last_autosave: Instant::now(),
            last_web_state: Instant::now(),
            pending_recovery,
            was_active: false,
            main_window_only,
        }
    }

    fn scan_monitors(&mut self, event_loop: &ActiveEventLoop) {
        let found: Vec<MonitorHandle> = event_loop.available_monitors().collect();
        if found != self.monitors {
            if !self.monitors.is_empty() {
                self.monitors_changed = Some(Instant::now());
            }
            for (i, m) in found.iter().enumerate() {
                log::info!(
                    "display {i}: {:?} {}x{} at {:?}, scale {}",
                    m.name(),
                    m.size().width,
                    m.size().height,
                    m.position(),
                    m.scale_factor()
                );
            }
        }
        self.monitors = found;
        let primary = event_loop.primary_monitor();
        self.primary = primary
            .as_ref()
            .and_then(|p| self.monitors.iter().position(|m| m == p));
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
        self.ui.midi_ports = remote::midi_inputs();
        self.ui.midi_out_ports = crate::output::midi_outputs();
        // Listing and opening audio devices can take a while; not during a show.
        if !self.engine.is_active() {
            self.ui.audio_devices = audio::output_devices();
            self.engine.audio.retry_failed();
        }
        self.last_monitor_scan = Instant::now();
    }

    /// Keep the OSC and MIDI listeners matching the show's settings, and act
    /// on what they have received.
    fn run_remote(&mut self, now: Instant) {
        if self.remote_seen.0 != self.show.remote {
            self.remote_seen = (self.show.remote.clone(), now);
        }
        let settled = now.duration_since(self.remote_seen.1) > Duration::from_millis(400);
        if settled {
            self.remote.sync(&self.show.remote);
        }
        self.ui.remote_status = self.remote.status.clone();
        if self.remote.web_running()
            && now.duration_since(self.last_web_state) >= Duration::from_millis(250)
        {
            self.last_web_state = now;
            self.remote
                .set_web_state(crate::web::snapshot_json(&self.show, &self.engine));
        }
        let commands = self.remote.drain();
        if commands.is_empty() {
            return;
        }
        for c in commands {
            log::info!("remote: {c:?}");
            ui::run_remote(c, &mut self.show, &mut self.engine, &mut self.ui);
        }
        if let Some(ctl) = &mut self.control {
            ctl.next_repaint = Some(now);
        }
    }

    /// The displays have just changed, and the main window may be moved about
    /// by the system for a moment.
    fn displays_settling(&self) -> bool {
        self.monitors_changed
            .is_some_and(|t| t.elapsed() < HOME_GRACE)
    }

    /// The display the main window is on; while the displays are changing,
    /// the one it lives on, wherever the system has put it for now.
    fn control_monitor(&self) -> Option<usize> {
        let home = self
            .control_home
            .clone()
            .filter(|_| self.displays_settling());
        home.or_else(|| {
            self.control
                .as_ref()
                .and_then(|c| c.window.current_monitor())
        })
        .and_then(|m| self.monitors.iter().position(|x| *x == m))
    }

    /// Keep the main window on the display it lives on when displays come
    /// and go. While steady, remember where it is; for a few seconds after a
    /// change, put it back if the system has moved it.
    fn keep_main_window_home(&mut self) {
        let Some(ctl) = &self.control else { return };
        let here = ctl.window.current_monitor();
        if !self.displays_settling() {
            if here.is_some() {
                self.control_home = here;
            }
            return;
        }
        let Some(home) = self.control_home.clone() else {
            return;
        };
        let at_home = here.as_ref() == Some(&home);
        if should_return_home(true, self.monitors.contains(&home), at_home)
            && self.last_homing.elapsed() > Duration::from_millis(500)
        {
            log::info!(
                "main window was moved off its display ({:?}); putting it back",
                here.as_ref().and_then(|m| m.name())
            );
            self.last_homing = Instant::now();
            ctl.window.set_minimized(false);
            ctl.window.set_outer_position(home.position());
            ctl.window.set_maximized(true);
            ctl.window.focus_window();
        }
    }

    /// Decide where each enabled output is drawn.
    fn route_outputs(&mut self) -> HashMap<OutputId, Option<Target>> {
        if self.main_window_only {
            return self.show.outputs.iter().map(|o| (o.id, None)).collect();
        }
        let names: Vec<&str> = self.ui.monitors.iter().map(|m| m.name.as_str()).collect();
        let found = self.control_monitor();
        if self.control_logged != Some(found) {
            log::info!(
                "main window is on display {}",
                found.map_or("?".to_string(), |m| m.to_string())
            );
            self.control_logged = Some(found);
        }
        let Some(control) = control_for_routing(found, self.primary, self.started.elapsed()) else {
            // Not placed yet: no output windows until it is, so none lands on
            // top of it.
            return self.show.outputs.iter().map(|o| (o.id, None)).collect();
        };
        route_outputs(&self.show.outputs, &self.show.sub_displays, &names, control)
    }

    fn create_screen(&mut self, event_loop: &ActiveEventLoop, monitor: usize) {
        let Some(gpu) = &self.gpu else { return };
        let Some(m) = self.monitors.get(monitor) else {
            return;
        };
        let mut attrs = Window::default_attributes()
            .with_title("NewQ output")
            .with_inner_size(LogicalSize::new(960.0, 540.0));
        if cfg!(target_os = "macos") {
            // Native fullscreen gets a Space of its own on that display, which
            // can take the main window out of sight. Cover the display
            // instead (below); the window just has to start on it.
            attrs = attrs.with_position(m.position());
        } else {
            attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(Some(m.clone()))));
        }
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.engine
                    .log(format!("Could not open an output window: {e}"));
                return;
            }
        };
        #[cfg(target_os = "macos")]
        if !winit::platform::macos::WindowExtMacOS::set_simple_fullscreen(&*window, true) {
            log::warn!("simple fullscreen refused on display {monitor}; using native fullscreen");
            window.set_fullscreen(Some(Fullscreen::Borderless(Some(m.clone()))));
        }
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
        log::info!(
            "output window on display {monitor} ({:?}): {}x{}, scale {}",
            m.name(),
            size.0,
            size.1,
            window.scale_factor()
        );
        let Some(config) = gpu.surface_config(&surface, size) else {
            self.engine.log(format!(
                "This graphics card can't draw to display {}: outputs on it are not shown",
                monitor + 1
            ));
            return;
        };
        surface.configure(&gpu.device, &config);
        let thread = OutputThread::spawn(gpu.clone(), window.clone(), surface, config);
        // A new fullscreen window takes the focus (and on a Mac, its own
        // Space); bring the controls back in front.
        if let Some(ctl) = &self.control {
            ctl.window.focus_window();
        }
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
            // Give the menu bar and Dock back before the window goes.
            #[cfg(target_os = "macos")]
            winit::platform::macos::WindowExtMacOS::set_simple_fullscreen(&*window, false);
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
                    color: l.color,
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
                    color: ColorAdjust::default(),
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

        if let Some(rec) = self.pending_recovery.take() {
            offer_recovery(&rec, &mut self.show, &mut self.engine, &mut self.ui);
        }

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

        // Keep a copy of unsaved changes, so a crash doesn't lose them.
        if self.last_autosave.elapsed() >= recovery::INTERVAL {
            self.last_autosave = Instant::now();
            let path = self.ui.show_path.as_deref();
            if !self.ui.dirty {
                recovery::clear(path);
            } else if let Err(e) = recovery::write(&self.show, path) {
                log::warn!("could not write the recovery file: {e:#}");
            }
        }
        if self.ui.dirty {
            let due = self.last_autosave + recovery::INTERVAL;
            ctl.next_repaint = Some(ctl.next_repaint.map_or(due, |t| t.min(due)));
        }

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
    /// A remote-control message arrived; `about_to_wait`, which runs next, acts on it.
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: ()) {}

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
        let config = gpu
            .surface_config(&surface, (size.width, size.height))
            .expect("the graphics card can't draw to the main window");
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
        self.keep_main_window_home();
        self.reconcile_outputs(event_loop);
        self.engine.audio.sync(&self.show.audio_outputs);
        self.run_remote(now);
        self.engine.on_clock(&self.show, crate::clock::now());
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
/// After the displays change, how long the main window is held on its own
/// display while the system settles.
const HOME_GRACE: Duration = Duration::from_secs(5);

/// Whether the main window should be put back on its display: only just
/// after the displays changed, only if that display is still there, and only
/// if it isn't on it.
fn should_return_home(settling: bool, home_connected: bool, at_home: bool) -> bool {
    settling && home_connected && !at_home
}

/// How long to wait for the main window to be placed on a display before
/// assuming it is on the primary one.
const CONTROL_WAIT: Duration = Duration::from_secs(2);

/// Which display routing should treat the main window as being on. The
/// window's own answer when it has one; while it has none, `None` (hold off
/// opening outputs, which could land on top of it); after `CONTROL_WAIT`,
/// the primary display.
fn control_for_routing(
    found: Option<usize>,
    primary: Option<usize>,
    waited: Duration,
) -> Option<Option<usize>> {
    match found {
        Some(m) => Some(Some(m)),
        None if waited < CONTROL_WAIT => None,
        None => Some(primary),
    }
}

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

/// Ask whether to take back the changes a crashed session left in `rec`.
fn offer_recovery(rec: &std::path::Path, show: &mut Show, engine: &mut Engine, ui: &mut UiState) {
    let restore = rfd::MessageDialog::new()
        .set_title("Recover unsaved changes?")
        .set_description(
            "NewQ didn't close properly last time and kept a copy of the changes that \
             hadn't been saved. Restore them?",
        )
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes;
    if restore {
        match Show::load(rec) {
            Ok(recovered) => {
                // The show on disk stays the saved one, so the title shows the edits.
                ui.set_saved(show);
                *show = recovered;
                engine.playhead = show.cues.first().map(|c| c.id);
                ui.selected = engine.playhead;
                return;
            }
            Err(e) => engine.log(format!("Could not read the recovery file: {e:#}")),
        }
    }
    // Declined, or the question couldn't be asked: keep the copy, out of the way.
    recovery::set_aside(rec);
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
    fn the_main_window_goes_home_only_after_a_change_and_only_if_home_is_there() {
        assert!(should_return_home(true, true, false));
        assert!(!should_return_home(false, true, false), "steady: leave it");
        assert!(
            !should_return_home(true, false, false),
            "home was unplugged"
        );
        assert!(!should_return_home(true, true, true), "already there");
    }

    #[test]
    fn outputs_wait_for_the_main_windows_display() {
        let s = Duration::from_secs;
        // Known: use it, whenever.
        assert_eq!(control_for_routing(Some(1), Some(0), s(0)), Some(Some(1)));
        // Unknown at first: hold off.
        assert_eq!(control_for_routing(None, Some(0), s(1)), None);
        // Still unknown after the wait: the primary display, if there is one.
        assert_eq!(control_for_routing(None, Some(0), s(3)), Some(Some(0)));
        assert_eq!(control_for_routing(None, None, s(3)), Some(None));
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
