# NewQ

A cue-based playback system for theatre and live events, in the spirit of QLab.
Rust, with FFmpeg run as subprocesses, wgpu for compositing and egui for the UI.
The README describes the features; this file is about working on the code.

## Layout

- `src/show.rs`: the show data model and `.newq` (de)serialisation, plus the
  group, arm, cue-list and audio-output structure helpers. Cue lists: `Cue::list`
  (0 = main) and `Show::lists`; the cues of a list are kept together in
  `Show::cues`, the main list first (`normalize_lists` repairs a loaded show), so
  `next_top`, `move_cue` and `insert_slot` stay inside a list. The engine's
  `playhead` is the active list's; the other lists' are in `list_playheads`.
- `src/engine.rs`: GO, pre-waits, continues, transitions, fades, groups, preload.
- `src/media/`: `ffmpeg.rs` (find and probe), `video.rs`, `audio.rs` (devices,
  routing, mixer), `playback.rs` (looping passes).
- `src/gpu.rs`, `src/render.rs`: the compositor and the per-window render threads.
  Texture uploads happen on the main thread (`app.rs::update_outputs`) and wgpu holds each
  one's staging copy until the next `queue.submit`; a Mac stops presenting a background
  window, so nothing else submits: `Gpu::flush_uploads` after each frame's uploads (this
  was 81 GB in a Medium test run in the background).
- `src/awake.rs`: `KeepAwake` holds the computer awake (caffeinate, `SetThreadExecutionState`,
  systemd-inhibit) while a test runs; `SleepWatch` notices a sleep by the wall clock running ahead of
  `Instant`, which stops in sleep on a Mac and Linux (Test Mode reports it and leaves the laps it
  fell in out of the lap checks).
- `src/splash.rs`: the startup splash window, drawn on the processor with `softbuffer` (`compose`
  makes the picture and is tested; the window code is in `Splash`). `app.rs::resumed` opens it,
  and the main window's set-up (`open_main_window`, the slow part) begins after the splash's first
  redraw event so it is on screen first; `about_to_wait` closes it (`splash::due`).
  `NEWQ_NO_SPLASH=1` skips it. Not tried on a real Mac or Windows desktop.
- `src/macwin.rs` (Mac only): an output window covers its display as a borderless window above the
  menu bar's level, on all Spaces, stationary, no shadow (winit's simple fullscreen hid the menu bar
  for the whole app, only auto-hid it, and left the window to Mission Control; it stays the fallback).
  Checked for the Mac target with `scripts/cross_clippy.sh mac`, not tried on a Mac.
- Starting video and audio: FFmpeg is spawned by the reader thread (`video.rs::start`, `spawn_hap`,
  `audio.rs`), not the main loop: a process start took 3 to 4 s now and then on a Mac. A start that
  fails shows as `error()` and no frames. `Engine::update_preload` also preloads the children of
  the group that an auto-follow leads to (`MAX_PRELOAD_CHILDREN`), so a group of layers starts warm.
  `crumbs.rs` leaves a note after each part of the main loop; a stall over 100 ms is logged with them.
- Display loss: `app.rs::control_for_outputs` treats the main window as being on no display while its
  remembered display (`control_home`) is disconnected (a lid closed), so an output stays on its display
  even though the system moved the window there. `HomeTracker` (app.rs) makes a display the window's home
  only after it has stayed there 10 s and never while the home is disconnected: a closing lid moves the
  window to the other display a moment *before* the system reports the first gone.
  An output window is kept when its display only changes number (`renumbered`; the lid closing makes the
  external display 0): closing and recreating it showed the desktop. Its render slot is its own (`ScreenWindow::slot`).
- Output windows are kept over their display (`app.rs::pin_outputs`, each pass, Mac): `macwin::frames`
  compares the window's frame with its screen's (both global points) and `monitor::should_pin` decides;
  `macwin::watch_screens` (NSApplicationDidChangeScreenParametersNotification) sets a flag and wakes
  the loop so the look at the displays is prompt; the 2 s poll is the safety net. `pin_outputs` runs before
  and after `scan_monitors` (which the system can block for 200 ms while rearranging), `pin_output(m)` runs
  on the output window's `Moved`/`Resized`, at most once per `monitor::PIN_SPACING` per window. The window
  is backed with black (`NSWindow::setBackgroundColor`). The 0.5 to 0.7 s freeze at a lid change is the
  system holding the next drawable (`nextDrawable` blocks, wgpu has no timeout); nothing to do about it.
  Not tried on a Mac (checked by `scripts/cross_clippy.sh` and CI).
- `src/freeze.rs` (Mac): `about_to_wait` beats; a watchdog thread runs `/usr/bin/sample <pid> 2 -file
  newq-freeze-<time>.txt` in the log folder when the beat stops for 1.5 s while outputs are open (at most 5
  a run); Test Mode copies them into its run folder (`copy_samples`). `macwin::hide_pointer`
  (`NSCursor::setHiddenUntilMouseMoves`) is called when the system says the displays were rearranged and
  after each put-back, since the pointer shows on the output when a lid closes.
