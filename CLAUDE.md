# NewQ

A cue-based playback system for theatre and live events, in the spirit of QLab.
Rust, with FFmpeg run as subprocesses, wgpu for compositing and egui for the UI.
The README describes the features; this file is about working on the code.

## Layout

- `src/show.rs`: the show data model and `.newq` (de)serialisation, plus the
  group, arm and audio-output structure helpers.
- `src/engine.rs`: GO, pre-waits, continues, transitions, fades, groups, preload.
- `src/media/`: `ffmpeg.rs` (find and probe), `video.rs`, `audio.rs` (devices,
  routing, mixer), `playback.rs` (looping passes).
- `src/gpu.rs`, `src/render.rs`: the compositor and the per-window render threads.
- `src/app.rs`: windows and wiring. `src/ui.rs`: the control UI.
- `src/remote.rs`: OSC and MIDI. `src/text.rs`: text cue rendering.

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
- Hardware can't be tested here (audio devices, MIDI ports, real displays). The
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

There is no window manager, so keystrokes go nowhere until a click gives the
window focus; clicking the GO button (85, 70) is the reliable way to fire a cue.
Don't record audio with an ALSA `file` plugin: the null device isn't paced and
writes gigabytes in seconds.

## CI and releases

- CI runs fmt, clippy, tests and the real-media tests on Ubuntu, Windows and
  macOS. Occasionally the macOS runner never starts and the job is cancelled
  after 15 minutes with no steps run; that's not a failure of the change. Re-run it.
- **Releasing:** bump `version` in `Cargo.toml` (and `Cargo.lock`) in a PR, merge,
  and wait for CI to pass on `main`. Then, as the repository owner, comment
  `/release vX.Y.Z dry-run` and then `/release vX.Y.Z` on any issue or PR. The
  `Cut release` workflow checks the tag against `Cargo.toml`, tags `main` and
  starts the `Release` workflow, which builds and publishes the downloads.
  A suffix (`v0.3.0-rc1`) makes it a pre-release.
- Cloud sessions can't push tags (the git proxy only accepts branches), which
  is why releases go through that comment.
- Large tool outputs: page GitHub list calls (`per_page` of a few).
