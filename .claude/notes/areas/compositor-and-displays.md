# The compositor and displays

Covers `src/gpu.rs`, `src/render.rs`, `src/upload.rs`, `src/adapters.rs`, `src/mesh.rs`, `src/blend.rs`, `src/pattern.rs`, `src/color.rs`, `src/monitor.rs`, display routing in `src/app.rs`.

Moved word for word from the Layout section of `CLAUDE.md` (commit 2589061). Read this before
working in these files, and keep it true when the code changes. A heading that says "Part of"
is a piece of a longer entry that covered several areas.

See also: `network.md`'s "after the 0.24.4 review" block has `upload::first`, the uploads thread's
restart and `adapters::look_every`.

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

## Mapped projectors' pictures (mapping plan round 1, `.claude/notes/mapping-plan.md` 4.1, 4.5)

  `src/mapped.rs` (`mapped_tests.rs` beside it): a mapped projector's display shows, per pixel,
  its feed's canvas at the place a `LensMap` gives (byte layout in the module header, packing
  functions `pack_place`/`pack_how`/`pack_depth` and `WGSL_UNPACK`; `CpuMap` and
  `LensMap::from_cpu` for made-up maps; `LensMap::stamp` is unique per map). `Drawer` (one per
  thread that draws: made at the first mapped frame by `draw_mapped`, in error scopes, so a window
  with no mapped display never makes one) draws each `MappedFrame`'s canvases with the compositor
  through `Slot::Map(owner, frame * 4 + feed)` (`MapOwner::number`: windows 0-7, captures 8-15,
  throws 16-31; indices past every `Slot::View`), then the map pass (`MAP_SHADER`, one triangle,
  `textureLoad` of the map, `textureSampleLevel` of the feed, times coverage and, with `shares`,
  `share^(1/gamma)`) into `frame.target`, marked filled once (its stamp then stays). Canvases,
  uniforms, bind groups (by map `Arc` and canvas serial) and the target's view are kept between
  frames. `render.rs`: `OutputJob::Frame::mapped` (`OutputThread::submit_mapped`) and
  `CaptureJob::Picture { groups, mapped }` (`CaptureThread::submit_mapped`); both loops call
  `draw_mapped` before their own composite and submit it all together (with none, `output_loop`
  submits `[cmd]` as before). `Shot::mapped` (map stamp, shares, canvases' layer shots, moving
  screens) tells a mapped window's new pictures. `app.rs`: `lens_maps` asks
  `StageLive::lens_map` every pass for each `mapped::wanted` (the window at its area's size, a
  stream at the stream's size) and keeps `mapped_targets` (made again only at a new size, cleared
  with a lost device); a mapped display's `OutputSetup` ignores its corners, mesh, part and picture
  size and takes `Projector::fine` as its mesh; its one layer is its target (`whole_layer`), none
  till its map lands (black); it is not in `pictures`. `mapped_frames` builds the frames after the
  loop from `stage_live::feed_groups` in the map's feed order. The screen's submit carries the
  frames of the displays on it (`maps_sent`: a new map, or a canvas's groups moved, redraws
  though nothing plays); `make_map_slots` makes the `Slot::Map`s on the main thread
  (`LayerTexture::ensure_slot`) so the output thread makes none. `lens_maps`, `canvases_of`,
  `mapped_frames`, `may_stream` are free functions, tested in `app::tests`. `is_preview_work`/`uploads_now`:
  a canvas a map here uses (`map_feeds`, and `StageLive::feeds_output`) is never preview work.
  `update_streams`/`may_stream`: a mapped display is streamed though it keeps `part_of`, at
  `stream_picture` (the projector's own pixels), as its target stretched, `shares` off. Tests:
  made-up maps pixel for pixel, the fine-tune mesh, two windows on one canvas, slots apart from
  views, naga HLSL/MSL, an unmapped display unchanged byte for byte, the new-picture count, the
  stream through FFmpeg, the preview rule (pure). Tried on lavapipe only; never a real projector,
  card or the app with a baked map.
  Round 3 (agent B), **the live pass** (`mapped_live.rs`, `mapped_live_tests.rs`; plan 4.2): a
  screen `Show::movable` names is never in a map; every piece of one (moved or at home) is a
  `MovingScreen` in the frame, drawn in the map pass's own render pass after its triangle: corners
  through `MappedFrame::lens` times `matrix`, uv through the piece's `region` into its feed's
  canvas, hidden where `LensMap::depth` is nearer than the fragment's distance from the lens
  (`live::eye_of(lens)`) by more than `lensmap::SCREEN_SLACK` (depth 1.0, all 255, is nothing; a
  map without depth hides nothing), faded over the last pixel by uv (one-piece screens only: a
  faceted screen's pieces don't fade, or their joins would show), black and in the way when turned
  away (`front_only`) or showing no canvas of the frame (unfed, LED, a fifth). No depth buffer: the
  main thread sorts the pieces far to near (`live::moving_for`, by each piece's box middle from
  the lens). `LiveKit` (pipeline) is made only at the first frame with a moving screen, in its own
  error scope; `FrameLive` keeps a frame uniform, one pieces buffer of `MAX_PIECES` (256) slots
  with dynamic offsets (stride from `min_uniform_buffer_offset_alignment`) and a reused staging
  `Vec`; meshes are uploaded once by `mesh_key` (`Drawer::meshes`, dropped only when a new one is
  made and they weren't drawn lately). A frame with none draws exactly as before (byte for byte,
  also on a drawer that drew some). `app.rs`: `MappedTargets` carries the shape each map was made
  at (`p.aspect(size)`, or `shape_of` for a throw) and `mapped_frames` sets `lens` from
  `lensmap::lens_matrix(p, aspect)`; `MovablePieces` (`live::movable_pieces` from
  `UiState::stage.scene`, made again only with the scene's key or the movable set) and one
  `Engine::moved_screens` snapshot a pass, used by every frame (window, stream, throw) and by
  `StageLive::send`; `maps_sent` and `Shot` keep moving screens as plain values
  (`mapped::moving_shots`, no mesh compared) and `Shot` the lens. Tests: moved flat, curved and
  faceted screens, a box in the way, in front of and behind a still screen, turned away and rear,
  two projectors from one tick, 16 moving screens and a still one, none (byte for byte), a stream's
  owner, naga HLSL/MSL; each against `lensmap::reference` with the screen standing there, through
  the map pass, within 3 steps away from edges, with made-up depth pictures (`pick`'s distance).
  Tried on lavapipe only.