- `src/power.rs`: on battery or not (pmset, GetSystemPowerStatus, /sys/class/power_supply) on a thread of
  its own; `app.rs::keep_awake_for_outputs` holds `KeepAwake` while an output window is open
  (`Prefs::let_computer_sleep` turns it off), logs sleeps (`SleepWatch`) and the power source, and sets
  `UiState::keeping_awake/on_battery` for the status bar and the General page's badge.
- Panics are lived through, not fatal (`crash.rs`): `survive(what, f)` is a `catch_unwind` that the main
  loop wraps each pass in (`about_to_wait` -> `App::pass`, with the parts the rest can do without, the
  engine tick and the outputs each in their own `App::part`) and each window event (`after_window_panic`
  -> `UiState::after_panic` lets go of the selection and open dialogs). The hook writes a report for the
  first two panics at one place and 40 a run, then only counts; a panic that is survived or on a worker
  thread shows no message box and becomes a status-bar notice (`take_notices`). Only the main thread
  outside `survive` is fatal; `open_main_window` reports a failure to start itself and exits. Worker
  threads say they died with `crash::OnPanic` (the video/audio readers set their cue's error, a stream
  reports a failure and is retried); `watch_render_threads` restarts an output or control render thread
  that ended (at most one restart every 2 s); `mix_each` drops a voice that panics in the audio callback
  (a panic leaving a CoreAudio callback aborts); `update_hosting` replaces a dead MediaMTX runner.
  `NEWQ_TEST_PANIC=<engine|ui|outputs|render|audio>@<seconds>` makes one happen. Never take time from an
  `Instant` (`Instant::now() - d` panics soon after boot): use `Option<Instant>` or `checked_sub`.
  A `clamp` whose bounds come from a show or a file panics when they are inverted or NaN (`ui.rs::within`).
- `src/logging.rs`: the log. `LogLevel` (`Prefs::log_level`, Settings ▸ General ▸ Recovery and logs;
  `set_level` applies it at once) or `NEWQ_LOG` (`Spec`: a level and `part=level` pairs; it wins, and the
  setting is greyed). Always to `newq.log` in the log folder (and stderr except on a Windows release),
  through a bounded queue to a `log-writer` thread (`write_loop`; a full queue drops and counts), put
  aside as `newq.previous.log` at 20 MB (`Rotating`). Libraries (targets not `newq::`) are a step quieter
  (`for_libraries`). Levels: `Engine::problem` (warn) for what went wrong and `Engine::log` (info) both
  show in the status bar; debug is each cue's start/playing/out/end (`Engine::log_cue_changes`) and each
  FFmpeg's command line; trace is preloads and FFmpeg exits. The panic hook flushes the log.
