# NewQ

A cue-based playback system for theatre and live events, in the spirit of QLab.
Rust, with FFmpeg run as subprocesses, wgpu for compositing and egui for the UI.
The README describes the features; this file is about working on the code.

This file is what every session needs. What each part of the code does, file by file, is in the
area files under `.claude/notes/areas/` (the map below): read the area file before working there,
and keep it true when the code changes.

## Hard rules

These hold anywhere in the code. Each area file has its own rules besides.

- **Show files stay backwards compatible.** A new field is `#[serde(default)]` and is left out of
  the file when it has its default value, with a test that old files load and that the default
  isn't written.
- **A later NewQ's kinds are read, not refused.** Kinds that cross the network read an unknown
  value leniently (`Unknown`/`Other` by `#[serde(other)]`, an untagged variant that keeps it, or the
  default), so a newer computer never closes a session. No value stops a show from loading
  (`show/compat.rs`): a cue whose kind or settings this NewQ can't read is `CueKind::Unknown`, kept
  as written, marked and never played; the main small kinds keep an unknown value in an untagged
  `Other(Kept)`; a cue's and the show's unknown fields are kept in `extra`. A new kind in the show
  model gets one of these, and a new `match` on one handles them. `ControlAction` and
  `TimelinePoint` stay strict: an unknown one makes its cue `Unknown`, never a different action.
- **A partial file is never handed out** (R20). A follower reaches media only through
  `net::media::local_path` and `Store::resolve`; an object is indexed only after its hash matched.
