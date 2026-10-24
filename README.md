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
- **Cue types:** Video, Image, Audio, Text, Fade, Stop, Wait and Group.
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
- **Outputs and display selection:** each output is *Automatic* by default:
  fullscreen on a second display when one is connected, otherwise shown inside
  NewQ's main window. Plugging displays in or out switches automatically. You
  can instead pin an output to a specific display (it falls back to the main
  window while that display is unplugged) or keep it in the main window. Every
  output also has a live preview in the main window, including ones that are
  fullscreen elsewhere. *Test pattern* shows colour bars with the output's
  number so you can tell which projector is which. Displays are remembered by
  name, and by position if the name changes (never onto NewQ's own screen).
- **Corner pin (keystone) per output:** click *Corner pin…* and drag the four
  corners on the output's preview (hold Shift for fine moves), or type exact
  positions. Click a corner to select it, then the arrow keys move it one
  pixel at a time (Shift + arrow: 5 pixels); this works on sub display
  areas too, where clicking inside an area selects all of it. The warp is perspective-correct, applies to everything on the
  output, and shows on the preview too; turn on *Test pattern* to line it up
  on the real screen. *Reset* returns to the full screen.
- **Sub displays:** split a display into areas that each work like a
  display of their own, e.g. one projector covering two set pieces, or one LED
  processor input feeding several screens. Add one under *Sub displays*
  (or *Outputs → Add sub display on*), then choose *On sub display …*
  as an output's placement. Click *Edit area…* to see the whole display live
  with every area outlined: drag inside an area to move it, drag its corners
  to reshape it into any four-cornered shape, or type exact pixels. An
  output's corner pin works inside its sub display, and everything is
  clipped to the area. Each display gets a single fullscreen window, however
  many outputs are on it.
- **Loops and slices:** set an in and out point on a video or audio cue and
  tick *Loop* to repeat just that slice, either forever or a set number of
  plays. Each pass is cut to an exact length and the next one is already
  decoding before the current one ends, so the join has no gap and sound and
  picture stay in step. With no out point, the loop runs from the in point to
  the end of the file.
- **Group cues:** a group holds other cues and is fired with one GO. *Start all
  together* fires every child at once (a child's pre-wait is its offset from the
  start, so a group is a small timeline); *Play in order* fires each child when
  the one before it finishes. The group is running until all of its children
  have finished, so auto-follow after a group waits for the last child, and
  stopping or fading a group (or a Fade/Stop cue aimed at it) stops or fades its
  children. The playhead moves over a whole group at a time, and groups can be
  collapsed in the cue list. Adding a cue while a group is selected puts it
  inside; *Wrap in group*, *Move into group* and *Move out of group* (cue
  menu or right-click) move cues in and out, and Ctrl+↑/↓ moves a cue within or
  out of its group. Groups don't nest.
- **Text cues:** titles, surtitles and lower-thirds. Type the text (several
  lines are fine), then set its size (a share of the output's height), colour,
  alignment, where it sits (top, middle or bottom) and where it wraps. It's
  drawn at the output's own resolution and then plays like an image cue, so it
  gets the same transitions, fades, layers, hold time and geometry, and a Fade
  or Stop cue can target it. There's one built-in font (a light weight, so
  *Shadow* and *Bold* help over video), and editing a text cue takes effect
  the next time it's fired.
- **Camera cues:** show a live camera or capture card (HDMI/SDI/USB) as a
  layer. Pick the device and the mode to ask of it (size and frame rate, which
  the device must support), then set the output, layer, transition, fades and
  geometry as for a video cue. A camera cue runs until stopped, or for the time
  in *Run for*; a Fade or Stop cue can target it. The camera is opened as soon
  as the cue is next in line, so GO is instant, and only the newest picture is
  kept, so it stays live rather than lagging. One cue at a time can use a given
  camera. There's no camera sound: add an audio cue for that. *NewQ test
  pattern* in the list is a moving picture for trying the cue without a camera.
  Cameras are reached through FFmpeg (Video4Linux2 on Linux, DirectShow on
  Windows, AVFoundation on macOS, where the first use asks for camera
  permission). Cameras can't be tested in NewQ's automated tests, only the
  built-in test pattern; please report any device that doesn't work.
- **Per-cue geometry:** under *Geometry* in a visual cue's inspector, move,
  scale (or mirror, with a negative scale), rotate and crop the picture on its
  output. Position is a share of the output's size, rotation is clockwise about
  the middle of the picture, and edits apply live while the cue plays. Parts
  that leave the output are clipped. Two videos can share one screen this way,
  without sub displays.
- **Remote control (OSC and MIDI):** turn these on under *Remote control* in
  the side panel; the settings are saved in the show. Both are off until you
  enable them, because anyone who can reach the OSC port on your network can
  drive the show. Cues are named by their cue number.
  - *OSC over UDP* (port 53000 by default): `/go`, `/panic`, `/stop`,
    `/pause`, `/resume`, `/playhead/next`, `/playhead/previous`, and
    `/cue/<number>/start` (fire it, playhead unchanged), `go` (select it and
    GO), `select` (or `load`), `stop`, `fade` (fade out with its own time),
    `arm` and `disarm`.
    Arguments are ignored and bundles work.
  - *MIDI input*: pick a port and, if you like, a channel. A note-on fires the
    cue whose number is the note number; a program change selects the cue
    with that number; and MIDI Show Control GO (with or without a cue
    number), STOP, RESUME, LOAD and ALL OFF drive the transport.
- **Audio outputs and channel routing:** under *Audio outputs* in the side
  panel, add named outputs and put each on any audio device (or the system
  default). Each cue then picks an output and the first device channel its
  stereo pair plays on, so one show can feed a PA, a monitor desk and a
  multichannel interface at once. The first output is the default for cues
  that don't choose; with no outputs, everything plays on the system default
  as before. NewQ opens each device with the most channels it offers; a
  device that is missing is reported in the panel, and retried while nothing
  is playing.
- **Arm and disarm cues:** untick *Armed* in a cue's inspector (or press A, or
  use the right-click menu) to skip it without deleting it, which is handy in
  rehearsal. A disarmed cue is passed over as if it weren't in the list: GO
  fires the next armed cue, auto-continue and auto-follow skip it, a group
  doesn't start it, and firing it by hand (or over OSC) does nothing. A
  disarmed group takes its children with it. Disarmed rows are greyed out,
  and the *Standing by* display shows the cue GO will really fire.
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
| A                     | Arm / disarm the selected cue                   |
| ↑ / ↓                 | Move selection and playhead                     |
| Ctrl+↑ / Ctrl+↓       | Reorder the selected cue                        |
| ← ↑ → ↓ / Shift+arrow | Move the selected corner 1 px / 5 px (while editing) |
| Ctrl+D / Del          | Duplicate / delete cue                          |
| Ctrl+N / Ctrl+O / Ctrl+S | New / open / save show                       |
| Ctrl+Shift+S          | Save as                                         |

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

Bump `version` in `Cargo.toml`, merge to `main`, and wait for CI to pass there.
Then either push a version tag and the `Release` workflow builds and publishes
everything:

```sh
git tag v0.2.1 && git push origin v0.2.1
```

or, without pushing anything, comment `/release v0.2.1` on any issue or pull
request as the repository owner. The `Cut release` workflow checks that the tag
matches `Cargo.toml`, is new, and that CI passed on the tip of `main`, then tags
it and starts the `Release` workflow. Add `dry-run` (`/release v0.2.1 dry-run`)
to run the checks without tagging. The workflow can also be run from the
Actions tab (or `gh workflow run cut-release.yml -f tag=v0.2.1`). A tag with a suffix, like `v0.2.1-rc1`, is
published as a pre-release.

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

- Video alpha channels (ProRes 4444, VP9 alpha) aren't supported yet; images
  with alpha are.
- No timecode triggering or cue carts yet.
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