- `src/app.rs`: windows and wiring. `src/ui.rs`: the control UI.
  Group crossfade: in a "play in order" group a child whose next sibling crossfades in (`crossfade_into_next`) is not faded out by itself, and `advance_groups` starts the next one that long before it ends; a held next cue (`holds`: picture, text, shape, camera) gets the overlap added to its hold (`Engine::started_early` -> `ActiveCue::extra_hold`, which the per-tick hold update adds back), so the group keeps its timing; a video can't, so `cue_duration` subtracts a crossfade into a video, and a next child with a pre-wait doesn't overlap. Shape cues: `show::ShapeCue` (`ShapeKind`), drawn by `shape::render` at the display's size on a thread (`Engine::prepare_shape`, which hands the picture over as `PreparedKind::Text`, like a text cue). Editing several cues: `inspector` wraps `inspector_one` and `spread_edit` makes in the other picked cues what changed (`bulk_edit::spread` compares the cue as JSON before and after, skipping the fields that are a cue's own, `target` and `device` included). A list missing from the file is empty, so a list that appears or empties is a length change and isn't spread; an item with a `kind` (an effect) is addressed as `<index>#<kind>` and changes only the same kind; `start`/`end` reach only the same cue type (`SAME_KIND_ONLY`); `inspector` spreads only on a frame with user input. `Engine::tick` copies the show's layer, geometry, colour, effects, fit, output and hold time into the cues playing (the block under `if let Some(m) = media`), and a shape or typed text is drawn again when `drawing_key` changes (`ImageState::redraw`); what is fixed at start (file, in/out, loop, speed, audio routing, fade-in) is not.
  Selection state: `multi` only counts while `selected == multi_for` (`sync_selection` at the start of `draw` and `apply`, and `forget_multi` in `load_into`, `restore` and the test stash), so an old selection never returns when ids repeat in another show. The clipboard remembers the show it came from (`show_epoch`) and whether it was cut: `paste_cues` remaps a fade/stop/control cue's target to the copy, clears targets from another show, and gives a cut its old ids back. The right-click menu's Fire/Fade out/Stop name the clicked cue (`FireCue` etc.), not `selected`. Copying also writes a short text to the system clipboard, since egui-winit sends Paste only when it holds some text. In a group, `crossfade_into_next` applies only to pictures on the same display and layer, and the automatic fade-out waits for the crossfade only while it is coming or running (`running` set in `tick`).
  Layers: `MediaCue::layer` (higher on top; `Engine::layers` sorts by it); with `Show::own_layers` (new shows; a file without it, from 0.18.0 and before, has it off and replaces everything on the display as it did) `apply_entry` replaces or fades only the cues on the entering cue's own layer and display (`layer_no`, `ActiveCue::own_layer`), a dip's matte replaces all. The children a "start all together" group fires share the display (`together`): the group records them in `Engine::together_for`, read at each start attempt (a start can wait for its file and come back) and removed when the start ends, so a child fired on its own later replaces as usual. Copy and paste: `UiState::clipboard`, `Show::copy_cues` and `Show::paste_cues` (new ids, display/audio output mapped to the pasting show's); Ctrl+C/X/V are `Event::Copy/Cut/Paste`, handled in `keyboard` after the typing check.
  The cue list picks several cues (`UiState::multi`, only while `selected` is still `multi_for`, so any other way of choosing ends it; read it with `UiState::selection`): Ctrl/Cmd-click toggles, Shift-click takes a run of visible rows. Rows drag with egui's drag and drop (`DraggedCues` payload); the drop spot is worked out per row (top half before, lower half after, a group's middle into it) and `Show::move_cues` (`DropAt`) does the move, keeping groups whole and children in their groups. Delete, Duplicate, Move and arm act on the whole selection; so do `GroupCue` (`Show::group_cues`) and `MoveIntoGroup` (`move_cues` with `DropAt::Into`) from the cue menu, which keeps the selection when you right-click one of its cues (`cue_context_menu`'s `picked`). Tests drive the list with the mouse (`ListMouse`, rows recorded under `cfg(test)`); not looked at on a real screen beyond that.
  `src/ui/modern.rs`: the optional new interface (`Prefs::new_layout`, Window menu; `Action::SetNewLayout`): `draw` replaces the transport and dock when it is on, with `View`s Editor (cue list, inspector, active), Operator and Patch (painter-drawn signal flow; a drag from a cue port sets `MediaCue::output` or `audio_output`), reusing `cue_list`, `inspector`, `active_panel` and `output_previews`. The settings take the view's place while open. Tests draw each view and drag routes; `PatchRects` is test-only.
  `src/ui/settings.rs`: the settings page (a sidebar of `SettingsPage`s in
  groups, with badges; it replaces the cue list while open). A setting goes on
  the page it belongs to; the displays, audio and sub display panels are
  functions in `ui.rs` that the pages call.
- `src/dock.rs`: the pane layout (an `egui_dock` tree plus the popped-out panes),
  saved per computer as `layout.json` in the log folder, not in the show.
  `ui.rs::dock_area` draws the tiles; each pop-out is an OS window in
  `app.rs` (`PopoutWindow`) with its own egui context and render thread, drawn by
  `ui::draw_popout`. Opening a surface while other windows draw can fail to
  configure, so `Gpu::configure_checked` retries. A lost device drops the
  pop-outs and `reconcile_popouts` opens them again from the layout.
- `src/ndi.rs`: NDI in and out. The runtime (NDI's own library, not shipped) is
  loaded with `libloading` only inside a helper that is NewQ's own program run as
  `newq --ndi-list|--ndi-receive|--ndi-send` (see `main.rs`), so its faults can't
  take the show down; pictures cross over stdin/stdout. An NDI source is a camera
  cue's device (`ndi:` + the source's name), and `StreamKind::Ndi` is the output.
  The structures follow NDI's SDK 6 headers (not vendored: their licence);
  tests build a fake runtime in C with `cc` (skipped without one) and the pure
  converters are tested, but no real NDI has been tried.
- `src/media/hap.rs`: Hap frames (section header, Snappy, chunked); FFmpeg only
  demuxes (`-c copy`, `video.rs::spawn_hap`) and the texture data goes to
  `LayerTexture::new_bc/upload_bc` (BC1/BC3, Hap Q's YCoCg is shader mode 2). Needs
  `TEXTURE_COMPRESSION_BC` (`video::plays_as_hap`; `Gpu::new` sets the flag); a
  speed is in the frames' times. `media/convert.rs` makes Hap copies; `bench.rs` is
  `newq --bench-video` (headless; `Gpu::headless`). Tests compare our decode to
  FFmpeg's own for every format.
- `src/speed.rs`: keyframed playback speed. `SpeedMap` integrates a cue's speed
  (`MediaCue::rate`, an `Animated`) into file time and back; `Stretcher` is a
  WSOLA time stretcher the audio reader runs along a `SpeedPlan`; the video
  reader (`video.rs::read_at_speed`) picks frames by the same plan. A steady
  speed still uses FFmpeg's `atempo`/`setpts`. Loops follow the plan too: the readers
  see the passes as one run of frames, and `ActiveCue::pass_len` is then in the
  file's seconds (`pass_time` maps the cue's time into them).
- Animated geometry: `show::Motion` (six optional `Animated` tracks on
  `MediaCue::motion`, evaluated in `Engine::tick`); a fade cue's `geometry` ramps an
  `ActiveCue::live_transform` (`LiveTransform`) that then wins over the tracks.
  Fade cue `matrix` targets set `VoiceShared::xp_target/xp_rate` (up to
  `audio::MAX_FADEABLE` crosspoints, kept in `Matrix::keys` order); `mix_voice`
  moves the gains per buffer. Silent crosspoints are kept in the matrix for this.
- `src/feedback.rs`: OSC feedback to the address in `RemoteSettings::osc_feedback`
  (`snapshot`/`changes`/`repeat` are pure and tested; `Feedback` paces and sends).
- `timecode::Generator` makes MTC messages; `output::MtcRunner` is its paced thread
  (`ActiveKind::Timecode`, dropped to stop). Send cues' `Program` target runs only
  when `Prefs::allow_scripts` (per computer; `output::set_scripts_allowed`).
- `src/testmode/`: Test Mode (Help menu), no command line. `assets.rs` is the pinned
  catalog of free media (hashes, `.part` resume downloads with `ureq`, checks by
  decoding, generated fallbacks named `generated_*`; tested against a loopback HTTP
  server, and an ignored live test `real_downloads_match_the_catalog`), `plan.rs` builds
  the Short/Medium/Long shows from the user's show (keeping its outputs), `session.rs`
  runs one (`TestMode::update` each frame from `app.rs`; always restores via
  `UiState::begin_test/end_test`, writes the run folder, never lets autosave run while
  `test_locked`), `analysis.rs` turns samples into findings (pure, tested; growth is judged on per-window
  peaks, middle third against last third by medians, so a warm-up step that levels off is only a
  `Severity::Watch` "tracking", which doesn't count against the verdict; `session::FindingLog`
  writes every finding the window shows, and when it goes, to `events.log` and the report), `decode.rs` is the
  hardware decoding comparison (a 4K clip made from the downloaded film, four looping
  tiles played with `video::set_hw_decode` off then on; a cue with a plain cut
  transition takes the place of the layers before it, so tiles use `FadeIn`),
  `gui.rs` is the window (its "Show the test on" ticks are `Prefs::test_displays`;
  `plan::use_displays` replaces the show's outputs with them before the cues are made, the first
  display shows the picture and the others mirror it with a whole-area `part_of`; the
  main window's own monitor is never offered). The full check (`feature.rs`, the "Run the full check"
  button, `Phase::FullPrep`/`Full`, driven by `full.rs`): `make_media` makes a clip, tone, stills and a Hap
  clip in the run folder; `build` makes a show with every cue kind and a list of `Part`s (acts at times,
  `Check`s); `Runner` plays them one at a time, its clock starting when the part's `lead` cue is started,
  noting `Obs` every 20 ms; `judge` (pure, tested on made-up notes) gives each check's `Verdict`. A problem
  the engine reports in a part (`Engine::take_problems`) fails it unless the part expects it. The copy has
  `own_layers` on (an old show's off makes cues in a together group replace each other: that is tested
  behaviour, not a fault), no streams, no OSC feedback. `full.rs` does what needs the app: `Ask::RecordOn`
  sets a `StreamKind::Record` on the first display and `judge_recording` probes the file once it has stopped
  growing; `Ask::ServeOn` streams to a server on `streamlink::LOOPBACK` (127.0.0.1, free ports) and asks it
  over RTSP whether the stream is there; skipped unless `mediamtx::locate()` and `Prefs::allow_hosting`.
  `bundle.rs` zips the run folder plus newq.log, newq.previous.log and newq-crash.log beside it
  (`NewQ-test-<run>.zip`, on a thread; every test's `Done::bundle`), and `reveal` shows it (`explorer
  /select,"…"` needs `raw_arg`; `open -R`; `xdg-open` of the folder). `selftest.rs` is `newq --self-test
  [--out dir]`: the same `Runner` on an `Engine` with `AudioOut::silent()` (a paced mixer thread, no
  device), exit 0/1/2; CI runs it on all three systems and uploads the zip. Engine-level only: "reached the
  screen" means the cue's layer with a frame was in `Engine::layers`, not that pixels were looked at. The
  window's run was tried under xvfb (recording and a real MediaMTX passed); the sound parts never met a
  real device. Cross-target clippy for Windows and Mac runs from Linux with
  `scripts/cross_clippy.sh` (a stand-in C compiler for `ring`, since nothing is linked); CI still builds and
  tests them.
- `src/clicker.rs`: presenter remote. `decide` is the pure GO/Back logic (Back pauses, Back again
  fades out and stops, GO resumes; `BACK_GUARD` ignores a double press); `Shared` is the learned keys
  and learning flow; a Windows low-level keyboard hook and a Mac `CGEventTap` (Input Monitoring) feed
  it with `Command::Clicker`, the in-window `KeyboardInput` is the fallback. Keys are per computer in
  `Prefs`. Not tried on a real Mac or Windows machine. **Switched off** (`clicker::AVAILABLE`): it
  started the global capture at every launch (a Mac would ask for Input Monitoring) and can't tell the
  remote from the laptop keyboard; see `.claude/notes/next-ideas.md` for the device-aware plan.
- `src/pdf.rs`: draws a PDF's pages as 1920-pixel PNGs with `hayro` (pure Rust, no unsafe, MIT/Apache; the
  PDF's own fonts, Foxit substitutes for the standard 14; no encrypted PDFs). A page that panics is an error
  naming it, not an abort. Compared against poppler on a LibreOffice-made deck: the same pictures.
- `src/pptx.rs` and `src/slides_export.rs`: slides from a presentation. `pptx.rs` reads the slide
  order, hidden slides and first transition (advance and fade) from the XML (pure, tested);
  `slides_export.rs` makes pictures: a PDF through `pdf.rs` (no program needed), PowerPoint (PowerShell
  COM), PowerPoint/Keynote (AppleScript), LibreOffice (makes a PDF that `pdf.rs` draws), or
  `NEWQ_SLIDE_EXPORTER`; `match_timings` pairs pictures with slides.
  `ui.rs` (`start_deck_dialog`, `start_deck_import`, `poll_deck`) runs it on a thread with Cancel and
  `Action::ImportSlides` -> `Show::add_slides_timed`. Not tried against a real office program.
  A deck with no advance times imports with `SlideShow::advance = Advance::OnGo`: the group fires only
  its first slide (`GroupState::deck`), `Engine::deck_step` fires the next or previous child (a fresh
  instance, which crossfades over the older ones: `apply_entry` only fades out layers started before it),
  and `Engine::go` steps it before it fires anything else; past the last slide the deck lets go
  (`DeckStep::AtEnd`). `ui::remote_button` is the one handler for the presenter remote's buttons and for the
  show-mode arrow keys (`keyboard`, and `app.rs::handle_output_key` for the output windows).
- Mac signing: `release.yml` imports a Developer ID certificate into a temporary keychain when the
  secrets `MACOS_CERT_P12`, `MACOS_CERT_PASSWORD` (and, to notarise, `APPLE_API_KEY_P8`,
  `APPLE_API_KEY_ID`, `APPLE_API_ISSUER_ID`) exist; `package_release.sh` then signs ffmpeg, ffprobe
  and the app (hardened runtime, `assets/macos/entitlements.plist`), notarises with `notarytool`,
  staples and zips. Without the secrets it ad-hoc signs as before. A step's own `env` can't be used in
  its `if`, hence the `signing` step's outputs. Works: used for every release since 0.16.0 (the
  notarisation wait is about 30 minutes).
- Windows signing: the `Sign newq.exe` step in `release.yml` runs only with the secrets
  `WINDOWS_CERT_PFX` and `WINDOWS_CERT_PASSWORD`. Untested, and a placeholder: new certificates
  are hardware-backed and can't be exported as a .pfx (SignPath or a cloud service is the way).
- `src/stats.rs`: the `NEWQ_STATS` CSV log and `--stats-report`. `src/soak.rs`: builds the
  self-running soak show: `newq --soak` makes the media with our FFmpeg, runs the app on it
  and writes the report; `--make-soak-show` writes just the show. sysinfo
  lists Linux threads as processes (`thread_kind()`), skip them when summing children.
- `src/fx.rs`: cue effects. `show::Effect` (kind and up to four `Animated`
  numbers, each a value or keyframes) is on `MediaCue::effects`. `fx::VideoFx::at`
  turns the picture effects into the numbers in the compositor shader (blur,
  sharpen, pixelate, negative, vignette in `gpu.rs`, one pass, a test renders them);
  the sound effects (`AudioChain`: biquad EQ/filter, compressor, echo, freeverb)
  are built when a voice is, kept in step with the show by `FxShared` atomics
  that `Engine::tick` sets from the cue's time, and run per frame in
  `audio.rs::mix_voice`. `render.rs::FrameTimer` and the mixer callback's load
  feed the status-bar warnings.
- `src/remote.rs`: OSC and MIDI. `src/text.rs`: text cue rendering.
  `src/history.rs`: undo (JSON snapshots of the show, bursts merged).
  `src/timecode.rs`: MIDI Time Code decoding and the trigger follower.
  `src/output.rs`: send cues' OSC and MIDI messages and the sender thread.
  `src/projector.rs`: PJLink (commands, MD5 digest, the conversation; tested
  against a fake projector on loopback). Projector and TCP sends run on a
  thread per host from `output.rs`.
  `src/export.rs`: the cue sheet as CSV or a printable page.
  `src/clock.rs`: time-of-day triggers (local time via `jiff`).
  `src/crash.rs`: the log file and crash reports (Windows has no console).
  Reports go to the log folder (`%LOCALAPPDATA%\NewQ` etc.); ask for
  `newq-crash.log` and `newq.log` when a user reports a crash.
  `src/web.rs` and `web_page.html`: the web remote (HTTP on plain `std`
  sockets, handlers tested as functions and over loopback).
  Displays: the show has `DisplaySlot`s (Display 1, 2…) each assigned to a
  monitor by the user; `show.rs::slot_state` finds the monitor and never picks
  another, `app.rs::route_outputs` places outputs, and `Show::assign_displays`
  gives older shows (Auto/Display placements) their displays once.
  `assets/logo/`: the logo (SVGs, PNGs, `newq.ico`, `newq.icns`, the raw 128 px
  window icon `src/branding.rs` embeds, and the 1280x640 GitHub social preview).
  Change the artwork in `scripts/make_logo_assets.py` and re-run it (it needs
  fontTools, node with playwright and ImageMagick; see its header), then commit
  what it writes. `build.rs` puts `newq.ico` in the Windows executable; the
  packaging script puts `newq.icns` in the Mac bundle and the icon and a
  `.desktop` file (`assets/linux/`) in the Linux download.
  `src/mesh.rs`: the mesh warp (a Hermite spline through a grid of points);
  the vertex shader in `gpu.rs` does the same sums, cutting each layer into
  a 48x48 grid of cells, and a test renders it and compares.
  `src/blend.rs`: the edge blend maths (test-only reference for the blend
  shader in `gpu.rs`). A display can show a part of another's picture
  (`OutputConfig::part_of`): `app.rs::update_outputs` draws the other
  display's layers through that part; windows drawing the same layers need
  different `Slot::Window`s or their uniforms overwrite each other.
  `src/stream.rs`: sending a display's picture out through an FFmpeg process
  (argument building, `Sink`); `render.rs::CaptureThread` draws the picture off
  screen at the stream's rate and feeds it. A test streams to a file through
  real FFmpeg; the Linux virtual camera itself can't be tested here.
  `StreamKind::Server` (RTSP/RTMP/SRT to MediaMTX and the like) takes its H.264 encoder from
  `stream::encoder` (libx264, then VideoToolbox, NVENC, QSV, AMF: listed *and* proven by a short
  test encode, once per run), and `missing_for` checks the muxer and protocol (the Mac bundle has
  no libsrt, so no SRT). The Mac FFmpeg is built with network on and the `mpegts,rtsp,rtp,flv`
  muxers for this; `package_release.sh::verify_ffmpeg` checks them. Tests publish to FFmpeg's own
  RTSP/RTMP listeners. A hardware encoder must be checked for its keyframe spacing, not just that it
  encodes: AMF's `lowlatency` usage sent one keyframe ever (`ffprobe -show_entries packet=flags`).
  A stream's FFmpeg must never be waited on without a limit: `Sink::finish` kills it after
  `finish_patience(kind)` (3 s; a Custom output gets 60 s for closing a file), `CaptureThread::stop`
  ends it through a `Killer` when the thread is stuck writing to it (a hung server fills the pipe),
  and on Windows it is in a kill-on-close job (`mediamtx::tie_to_us`). Linux and Mac have no
  equivalent for a crash yet. The stress tests (`mediamtx/stress.rs`, `gpu.rs::stress_stop_…`) are
  `#[ignore]`d; run them with `NEWQ_MEDIAMTX_DIR` and `NEWQ_FFMPEG_DIR` set and `--ignored --nocapture`.
  sysinfo only fills `Process::cmd()` when asked (`ProcessRefreshKind::with_cmd`); without it a
  "no FFmpeg left" check passes for the wrong reason.
  `src/mediamtx.rs`: NewQ runs MediaMTX itself (`StreamKind::Server::host_here`): `locate` (next to the
  executable, `NEWQ_MEDIAMTX_DIR`, `third_party/mediamtx`, PATH), `Hosting::config` writes its YAML (listen
  address of the chosen adapter, per-protocol ports; publishing only from loopback and the adapter, read for all,
  `overridePublisher: no`), `Hosting::conflict` finds two ways in on one port, `Hosted` is the child process
  (killed on drop; Linux `PR_SET_PDEATHSIG`, a Windows job object, so a crash leaves none; its last lines say why it
  stopped; output appended to `<log folder>/mediamtx/mediamtx.log`). Each run has a folder named for its pid with
  `mediamtx.yml` and `server.pid`; `stop_leftovers` kills a server whose NewQ is gone (or is us) and only if the pid
  is still the MediaMTX exe. `Runner` is the worker thread (start/stop/retry every 10 s; status in `Status`).
  `plan`/`align` make every display that runs its own server agree on one adapter and port set (first wins).
  `app.rs::update_hosting` works out the plan each frame, settles a change for 1 s, refuses a missing adapter and a
  computer without `Prefs::allow_hosting`, and sets `UiState::hosting`. Real-server tests need
  `NEWQ_MEDIAMTX_DIR` (they pick their own ports; WebRTC's and RTSP-over-UDP's are settings for that).
  `src/streamlink.rs`: network adapters (`if-addrs`), the player/protocol links, the QR modules (`qrcode`),
  and `probe_server` (TCP connects; SRT is UDP so unchecked). The UI is `ui.rs::server_share_panel`.
  An adapter can't make FFmpeg bind its outgoing socket (RTSP/RTMP/SRT have no such option), so it picks the
  listen address, the publish address for a local server, and the address in the links. Tests: the real
  server and a real publish/play-back run when `NEWQ_MEDIAMTX_DIR` (or PATH) has `mediamtx`, else skip.
  `scripts/mediamtx_pins.txt` pins the bundled version, its mirror release and SHA-256s; `package_release.sh::bundle_mediamtx`
  fetches it from this repository's mirror (a universal `lipo` binary on the Mac, signed with the others).
  `src/pattern.rs`: the twelve test patterns (`show::PatternKind`), drawn on the
  CPU; `app.rs` caches one per display by kind, number and size.
  `src/live_text.rs`: clock and speaker timer text (the engine redraws the
  picture as the shown time changes; the renderer re-uploads on `version`).
  `src/relink.rs`: finding moved media and copying it into one folder.
  `src/gpu.rs` also notices a lost graphics device (`DeviceLost`) and
  `app.rs::watch_gpu` rebuilds every window's device, surface and render thread;
  `NEWQ_LOSE_GPU_AFTER=<seconds>` destroys the device on purpose to test it.
  `src/color.rs`: colour adjustment and chroma key maths; the shader in
  `gpu.rs` does the same per pixel (a test renders it on a real GPU if there
  is one; `NEWQ_REQUIRE_GPU=1` makes having none a failure).
  `src/monitor.rs`: what a display is called (a Mac's name for it) and where a
  window starting on it goes. winit's Mac `name()` is "Monitor #" and a number,
  and its `position()` is in that display's own pixels, so convert to points.
  `src/recovery.rs`: autosave copies. `src/media/camera.rs`: camera cues'
  device lists and ffmpeg input: `CaptureFormat` (asked for as `-pixel_format`, `-vcodec` or `-input_format`
  per system), the device's modes (`list_modes`, run on a thread by `Engine::query_camera_modes`; parsers for
  dshow, v4l2, avfoundation and decklink output, tested on sample text only) and Blackmagic `decklink:` devices
  (listed only when the FFmpeg in use has the `decklink` input: NewQ's bundled one can't, the SDK terms stop us
  shipping one; `CameraCue::mode_code` is the `-format_code`). Automatic names the format the device offers (`resolve_format`), never waiting: `modes_known` answers from its cache and asks on a thread, the answer used from the next preload (it is in the camera's `PrepSig`). None of it has met a real capture card. A display's Send out can also go to a Blackmagic device (`StreamKind::DeckLink`: `-pix_fmt uyvy422 [-s WxH] [-r N000/1001] -f decklink <device>`, devices from `camera::list_decklink_outputs`; same FFmpeg-build limit, also untried on hardware). A display's Send out can record to a file (`StreamKind::Record`: `record_path`, `crop_filter`, `hevc_args`; H.265 encoder found by `hevc_encoder`; closing the file gets the Custom-sized `finish_patience`, and `App::exiting` stops every stream so a quit closes it; the file is made first by `reserve_record_file`, so names never collide; inputs are stamped with `-use_wallclock_as_timestamps` and the output is `-fps_mode cfr`). Its sound is a tap on an audio device (`AudioOut::tap`: `TapSlot` is try-locked in the audio callback, never blocking it; each tap has an id so a recording ending cannot clear its replacement, and a device that comes back at the same rate keeps it) that `stream::AudioPump` hands to FFmpeg over a local TCP socket as f32le (FFmpeg connects), encoded to AAC; silence fills a quiet device so FFmpeg never waits. `src/media/waveform.rs`: peaks for the
  inspector's waveform.
  The `NewQ test pattern` device stands in for a camera in tests.

## Build and test

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings      # CI fails on any warning
NEWQ_REQUIRE_MEDIA=1 cargo test                 # over 700 tests, about 3 minutes once built
```

- The tests decode real media with FFmpeg: `scripts/make_test_media.sh` makes
  `test_media/*.mp4|png|wav` (git-ignored). `NEWQ_REQUIRE_MEDIA=1` makes a
  missing ffmpeg or clip fail the tests instead of skipping them.
- Linux needs `libasound2-dev libudev-dev libxkbcommon-dev pkg-config ffmpeg`.
  In cloud sessions the SessionStart hook (`.claude/hooks/session-start.sh`)
  installs these, fetches the crates and makes the test media.
- Use `git grep`, not `grep -r`: `test_media/` and `target/` are big.
- Hardware can't be tested here (audio devices, MIDI ports and timecode sources,
  cameras, real displays). The
  logic is tested without it: `Engine::for_tests()`, the fake `Device::for_test`
  in `audio.rs`, and the pure parsers in `remote.rs`. Say so when something was
  only tested that way.

## Conventions

- **Show files stay backwards compatible.** A new field is `#[serde(default)]` and
  is left out of the file when it has its default value, with a test that old
  files load and that the default isn't written.
- New features get tests, a README entry (and a keyboard-table row if they have
  a key), and are checked by running the app where the UI changed.
- Commits are one feature each. PRs are merged with a **rebase merge** to keep
  `main` linear. After a merge, restart the session branch from `main`.

## Running the app headless (cloud sessions)

```sh
sudo apt-get install -y xvfb xdotool imagemagick libxkbcommon-x11-0 libegl1 \
  libgl1-mesa-dri mesa-vulkan-drivers libvulkan1
WGPU_BACKEND=vulkan NEWQ_MUTE=1 xvfb-run -a -s "-screen 0 1500x900x24" \
  sh -c "target/debug/newq show.newq & sleep 9; xdotool mousemove 85 70 click 1; \
         sleep 2; import -window root shot.png"
```

There is no window manager, so keystrokes go nowhere until the window has focus.
Clicking the GO button (85, 70) fires a cue; for keys, focus the window and send
them to it: `W=$(xdotool search --name NewQ | head -1); xdotool windowfocus $W;
xdotool key --window $W space`.
Don't record audio with an ALSA `file` plugin: the null device isn't paced and
writes gigabytes in seconds.

## CI and releases

- CI runs fmt, clippy, tests and the real-media tests on Ubuntu, Windows and
  macOS, once per change: on pull requests (a newer push cancels the older run)
  and on `main` after a merge. A rebase merge of a pull request that CI passed on
  has the same tree, so its `main` run skips the builds (the `gate` job) and still
  counts for `Cut release`; a merge after `main` moved on is tested in full.
  Slow steps are capped (`apt` once stalled for 40 minutes). A branch has no CI until it has a pull request. Changes
  that only touch `*.md` or `LICENSE-*` skip CI; `scripts/check_main_ci.sh` (used by
  `Cut release`) accepts such commits on top of a commit CI passed on. Occasionally the macOS runner never starts and the job is cancelled
  after 15 minutes with no steps run; that's not a failure of the change. Re-run it.
- **Releasing:** follow `.claude/skills/release/SKILL.md` (the whole sequence, the traps,
  and how to keep going without being poked). In short: bump `version` in `Cargo.toml` (and `Cargo.lock`) in a PR, merge,
  and wait for CI to pass on `main`. Then, as the repository owner, comment
  `/release vX.Y.Z dry-run` and then `/release vX.Y.Z` on any issue or PR. The
  `Cut release` workflow checks the tag against `Cargo.toml`, tags `main` and
  starts the `Release` workflow, which builds and publishes the downloads.
  (It can also be run from the Actions tab, or by the owner's token.)
  A suffix (`v0.3.0-rc1`) makes it a pre-release.
- **FFmpeg for Windows and Linux is pinned to one build** (`scripts/ffmpeg_pins.txt`):
  a dated BtbN build, copied into a release of this repo (`ffmpeg-<version>`,
  not a NewQ release) so it can't change or vanish upstream, and checked against
  its SHA-256 while packaging. Nothing moves it but a deliberate change: to get a
  newer FFmpeg, run `scripts/update_ffmpeg_pins.sh <BtbN autobuild tag>`, commit,
  merge, run the **Mirror FFmpeg** workflow (owner only), then a release dry run.
- **MediaMTX is mirrored the same way** (`scripts/mediamtx_pins.txt`): MediaMTX's own release builds,
  copied into a release of this repo (`mediamtx-<version>`) by the **Mirror MediaMTX** workflow
  (`scripts/mirror_mediamtx.sh`, owner only), which checks each file against the pins and the pins
  against MediaMTX's `checksums.sha256`; packaging downloads from the mirror. To move to a newer one:
  change `version`, `release` and the SHA-256s, commit, merge, run Mirror MediaMTX, then a release dry
  run. `scripts/mirror_mediamtx.sh --check` verifies without uploading.
- Cloud sessions can't push tags (the git proxy only accepts branches), which
  is why releases go through that comment.
- **Claude can't post the comment, but can run the workflow.** The GitHub
  comment tool rewrites slash commands in comments Claude posts (`/release`
  becomes `·/·r·elease`), so the workflow skips them. That looks deliberate, so
  don't work around it. The same workflow also has a manual trigger
  (`workflow_dispatch`, inputs `tag` and `dry_run`) that accepts only the
  repository owner's account. A session with the owner's token (set up with
  `/web-setup`) can start it with `actions_run_trigger` `run_workflow` on
  `cut-release.yml`. Do the dry run first, then the real one, and check the
  result: the `Release` workflow publishes the downloads.
- Large tool outputs: page GitHub list calls (`per_page` of a few).
