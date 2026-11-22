# The compositor and displays

Covers `src/gpu.rs`, `src/render.rs`, `src/upload.rs`, `src/adapters.rs`, `src/mesh.rs`, `src/blend.rs`, `src/pattern.rs`, `src/color.rs`, `src/monitor.rs`, display routing in `src/app.rs`.

Moved word for word from the Layout section of `CLAUDE.md` (commit 2589061). Read this before
working in these files, and keep it true when the code changes. A heading that says "Part of"
is a piece of a longer entry that covered several areas.

- `src/gpu.rs`, `src/render.rs`: the compositor and the per-window render threads.
  Every `queue.submit` goes through `Gpu::submit` and every `surface.configure` through `Gpu::configure`
  (`Gpu::gate`, a RwLock): configuring waits for the device to go idle and fails if another thread submits
  meanwhile; a Mac whose external display went off left the control window undrawn for 34 s.
  `src/upload.rs`: texture uploads are off the main thread. `Uploader` (an "uploads" thread) takes each
  frame from `update_outputs` (a copy into a recycled buffer, or the engine's `Arc<RgbaImage>` for
  stills, clocks and cards), latest wins per texture (`Pending`), writes it, submits (`Gpu::flush_uploads`:
  wgpu holds each staging copy until a submit, and a Mac stops presenting a background window, so nothing
  else submits; this was 81 GB in a Medium test run in the background) and polls the device for a loss
  every `POLL_EVERY`. A layer is drawn only once `LayerTexture::is_filled`; `App::uploads_waiting` redraws
  when it arrives. Two Mac tests had the main loop wait 100-180 ms in `write_texture` with 1-5 ms of CPU;
  headless, another thread's submit made a 1080p upload wait up to 2 s. `NEWQ_UPLOADS_ON_MAIN=1` keeps the
  old path. Textures are still made on the main thread. Tried headless only.
  `src/adapters.rs` (pure): which adapter (`better`: wgpu's pick stands unless software; else discrete,
  integrated, virtual, other; software (`Kind::of`: Cpu or Basic Render Driver, llvmpipe…) only alone),
  `SOFTWARE_GRACE` (3 s after a loss), `look_every`, `switch_back`/`failed_switch`/`after_failed_switches`
  (30 s, 2 min, 8 min, give up), `SoftwareDrawing` (`StatusReport.software_drawing`, serde default),
  `status`. Windows' basic display driver per screen (no driver for its chip, Code 31):
  `monitor::windows_adapter` (`EnumDisplayDevicesW` on the output's `\\.\DISPLAYn`: `DeviceString`, and the
  untranslated `Service` of `video_service_key(DeviceKey)`) -> `adapters::is_basic_display_driver` (pure)
  -> `Identity`/`MonitorInfo::basic_driver` -> `MonitorReport.basic_driver` and `MonitorRate.basic_driver`
  (serde default, unwritten when false); words `basic_driver_sentence`/`_report`, said by
  `ui/network::slow_drawing`, `net_judge::drawing_findings` (a Problem), `basic_driver_status` (status
  bar), `monitor_label` and an `Engine::problem` when the monitors change. The Windows call never ran here.
  `Gpu::new(instance, window, allow_software)`; every rebuild uses a fresh `gpu::instance_for`
  (an old DXGI factory never sees the card back). `app.rs`: `SoftwareWatch` looks on a `gpu-look` thread
  (`gpu::hardware_offered`) and sets `to_hardware`, which `watch_gpu` rebuilds by the loss path.
  `gpu::drawing_in_software()` feeds `LocalFacts.software`, `slow_drawing` and Test Mode. `render.rs::Ready`:
  a surface that won't configure skips frames (`Gpu::try_configure`, retried every 250 ms;
  `get_current_texture` in `catch_expected`). A 0.24.3 laptop lost its NVIDIA driver to a Windows driver
  update mid-test and stayed on Basic Render Driver. Tried under Xvfb with a fake look answer only.

## Part of the `src/remote.rs` entry (a run of other files' notes): display slots

  Displays: the show has `DisplaySlot`s (Display 1, 2…) each assigned to a
  monitor by the user; `show.rs::slot_state` finds the monitor and never picks
  another, `app.rs::route_outputs` places outputs, and `Show::assign_displays`
  gives older shows (Auto/Display placements) their displays once.

## Part of the `src/remote.rs` entry (a run of other files' notes): mesh warp, edge blend, part of a picture

  `src/mesh.rs`: the mesh warp (a Hermite spline through a grid of points);
  the vertex shader in `gpu.rs` does the same sums, cutting each layer into
  a 48x48 grid of cells, and a test renders it and compares.
  `src/blend.rs`: the edge blend maths (test-only reference for the blend
  shader in `gpu.rs`). A display can show a part of another's picture
  (`OutputConfig::part_of`): `app.rs::update_outputs` draws the other
  display's layers through that part; windows drawing the same layers need
  different `Slot::Window`s or their uniforms overwrite each other.

## Part of the `src/remote.rs` entry (a run of other files' notes): test patterns

  `src/pattern.rs`: the fourteen test patterns (`show::PatternKind`), drawn on the CPU by
  `render_shaped` (`render` is the tests'), given the shape of the output's canvas: the default
  `SmpteHd` (`smpte_hd`, ARIB STD-B28 / SMPTE RP 219 from the standard's figure and Table A-5:
  rows 7/12, 1/12, 1/12, 3/12; the coloured ends stay 16:9's width on wider displays; tested pixel
  by pixel at 1920x1080) and the classic `Smpte` (0.24.6's default) keep their bars 4:3 and fills the rest with grey side panels (none at 4:3 or narrower;
  0.24.6's port had 16:9/16:10 variants, dropped before release); `app.rs` caches one per display by
  kind, number, drawn size and canvas. `PatternKind::draws_number` greys the Number checkbox. The SMPTE bars are full-range RGB: black is 0 everywhere,
  PLUGE above black `ABOVE_BLACK` (10), below black can't exist so it is 0. `PatternKind`
  deserialises leniently (an unknown name is the default), shows too. `OutputConfig::pattern_number`
  (the Number checkbox, on unless written `false`): `app.rs` passes number 0, which `pattern::render`
  draws as no number.

## Part of the `src/remote.rs` entry (a run of other files' notes): device loss, colour, monitor names

  `src/gpu.rs` also notices a lost graphics device (`DeviceLost`) and
  `app.rs::watch_gpu` rebuilds every window's device, surface and render thread;
  `NEWQ_LOSE_GPU_AFTER=<seconds>` destroys the device on purpose to test it.
  `src/color.rs`: colour adjustment and chroma key maths; the shader in
  `gpu.rs` does the same per pixel (a test renders it on a real GPU if there
  is one; `NEWQ_REQUIRE_GPU=1` makes having none a failure).
  `src/monitor.rs`: what a display is called (a Mac's name for it) and where a
  window starting on it goes. winit's Mac `name()` is "Monitor #" and a number,
  and its `position()` is in that display's own pixels, so convert to points.
