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
  window while that display is unplugged) or keep it in the main window. The
  main window has a live preview of an output, including ones that are
  fullscreen elsewhere; with more than one output, a menu above it picks which
  one is previewed. Under the preview is a **scrub bar** for the cue playing on
  that output: drag it to jump around in a video, a sound or a timed still (it
  works while paused; a looping cue and a camera can't be scrubbed). *Test pattern* shows colour bars with the output's
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
- **Send cues (OSC and MIDI out):** a cue that sends a message when it
  fires, so one GO can also start the lights and sound. *OSC* goes to an
  address and port over UDP, with an OSC address and arguments (`1 0.5
  "some text"`: whole numbers go as integers, other numbers as floats, the
  rest as text). *MIDI* goes to one of the MIDI outputs: note on or off,
  control change, program change, or MIDI Show Control (GO, STOP, RESUME,
  LOAD or ALL OFF, with a cue number, to all devices). A send cue takes no
  time, so Auto-continue and Auto-follow work as for a Stop cue, and ▶ Fire
  tries it. Sending happens on its own thread, so a slow name lookup or a
  missing device can't stall the show; problems are shown in the status bar.
  What NewQ sends is what its own OSC and MIDI listeners understand, so two
  copies of NewQ can run each other. An IP address is safer than a name.
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
- **Web remote:** tick *Web remote* under Remote control and a phone or
  tablet on the same network can open the address shown (`http://<this
  computer>:8080`) to run the show from a browser: a big GO, Stop All, Pause,
  Prev and Next, and a line for every cue with its status and a Fire or Stop
  button; the playhead and what is playing follow the show, and it shows the
  timecode when MIDI Time Code is running. It needs no app on the phone.
  Set a PIN to ask for one (the page prompts for it and remembers it); the PIN
  is kept in the show file. Anyone on your network who can reach the port,
  and has the PIN if there is one, can run the show, so use a PIN on a shared
  network. Changes need a header that a web page on another site can't send,
  and a link can't press GO, since only POST does anything. It is off unless
  the show turns it on. Other programs can use the same endpoints:
  `GET /api/state` for the state as JSON, and `POST /api/go`, `/api/panic`,
  `/api/stop_all`, `/api/pause`, `/api/resume`, `/api/next`, `/api/previous`
  and `/api/cue/<number>/fire|go|select|stop|fade`, with `X-NewQ: 1` (and
  `X-Pin` if set).
- **Hotkeys, MIDI notes and the cart:** give a cue its own key in its
  inspector (click *Hotkey*, then press a letter, digit or F-key; P and A are
  taken by the transport). Pressing the key fires just that cue, like the
  *Fire* button, and leaves the playhead where it is. A key fires one cue:
  giving it to another cue moves it. A *MIDI note* does the same from a
  note-on (with MIDI control on); a note with no cue bound to it still fires
  the cue whose number is the note, as before. Tick *On cart* for the cues you
  want as buttons, then *Cue ▸ Show cart* for a grid of them: click to fire,
  right-click to stop or fade out. Buttons show the hotkey, fill as the cue
  plays, and have a green dot when preloaded. Hotkeys don't fire while you're
  typing in a text box. Copies of a cue don't inherit its hotkey or note.
- **Colour and chroma key:** under *Colour and key* in a visual cue's
  inspector, adjust brightness, contrast, saturation and hue, and optionally
  make a colour clear (a green or blue screen) so whatever is under the cue
  shows through. The key measures distance on the chroma plane, so lighter and
  darker shades of the screen colour go too; *Tolerance* is how far from the
  screen colour is fully clear, *Softness* a fading edge beyond it, and *Spill*
  pulls the screen colour's tint out of what is kept (hair and edges). The
  order is key first, then the colour adjustments, then opacity. Edits apply
  live while the cue plays, and work on video, images, text and cameras.
  Adjustments are made in the same gamma space as the fades, like a hardware
  mixer, not in linear light.
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
- **Clock triggers:** give a cue (or a group) a *Clock* time in its inspector
  and it fires when the time of day reaches it, to the second, using this
  computer's local time: house-open music at 18:30, a pre-show loop, a reminder
  Wait. The status bar shows the next one and how long is left. A clock cue
  fires only for time that passes while NewQ is running: opening a show at 18:31
  doesn't start the 18:30 cue, a laptop that slept through it doesn't start it on
  waking, and setting the computer's clock doesn't fire the cues it skips. It
  fires again each day the show is left running. Disarmed cues are skipped, and
  the playhead doesn't move.
- **MIDI Time Code triggers:** give a cue (or a group) a timecode in its
  inspector, turn on *Follow MIDI Time Code* under Remote control (it listens
  to the MIDI input you chose), and the cue fires when the incoming time
  reaches it, wherever the playhead is. NewQ follows the clock from 24, 25,
  29.97 and 30 fps sources. Cues are fired only by time that actually plays
  past them: locating to a later time doesn't fire the cues it skipped,
  rewinding and playing again fires them again, and stopping and starting the
  clock in place doesn't fire anything twice. Playing from a position fires a
  cue set for that very frame. Disarmed cues are skipped. The current time
  shows in the status bar while it runs. A group's children measure their
  pre-waits from when the group fires. Only MIDI Time Code is read, not LTC
  (audio) timecode. 29.97 drop-frame times are compared by their numbers on
  the clock, which is how MIDI Time Code counts them.
- **Undo and redo:** Ctrl+Z and Ctrl+Y (or the Edit menu) step back and
  forward through edits to the show: cues, their order and groups, outputs,
  audio and remote settings. Dragging a slider or typing a name is one step
  however long it takes, and each button or menu action is a step of its own.
  Up to 200 steps (or 64 MB) are kept. Undo never stops what is playing, and
  opening or creating a show starts a new history. While you are typing in a
  text box, Ctrl+Z undoes the typing there instead.
- **Autosave and recovery:** every 30 seconds, unsaved changes are copied to
  `<show>.newq.recovery` beside the show (a show that has no name yet uses the
  temporary folder). If NewQ doesn't close properly, the next start offers to
  restore them. Saving, or choosing not to save when closing, deletes the copy.
  If you decline the offer the copy is kept as `.recovery.old` rather than
  deleted. Saving writes to a temporary file and renames it, so a crash during
  a save can't leave half a show.
- **Waveform and visual trimming:** audio and video cues draw the file's
  sound under *In / Out* in the inspector, with the played part highlighted.
  Drag the yellow ends to set the in and out points by eye; a click moves
  the nearer end. The in point can't pass the out point, and dragging the out
  end to the end of the file clears the out point. The sound is read once in
  the background and kept, so even long files only wait the first time. A file
  with no sound (or an image) shows no waveform. The In / Out boxes still take
  typed times, and both stay in step.
- **Cue sheet export:** *File ▸ Export cue sheet…* writes the cue list as a
  web page to print (landscape, with the header repeated on each page, groups
  shaded, children indented and disarmed cues struck through) or, if you name
  the file `.csv`, as a spreadsheet file (UTF-8 with a byte order mark, so
  Excel reads accents and symbols correctly). Each cue has its number, name,
  type, target, fades, pre-wait, duration, continue mode, whether it is
  armed, hotkey, MIDI note, timecode and notes.
- **Media manager:** *File ▸ Media…* lists every file the show uses and
  which can't be found, and it opens by itself when a show is opened with
  files missing. *Search a folder for missing files…* looks in a folder (and
  everything inside it) for files with the same names, preferring the one in
  matching folders when a name appears twice, and relinks them all in one go.
  *Find on this computer* searches this machine's folders and disks for the
  missing files' names in the background (the show's own folder and the usual
  media folders first, then your home folder and any other mounted disks,
  skipping hidden, system and application-data folders, for up to 90 seconds)
  and suggests a file for each one as soon as it turns up, best match first,
  with *Use suggestion* on the row and *Use all suggestions* at the top.
  *Locate…* on a row links one file by hand, and every cue using that file
  follows. *Copy all media into a folder…* gathers every file into one folder
  (name clashes get a number, existing files are never overwritten, the
  originals stay put) and points the cues at the copies, so a show and its
  media can move together. Copying runs in the background with a progress bar.
  Each relink or copy is one step in the undo history; the copied files stay on
  disk if you undo it.
