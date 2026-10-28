//! Window management: the control window (egui) and one window per output.
//!
//! The main thread runs the event loop, the engine and the UI. Each window
//! presents from its own render thread (see `render.rs`), so nothing here ever
//! blocks on vsync or on a window the OS is throttling.

use crate::dock::Pane;
use crate::engine::{Engine, LayerSource};
use crate::gpu::{self, Geometry, Gpu, Homography, LayerTexture};
use crate::media::audio;
use crate::mesh::Mesh;
use crate::pattern;
use crate::recovery;
use crate::remote::{self, Remote};
use crate::render::{
    CaptureThread, ControlThread, EguiFrame, OutputGroup, OutputLayer, OutputThread,
};
use crate::show::{
    ColorAdjust, Corners, DisplayState, EdgeBlend, Fit, LayerTransform, MonitorKey, OutputConfig,
    OutputId, PartOf, PatternKind, RemoteSettings, Show, StreamOut, display_state,
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

/// An enabled output as `update_outputs` draws it.
struct OutputSetup {
    id: OutputId,
    name: String,
    /// Whether it shows the test pattern, and the number it shows.
    test_pattern: bool,
    pattern: PatternKind,
    number: usize,
    corners: Corners,
    mesh: Option<Arc<Mesh>>,
    /// The part of another display's picture it shows instead of its own.
    part: Option<PartOf>,
    /// The size of its picture, when that isn't its monitor's.
    picture: Option<(u32, u32)>,
}

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
    /// Windows start hidden and appear once a frame is on them.
    shown: bool,
    created: Instant,
}

/// A pane in a window of its own. It has its own egui context (a context
/// draws one window), and its own render thread on the shared graphics device.
struct PopoutWindow {
    pane: Pane,
    window: Arc<Window>,
    ctx: egui::Context,
    state: egui_winit::State,
    thread: Option<ControlThread>,
    next_repaint: Option<Instant>,
    last_paint: Instant,
    shown: bool,
    created: Instant,
}

/// How long a window may stay hidden waiting for its first frame.
const FIRST_FRAME_WAIT: Duration = Duration::from_secs(3);

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
    sent: Vec<SentGeometry>,
    /// The fade at the edges last sent.
    blend: EdgeBlend,
    /// The title last given to the window (asking a window for its title is a
    /// round trip to the display server on some systems).
    title: String,
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

/// Give an output window a surface and a render thread. The error says what
/// to tell the user; "{display}" stands for the display's number.
fn start_output_thread(
    gpu: &Gpu,
    window: &Arc<Window>,
    monitor: usize,
) -> Result<(OutputThread, (u32, u32)), String> {
    let surface = gpu
        .instance
        .create_surface(window.clone())
        .map_err(|e| format!("Could not create surface: {e}"))?;
    let size = window.inner_size();
    let size = (size.width, size.height);
    let config = gpu.surface_config(&surface, size).ok_or_else(|| {
        "This graphics card can't draw to display {display}: outputs on it are not shown"
            .to_string()
    })?;
    surface.configure(&gpu.device, &config);
    Ok((
        OutputThread::spawn(
            gpu.clone(),
            window.clone(),
            surface,
            config,
            gpu::Slot::Window(monitor.min(u8::MAX as usize) as u8),
        ),
        size,
    ))
}

/// The graphics device was lost and its windows are being given a new one.
struct GpuRecovery {
    instance: wgpu::Instance,
    /// The old render threads, which must be gone (and their surfaces with
    /// them) before the windows can have new surfaces.
    stopping: Vec<JoinHandle<()>>,
    since: Instant,
    last_try: Option<Instant>,
    warned: bool,
}

/// A test-pattern texture, with the output number and size it shows.
/// A drawn test pattern: its picture, the display number and size it was
/// drawn for, and which pattern it is.
type Pattern = (Arc<LayerTexture>, usize, (u32, u32), PatternKind);

/// What of an output's placement was last sent to a window: id, warp, mesh and
/// the part of the picture the mesh sees.
type SentGeometry = (OutputId, Homography, Option<Arc<Mesh>>, [f32; 4]);

/// A display's picture being sent out as a stream.
struct RunningStream {
    thread: CaptureThread,
    config: StreamOut,
    size: (u32, u32),
    slot: u8,
}