- **R21 (owner's ruling): the network is a reference; a picture never waits for or jumps at the
  primary.**
- **Never take time from an `Instant`** (`Instant::now() - d` panics soon after boot): use
  `Option<Instant>` or `checked_sub`. **A `clamp` whose bounds come from a show or a file panics**
  when they are inverted or NaN (`ui.rs::within`).
- **Every preview gives way to the outputs, and every new kind of preview (a 3D view included) must
  go through the one governor** (`preview_pace.rs`, `UiState::preview_budget`).
- **Every `queue.submit` goes through `Gpu::submit` and every `surface.configure` through
  `Gpu::configure`.**
- **One NewQ per node folder** (`node::lock_node_dir`); several NewQs on one machine need separate
  `NEWQ_NODE_DIR`s.
- **No program NewQ starts outlives it**: `reaper::watch(&child)` after each spawn, and a stream's
  FFmpeg is never waited on without a limit.
- **Panics are lived through, not fatal** (`crash::survive`); a worker thread says it died with
  `crash::OnPanic`.
- **Pointers are never typed by hand.** "Settings ▸ This computer ▸ Power" is made by
  `settings::path(page, section)` from a `Section`, a menu item by its `settings::MenuItem`, so a
  pointer follows a move. A setting goes on the page it belongs to: the show's pages save in the
  show, THIS COMPUTER's in `Prefs`/`node.json`.
- **The words**: *display* (never "output" for one), *monitor* (the thing plugged in), *screen* (a
  3D stage surface; display computers' cards keep "Screen N"), *audio output*, *3D stage*, *Test
  Mode*, *Media manager*, *Show mode*.
- **`ui::keys::KEYS` must equal README's Keyboard table** (a test reads it).

## Map

One line per area: the files, and the area file to read before working there.

| Area | Code | Read |
|---|---|---|
| Show and engine | `show.rs`, `engine.rs`, `speed.rs`, `fx.rs`, `live_text.rs`, `shape.rs`, layers, groups (`text.rs`'s line is in `remote.md`) | `areas/show-and-engine.md` |
| Show network | `net/` (clock, store, media, hub, play, paths), `app/play.rs`, `engine/net.rs`, `engine/cards.rs`, `autostart.rs`, `bin/newq-display.rs`, `ui/network.rs`, `ui/net_media.rs` | `areas/network.md` |
| Timelines | `timeline/`, `engine/timeline.rs`, `engine/displays.rs`, `ui/timeline.rs`, `ui/timeline/` | `areas/timeline.md` |
| 3D stage | `stage3d/` (projectors, light, samples), `stage_render.rs`, `stage_live.rs`, `stage_counts.rs`, `render3d/`, `ui/stage_*.rs` | `areas/stage3d.md` |
| Test Mode | `testmode/` (with `selftest.rs`), `net/measure*.rs`, `stats.rs`, `soak.rs` (Test Mode's network load, `net/load.rs`, is in `network.md`) | `areas/testmode.md` |
| Previews and UI | `app.rs`, `ui.rs`, `ui/` (menus, keys, settings, modern, preview), `dock.rs`, `preview_pace.rs`, `multiview.rs`, `history.rs`, `export.rs` | `areas/previews-and-ui.md` |
| Compositor and displays | `gpu.rs`, `render.rs`, `upload.rs`, `adapters.rs`, `mesh.rs`, `blend.rs`, `pattern.rs`, `color.rs`, `monitor.rs`, display slots | `areas/compositor-and-displays.md` |
| Media and streams | `media/` (ffmpeg, video, audio, hap, camera, waveform), `ndi.rs`, `pdf.rs`, `pptx.rs`, `slides_export.rs`, `stream.rs`, `mediamtx.rs`, `streamlink.rs`, `relink.rs`, `recovery.rs` | `areas/media-and-streams.md` |
| Remote control | `remote.rs`, `feedback.rs`, `timecode.rs`, `output.rs`, `projector.rs` (PJLink), `clock.rs`, `web.rs`, `web_page.html`, `clicker.rs` | `areas/remote.md` |
| Platform | `awake.rs`, `splash.rs`, `macwin.rs`, `freeze.rs`, `power.rs`, `crash.rs`, `reaper.rs`, `logging.rs`, `crumbs.rs`, display loss | `areas/platform.md` |
| Packaging and CI | `.github/workflows/release.yml`, `scripts/package_release.sh`, signing, the FFmpeg and MediaMTX pins, the logo | `areas/packaging.md` |

Paths are under `src/` unless they say otherwise; the area files are under `.claude/notes/`. Plans
and rulings for the larger pieces are beside them (`network-plan.md`, `timeline-plan.md`,
`stage3d-plan.md`, `projection-plan.md` and others), named in the area files where they apply.
Reports go to the log folder (`%LOCALAPPDATA%\NewQ` etc.); ask for `newq-crash.log` and `newq.log`
when a user reports a crash.

## Build and test

```sh
scripts/check.sh quick ui::timeline stage3d   # fmt, clippy, and the tests named
scripts/check.sh full                         # fmt, clippy, the whole suite with the GPU and media
                                              # settings; cross clippy when it can find something
```

- About 1,900 tests: about 6.5 minutes of running on Ubuntu CI, about 7 locally, once built.
  CI fails on any clippy warning.
- `scripts/check.sh full` runs cross clippy (`scripts/cross_clippy.sh`, Windows and Mac from Linux)
  only when files with platform `cfg`s, `Cargo.toml`, `Cargo.lock` or `build.rs` changed. A shared
  function changed in an ordinary file can still break a Windows-only caller; CI catches that.
- The tests decode real media with FFmpeg: `scripts/make_test_media.sh` makes
  `test_media/*.mp4|png|wav` (git-ignored). `NEWQ_REQUIRE_MEDIA=1` makes a missing ffmpeg or clip
  fail the tests instead of skipping them.
- Linux needs `libasound2-dev libudev-dev libxkbcommon-dev pkg-config ffmpeg`. In cloud sessions the
  SessionStart hook (`.claude/hooks/session-start.sh`) installs these, fetches the crates and makes
  the test media.
- Use `git grep`, not `grep -r`: `test_media/` and `target/` are big.
- Hardware can't be tested here (audio devices, MIDI ports and timecode sources, cameras, real
  displays). The logic is tested without it: `Engine::for_tests()`, the fake `Device::for_test` in
  `audio.rs`, and the pure parsers in `remote.rs`. Say so when something was only tested that way.

### Checking, with agents

- **An agent** runs `scripts/check.sh quick <its areas>` and any test it wrote, never the full suite
  (several full suites at once on one machine fail for reasons that aren't in the code).
- **Only the integrator** runs `scripts/check.sh full`, once, alone, after combining the agents' work.
- **Agents commit as they go**, on their own branch, so a restart loses nothing.
- **Each agent's brief names the area files it must read.**

### Pull requests

- At the first push of a batch of work, open a **draft pull request**. Push each integrated batch
  to it, not each commit: a newer push cancels the CI run in progress, so pushing more often than a
  round takes means no run finishes. Before pushing again, look whether the last run finished.
- When all three systems are green, mark it ready and **rebase-merge** it into `main`. Merging a
  green batch is not a release; the owner alone decides when to release.
- Wait with the Monitor tool and `scripts/gh_wait.sh --for 25m <what>` (so a watch ends before the
  Monitor's limit), never with scheduled tasks (`send_later`/`create_trigger` are refused here).

## Conventions

- New features get tests, a README entry (and a keyboard-table row if they have a key), and are
  checked by running the app where the UI changed.
- **`CHANGELOG.md`** gets a line under `## Unreleased` with each change a user would notice, in
  words for people who run shows. A release moves them under `## X.Y.Z`; `scripts/check_release_tag.sh`
  (the dry run) refuses a version with no section, and the Release workflow puts the section at the
  top of the release page (`scripts/changelog_section.sh`).
- Commits are one feature each. PRs are merged with a **rebase merge** to keep `main` linear. After a
  merge, delete the session branch on GitHub (or let the repository's "Automatically delete head
  branches" do it) and restart it from `main`: a rebase merge gives the commits new ids, so a branch
  left behind holds old copies and the next push needs a force-push.

### Timing tests

1. A test waits for a *condition*, never for a time or for one wording. When several outcomes are
   legitimate it accepts each by name and fails at once on any other; it never sits out a long limit.
2. No assertion on how *little* time something took, or on an upper count of things per tick: CI
   runners oversleep.
3. A test that reads a file from the repository reads it whatever its line ends (Windows checks out
   CRLF).
4. Where the code can be driven by a clock the test owns (`Engine::tick_shared`, `net/sim.rs`,
   `World` in `net/play_tests.rs`), the test uses it, not the wall clock. Real time is for what only
   real time shows (FFmpeg, sockets, the sound device stand-in).
5. A test that fails in CI and passes when run again is a bug with a name: it goes in
   `.claude/notes/flaky-tests.md` (date, system, test, cause once known) and is fixed at the cause.
   Raising its timeout is not a fix; a second entry for the same test means it is rewritten onto a
   test-owned clock. The nightly `flaky-hunt.yml` runs the suite three times on each system to find
   them first.

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

The details (CI's jobs, the pins and mirrors, signing) are in `.claude/notes/areas/packaging.md`.

- CI runs on pull requests (a newer push cancels the older run) and on `main` after a merge, on
  Ubuntu, Windows and macOS; each system has a tests job and a self-test job sharing a cache that
  `warm-cache.yml` fills from `main`. A branch has no CI until it has a pull request. A rebase merge
  of a green pull request has the same tree, so `main`'s run skips the builds (the `gate` job).
  Changes that only touch `*.md` or `LICENSE-*` skip CI. A macOS runner that never starts (cancelled
  after 15 minutes, no steps run) is not a failure of the change: re-run it.
- **Releasing:** follow `.claude/skills/release/SKILL.md`. The `Cut release` workflow checks the tag
  against `Cargo.toml`, tags `main` and starts `Release`, which took 16-21 minutes before the Mac release
  was built as two parallel halves (about 11-12 expected since; signing, notarising and stapling take
  about a minute and a half).
- Cloud sessions can't push tags (the git proxy only accepts branches). The GitHub comment tool
  mangles slash commands (`/release` becomes `·/·r·elease`), so the `/release` comment can't be
  posted by Claude; don't work around it. Start `cut-release.yml` by `workflow_dispatch` (inputs
  `tag` and `dry_run`, owner's account only) with `actions_run_trigger` `run_workflow` and the
  owner's token (`/web-setup`): **the dry run first**, then the real one, and check the result.
- FFmpeg (Windows and Linux) and MediaMTX are pinned and mirrored; nothing moves them but a
  deliberate change (`areas/packaging.md`).
- Large tool outputs: page GitHub list calls (`per_page` of a few).
