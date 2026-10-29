<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/newq-logo-dark.svg">
    <img alt="NewQ" src="assets/logo/newq-logo.svg" width="380">
  </picture>
</p>

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

- **NDI video in and out:** with the NDI Runtime (from ndi.video; NewQ can't
  ship it) installed, *NDI sources on the network appear in a camera cue's list*
  as *NDI · name* and play like a camera, scaled to the mode set on the cue. A
  display's *Send out* section has *NDI source*: give it a name and other
  computers see it as an NDI source of that picture. Picture only, no sound.
  NewQ talks to NDI in a separate helper process, so a fault in NDI's library
  can't stop the show. SMPTE ST 2110 isn't supported: it needs PTP timing and
  packet pacing that a program can't do through a normal network card; use a
  2110 gateway fed from a display output or NDI.
- **Dante audio:** NewQ reaches a Dante network through an audio device: Dante
  Virtual Soundcard on this computer, or an interface with Dante built in. It
  shows in an audio output's device list marked *(Dante)*, with all its channels
  (up to 64) to route cues to; subscribe the channels in Dante Controller.
  Nothing else is needed or possible: Dante has no interface for a program to
  join the network by itself.
- **Colour labels, find, lock and renumber:** label a cue with a colour (right-click
  it ▸ *Colour*, or the swatches in the inspector) and it gets a stripe and a
  faint wash in the list. The *Find* box over the list (**Ctrl+F**) shows only
  the cues that match what you type, by number, name, notes, type (say
  "video"), file name or colour; a group stays with the matching child inside
  it, and **Esc** clears the box (and isn't Panic while you're typing in it).
  *Edit ▸ Renumber cues* numbers them 1, 2, 3 down the list (a group's
  children 3.1, 3.2), and can be undone. **Show mode and edit mode:** the **EDIT MODE / SHOW MODE** button at the top
  right (or *Edit ▸ Show mode*, **Ctrl+Shift+L**) switches between them. In show
  mode nothing in the show can be changed by accident: the inspector, settings,
  the list's editing commands, arming, dropping files, New and Open are all
  refused, and a cue's right-click menu is only Fire, Fade out and Stop; GO, stop,
  pause, the cart, hotkeys, OSC, MIDI and the web remote work as always. Leaving
  show mode asks first. The mode is remembered for this computer, so a restart
  in the middle of a show comes back in show mode; it isn't saved in the show.
- **Control cues:** *Cue ▸ Add Control cue* makes a cue that acts on another
  cue (or the list) and takes no time: **Start** (fire it as if by hand),
  **Pause** and **Resume** (hold just that cue, picture and sound, while the
  rest play on; a group's children too), **Load** (get it ready ahead of time so it
  starts at once), **Reset** (stop it now and forget what was ready),
  **Go to** (put the playhead on it without firing it), **Arm** and **Disarm**
  (change whether GO and continues fire it), **Devamp** (a loop that is playing
  finishes its pass and goes on instead of going round again) and **Memo** (does
  nothing: a note in the list). Pick the action and the cue in the inspector.
- **Audio matrix and meters.** Tick **Matrix** on a video or audio cue and each
  channel of its file (up to eight, so 5.1 and stems work) becomes a row of a grid
  whose columns are the output device's channels (up to 64, for Dante). Click a dot
  to connect, drag the number for the level in dB, right-click it to disconnect.
  A stereo file can have sound effects as well; a file with more channels plays
  without them. Settings → Audio shows a level meter for each channel of each
  output.
- **Vamping a sound.** A sound that loops with no count (set the loop between the
  in and out points) can be **devamped** (a Devamp control cue, `/cue/N/devamp`
  over OSC, or MSC): it finishes the pass it is on and stops. Tick **Devamp plays on**
  and it goes on instead, at the end of the pass, into whatever the file holds
  after the out point, to the end of the file. The loop is read less than a
  second ahead, so a devamp in the last second of a pass can ride the next pass.
  For something before the loop, put it in a cue of its own that auto-follows.
- **Video with transparency.** A clip with an alpha channel (ProRes 4444, HAP
  Alpha, QuickTime Animation, a PNG sequence in a movie) is found by NewQ and
  shows through to whatever is under it, on its own or over other cues. It
  costs more to play than an ordinary clip (the picture is sent to the
  graphics card as four bytes a pixel, not one and a half), so a 4K alpha clip
  is a bigger ask of a modest machine than a plain one.
