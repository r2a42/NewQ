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
- `src/splash.rs`: the startup splash window, drawn on the processor with `softbuffer` (`compose`
  makes the picture and is tested; the window code is in `Splash`). `app.rs::resumed` opens it,
  and the main window's set-up (`open_main_window`, the slow part) begins after the splash's first
  redraw event so it is on screen first; `about_to_wait` closes it (`splash::due`).
  `NEWQ_NO_SPLASH=1` skips it. Not tried on a real Mac or Windows desktop.
- `src/app.rs`: windows and wiring. `src/ui.rs`: the control UI.
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
  `test_locked`), `analysis.rs` turns samples into findings (pure, tested), `decode.rs` is the
  hardware decoding comparison (a 4K clip made from the downloaded film, four looping
  tiles played with `video::set_hw_decode` off then on; a cue with a plain cut
  transition takes the place of the layers before it, so tiles use `FadeIn`),
  `gui.rs` is the window (its "Show the test on" ticks are `Prefs::test_displays`;
  `plan::use_displays` replaces the show's outputs with them before the cues are made, the first
  display shows the picture and the others mirror it with a whole-area `part_of`; the
  main window's own monitor is never offered). Cross-target clippy for Windows/Mac can't run in the cloud (`ring` needs the
  target's C compiler); CI does it.
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
  device lists and ffmpeg input. `src/media/waveform.rs`: peaks for the
  inspector's waveform.
  The `NewQ test pattern` device stands in for a camera in tests.

## Build and test

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings      # CI fails on any warning
NEWQ_REQUIRE_MEDIA=1 cargo test                 # ~130 tests, ~5 s once built
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