- **Show files** are plain JSON (`.newq`) with media paths stored relative to
  the show file, so a show folder can move between machines.
- Drag and drop media files onto the window to create cues.

## If NewQ crashes or won't start

The Windows and Mac downloads have no console, so NewQ keeps a log instead. It is in
`%LOCALAPPDATA%\NewQ\newq.log` on Windows (`~/Library/Logs/NewQ` on a Mac,
`~/.local/state/newq` on Linux, where the log goes to the terminal unless
`NEWQ_LOG_FILE=1` is set); the log of the run before is kept as
`newq.previous.log`. It records the displays and graphics card it found. If
NewQ crashes it writes what went wrong, where, and a backtrace to
`newq-crash.log` in the same folder, and on Windows shows a message with that
path. Please send that file (and `newq.log`) when you report a crash.

If the displays are what stops NewQ starting, run it with `--main-window-only`
(or set `NEWQ_MAIN_WINDOW_ONLY=1`): every output then stays in the main window
and no output windows are opened, so you can open the app and set up the
displays from the Outputs panel.

## Keyboard

| Key                   | Action                                          |
|-----------------------|-------------------------------------------------|
| Space                 | GO                                              |
| Esc                   | Fade out everything (press twice: stop now)     |
| P                     | Pause / resume everything                       |
| A                     | Arm / disarm the selected cue                   |
| any cue's hotkey      | Fire that cue (letters, digits and F1–F12)      |
| ↑ / ↓                 | Move selection and playhead                     |
| Ctrl+↑ / Ctrl+↓       | Reorder the selected cue                        |
| ← ↑ → ↓ / Shift+arrow | Move the selected corner 1 px / 5 px (while editing) |
| Ctrl+D / Del          | Duplicate / delete cue                          |
| Ctrl+Z / Ctrl+Y       | Undo / redo (Ctrl+Shift+Z also redoes)          |
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
- Timecode triggers follow MIDI Time Code only (no LTC audio timecode yet).
- Very large files (4K60) push a lot of raw video through a pipe; NewQ
  decodes at the output window's resolution to keep this down.
- Fullscreen output on a second display has had little testing on Windows
  (starting with more than one display has crashed for at least one user: see
  the crash log section above), and on a Mac the main window has gone missing
  with two displays; macOS and Linux builds are covered by CI but haven't been
  run by hand.

## Trademarks

QLab is a trademark of Figure 53, LLC. NewQ is an independent project, written
from scratch and not affiliated with or endorsed by Figure 53; it mentions QLab
only to say what kind of program it is. NewQ contains no QLab code, files or
text.

## License

NewQ is licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT),
at your option. FFmpeg is a separate program under its own licence (LGPL/GPL
depending on the build); if you distribute FFmpeg binaries alongside NewQ,
follow FFmpeg's licence terms for them.