- **Hardware video decoding.** Settings → Media has a switch (saved for this
  computer, not in the show) that has FFmpeg decode video on the graphics card
  (NVDEC, VideoToolbox, D3D11/DXVA, VAAPI) where it can. It frees the processor
  for 4K and for many layers; each picture is still copied back to be composited,
  so it is not the zero-copy decoding of a native player. A file or card it can't
  do falls back to the processor. It is off by default and only the machine you
  will run on can say whether it helps.
- **Cue lists.** *Cue ▸ New cue list* (or the **+** by the tabs) adds a list of
  cues with a playhead of its own; the lists are tabs above the cue list. GO, the
  playhead, the arrow keys and the web remote work on the list being shown; the
  other lists keep their place until you come back to them. Right-click a tab to
  rename a list, GO on it without leaving the one you are in, or delete it with
  its cues. Right-click a cue and *Move to list* sends it (a group with its
  children) to the end of another list. Cues in any list can be fired, stopped or
  targeted by number, and a *Go to* control cue on a cue in another list sets
  that list's playhead. A continue or the end of a list never runs on into the
  next list, and each list is numbered from 1. Over OSC, `/cuelist/N/go` and
  `/cuelist/N/select` take the lists in the order of the tabs, from 1.
- **Screen capture, for Spout and Syphon senders.** A camera cue can take **Screen ·
  the whole desktop** (on Windows also *a window by its title*; on a Mac, further
  screens too) as its source. It is how to bring in what another program on the
  same computer shows, such as a media server or a Spout or Syphon sender, by
  capturing the screen or window it draws on. NewQ does not read Spout or Syphon's
  shared textures themselves (that needs each one's own graphics interface, and
  could not be tried here); NDI is the better route when a sender can send it
  (many Spout and Syphon tools have an NDI bridge), and NewQ takes NDI sources
  directly. On Linux this needs an X11 session (not Wayland), and NewQ's own
  outputs must not be on the screen it is capturing.
