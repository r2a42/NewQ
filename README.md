# NewQ

A simple, free, cross-platform cue-based playback system for theatre and live
events, in the spirit of QLab but much smaller. It focuses on video, with easy
fades and crossfades and straightforward output-display selection. It's built
in Rust on [FFmpeg](https://ffmpeg.org), [wgpu](https://wgpu.rs) and
[egui](https://github.com/emilk/egui).

## Download

Pre-built binaries for Windows, Linux and macOS are on the
[Releases page](https://github.com/r2a42/NewQ/releases). Every download
includes FFmpeg, so there's nothing else to install.

## Features

- **Cue list with a GO button.** Space fires the cue at the playhead and the
  playhead moves on. Cues can have pre-waits and can **auto-continue** (fire the
  next cue at the same time) or **auto-follow** (fire the next cue when this one
  finishes).
- **Cue types:** Video, Image, Audio, Fade, Stop and Wait.
- **Built-in transitions on every video/image cue:** no extra cues needed to
  change what's on screen. Each cue chooses how it enters its output:
  - **Crossfade** (the default for new cues): fades in while everything else on
    that output fades out and stops.
  - **Dip to black** / **Dip to white**: the output fades to the colour, the new
    content replaces the old underneath, then fades up from the colour.
  - **Cut**: replaces what's showing instantly.
  - **Fade in over**: fades in on top and leaves what's showing running (for layering).

  Crossfades and cuts wait for the incoming content's first frame, so there's
  never a gap. Set the time and curve (linear or S-curve) per cue, or
  right-click a cue in the list to change its transition.
- **Built-in fade-out on every cue:** used before the media ends naturally, and
  whenever you fade the cue out by hand (the *Fade out* buttons in the inspector,
  the Active panel and the right-click menu). New Stop cues default to fading
  everything out with each cue's own fade-out.
- **Fade cues** are still available for dimming or ramping another cue's
  opacity and volume mid-cue.
- **Stop All / Panic** (Esc) fades everything out; press it again to stop instantly.
- **Outputs and display selection:** each output is a window that you assign
  to a display, either borderless fullscreen or as a preview window. *Test
  pattern* shows colour bars with the output's number so you can tell which
  projector is which. The show remembers displays by name, so a show survives
  monitors being re-ordered.
- **Layers and fit modes:** fit (letterbox), fill (crop) and stretch; higher
  layers draw on top; per-cue opacity and volume can be adjusted live.
- **Instant GO:** the next cue(s) are decoded ahead of time (a green dot in the
  cue list means a cue is preloaded).
- **Audio/video sync:** video with sound is slaved to the audio clock.
- **Show files** are plain JSON (`.newq`) with media paths stored relative to
  the show file, so a show folder can move between machines.
- Drag and drop media files onto the window to create cues.

## Keyboard

| Key                   | Action                                          |
|-----------------------|-------------------------------------------------|
| Space                 | GO                                              |
| Esc                   | Fade out everything (press twice: stop now)     |
| P                     | Pause / resume everything                       |
| ↑ / ↓                 | Move selection and playhead                     |
| Ctrl+↑ / Ctrl+↓       | Reorder the selected cue                        |
| Ctrl+D / Del          | Duplicate / delete cue                          |
| Ctrl+N / Ctrl+O / Ctrl+S | New / open / save show                       |

Space and Esc also work while an output window has focus.

## Getting FFmpeg

NewQ runs `ffmpeg` and `ffprobe` as subprocesses, so you don't need FFmpeg
development libraries to build it. It looks for the executables in this order:

Release downloads already include FFmpeg; this section is for building from
source or swapping in a different FFmpeg.

1. the directory in `NEWQ_FFMPEG_DIR`
2. next to the NewQ executable, or in an `ffmpeg/` or `ffmpeg/bin/` folder beside it
3. `third_party/ffmpeg/bin/` in the project (for development)
4. your `PATH` (and Homebrew's prefixes on macOS)

Where to get it:

- **Windows:** download a build from <https://www.gyan.dev/ffmpeg/builds/> or
  <https://github.com/BtbN/FFmpeg-Builds/releases> and put `ffmpeg.exe` and
  `ffprobe.exe` in `third_party/ffmpeg/bin/` (or beside `newq.exe`).
- **macOS:** `brew install ffmpeg`
- **Linux:** `sudo apt install ffmpeg` (or your distribution's equivalent)

## Building

You need a stable Rust toolchain (<https://rustup.rs>).

```sh
cargo run --release                      # start with an empty show
cargo run --release -- path/to/show.newq # open a show
```

Platform notes:

- **Windows:** either the MSVC toolchain (needs the Visual Studio C++ build
  tools) or the GNU toolchain with MinGW works. This repository pins nothing, so
  run `rustup override set stable-x86_64-pc-windows-gnu` if you only have MinGW.
- **Linux:** install the ALSA headers first: `sudo apt install libasound2-dev`.
- **macOS:** no extra dependencies.

### Demo show and tests

```sh
scripts/make_test_media.sh third_party/ffmpeg/bin/ffmpeg   # creates test_media/*.mp4|png|wav
cargo run --release -- test_media/demo.newq
cargo test
```

The integration tests in `src/media/mod.rs` decode the test media with real
FFmpeg. They skip themselves if FFmpeg or the media are missing.

### Making a release

Push a version tag and the `Release` workflow builds and publishes everything:

```sh
git tag v0.2.0 && git push origin v0.2.0
```

`scripts/package_release.sh` does the packaging and can be run locally after a
release build.

### Environment variables

| Variable          | Effect                                                              |
|-------------------|---------------------------------------------------------------------|
| `NEWQ_FFMPEG_DIR` | Folder containing `ffmpeg` and `ffprobe`                            |
| `NEWQ_MUTE`       | Output silence (the audio clock still runs); useful for testing     |
| `NEWQ_REQUIRE_MEDIA` | Make integration tests fail (not skip) without FFmpeg/test media |
| `WGPU_BACKEND`    | Force a graphics API: `dx12`, `vulkan`, `metal`, `gl`               |
| `WGPU_POWER_PREF` | `low` prefers the integrated GPU, `high` the discrete GPU           |
| `NEWQ_TRACE_OUTPUT` | Log the layer opacities of every frame each output composites     |
| `RUST_LOG`        | Logging level, e.g. `RUST_LOG=debug`                                |

## How it works

```
 main thread ──── winit events, egui UI, Engine::tick (≈250 Hz while playing)
   │                 │
   │                 ├─ ffmpeg (video) ─► rawvideo yuv420p ─► VideoPlayer (frame queue)
   │                 ├─ ffmpeg (audio) ─► f32le stereo ─► ring buffer ─► cpal mixer
   │                 └─ uploads new frames to GPU textures
   │
   ├─ control render thread ── presents the egui UI
   └─ one render thread per output ── composites layers (YUV→RGB, opacity, fit)
```

- `src/show.rs`: the show data model and `.newq` (de)serialisation.
- `src/engine.rs`: GO, pre-waits, continue modes, transitions (crossfade/cut
  entry actions and dip mattes), fades (`Param` ramps), stops, preloading and
  A/V sync.
- `src/media/`: FFmpeg discovery and probing (`ffmpeg.rs`), video decoding
  (`video.rs`), audio decoding and mixing (`audio.rs`).
- `src/gpu.rs`: the wgpu compositor and YUV→RGB shader.
- `src/render.rs`: per-window render threads. Presenting can block on vsync,
  or for much longer when the OS throttles a covered window, so the main thread
  never presents. It posts the latest frame into a latest-wins mailbox instead.
- `src/app.rs`: windows, monitors and wiring. `src/ui.rs`: the control UI.

Fades and crossfades are done by NewQ, not FFmpeg filters, so they are
live: a fade cue can start at any moment, and opacity blending happens in gamma
space like a hardware video mixer.

## Known limitations / roadmap

- Looping plays the whole file; out points are ignored while looping.
- Audio goes to the system default device, as stereo.
- Video alpha channels (ProRes 4444, VP9 alpha) aren't supported yet; images
  with alpha are.
- No timecode/MIDI/OSC triggering, groups, or cue carts yet.
- Very large files (4K60) push a lot of raw video through a pipe; NewQ
  decodes at the output window's resolution to keep this down.
- Fullscreen output on a second display has been tested only on a single-monitor
  Windows machine so far; macOS and Linux builds are covered by CI but haven't
  been run by hand.

## License

NewQ is licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT),
at your option. FFmpeg is a separate program under its own licence (LGPL/GPL
depending on the build); if you distribute FFmpeg binaries alongside NewQ,
follow FFmpeg's licence terms for them.