/// What a device scan found; `audio` is None when it didn't look.
struct DeviceList {
    midi_in: Vec<String>,
    midi_out: Vec<String>,
    audio: Option<Vec<String>>,
    audio_in: Option<Vec<String>>,
}

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
    recovery: Option<GpuRecovery>,
    gpu_restarts: u32,
    /// For testing the recovery: when to destroy the graphics device
    /// (`NEWQ_LOSE_GPU_AFTER`, in seconds).
    lose_gpu_at: Option<Instant>,
    control: Option<ControlWindow>,
    popouts: Vec<PopoutWindow>,
    /// Pop-out render threads that are shutting down.
    popout_stopping: Vec<JoinHandle<()>>,
    /// The layout as last written, to save it again only when it changes.
    saved_layout: String,
    last_layout_save: Instant,
    /// Fullscreen windows by monitor index.
    screens: HashMap<usize, ScreenWindow>,
    /// Where each output with a screen is drawn.
    targets: HashMap<OutputId, Target>,
    /// Windows whose render thread is shutting down; kept alive until it exits.
    closing: Vec<(Arc<Window>, JoinHandle<()>)>,
    /// GPU textures for playing layers, with the serial of the uploaded frame.
    textures: HashMap<u64, (Arc<LayerTexture>, u64)>,
    /// The size each output was last shown at, kept while its display is away.
    last_sizes: HashMap<OutputId, (u32, u32)>,
    /// Test-pattern textures per output.
    patterns: HashMap<OutputId, Pattern>,
    /// The displays whose picture is being sent out.
    streams: HashMap<OutputId, RunningStream>,
    /// Streams that failed to start or stopped, by display, so they are not
    /// tried again until their settings change.
    stream_failed: HashMap<OutputId, (StreamOut, (u32, u32))>,
    monitors: Vec<MonitorHandle>,
    /// What the system says each of `monitors` is.
    identities: Vec<crate::monitor::Identity>,
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
    /// A look at the MIDI ports and audio devices, running on a thread (asking
    /// the system for them can stall for a while).
    device_scan: Option<std::sync::mpsc::Receiver<DeviceList>>,
    last_tick: Instant,
    /// When the clock triggers were last looked at.
    last_clock_look: Instant,
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
        let layout_dir = crate::crash::log_dir();
        ui.layout = crate::dock::Layout::load(&layout_dir);
        ui.prefs = crate::prefs::Prefs::load(&layout_dir);
        crate::media::video::set_hw_decode(ui.prefs.hw_decode);
        ui.show_mode = ui.prefs.show_mode;
        ui.layout_dir = Some(layout_dir);
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
            recovery: None,
            gpu_restarts: 0,
            lose_gpu_at: std::env::var("NEWQ_LOSE_GPU_AFTER")
                .ok()
                .and_then(|s| s.parse::<f64>().ok())
                .filter(|s| s.is_finite() && *s >= 0.0)
                .map(|s| Instant::now() + Duration::from_secs_f64(s)),
            control: None,
            screens: HashMap::new(),
            targets: HashMap::new(),
            closing: Vec::new(),
            popouts: Vec::new(),
            popout_stopping: Vec::new(),
            saved_layout: String::new(),
            last_layout_save: Instant::now(),
            textures: HashMap::new(),
            last_sizes: HashMap::new(),
            patterns: HashMap::new(),
            streams: HashMap::new(),
            stream_failed: HashMap::new(),
            monitors: Vec::new(),
            identities: Vec::new(),
            primary: None,
            control_logged: None,
            started: Instant::now(),
            control_home: None,
            monitors_changed: None,
            last_homing: Instant::now(),
            last_monitor_scan: Instant::now(),
            device_scan: None,
            last_tick: Instant::now(),
            last_clock_look: Instant::now(),
            last_autosave: Instant::now(),
            last_web_state: Instant::now(),
            pending_recovery,
            was_active: false,
            main_window_only,
        }
    }

    fn scan_monitors(&mut self, event_loop: &ActiveEventLoop) {
        let found: Vec<MonitorHandle> = event_loop.available_monitors().collect();
        let changed = found != self.monitors;
        if changed {
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
        // Asked of the system when the set of monitors changes, not every look:
        // an answer that came and went would make a display come and go.
        if changed || self.identities.len() != self.monitors.len() {
            self.identities = self.monitors.iter().map(crate::monitor::identify).collect();
        }
        self.ui.monitors = self
            .monitors
            .iter()
            .enumerate()
            .map(|(i, m)| MonitorInfo {
                name: m.name().unwrap_or_else(|| format!("Display {}", i + 1)),
                model: self.identities[i].title.clone(),
                hardware: self.identities[i].hardware.clone(),
                size: (m.size().width, m.size().height),
                refresh_hz: m.refresh_rate_millihertz().map(|r| r as f32 / 1000.0),
                primary: primary.as_ref() == Some(m),
            })
            .collect();
        self.scan_devices();
        self.last_monitor_scan = Instant::now();
    }

    /// Take what the device scan found, if it has finished.
    fn collect_devices(&mut self) {
        if let Some(rx) = &self.device_scan {
            match rx.try_recv() {
                Ok(found) => {
                    self.device_scan = None;
                    self.ui.midi_ports = found.midi_in;
                    self.ui.midi_out_ports = found.midi_out;
                    if let Some(devices) = found.audio {
                        self.engine.audio.retry_failed(&devices);
                        self.ui.audio_devices = devices;
                    }
                    if let Some(devices) = found.audio_in {
                        self.ui.audio_input_devices = devices;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.device_scan = None,
            }
        }
    }

    /// Start a device scan unless one is still running. Listing and opening
    /// audio devices can take a while; not during a show.
    fn scan_devices(&mut self) {
        self.collect_devices();
        if self.device_scan.is_some() {
            return;
        }
        let with_audio = !self.engine.is_active();
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("device-scan".into())
            .spawn(move || {
                let _ = tx.send(DeviceList {
                    midi_in: remote::midi_inputs(),
                    midi_out: crate::output::midi_outputs(),
                    audio: with_audio.then(audio::output_devices),
                    audio_in: with_audio.then(audio::input_devices),
                });
            });
        if spawned.is_ok() {
            self.device_scan = Some(rx);
        }
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
            ctl.window.set_outer_position(crate::monitor::window_origin(
                home.position(),
                home.scale_factor(),
            ));
            ctl.window.set_maximized(true);
            ctl.window.focus_window();
        }
    }

    /// Decide where each enabled output is drawn.
    fn route_outputs(&mut self) -> HashMap<OutputId, Option<Target>> {
        if self.main_window_only {
            return self.show.outputs.iter().map(|o| (o.id, None)).collect();
        }
        let keys = self.monitor_keys();
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
        route_outputs(&self.show.outputs, &keys, control)
    }

    /// The connected monitors as shows recognise them.
    fn monitor_keys(&self) -> Vec<MonitorKey> {
        ui::monitor_keys(&self.ui.monitors)
    }

    /// Settle the displays of a show that was just opened, once it is known
    /// which monitor the main window is on: give the outputs of an older show
    /// that were "automatic" a monitor, and let displays assigned before
    /// hardware ids were known learn theirs. After that nothing moves an
    /// output to another monitor.
    fn settle_displays(&mut self) {
        if self.main_window_only {
            return;
        }
        let found = self.control_monitor();
        let Some(control) = control_for_routing(found, self.primary, self.started.elapsed()) else {
            return;
        };
        let keys = self.monitor_keys();
        let mut notes = Vec::new();
        if self.show.needs_automatic_assignment() {
            notes.extend(self.show.assign_automatic_displays(&keys, control));
        }
        notes.extend(self.show.learn_hardware_ids(&keys, control));
        for note in notes {
            self.engine.log(format!("Displays: {note}"));
        }
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
            attrs = attrs.with_position(crate::monitor::window_origin(
                m.position(),
                m.scale_factor(),
            ));
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
        {
            // The window must be on the display it is to cover before it
            // is made to cover it.
            if window.current_monitor().as_ref() != Some(m) {
                log::warn!(
                    "output window started on {:?}, not {:?}; moving it",
                    window.current_monitor().and_then(|x| x.name()),
                    m.name()
                );
                window.set_outer_position(crate::monitor::window_origin(
                    m.position(),
                    m.scale_factor(),
                ));
            }
        }
        #[cfg(target_os = "macos")]
        if !winit::platform::macos::WindowExtMacOS::set_simple_fullscreen(&*window, true) {
            log::warn!("simple fullscreen refused on display {monitor}; using native fullscreen");
            window.set_fullscreen(Some(Fullscreen::Borderless(Some(m.clone()))));
        }
        window.set_cursor_visible(false);
        let (thread, size) = match start_output_thread(gpu, &window, monitor) {
            Ok(started) => started,
            Err(e) => {
                self.engine
                    .log(e.replace("{display}", &(monitor + 1).to_string()));
                return;
            }
        };
        log::info!(
            "output window on display {monitor} ({:?}): {}x{}, scale {}",
            m.name(),
            size.0,
            size.1,
            window.scale_factor()
        );
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
                blend: EdgeBlend::default(),
                title: String::new(),
            },
        );
    }

    /// Notice a lost graphics device and give every window a new one: stop the
    /// render threads, then start a device, surfaces and threads afresh.
    fn watch_gpu(&mut self) {
        if let (Some(at), Some(gpu)) = (self.lose_gpu_at, &self.gpu)
            && Instant::now() >= at
        {
            self.lose_gpu_at = None;
            log::warn!("destroying the graphics device, as asked");
            gpu.device.destroy();
        }
        if let Some(gpu) = &self.gpu {
            // The loss is reported when the device is polled.
            let _ = gpu.device.poll(wgpu::PollType::Poll);
        }
        if self.recovery.is_none()
            && let Some(gpu) = &self.gpu
            && let Some(why) = gpu.lost.why()
        {
            self.engine.log(format!(
                "The graphics device was lost ({why}); restarting the displays"
            ));
            let mut stopping = Vec::new();
            if let Some(ctl) = &mut self.control {
                stopping.extend(ctl.thread.take().map(ControlThread::quit));
            }
            // Pop-outs are opened again from the layout on the new device.
            for mut p in self.popouts.drain(..) {
                stopping.extend(p.thread.take().map(ControlThread::quit));
            }
            for screen in self.screens.values_mut() {
                stopping.extend(screen.thread.take().map(OutputThread::quit));
                screen.sent.clear();
                screen.needs_redraw = true;
            }
            self.recovery = Some(GpuRecovery {
                instance: gpu.instance.clone(),
                stopping,
                since: Instant::now(),
                last_try: None,
                warned: false,
            });
            self.gpu = None;
            // Everything on the old device is gone with it, streams included
            // (they start again once there is a new device).
            self.stop_streams();
            self.textures.clear();
            self.patterns.clear();
        }
        let Some(rec) = &mut self.recovery else {
            return;
        };
        if rec.stopping.iter().any(|h| !h.is_finished()) {
            if !rec.warned && rec.since.elapsed() > Duration::from_secs(5) {
                rec.warned = true;
                log::warn!("a render thread is slow to stop on the lost device");
            }
            return;
        }
        if rec
            .last_try
            .is_some_and(|t| t.elapsed() < Duration::from_secs(2))
        {
            return;
        }
        rec.last_try = Some(Instant::now());
        let Some(ctl) = &mut self.control else {
            return;
        };
        let (gpu, surface) = match Gpu::new(rec.instance.clone(), ctl.window.clone()) {
            Ok(made) => made,
            Err(e) => {
                // The driver may need a moment; keep trying.
                log::warn!("no graphics device yet: {e:#}");
                return;
            }
        };
        let size = ctl.window.inner_size();
        let Some(config) = gpu.surface_config(&surface, (size.width, size.height)) else {
            log::warn!("the new graphics device can't draw to the main window");
            return;
        };
        surface.configure(&gpu.device, &config);
        ctl.thread = Some(ControlThread::spawn(
            gpu.clone(),
            ctl.window.clone(),
            surface,
            config,
        ));
        ctl.next_repaint = Some(Instant::now());
        // The new renderer has none of the textures egui sent the old one
        // (its fonts); asking for the fonts again sends the lot.
        // (Setting the very same fonts does nothing, so each restart adds an
        // empty font family of its own to make them different.)
        let mut fonts = egui::FontDefinitions::default();
        self.gpu_restarts += 1;
        fonts.families.insert(
            egui::FontFamily::Name(format!("restart-{}", self.gpu_restarts).into()),
            Vec::new(),
        );
        self.egui_ctx.set_fonts(fonts);
        for (monitor, screen) in &mut self.screens {
            match start_output_thread(&gpu, &screen.window, *monitor) {
                Ok((thread, size)) => {
                    screen.thread = Some(thread);
                    screen.size = size;
                }
                Err(e) => self
                    .engine
                    .log(e.replace("{display}", &(monitor + 1).to_string())),
            }
        }
        self.gpu = Some(gpu);
        self.recovery = None;
        self.engine.log("The graphics device was restarted");
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
            // Whole displays go underneath, then sub displays in list order.
            let order = usize::from(o.is_sub_display());
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
                if screen.title != title {
                    screen.window.set_title(&title);
                    screen.title = title;
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
                        .map(|x| x.title().to_string())
                        .unwrap_or_default();
                    if cfg.is_sub_display() {
                        OutputRoute::SubDisplay {
                            name: cfg.name.clone(),
                            display,
                        }
                    } else {
                        OutputRoute::Display(display)
                    }
                }
            };
            self.ui.routes.insert(cfg.id, route);
        }
        let keys = self.monitor_keys();
        let control = self.control_monitor();
        self.ui.display_states = self
            .show
            .outputs
            .iter()
            .filter(|o| !o.is_sub_display())
            .map(|d| {
                (
                    d.id,
                    display_state(d.monitor.as_ref(), d.twins, &keys, control),
                )
            })
            .collect();
        self.ui.sub_monitors = self
            .show
            .outputs
            .iter()
            .filter_map(|o| match self.ui.display_states.get(&o.within?)? {
                DisplayState::Connected(m) | DisplayState::OnMainWindowDisplay(m) => {
                    Some((o.id, *m))
                }
                _ => None,
            })
            .collect();
        // Cues sent to a display whose monitor isn't there keep playing, to
        // nothing; the UI says so until it is back.
        self.ui.dark_outputs = self
            .show
            .outputs
            .iter()
            .filter(|o| o.enabled)
            .filter(|o| {
                let display = o.within.unwrap_or(o.id);
                self.ui.display_states.get(&display) == Some(&DisplayState::Missing)
            })
            .map(|o| o.id)
            .collect();

        // Decode at the size each output is shown at. Outputs in the main
        // window decode at the default (1080p) size.
        let mut sizes: HashMap<OutputId, (u32, u32)> = self
            .targets
            .iter()
            .filter_map(|(id, t)| Some((*id, t.area.size_on(self.screens.get(&t.monitor)?.size))))
            .collect();
        // A picture given a size of its own (one that projectors share) is
        // decoded at that size, monitor or not.
        for o in &self.show.outputs {
            if let Some(size) = o.picture {
                sizes.insert(o.id, size);
            }
        }
        // A display whose monitor has gone keeps the size it had, so what
        // plays meanwhile is ready for it when it is back.
        self.last_sizes.extend(sizes.iter().map(|(k, v)| (*k, *v)));
        self.last_sizes
            .retain(|id, _| self.ui.dark_outputs.contains(id) || sizes.contains_key(id));
        for (id, size) in &self.last_sizes {
            sizes.entry(*id).or_insert(*size);
        }
        self.engine.output_sizes = sizes;
        // Release windows once their render thread has dropped the surface.
        self.closing.retain(|(_, h)| !h.is_finished());
    }

    /// Stop every stream, without waiting for them.
    fn stop_streams(&mut self) {
        for (_, running) in self.streams.drain() {
            let _ = std::thread::Builder::new()
                .name("stream-stop".into())
                .spawn(move || running.thread.stop());
        }
    }

    /// Start, restart and stop the streams the show asks for, and give each
    /// the picture it is to send: the display's own picture on its own canvas,
    /// without the shaping a projector needs.
    fn update_streams(&mut self, groups: &HashMap<OutputId, OutputGroup>) {
        let Some(gpu) = self.gpu.clone() else { return };
        let wanted: HashMap<OutputId, (StreamOut, (u32, u32))> = self
            .show
            .outputs
            .iter()
            .filter(|o| o.enabled && !o.is_sub_display() && self.show.part_of(o.id).is_none())
            .filter_map(|o| {
                let s = o.stream.as_ref().filter(|s| s.enabled)?.clamped();
                let size = crate::stream::size_for(self.picture_size(o.id), s.height);
                Some((o.id, (s, size)))
            })
            .collect();
        self.stream_failed
            .retain(|id, failed| wanted.get(id) == Some(&(failed.0.clone(), failed.1)));
        // Stop what is no longer wanted, or is wanted differently.
        let stale: Vec<OutputId> = self
            .streams
            .iter()
            .filter(|(id, r)| {
                wanted.get(id) != Some(&(r.config.clone(), r.size)) || !r.thread.is_running()
            })
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            if let Some(r) = self.streams.remove(&id) {
                let name = self.show.output(id).map_or("?", |o| o.name.as_str());
                if let Some(why) = r.thread.failure() {
                    self.engine.log(format!("Stream from {name}: {why}"));
                    self.stream_failed.insert(id, (r.config.clone(), r.size));
                }
                let _ = std::thread::Builder::new()
                    .name("stream-stop".into())
                    .spawn(move || r.thread.stop());
            }
        }
        for (id, (config, size)) in &wanted {
            if self.streams.contains_key(id) || self.stream_failed.contains_key(id) {
                continue;
            }
            let name = self
                .show
                .output(*id)
                .map(|o| o.name.clone())
                .unwrap_or_default();
            let fail = |app: &mut App, why: String| {
                app.engine.log(format!("Stream from {name}: {why}"));
                app.stream_failed.insert(*id, (config.clone(), *size));
            };
            let Some(slot) = (0..gpu::CAPTURE_SLOTS as u8)
                .find(|n| !self.streams.values().any(|r| r.slot == *n))
            else {
                fail(
                    self,
                    format!("only {} streams at a time", gpu::CAPTURE_SLOTS),
                );
                continue;
            };
            let Some(ff) = self.engine.ff.clone() else {
                fail(self, "FFmpeg was not found".into());
                continue;
            };
            // FFmpeg is started on the stream's own thread, so neither
            // looking for what it can encode nor starting it holds up the show.
            let thread = CaptureThread::spawn(
                gpu.clone(),
                gpu::Slot::Capture(slot),
                *size,
                config.fps,
                ff,
                config.kind.clone(),
            );
            self.engine.log(format!(
                "Sending {name} out: {}×{} at {} fps",
                size.0, size.1, config.fps
            ));
            self.streams.insert(
                *id,
                RunningStream {
                    thread,
                    config: config.clone(),
                    size: *size,
                    slot,
                },
            );
        }
        for (id, running) in &self.streams {
            let Some(g) = groups.get(id) else { continue };
            let mut clean = g.clone();
            clean.geometry = Geometry {
                canvas: self.picture_size(*id),
                warp: gpu::IDENTITY,
                pin: gpu::IDENTITY,
                mesh: None,
                slice: Geometry::WHOLE,
            };
            running.thread.submit(vec![clean]);
        }
    }

    /// The size of display `id`'s picture: the one it is given, or its
    /// monitor's, or 1080p while it is only in the main window.
    fn picture_size(&self, id: OutputId) -> (u32, u32) {
        if let Some(size) = self.show.output(id).and_then(|o| o.picture) {
            return size;
        }
        self.targets
            .get(&id)
            .and_then(|t| Some(t.area.size_on(self.screens.get(&t.monitor)?.size)))
            .unwrap_or((1920, 1080))
    }

    /// Upload new frames and hand each output's current layer stack to the
    /// window of the display it is on, and to the UI for the live previews.
    fn update_outputs(&mut self, playing: bool) {
        let Some(gpu) = &self.gpu else { return };
        self.ui.previews.clear();
        let configs: Vec<OutputSetup> = self
            .show
            .outputs
            .iter()
            .enumerate()
            .filter(|(_, o)| o.enabled)
            .map(|(i, o)| {
                // A sub display's area is its keystone.
                let corners = if o.is_sub_display() {
                    Corners::default()
                } else {
                    o.corners
                };
                // A mesh does the work of the four corners on a display.
                let mesh = o
                    .mesh
                    .clone()
                    .filter(|m| !o.is_sub_display() && m.is_valid())
                    .map(Arc::new);
                OutputSetup {
                    id: o.id,
                    name: o.name.clone(),
                    test_pattern: o.test_pattern,
                    pattern: o.pattern,
                    number: i + 1,
                    corners,
                    mesh,
                    part: self.show.part_of(o.id),
                    picture: o.picture,
                }
            })
            .collect();
        let mut groups: HashMap<OutputId, OutputGroup> = HashMap::new();
        for setup in configs {
            let OutputSetup {
                id,
                name,
                test_pattern: want_pattern,
                pattern: kind,
                number,
                corners,
                mesh,
                part,
                picture,
            } = setup;
            let target = self.targets.get(&id).copied();
            let screen = target.and_then(|t| Some((t, self.screens.get(&t.monitor)?.size)));
            // The output's own canvas: its area of a display, or 1080p in the
            // main window, unless it is given a size of its own.
            let own_canvas = match screen {
                Some((t, size)) => t.area.size_on(size),
                None => (1920, 1080),
            };
            let own_canvas = picture.unwrap_or(own_canvas);
            // A display that shows part of another's picture draws that
            // display's layers, on that picture's canvas, through the part.
            let (source, canvas) = match part {
                Some(p) => (p.display, self.picture_size(p.display)),
                None => (id, own_canvas),
            };
            let (part_hom, slice) = match part {
                Some(p) => {
                    let [x, y, w, h] = p.area;
                    let (w, h) = (w.max(0.01), h.max(0.01));
                    (
                        [
                            [1.0 / w, 0.0, -x / w, 0.0],
                            [0.0, 1.0 / h, -y / h, 0.0],
                            [0.0, 0.0, 1.0, 0.0],
                        ],
                        [x, y, 1.0 / w, 1.0 / h],
                    )
                }
                None => (gpu::IDENTITY, Geometry::WHOLE),
            };
            let (pin, slice) = if mesh.is_some() {
                (gpu::IDENTITY, slice)
            } else {
                (
                    gpu::compose(&gpu::corner_pin(&corners.0), &part_hom),
                    Geometry::WHOLE,
                )
            };
            let warp = match screen {
                Some((t, _)) => gpu::compose(&gpu::corner_pin(&t.area.0), &pin),
                None => pin,
            };

            let mut layers = Vec::new();
            for l in self.engine.layers(source) {
                let entry = match &l.source {
                    LayerSource::Video(p) => {
                        let Some(frame) = p.current() else { continue };
                        let new_texture = || {
                            let tex = match frame.hap {
                                Some(f) => LayerTexture::new_bc(gpu, p.width, p.height, f),
                                None if p.rgba => LayerTexture::new_rgba(gpu, p.width, p.height),
                                None => LayerTexture::new_yuv(gpu, p.width, p.height, p.matrix),
                            };
                            (Arc::new(tex), 0)
                        };
                        let entry = self.textures.entry(l.instance).or_insert_with(new_texture);
                        // A seek can bring a player of another size (the output
                        // was resized since the cue started), or of another kind of
                        // frame: they need a texture of their own, or would not be shown.
                        let right_kind = match frame.hap {
                            Some(f) => entry.0.is_bc(f),
                            None => !entry.0.is_bc_any() && (p.rgba == entry.0.is_rgba()),
                        };
                        if (entry.0.width, entry.0.height) != (p.width, p.height) || !right_kind {
                            *entry = new_texture();
                        }
                        if entry.1 != p.serial {
                            match frame.hap {
                                Some(_) => entry.0.upload_bc(&gpu.queue, &frame.data),
                                None if p.rgba => entry.0.upload_rgba(&gpu.queue, &frame.data),
                                None => entry.0.upload_yuv(&gpu.queue, &frame.data),
                            }
                            entry.1 = p.serial;
                        }
                        entry
                    }
                    LayerSource::Image(img, version) => {
                        let version = *version;
                        let new_texture = || {
                            let tex = LayerTexture::new_rgba(gpu, img.width, img.height);
                            tex.upload_rgba(&gpu.queue, &img.data);
                            (Arc::new(tex), version)
                        };
                        let entry = self.textures.entry(l.instance).or_insert_with(new_texture);
                        // A clock or timer is drawn again as the time changes.
                        if entry.1 != version {
                            if (entry.0.width, entry.0.height) == (img.width, img.height) {
                                entry.0.upload_rgba(&gpu.queue, &img.data);
                                entry.1 = version;
                            } else {
                                *entry = new_texture();
                            }
                        }
                        entry
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
                    fx: l.fx,
                });
            }

            // The test pattern of the picture it shows part of, so projectors
            // can be lined up across their overlap.
            if source != id
                && let Some((tex, ..)) = self.patterns.get(&source)
            {
                layers.push(OutputLayer {
                    texture: tex.clone(),
                    opacity: 1.0,
                    fit: Fit::Stretch,
                    transform: LayerTransform::default(),
                    color: ColorAdjust::default(),
                    fx: Default::default(),
                });
            }
            // The test pattern is drawn at the canvas's shape, so its circles
            // and grid stay true on a sub display of any size.
            let mut pattern_changed = false;
            if want_pattern {
                // Drawn small and stretched, except the patterns made of single
                // pixels, which need the display's own size (up to 4K).
                let size = if kind.pixel_exact() {
                    let scale = (3840.0 / own_canvas.0.max(own_canvas.1) as f32).min(1.0);
                    let px = |v: u32| ((v as f32 * scale).round() as u32).max(2);
                    (px(own_canvas.0), px(own_canvas.1))
                } else {
                    let scale = (1280.0 / own_canvas.0.max(own_canvas.1) as f32).min(1.0);
                    // In 32 px steps, so dragging an area doesn't redraw it every frame.
                    let step = |v: u32| ((v as f32 * scale / 32.0).round() as u32).max(1) * 32;
                    (step(own_canvas.0), step(own_canvas.1))
                };
                if self
                    .patterns
                    .get(&id)
                    .is_none_or(|(_, n, s, k)| *n != number || *s != size || *k != kind)
                {
                    let tex = LayerTexture::new_rgba(gpu, size.0, size.1);
                    tex.upload_rgba(&gpu.queue, &pattern::render(kind, size.0, size.1, number));
                    self.patterns
                        .insert(id, (Arc::new(tex), number, size, kind));
                    pattern_changed = true;
                }
            } else if self.patterns.remove(&id).is_some() {
                pattern_changed = true;
            }
            if let Some((tex, ..)) = self.patterns.get(&id) {
                // On a display showing part of a picture, the pattern covers
                // that part (the layer is placed on the whole picture).
                let transform = match part {
                    Some(p) => {
                        let [x, y, w, h] = p.area;
                        LayerTransform {
                            x: x + w / 2.0 - 0.5,
                            y: y + h / 2.0 - 0.5,
                            scale_x: w,
                            scale_y: h,
                            ..LayerTransform::default()
                        }
                    }
                    None => LayerTransform::default(),
                };
                layers.push(OutputLayer {
                    texture: tex.clone(),
                    opacity: 1.0,
                    fit: Fit::Stretch,
                    transform,
                    color: ColorAdjust::default(),
                    fx: Default::default(),
                });
            }
            if pattern_changed {
                // (Other displays may show part of this picture.)
                for s in self.screens.values_mut() {
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
                    geometry: Geometry {
                        canvas,
                        warp,
                        pin,
                        mesh: mesh.clone(),
                        slice,
                    },
                },
            );
            // The preview is the shape of what the display shows: the part of
            // the picture, not all of it.
            let preview_canvas = match part {
                Some(p) => (
                    ((canvas.0 as f32 * p.area[2]).round() as u32).max(1),
                    ((canvas.1 as f32 * p.area[3]).round() as u32).max(1),
                ),
                None => canvas,
            };
            self.ui.previews.push(PreviewOutput {
                id,
                name,
                layers,
                warp: pin,
                mesh,
                slice,
                canvas: preview_canvas,
            });
        }

        self.update_streams(&groups);

        self.ui.screens.clear();
        for (m, screen) in &mut self.screens {
            let list: Vec<OutputGroup> = screen
                .outputs
                .iter()
                .filter_map(|id| groups.get(id).cloned())
                .collect();
            let sent: Vec<SentGeometry> = screen
                .outputs
                .iter()
                .filter_map(|id| {
                    let g = &groups.get(id)?.geometry;
                    Some((*id, g.warp, g.mesh.clone(), g.slice))
                })
                .collect();
            // The fade at the edges is the display's own, whatever sub displays
            // share its monitor.
            let blend = screen
                .outputs
                .iter()
                .filter_map(|id| self.show.output(*id))
                .find(|o| !o.is_sub_display())
                .map(|o| o.blend.clamped())
                .unwrap_or_default();
            if playing || screen.needs_redraw || sent != screen.sent || blend != screen.blend {
                screen.needs_redraw = false;
                screen.sent = sent;
                screen.blend = blend;
                if let Some(t) = &screen.thread {
                    t.submit(screen.size, list.clone(), blend);
                }
            }
            self.ui.screens.insert(*m, (screen.size, list));
        }
    }

    /// Give every popped-out pane in the layout a window, and close the
    /// windows of panes that are not popped out any more.
    fn reconcile_popouts(&mut self, event_loop: &ActiveEventLoop) {
        let Some(gpu) = self.gpu.clone() else { return };
        let wanted: Vec<Pane> = self.ui.layout.popouts.iter().map(|p| p.pane).collect();
        let mut i = 0;
        while i < self.popouts.len() {
            if wanted.contains(&self.popouts[i].pane) {
                i += 1;
            } else {
                let mut gone = self.popouts.remove(i);
                self.popout_stopping
                    .extend(gone.thread.take().map(ControlThread::quit));
            }
        }
        for want in self.ui.layout.popouts.clone() {
            if self.popouts.iter().any(|p| p.pane == want.pane) {
                continue;
            }
            let (w, h) = match want.size {
                Some([w, h]) => (w, h),
                None if want.pane == Pane::Preview => (720.0, 440.0),
                None => (480.0, 620.0),
            };
            let mut attrs = Window::default_attributes()
                .with_title(format!("NewQ: {}", want.pane.title()))
                .with_visible(false)
                .with_window_icon(crate::branding::window_icon())
                .with_inner_size(LogicalSize::new(w as f64, h as f64))
                .with_min_inner_size(LogicalSize::new(240.0, 160.0));
            if let Some([x, y]) = want.pos {
                attrs = attrs.with_position(winit::dpi::LogicalPosition::new(x as f64, y as f64));
            }
            let window = match event_loop.create_window(attrs) {
                Ok(w) => Arc::new(w),
                Err(e) => {
                    log::warn!("could not open a window for {}: {e}", want.pane.title());
                    self.ui.layout.dock_back(want.pane);
                    continue;
                }
            };
            let surface = match gpu.instance.create_surface(window.clone()) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("no surface for {}: {e}", want.pane.title());
                    self.ui.layout.dock_back(want.pane);
                    continue;
                }
            };
            let size = window.inner_size();
            let Some(config) = gpu.surface_config(&surface, (size.width, size.height)) else {
                self.ui.layout.dock_back(want.pane);
                continue;
            };
            if !gpu.configure_checked(&surface, &config) {
                log::warn!("could not configure a window for {}", want.pane.title());
                self.ui.layout.dock_back(want.pane);
                continue;
            }
            let ctx = egui::Context::default();
            ui::setup_style(&ctx);
            let state = egui_winit::State::new(
                ctx.clone(),
                egui::ViewportId::ROOT,
                &window,
                Some(window.scale_factor() as f32),
                window.theme(),
                Some(gpu.device.limits().max_texture_dimension_2d as usize),
            );
            let thread = ControlThread::spawn(gpu.clone(), window.clone(), surface, config);
            self.popouts.push(PopoutWindow {
                pane: want.pane,
                window,
                ctx,
                state,
                thread: Some(thread),
                next_repaint: Some(Instant::now()),
                last_paint: Instant::now(),
                shown: false,
                created: Instant::now(),
            });
        }
    }

    /// Show the control window and the pop-outs once each has had a frame
    /// drawn on it (or after a few seconds, so a window can't stay hidden).
    /// Until then they are drawn while hidden: some systems don't ask a hidden
    /// window to repaint, so ask for it here.
    fn reveal_windows(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(ctl) = &self.control
            && !ctl.shown
            && ctl.last_paint.elapsed() > Duration::from_millis(30)
        {
            self.render_control(event_loop);
        }
        if let Some(ctl) = &mut self.control
            && !ctl.shown
            && (ctl.thread.as_ref().is_some_and(|t| t.has_presented())
                || ctl.created.elapsed() > FIRST_FRAME_WAIT)
        {
            ctl.shown = true;
            ctl.window.set_visible(true);
            ctl.window.focus_window();
            ctl.next_repaint = Some(Instant::now());
        }
        let hidden: Vec<WindowId> = self
            .popouts
            .iter()
            .filter(|p| !p.shown && p.last_paint.elapsed() > Duration::from_millis(30))
            .map(|p| p.window.id())
            .collect();
        for id in hidden {
            self.render_popout(id);
        }
        for p in &mut self.popouts {
            if !p.shown
                && (p.thread.as_ref().is_some_and(|t| t.has_presented())
                    || p.created.elapsed() > FIRST_FRAME_WAIT)
            {
                p.shown = true;
                p.window.set_visible(true);
                p.next_repaint = Some(Instant::now());
            }
        }
    }

    fn render_popout(&mut self, id: WindowId) {
        let Some(p) = self.popouts.iter_mut().find(|p| p.window.id() == id) else {
            return;
        };
        p.last_paint = Instant::now();
        let raw = p.state.take_egui_input(&p.window);
        let had_input = !raw.events.is_empty();
        let pane = p.pane;
        let (show, engine, st) = (&mut self.show, &mut self.engine, &mut self.ui);
        let mut full = p
            .ctx
            .run_ui(raw, |ui| ui::draw_popout(ui, pane, show, engine, st));
        p.state
            .handle_platform_output(&p.window, full.platform_output);
        p.next_repaint = full
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .map(|v| Instant::now() + v.repaint_delay.min(Duration::from_secs(3600)));
        let jobs = p.ctx.tessellate(full.shapes, full.pixels_per_point);
        let size = p.window.inner_size();
        let mut textures = std::mem::take(&mut full.textures_delta);
        match &p.thread {
            Some(t) => t.submit(EguiFrame {
                size: (size.width, size.height),
                pixels_per_point: full.pixels_per_point,
                jobs,
                textures,
            }),
            None => textures.clear(),
        }
        self.note_popout_geometry(id);
        // The main window notices changes to the show; tell it to look.
        if had_input && let Some(ctl) = &mut self.control {
            ctl.next_repaint = Some(Instant::now());
        }
    }

    /// Remember where a popped-out pane's window is.
    fn note_popout_geometry(&mut self, id: WindowId) {
        let Some(p) = self.popouts.iter().find(|p| p.window.id() == id) else {
            return;
        };
        let sf = p.window.scale_factor();
        let size = p.window.inner_size().to_logical::<f32>(sf);
        let pos = p
            .window
            .outer_position()
            .ok()
            .map(|q| q.to_logical::<f32>(sf))
            .map(|q| [q.x, q.y]);
        self.ui
            .layout
            .set_popout_geometry(p.pane, pos, [size.width, size.height]);
    }

    /// Write the layout when it has changed (looked at no more than once a
    /// second: this runs every tick, and serialising the layout each time
    /// would be work on every frame; and again on the way out).
    fn save_layout(&mut self, force: bool) {
        let Some(dir) = self.ui.layout_dir.clone() else {
            return;
        };
        if !force && self.last_layout_save.elapsed() < Duration::from_secs(1) {
            return;
        }
        self.last_layout_save = Instant::now();
        let Ok(now) = serde_json::to_string(&self.ui.layout) else {
            return;
        };
        if now != self.saved_layout {
            match self.ui.layout.save(&dir) {
                Ok(()) => self.saved_layout = now,
                Err(e) => log::warn!("could not save the window layout: {e}"),
            }
        }
    }

    fn render_control(&mut self, event_loop: &ActiveEventLoop) {
        let Some(ctl) = &mut self.control else { return };
        ctl.last_paint = Instant::now();

        if let Some(rec) = self.pending_recovery.take() {
            offer_recovery(&rec, &mut self.show, &mut self.engine, &mut self.ui);
        }

        let raw = ctl.state.take_egui_input(&ctl.window);
        // A change here shows in the pop-outs too.
        if !raw.events.is_empty() {
            for p in &self.popouts {
                p.window.request_redraw();
            }
        }
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
                recovery::clear_in_background(path);
            } else {
                // On a thread of its own: the write and its flush are a dropped
                // frame on every output if they happen here.
                recovery::write_in_background(self.show.clone(), path.map(PathBuf::from));
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
                        .with_visible(false)
                        .with_window_icon(crate::branding::window_icon())
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
            shown: false,
            created: Instant::now(),
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

        if let Some(i) = self.popouts.iter().position(|p| p.window.id() == id) {
            let p = &mut self.popouts[i];
            let response = p.state.on_window_event(&p.window, &event);
            if response.repaint && !matches!(event, WindowEvent::RedrawRequested) {
                p.window.request_redraw();
            }
            match event {
                WindowEvent::CloseRequested => {
                    let pane = p.pane;
                    self.ui.layout.dock_back(pane);
                    if let Some(ctl) = &self.control {
                        ctl.window.request_redraw();
                    }
                }
                WindowEvent::Resized(_) => {
                    p.window.request_redraw();
                    self.note_popout_geometry(id);
                }
                WindowEvent::Moved(_) => self.note_popout_geometry(id),
                WindowEvent::RedrawRequested => self.render_popout(id),
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

        self.watch_gpu();
        self.collect_devices();
        if now.duration_since(self.last_monitor_scan) > Duration::from_secs(2) {
            self.scan_monitors(event_loop);
        }
        self.keep_main_window_home();
        self.settle_displays();
        self.reconcile_outputs(event_loop);
        self.reconcile_popouts(event_loop);
        self.popout_stopping.retain(|h| !h.is_finished());
        self.save_layout(false);
        self.engine.audio.sync(&self.show.audio_outputs);
        self.run_remote(now);
        // However long it has been since the last look, by a steady clock: a
        // dialog or a stall is time that passed, a sleep is not.
        let gap = now.duration_since(self.last_clock_look).as_secs_f64();
        self.last_clock_look = now;
        self.engine.on_clock(&self.show, crate::clock::now(), gap);
        self.engine.tick(&self.show, dt);
        // What control cues asked of the show (arming, disarming).
        for edit in self.engine.take_show_edits() {
            match edit {
                crate::engine::ShowEdit::Arm(id, on) => {
                    if let Some(c) = self.show.cues.iter_mut().find(|c| c.id == id) {
                        c.armed = on;
                    }
                }
            }
            self.ui.json_stale = true;
        }

        // Drop GPU textures of cues that are no longer playing.
        let live: HashSet<u64> = self.engine.active.iter().map(|a| a.instance).collect();
        let before = self.textures.len();
        self.textures.retain(|k, _| live.contains(k));
        let removed_layer = self.textures.len() != before;

        let playing = self.engine.is_active();
        // One extra frame after the last layer stops, so outputs go to black.
        self.update_outputs(playing || removed_layer);

        self.reveal_windows(event_loop);

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
        for p in &mut self.popouts {
            let mut due = p.next_repaint.is_some_and(|t| t <= now);
            if playing {
                let frame = Duration::from_secs_f64(1.0 / CONTROL_FPS_WITH_EMBEDDED);
                due |= now.duration_since(p.last_paint) >= frame;
                next = next.min(p.last_paint + frame);
            }
            if due {
                p.next_repaint = None;
                p.window.request_redraw();
            }
            if let Some(t) = p.next_repaint {
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
        self.save_layout(true);
        if let Some(t) = self.control.as_mut().and_then(|c| c.thread.take()) {
            drop(t.quit());
        }
        for p in &mut self.popouts {
            if let Some(t) = p.thread.take() {
                drop(t.quit());
            }
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

/// Decide where each enabled output is drawn (`None` = only in the main
/// window). An output is on the monitor its display was assigned to, and
/// nowhere else: with that monitor gone it isn't shown on another, and
/// nothing is picked automatically.
fn route_outputs(
    outputs: &[OutputConfig],
    monitors: &[MonitorKey],
    control: Option<usize>,
) -> HashMap<OutputId, Option<Target>> {
    outputs
        .iter()
        .filter(|o| o.enabled)
        .map(|o| {
            let display = match o.within {
                None => Some(o),
                Some(d) => outputs.iter().find(|x| x.id == d && x.within.is_none()),
            };
            let target = display.and_then(|d| {
                match display_state(d.monitor.as_ref(), d.twins, monitors, control) {
                    DisplayState::Connected(monitor) => Some(Target {
                        monitor,
                        area: if o.is_sub_display() {
                            o.area
                        } else {
                            Corners::default()
                        },
                    }),
                    _ => None,
                }
            });
            (o.id, target)
        })
        .collect()
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
                engine.forget_lists();
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
    use crate::show::DisplayRef;

    fn keys(names: &[&str]) -> Vec<MonitorKey> {
        names
            .iter()
            .map(|n| MonitorKey {
                identity: n.to_string(),
                raw: n.to_string(),
                hardware: None,
            })
            .collect()
    }

    /// A display assigned to monitor `name` (at `index`), or to none.
    fn display(id: u64, monitor: Option<(&str, usize)>) -> OutputConfig {
        let mut o = OutputConfig::new(OutputId(id), format!("Display {id}"));
        o.monitor = monitor.map(|(name, index)| DisplayRef {
            name: name.into(),
            index,
            hardware: None,
        });
        o
    }

    /// A sub display: the area `area` of display `of`.
    fn sub(id: u64, of: u64, area: Corners) -> OutputConfig {
        let mut o = OutputConfig::new(OutputId(id), format!("Sub {id}"));
        o.within = Some(OutputId(of));
        o.area = area;
        o
    }

    /// Routes as monitor indices.
    fn route(
        outputs: &[OutputConfig],
        monitors: &[&str],
        control: Option<usize>,
    ) -> HashMap<OutputId, Option<usize>> {
        route_outputs(outputs, &keys(monitors), control)
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
    fn a_display_is_on_the_monitor_it_was_assigned_to() {
        let outs = [
            display(1, Some(("Samsung", 2))),
            display(2, Some(("Projector", 1))),
        ];
        let r = route(&outs, &["Laptop", "Projector", "Samsung"], Some(0));
        assert_eq!((r[&OutputId(1)], r[&OutputId(2)]), (Some(2), Some(1)));
        // Plugged in in another order: still the same monitors, found by name.
        let r = route(&outs, &["Laptop", "Samsung", "Projector"], Some(0));
        assert_eq!((r[&OutputId(1)], r[&OutputId(2)]), (Some(1), Some(2)));
    }

    #[test]
    fn a_display_that_goes_away_is_not_replaced_by_another_monitor() {
        let outs = [display(1, Some(("Samsung", 1)))];
        // The Samsung is gone and a different monitor is in its place: nothing shows on it.
        let r = route(&outs, &["Laptop", "Dell"], Some(0));
        assert_eq!(r[&OutputId(1)], None);
        let r = route(&outs, &["Laptop"], Some(0));
        assert_eq!(r[&OutputId(1)], None);
        // And it is back where it was when the Samsung returns.
        let r = route(&outs, &["Laptop", "Samsung"], Some(0));
        assert_eq!(r[&OutputId(1)], Some(1));
    }

    #[test]
    fn a_display_with_no_monitor_or_on_the_main_windows_monitor_shows_nothing() {
        let outs = [display(1, None), display(2, Some(("Laptop", 0)))];
        let r = route(&outs, &["Laptop", "TV"], Some(0));
        assert_eq!((r[&OutputId(1)], r[&OutputId(2)]), (None, None));
        let monitors = keys(&["Laptop", "TV"]);
        let state = |o: &OutputConfig, control| {
            display_state(o.monitor.as_ref(), o.twins, &monitors, control)
        };
        assert_eq!(state(&outs[0], Some(0)), DisplayState::Unassigned);
        assert_eq!(
            state(&outs[1], Some(0)),
            DisplayState::OnMainWindowDisplay(0)
        );
        // Not covered by a sub display either, and free when the main window is elsewhere.
        assert_eq!(state(&outs[1], Some(1)), DisplayState::Connected(0));
    }

    #[test]
    fn identical_monitors_are_only_used_when_they_can_be_told_apart() {
        let state = |monitor: (&str, usize), twins: u32, connected: &[&str], control| {
            let r = DisplayRef {
                name: monitor.0.into(),
                index: monitor.1,
                hardware: None,
            };
            display_state(Some(&r), twins, &keys(connected), control)
        };
        // Two of the same TV when the display was assigned, the second one chosen.
        let both = ["Laptop", "Samsung", "Samsung"];
        assert_eq!(
            state(("Samsung", 2), 2, &both, Some(0)),
            DisplayState::Connected(2)
        );
        // One unplugged: the one left might be either, so neither is used.
        assert_eq!(
            state(("Samsung", 2), 2, &["Laptop", "Samsung"], Some(0)),
            DisplayState::Missing
        );
        // A look-alike added later to a display assigned when there was one:
        // it keeps the position it had, and isn't guessed otherwise.
        assert_eq!(
            state(("Samsung", 1), 1, &both, Some(0)),
            DisplayState::Connected(1)
        );
        assert_eq!(
            state(("Samsung", 0), 1, &both, Some(0)),
            DisplayState::Missing
        );
    }

    #[test]
    fn older_shows_recorded_the_systems_name_and_new_ones_the_model() {
        let monitors = vec![
            MonitorKey {
                identity: "Built-in Retina Display".into(),
                raw: "Monitor #1".into(),
                hardware: None,
            },
            MonitorKey {
                identity: "SAMSUNG".into(),
                raw: "Monitor #2".into(),
                hardware: None,
            },
        ];
        let find = |name: &str| {
            let r = DisplayRef {
                name: name.into(),
                index: 1,
                hardware: None,
            };
            display_state(Some(&r), 1, &monitors, Some(0))
        };
        assert_eq!(find("SAMSUNG"), DisplayState::Connected(1));
        assert_eq!(find("Monitor #2"), DisplayState::Connected(1));
    }

    fn hw(identity: &str, raw: &str, hardware: Option<&str>) -> MonitorKey {
        MonitorKey {
            identity: identity.into(),
            raw: raw.into(),
            hardware: hardware.map(str::to_string),
        }
    }

    fn assigned(name: &str, index: usize, hardware: Option<&str>) -> DisplayRef {
        DisplayRef {
            name: name.into(),
            index,
            hardware: hardware.map(str::to_string),
        }
    }

    #[test]
    fn the_same_monitor_is_found_by_its_hardware_whichever_port_or_name_it_has() {
        let r = assigned("SAMSUNG", 1, Some("SAM0F6B:00012345"));
        // It moved to another port: the system's name and position changed.
        let moved = [
            hw("Dell", r"\\.\DISPLAY1", Some("DEL0001:AAA")),
            hw("SAMSUNG", r"\\.\DISPLAY3", Some("SAM0F6B:00012345")),
        ];
        assert_eq!(
            display_state(Some(&r), 1, &moved, Some(0)),
            DisplayState::Connected(1)
        );
        // Another monitor with the same name but not the same hardware isn't it.
        let other = [
            hw("Dell", "A", Some("DEL0001:AAA")),
            hw("SAMSUNG", "B", Some("SAM0F6B:99999999")),
        ];
        assert_eq!(
            display_state(Some(&r), 1, &other, Some(0)),
            DisplayState::Missing
        );
        // The same make and model with another serial number is another monitor too.
        // When the system tells nothing at all, the name has to do.
        let silent = [hw("Dell", "A", None), hw("SAMSUNG", "B", None)];
        assert_eq!(
            display_state(Some(&r), 1, &silent, Some(0)),
            DisplayState::Connected(1)
        );
    }

    #[test]
    fn identical_monitors_share_a_hardware_model_and_are_told_apart_by_order() {
        let tv = |serial: &str| hw("SAMSUNG", "x", Some(&format!("SAM0F6B{serial}")));
        // No serial numbers: the same id twice.
        let monitors = [hw("Laptop", "L", Some("APP0001")), tv(""), tv("")];
        assert_eq!(crate::show::twins_of(&monitors, 1), 2);
        assert_eq!(crate::show::twins_of(&monitors, 0), 1);
        let r = monitors[2].reference(2);
        assert_eq!(
            display_state(Some(&r), 2, &monitors, Some(0)),
            DisplayState::Connected(2)
        );
        // With serial numbers they are different monitors, found whatever the order.
        let pair = [tv(":A1"), tv(":B2"), hw("Laptop", "L", Some("APP0001"))];
        let b = pair[1].reference(1);
        assert_eq!(crate::show::twins_of(&pair, 1), 2, "still look alike");
        let swapped = [pair[1].clone(), pair[0].clone(), pair[2].clone()];
        // (`twins` as the UI records it: two of that model, so 2.)
        let twins = crate::show::twins_of(&pair, 1);
        assert_eq!(
            display_state(Some(&b), twins, &swapped, Some(2)),
            DisplayState::Connected(0)
        );
        // With the other one unplugged it is still found, and a serial that
        // isn't there is still missing.
        let alone = [pair[1].clone(), pair[2].clone()];
        assert_eq!(
            display_state(Some(&b), twins, &alone, Some(1)),
            DisplayState::Connected(0)
        );
        let gone = [pair[0].clone(), pair[2].clone()];
        assert_eq!(
            display_state(Some(&b), twins, &gone, Some(1)),
            DisplayState::Missing
        );
        // Without serials a lone look-alike could be either, so it is not used.
        let plain = [tv(""), tv(""), hw("Laptop", "L", Some("APP0001"))];
        let r = plain[1].reference(1);
        let one = [plain[0].clone(), plain[2].clone()];
        assert_eq!(
            display_state(Some(&r), 2, &one, Some(1)),
            DisplayState::Missing
        );
    }

    #[test]
    fn disabled_displays_are_not_routed() {
        let mut o = display(1, Some(("TV", 1)));
        o.enabled = false;
        let r = route(&[o], &["Laptop", "TV"], Some(0));
        assert!(!r.contains_key(&OutputId(1)));
    }

    #[test]
    fn sub_displays_are_areas_of_their_displays_monitor() {
        let left = Corners::rect(0.0, 0.0, 0.5, 1.0);
        let right = Corners::rect(0.5, 0.0, 0.5, 1.0);
        let outs = [
            display(1, Some(("Projector", 1))),
            sub(10, 1, left),
            sub(11, 1, right),
            sub(12, 99, left),
        ];
        let monitors = keys(&["Laptop", "Projector"]);
        let r = route_outputs(&outs, &monitors, Some(0));
        assert_eq!(
            r[&OutputId(10)],
            Some(Target {
                monitor: 1,
                area: left
            })
        );
        assert_eq!(
            r[&OutputId(11)],
            Some(Target {
                monitor: 1,
                area: right
            })
        );
        // The whole display is still the whole of it.
        assert_eq!(r[&OutputId(1)].map(|t| t.area), Some(Corners::default()));
        // A sub display of a display that isn't there shows nowhere.
        assert_eq!(r[&OutputId(12)], None);
        // Projector unplugged: everything on it shows nowhere else.
        let r = route_outputs(&outs, &keys(&["Laptop", "TV"]), Some(0));
        assert!(r.values().all(|t| t.is_none()));
    }
}