- **Hap video, the fast way to play many big pictures.** A Hap file (QuickTime
  `.mov`, made by FFmpeg, Resolume, TouchDesigner and the like) holds frames the
  graphics card reads as they are, so playing one costs a decompress and an upload
  with no decoding or colour conversion. On a card that reads block-compressed
  textures (nearly all of them, and Metal on a Mac) NewQ plays Hap that way,
  with Hap, Hap Alpha and Hap Q, at any speed, in loops and with keyframed speed;
  anything else (or a card that can't) plays it like other video. Hap files are
  large (about 10 to 60 times an H.264 file's size), so use a fast disk. An
  ordinary video cue's file row has **Convert to Hap**, which makes a copy beside
  the original (`name_hap.mov`, sizes made a whole number of blocks) in the
  background and then plays that. Set `NEWQ_NO_HAP=1` to turn the fast path off.
- **Benchmark.** `newq --bench-video clip.mp4 --layers 4 --seconds 10 [--gpu]
  [--size 3840x2160] [--no-hap]` plays a file on several layers for a while and
  reports frames per second, dropped 60 Hz ticks, processor used (NewQ and the
  FFmpeg processes together), memory and, with `--gpu`, the time to upload and
  draw every layer. Run it on the computer that will run the show, with the
  files it will play, to see how many layers it can carry.
- **Speed, steady or keyframed.** A video or audio cue has a **Speed** (× 0.25 to
  × 4); sound keeps its pitch. Press **Animate** to ramp or keyframe it over the cue's
  own time: click the lane to add a keyframe, drag one, and choose how it eases into
  the next, as with an effect's numbers. A cue can start slow and speed up, hold,
  and slow again. The cue's length, scrub bar and fades follow, and the cue ends
  when the file does. Sound is time-stretched along the keyframes, so it is best
  between half and double speed (pitch is kept; very slow or fast sound gets
  grainy); the picture holds or skips frames to keep up. A loop follows the
  keyframes too, going round faster and slower; its bar then shows the place in the
  file's slice, and Devamp still ends it at the end of the pass (*Devamp plays on*
  isn't used with keyframed speed).
- **Animated position, scale, rotation and opacity.** The *Animate* section of a
  picture cue's inspector puts any of the six on keyframes over the cue's own time
  (the same lane as speed and effects); an animated track replaces the static value
  while the cue plays. A **Fade cue** can also move a playing picture to a position,
  scale or rotation over its duration (from wherever it is then), and can fade the
  target's audio **crosspoints** (the ones its matrix already has) to new levels
  live. A seek or a restart of the cue goes back to the cue's own settings. Speed
  can be keyframed but not faded by a fade cue.
- **Effects with keyframes:** the *Effects* section of a cue's inspector takes
  any number of effects. **Picture:** *Blur* (a disc blur, up to 100 px),
  *Sharpen*, *Vignette*, *Pixelate* and *Negative*, drawn by the graphics card in
  the same pass as the picture. **Sound:** a *3-band EQ*, a *high/low-pass
  filter*, a *compressor*, *echo* and *reverb*, run on the cue's sound before it
  reaches its output. Each effect can be switched on and off, even while the cue
  plays, and put in order. Every number (blur radius, EQ gain, echo time…) has an
  **Animate** switch: it becomes a lane of keyframes over the cue's time, where
  you click to add a keyframe, drag one to move it in time and value, right-click
  to remove it, and choose how it eases into the next. The white line is the cue's
  own time while it plays. Effects follow the cue's time, so a scrub or a loop
  carries them with it. There is no fixed limit on how many run at once: a
  plain picture is read once a pixel, a blur reads it up to about fifty times, so
  a few big blurs ask a lot of a slow graphics card, and long reverbs and echoes
  take a little memory when a cue starts. NewQ watches for trouble and says so in
  the status bar: *graphics behind* when frames start arriving late while effects
  are running, and *audio at N% of its time* when mixing the sound takes most of
  the time it has (above about 90% it breaks up). Adding or removing a *sound*
  effect applies the next time the cue plays; everything else is live.
- **Live audio:** *Cue ▸ Add Live audio cue* plays an audio input (a Dante Virtual
  Soundcard or Dante interface, a mixer's USB output, a line input) on one of the
  audio outputs, from the input channel you choose (and the next). It has the
  volume, fades, output and channels of any audio cue and runs until stopped
  or for a time. The input is opened when the cue starts, not ahead of GO. If the
  input and output are on different clocks a little sound is dropped now and then
  so the delay doesn't grow; on one Dante device they stay together. Tested without
  hardware (the channel and sample-rate conversion and the cue's lifecycle), not
  against a real input.
- **Slide shows:** *Cue ▸ Add Slide show…* takes any number of images (chosen
  together, put in name order) and makes one cue that crossfades through them.
  Set one *Fade* (how long each slide takes to fade in over the last) and one
  *Duration* (how long each is shown in full before the next starts) for the
  whole show, and give any slide its own: change its fade or duration in the
  slide list (it turns yellow; Reset puts it back to the show's). The same list
  adds images, removes them and puts them in order, and says how long the
  show runs. A slide show is a group of image cues whose start times and
  fades are worked out from those settings, so each slide is still an
  ordinary image cue to give a position, size or colour (its timing is the
  slide show's). It plays once and the last slide fades out.
- **Panes you can rearrange and pop out:** the main window is a set of panes
  (*Cue list*, *Inspector*, *Preview*, *Active*, the *Cart* and the *Settings*)
  that tile it: drag a pane's tab onto another to move it, beside or above or
  below or as a tab, and drag the lines between panes to resize. Making one
  bigger makes its neighbours smaller, so nothing overlaps. *Double-click a
  tab* (or right-click it ▸ *Pop out to a window*, or *Window ▸ Pop out*) to give a
  pane a window of its own, e.g. the preview on a second monitor; *Dock back*
  (or closing that window) puts it back. The *Window* menu shows and hides
  each pane and has *Reset layout*. The arrangement is remembered on this
  computer (not in the show file).
- **Settings:** every setting of the show is in one place, *Settings ▸ Open
  settings…* (**Ctrl+,** to open and close it). It takes the place of the cue
  list, so GO, Stop All and the live preview stay where they are and a setting
  can be tried while the show runs; *Close settings* returns. A sidebar groups
  the pages: **Show** (*General*: the show's name, how long Stop All takes to
  fade, where the recovery copies and logs go; *Media*: whether every file can
  be found), **Picture and sound** (*Displays*, *Sub displays*, *Audio
  outputs* and *Sync*) and **Remote control** (*OSC*, *MIDI* with MIDI Time
  Code, and the *Web remote*). Beside a page the sidebar shows what needs
  attention or is on: a red warning when a display's monitor is missing, an
  output has a problem, files can't be found or a listener failed to start, a
  green *on* for a listener that is working, and counts of displays and audio
  outputs. The *Settings* menu lists the same pages and jumps straight to one.
  Settings are saved in the show, as before.
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
- **Timecode cue.** Makes MIDI Time Code on a MIDI output from a start position at
  24, 25, 29.97 or 30 fps, for as long as you say (or until stopped); stopping the cue
  stops the clock and pausing the show holds it. For desks and recorders that chase
  timecode. (Tested against our own MTC reader and a timing test; not with real
  hardware here.)
- **Program (script) cues.** A Send cue's *Program* tab runs a program with arguments
  when it fires (no shell, so name a script for pipes; quotes keep words together),
  stopped after 60 seconds, with failures in the log. Because a show can come from
  anywhere, it does nothing until *Settings → General → Programs* is ticked on this
  computer (a per-computer setting, not stored in the show).
- **OSC feedback.** *Settings → OSC → Feedback*: give a `host:port` and NewQ sends
  the show's state there as OSC: `/newq/playhead <number> <name>`, `/newq/paused`,
  `/newq/running`, `/newq/cue/<number>/status` (idle, prewait, playing) and
  `/newq/cue/<number>/progress` (0 to 1), at most ten times a second and only what
  changed, with everything repeated every five seconds so a controller started late
  catches up. For TouchOSC pages, tally lights and other desks. (Only tested here
  against a loopback listener, not a real controller.)
- **Fade cues** are still available for dimming or ramping another cue's
  opacity and volume mid-cue.
- **Stop All / Panic** (Esc) fades everything out; press it again to stop instantly.
- **Displays and sub displays:** cues are sent to a **display** (*Display 1*,
  a name you choose such as *Stage TV*) or to a **sub display**, an area of
  one. Each display is assigned by hand to a monitor on the *Displays* page of the settings,
  so moving a show to another venue means reassigning each display once.
  **Nothing is ever reassigned automatically.** If the monitor a display is
  assigned to is unplugged, what was sent to it **keeps playing, to no
  destination**: nothing is drawn on any other monitor, the status bar, the
  cue list and the display's own row say so, and the main window's preview
  still shows what it would show. When the *same* monitor is plugged in again
  the picture is back on it at once, mid-cue and in time; or choose another
  monitor for the display, or send the cues to another display. A display
  with no monitor assigned shows only in the main window (the default for a
  new one). A monitor is recognised by what it is, not by which port or position the
  system lists it in: its make, model and serial number from the EDID it
  reports (read from the registry on Windows, `/sys/class/drm` on Linux, and
  CoreGraphics on a Mac), which also gives displays their real names. A
  display assigned before this was known learns its monitor's id when that
  monitor is connected (the log says so). If a system tells nothing, the name
  it reports is used. Monitors of one model without serial numbers can only
  be told apart by their order, so a display assigned to one of such
  look-alikes is only used while all of them are connected. A
  display can't cover the monitor NewQ's main window is on; it stays in the
  main window and the panel says so. The main window has a live preview of
  any display or sub display; with more than one, a menu above it picks which.
  *Test pattern* shows a picture for lining a display up, with a dropdown of
  twelve beside it (choosing one turns the pattern on; it works the same on
  sub displays, and the choice is saved with the show). *Colour bars and
  number* is the one NewQ has always had, for telling which projector is which.
  The others: *Grid* (squareness and where the picture lands), *Crosshatch*
  (a fine grid of one pixel lines, for straightness when setting a mesh or
  corner pin), *Checkerboard*, *Circles* (round when the shape is right),
  *Grey steps* (eleven steps, for brightness and contrast), *Grey ramp*
  (banding), *White* and *Black* (uniformity, black level), *Red, green, blue,
  cyan, magenta, yellow* (colour balance and dead pixels), *Edges and safe
  areas* (the edge, 90% and 80%, for overscan) and *Focus lines* (one and two
  pixel lines both ways). The grid, circles, edges and bars show the display's
  number as well. Crosshatch and Focus lines are drawn at the display's own
  pixel size (up to 4K) so each line is a real pixel line; the others are
  drawn smaller and stretched. The patterns are checked against what they should
  contain, not against a calibration meter: use a meter for real colour or
  brightness work. Shows from before displays and outputs were one
  thing are converted when they are opened, with the same cues pointing at the
  same things: an output becomes a display on its monitor (or a sub display if
  it was on one; a second output that shared a display becomes a sub display
  covering all of it), and one that was *Automatic* gets the first monitor the
  main window isn't on, once (the log says what was done).
- **Scrubbing:** each playing cue's **progress bar in the Active panel is also
  its scrubber**: click or drag it to jump around in a video, a sound or a
  timed still (it works while paused; a camera and a cue that is fading out
  can't be scrubbed). A **loop with a set number of plays** has a bar over all
  of them; a **loop that never ends** has a bar one pass long that moves as it
  plays and wraps round with each pass (the text says which loop it is on and
  the time in all), and clicking it jumps to that point in the pass, after
  which the loop carries on as before. Scroll over a bar to move that cue by the
  wheel. Hold **Shift** while dragging or scrolling to move every cue that can
  be scrubbed by the same amount, keeping their positions relative to each
  other.
- **Corner pin (keystone) per display:** click *Corner pin…* and drag the four
  corners on the display's preview (hold Shift for fine moves), or type exact
  positions. Click a corner to select it, then the arrow keys move it one
  pixel at a time (Shift + arrow: 5 pixels); this works on sub display
  areas too, where clicking inside an area selects all of it. The warp is
  perspective-correct, applies to everything on the display, and shows on the
  preview too; turn on *Test pattern* to line it up on the real screen.
  *Reset* returns to the full screen.
- **Mesh warp:** in the same editor, choose *Mesh* instead of *4 corners* for
  walls and screens that aren't flat or square (a curved cyclorama, a column,
  a corner, a dome segment). The picture is bent through a grid of points,
  3 × 3 to start with and up to 9 × 9; drag any point on the preview (Shift
  for fine moves, arrow keys after a click, or type pixels). Between the points
  the picture follows a smooth curve that passes through every one. To put
  a point exactly where the surface bends, *Ctrl-click* (*Cmd* on a Mac) the
  preview there: a point appears at that spot and the shape stays as it was
  (give or take a little on a strongly bent mesh). A grid can't have a lone
  point, so a new column and a new row cross at it, unless one already runs
  close by. *Remove column* / *Remove row* takes one out. Changing the grid size keeps the shape.
  *4 corners* goes back to a keystone using the mesh's corners, and *Flatten*
  puts every point back. Like the corner pin it applies to everything on a
  display and shows in the preview. A sub display is shaped by its area, not
  a mesh. Without a projector to look at, only the maths and a software GPU
  have checked this; the real test is a surface and a test pattern.
- **Edge blending (several projectors, one picture):** for a picture wider
  than one projector, overlap two or more projectors and let each show a part
  of it. *Displays → Add blended projectors* sets up a pair: a *Wide picture*
  display (send your cues to this one) and a *Left* and *Right projector*, each
  showing part of it, overlapping by 15%, with their overlapping edges faded.
  Give each projector its monitor as usual. To build your own, choose *Shows →
  Part of …* on a display and set which part (position and size in pixels of
  the picture); the picture's own size is set on the display it is a part of
  (*Picture size*), and can be any size, not tied to a monitor. Each display's
  *Edge blend* has the width to fade in from the left, right, top and bottom
  (the width of the overlap) and the projector's gamma (2.2 suits most): the
  fades are shaped so that the light of the two projectors adds up to an even
  picture in the overlap, which is what makes the join invisible. The wide
  picture's *Test pattern* is drawn across the projectors so you can line up
  their overlap (with each projector's corner pin or mesh warp, if it needs
  one), and a display's own test pattern is drawn on that display alone. Cues
  can't be sent to a display that shows part of another's picture, only to the
  picture itself; colour matching between projectors and black-level
  compensation are not done. The preview in the control window shows what a
  projector shows, not the fade: look at the screen. The fade and the parts
  were checked in tests against a software GPU, not against projectors.
- **Send a display out as video (virtual camera, OBS, streaming):** a display's
  *Send out* section sends its picture (the one cues are sent to, without any
  corner pin or blend) to a video call, OBS or a stream, as an FFmpeg output
  at the height and frame rate you choose (720 px tall at 30 fps to start with;
  a wide picture keeps its shape). It works for a display that has no monitor
  too, so a picture can go out without taking a screen. Three destinations:
  - *Virtual camera* (**Linux only**): a v4l2loopback device, which Zoom, Teams,
    OBS and browsers then list as a camera. Make the device once with `sudo
    modprobe v4l2loopback video_nr=10 card_label=NewQ exclusive_caps=1` (the
    `v4l2loopback-dkms` package), choose it in the list, and tick the box
    *before* the call starts. NewQ can't have been tested on a real device
    here, only on the FFmpeg end of the pipe.
  - *Network stream (for OBS)*: MPEG-TS over UDP (H.264 if your FFmpeg has
    it) to an address and port. In OBS add a *Media Source*, untick *Local
    File* and enter `udp://127.0.0.1:5000`; OBS's own **Virtual Camera**
    then makes it a camera for Zoom or Teams. This is the way on **Windows and
    macOS**, where a virtual camera needs a signed driver or system extension
    that NewQ doesn't have. It runs about half a second behind.
  - *Custom FFmpeg output*: the arguments that follow the input (the picture
    arrives as raw RGBA), for anything else FFmpeg can send to: RTMP to a
    streaming service, SRT, a file.
  Up to two displays can be sent out at once. A stream that can't start or
  that stops (the device is busy, the address can't be reached) is reported in
  the status bar and is not retried until its settings are changed.
- **Sub displays:** split a display into areas that each work like a display
  of their own, e.g. one projector covering two set pieces, or one LED
  processor input feeding several screens. Add one under *Sub displays* (or
  *Displays → Add sub display on*) and choose which display it is part of.
  Click *Edit area…* to see the whole display live with every area outlined:
  drag inside an area to move it, drag its corners to reshape it into any
  four-cornered shape (the area is its keystone), or type exact pixels.
  Everything is clipped to the area. Each monitor gets a single fullscreen
  window, however many displays and sub displays are on it.
- **Loops and slices:** set an in and out point on a video or audio cue and
  tick *Loop* to repeat just that slice, either forever or a set number of
  plays. Each pass is cut to an exact length and the next one is already
  decoding before the current one ends, so the join has no gap and sound and
  picture stay in step. With no out point, the loop runs from the in point to
  the end of the file.
- **Group cues:** a group holds other cues and is fired with one GO. *Start all
  together* fires every child at once (a child's pre-wait is its offset from the
  start, so a group is a small timeline, which the group's inspector draws: a bar for
  each child, with a line for where the group has got to while it plays; drag a bar
  to move that cue in time); *Play in order* fires each child when
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
- **Clock and speaker timer cues:** *Cue ▸ Add Clock cue* and *Add Timer cue*
  make a text cue that shows the time instead of typed words (or pick *Shows*
  in any text cue). A clock shows this computer's time, 24-hour or AM/PM, with
  or without seconds. A timer counts down from a length you set (or up, with an
  optional limit): a countdown reads its full length when it starts, turns
  amber when the time you choose is left, turns red at zero and then counts
  the overrun as `-0:12`. Put other words round it with `{time}` (`Left:
  {time}`). They're redrawn as the time changes and otherwise behave like any
  text cue: send them to a speaker's display or a sub display, fade them, stop
  them with a Stop cue. A timer runs on show time, so Pause holds it.
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
  Two more kinds of send cue reach equipment on the network:
  - *Projector* speaks **PJLink** (class 1, TCP port 4352, which most
    projectors from Epson, NEC, Panasonic, Sony, Barco, Optoma and others
    offer once it is switched on in their network settings): power on or off,
    shutter closed or open (PJLink's "video mute"; the lamp stays on), and
    select input. If the projector has a PJLink password, enter it; it is saved
    in the show file as typed. NewQ checks the projector's answer, so a refusal
    (it won't take power commands while warming up or cooling down) or a wrong
    password shows in the status bar rather than passing silently. Each
    projector is talked to on a thread of its own, in order, so one that has
    been switched off can't hold up the others.
  - *TCP / UDP* sends text or bytes you type to any host and port, for gear
    with its own control protocol (many projectors, matrix switchers, relay
    boxes). Escapes: `\r` `\n` `\t` `\0`, `\xNN` for a byte in hex, and `\\` for a
    backslash, so a command framed with STX and ETX is `\x02PON\x03`. TCP
    connects, sends and hangs up and doesn't read a reply; UDP is one packet.
    Look up the exact bytes in the device's manual: NewQ can't know them.
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
- **Web remote:** tick *Serve the web remote* on the *Web remote* page of the settings and a phone or
  tablet on the same network can open the address shown (`http://<this
  computer>:8080`) to run the show from a browser: a big GO, Stop All, Pause,
  Prev and Next, and a line for every cue with its status and a Fire or Stop
  button; the playhead and what is playing follow the show, and it shows the
  timecode when MIDI Time Code is running. It needs no app on the phone.
  Set a PIN to ask for one (the page prompts for it and remembers it); the PIN
  is kept in the show file. Anyone on your network who can reach the port,
  and has the PIN if there is one, can run the show, so use a PIN on a shared
  network. Changes need a header that a web page on another site can't send,
  and a link can't press GO, since only POST does anything. A web page that
  points its own domain at this computer is turned away too: the remote only
  answers requests sent to an IP address, a name with no dots or a `.local`
  name, so use the address NewQ shows (or `<computer>.local`). Five wrong
  PINs from one address shut it out for a minute, and at most 64 connections
  are served at once. It is off unless the show turns it on. Other programs can use the same endpoints:
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
  want as buttons, then *Cue ▸ Show cart* (or *Window ▸ Cart*) for a grid of them: click to fire,
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
- **Remote control (OSC and MIDI):** turn these on on the *OSC* and *MIDI* pages of
  the settings; they are saved in the show. Both are off until you
  enable them, because anyone who can reach the OSC port on your network can
  drive the show. Cues are named by their cue number.
  - *OSC over UDP* (port 53000 by default), in the shape of QLab's so the
    Companion and Stream Deck modules for it can drive NewQ: `/go`, `/panic`,
    `/stop`, `/pause`, `/resume`, `/reset` (stop everything and put the
    playhead back at the top), `/playhead/next`, `/playhead/previous`,
    `/playhead/<number>` (or `/select/…`), and `/cue/<number>/…` with `start`
    (fire it, playhead unchanged), `go` (select it and GO), `select`, `load`
    (get it ready), `stop` (or `hardStop`, `panic`), `fade` (fade out with its
    own time), `pause`, `resume`, `reset`, `devamp`, `arm`, `disarm`, and `armed`
    with 1 or 0 as its argument. Where a cue number goes you can say
    `selected`, `playhead` or `active` (every cue playing), or use
    `/cue_id/<id>/…` with a cue's own id. A `/workspace/<id>` in front is
    accepted and ignored. Other arguments are ignored and bundles work.
    (NewQ doesn't send QLab's replies and updates, so a button can fire a cue
    but won't show its state.)
  - *MIDI input*: pick a port and, if you like, a channel. A note-on fires the
    cue whose number is the note number; a program change selects the cue
    with that number; and MIDI Show Control GO (with or without a cue
    number), STOP, RESUME, LOAD (gets the cue ready and puts the playhead on
    it), ALL OFF and RESET drive the transport. On the *MIDI* page *Show control
    device* limits that to messages sent to one device id (and to everyone, 127),
    for when several machines share a MIDI line.
- **Audio outputs and channel routing:** on the *Audio outputs* page of the settings,
  add named outputs and put each on any audio device (or the system
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
  inspector, turn on *Follow MIDI Time Code* on the *MIDI* page of the settings (it listens
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
- **Lining sound up with picture:** *Settings ▸ Sync* (or *File ▸ Sync…*) sets how long to hold
  back the sound and the picture, in milliseconds, for the whole show, and
  again for each audio output (a Bluetooth speaker, a long cable run) and each
  video output (a projector that processes its picture). Whichever arrives
  first is the one to delay: if the picture is ahead, delay the picture. A
  video with its own sound keeps the two together whatever the delays are.
  *Add a sync test cue* makes a looping clip that flashes white and beeps
  together once a second, to adjust by eye and ear. Delays are saved with the
  show, change cues as they start, and go up to 2,000 ms. A picture held back
  also appears late, so a fade-in starts that much later.
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
- **Graphics device recovery:** if the graphics driver resets or the card is
  taken away mid-show, NewQ notices, says so in the status bar, and starts the
  main window and every output window again on a new device, usually in well
  under a second. Cues keep playing; a picture may blink out for that moment.

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
displays from the *Displays* page of the settings.

## Test mode: try this computer before the show

*Help ▸ Test mode…* plays a show of its own to find out how this computer copes, with
nothing to type: no command line, no files to find. Choose a test and press Start.

| Test | Length | What it does |
|---|---|---|
| Short | about a minute | One lap of everything: clips at 24, 30 and 60 fps, a slide show of large pictures, layers with effects and a moving picture, a 5.1 trailer with a downmix. |
| Medium | 30 minutes to 2 hours (1 by default) | The same lap with a second trailer through sound effects, round and round: the run that finds a slow leak. |
| Long | 4 to 12 hours (8 by default) | A longer lap with H.265, a 30 MB 10-second clip, six large pictures, more effects and a burst of twenty quick cues, for an overnight run. |

**The media.** The first time, Test mode downloads free films and pictures (the
Blender Foundation's *Big Buck Bunny* and *Sintel* and the *Jellyfish* clip, under
CC-BY, from test-videos.co.uk and download.blender.org, and NASA pictures, which are
public domain) into `test-mode/media` in the log folder. The window says how much
first (about 20 MB for Short, about 100 MB for Long) and you can untick the box. Each file is checked
against a known checksum and by decoding a few frames; a file that is missing, damaged
or can't be fetched is made on this computer instead (colour bars, tones, test
pictures), so with no internet the test still runs, just with plainer media. The
window shows each file's progress, and a note when it fell back. The credits are saved
in every report.

**Your show is not touched.** It is put aside in memory while the test runs and put
back exactly as it was, with its file, undo history, selection and playhead; nothing is
saved over it, and its autosave copy is left alone. A show with unsaved changes can't
start a test (save first), and the test show can't be edited or saved. If you close
NewQ mid-test it ends properly first.

**The log.** Each run has a folder `test-mode/runs/DATE_TIME_LEVEL` in the log folder
holding `report.txt` (verdict, findings, laps, measurements, media and credits),
`stats.csv` (a row every 2 to 15 seconds), `laps.csv`, `events.log` (what the engine
said, with times), `system.txt` (OS, processor, graphics, displays, sound, FFmpeg) and
an excerpt of `newq.log`. The window shows the path while it runs and at the end, with
buttons to open the folder and copy the path. The verdict is *Passed*, *Passed, with
notes* or *Problems found*: memory, FFmpeg processes, open files or threads that keep
growing, main-loop stalls, late frames, heavy audio load, laps that run late or early,
or cues that failed to start.

## Leaving a show running unattended: the soak test

A show that has to run for hours can fail in ways a short rehearsal never shows: a
slow memory leak, FFmpeg processes that pile up, a decoder that stalls once in a
thousand starts. NewQ comes with a show built to find them and a log to read in the
morning.

```sh
newq --soak --short     # a one-minute lap, to try it first
newq --soak             # the real thing: laps of about 5 minutes
```

That one command works the same on Windows, macOS and Linux (on Windows run
`NewQ.exe --soak` from PowerShell or cmd; the Windows download has no console, so it
prints nothing and the prompt comes back at once, but it is running, and the report
is in the file). It makes the test clips it needs (about
50 MB, once, with the FFmpeg NewQ already uses; a build of FFmpeg without the Hap encoder, such as Homebrew's or the one in the Mac download, gets a show without the Hap cue, and one without x264, such as the Mac and Windows downloads', gets MPEG-4 clips in place of H.264) in a `soak` folder inside the log
folder (or `--dir FOLDER`), writes the show, opens NewQ on it with the stats log on and
GO pressed after five seconds, and when you close the window writes the report (below)
to the screen and to `stats.report.txt` beside the log.


The show plays itself: every cue hands over to the next, and the last starts the first
again, so it laps for as long as you leave it. Each lap starts and stops H.264 (MPEG-4 where FFmpeg has no x264) and Hap
video, loops with keyframed speed, position, blur and rotation, a fade cue that moves a
picture, music through an echo, a live clock, a three-picture slide show, an OSC send
and a wait. About two hundred laps in eight hours is a couple of thousand FFmpeg
processes started and stopped.

To run any show unattended with the same log, set `NEWQ_STATS=1` (and `NEWQ_AUTOSTART=5` to press GO) in the environment before starting NewQ; `--soak` does this for you.

Before leaving: switch off sleep and the screen saver, set the displays and audio
device in *Settings*, and watch the first lap. In the morning close NewQ and read the
report (or make it again from the log with `newq --stats-report FILE.csv`, which also saves `FILE.report.txt` beside the log, the only place the report shows on Windows). It shows how memory, open files and threads moved (with the trend per hour, after the
first tenth of the run), the most FFmpeg processes at once, the worst late-frame share
and audio load, the longest stall of the main loop, and a list of anything that looks
wrong (memory growing, a stall over a second, a gap in the samples that means the
program froze, slept or was restarted). `newq.log` and `newq-crash.log` in the log
folder say what happened if it did stop. The CSV has a row every ten seconds, so a
night is about 3,000 rows.

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
| `NEWQ_STATS`      | Write a CSV of memory, CPU, FFmpeg processes, late frames, audio load and main-loop stalls (a file, or `1` for `newq-stats.csv` in the log folder) |
| `NEWQ_STATS_SECS` | Seconds between rows of that file (default 10)                      |
| `NEWQ_AUTOSTART`  | Press GO this many seconds after starting (`1` = 5), once, for shows that run unattended |

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
