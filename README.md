<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/logo/newq-logo-dark.svg">
    <img alt="NewQ" src="assets/logo/newq-logo.svg" width="380">
  </picture>
</p>

NewQ is a free, cross-platform show playback and control system for theatre and
live events. The same program runs a slideshow from one laptop, or a rig of
networked computers playing video, sound and live cameras to projectors and LED
walls across a venue. It gathers the best ideas from years of show software into
one program, and adds the ones nobody else ships.

- **Small shows:** drop in pictures, films, sound, a PowerPoint or a PDF, and
  press GO. A presenter remote, a speaker timer and a clock are built in.
- **Theatre:** cue lists with fades and crossfades, layers, sound matrix routing
  and effects, show and edit modes, OSC, MIDI, MIDI Show Control and timecode in
  and out, projector control and a web remote for a phone.
- **Large rooms:** timelines, many displays with edge blending and mesh warp,
  cameras and capture cards, NDI, and streaming or recording any display. A show
  network keeps several computers in step on one clock, measured within a frame
  of each other, with a backup that can take control.
- **Planning:** a 3D view of the venue (a model of your own, or one of the
  samples) with the screens placed in it, showing what each display will look
  like in the room.
- **Before the show:** Test Mode checks that this computer, its displays and
  the network can play the show, and writes a report you can send.

It's built in Rust on [FFmpeg](https://ffmpeg.org), [wgpu](https://wgpu.rs) and
[egui](https://github.com/emilk/egui).

## Download

Pre-built binaries for Windows, Linux and macOS are on the
[Releases page](https://github.com/r2a42/NewQ/releases). Every download
includes FFmpeg and the MediaMTX streaming server, so there's nothing else to
install.

## Code signing policy

- **macOS:** the app is signed with an Apple Developer ID and notarised by Apple.
- **Windows:** the download is not code-signed yet, so SmartScreen warns the
  first time (see *Windows "protected your PC" warning* below). We have applied
  to [SignPath Foundation](https://signpath.org) for free code signing for
  open source projects; this section will name them once a certificate is issued.
- **What is signed:** `newq.exe` and the Mac app, which are built from this
  repository's source. The third-party programs bundled in the download (FFmpeg
  and MediaMTX) are separate open source programs; on Windows and Linux they are not signed by us, and in the Mac app they are
  signed with the app's Developer ID because macOS requires every program
  inside a notarised app to be.
- **How:** every release is built by the GitHub Actions workflow in
  [`.github/workflows/release.yml`](.github/workflows/release.yml) from a tagged
  commit; nothing is signed on anyone's own computer, and a Windows signature
  will need a manual approval for each release.
- **Team roles:** NewQ has one maintainer, Sean McNally
  ([@r2a42](https://github.com/r2a42)), who is its author, reviewer (of any
  change from someone else) and approver of signing requests. The accounts used
  for signing and for the repository use multi-factor authentication.

## Privacy

This program will not transfer any information to other networked systems
unless specifically requested by the user. NewQ has no telemetry and no update
check. It uses the network only for what you set up: OSC, MIDI-over-network,
the web remote, NDI, PJLink and TCP sends, and streams go to the addresses you
enter, a computer switched on to be part of a show network announces itself on
the local network and talks to the other NewQs there, and *Test Mode* downloads its free test media when you start a test with that box ticked. The zip a test makes
stays on the computer until you send it yourself.

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
- **Quitting during a show:** File ▸ Quit, Ctrl+Q (Command-Q on a Mac), the main
  window's close button and Alt+F4 ask first while cues are playing or show mode is
  on ("Cues are playing. Quit and stop them?"), and then about unsaved changes (Save,
  Don't save, Cancel; Save quits once the show is written, and a save that fails
  doesn't quit). These questions, and the ones for New and Open, are drawn in the
  window: the show plays on behind them, the keys still work, and Esc is Cancel.
  Closing a display's window in show mode or while cues play is ignored (the status
  bar says so) rather than switching that display off. A Save or Save As made while
  an earlier save is still being written (a slow USB stick or network share) is
  written as soon as that one has finished.
- **Control cues:** *Cue ▸ Add ▸ Control* makes a cue that acts on another
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
- **Hardware video decoding.** *Settings ▸ This computer ▸ Decoding* has a switch (saved for this
  computer, not in the show) that has FFmpeg decode video on the graphics card
  (NVDEC, VideoToolbox, D3D11/DXVA, VAAPI) where it can. It frees the processor
  for 4K and for many layers; each picture is still copied back to be composited,
  so it is not the zero-copy decoding of a native player. A file or card it can't
  do falls back to the processor. It is off by default and only the machine you
  will run on can say whether it helps: *Tools ▸ Test Mode ▸ Compare hardware decoding*
  measures it on that machine.
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
  are running, and *audio at N% of its time* when NewQ's mixing of the sound takes most of
  the time it has (above about 90% it breaks up; measured over a few buffers, so a moment
  the system held the sound up doesn't count as NewQ's mixing). Adding or removing a *sound*
  effect applies the next time the cue plays; everything else is live.
- **Live audio:** *Cue ▸ Add ▸ Live audio* plays an audio input (a Dante Virtual
  Soundcard or Dante interface, a mixer's USB output, a line input) on one of the
  audio outputs, from the input channel you choose (and the next). It has the
  volume, fades, output and channels of any audio cue and runs until stopped
  or for a time. The input is opened when the cue starts, not ahead of GO. If the
  input and output are on different clocks a little sound is dropped now and then
  so the delay doesn't grow; on one Dante device they stay together. Tested without
  hardware (the channel and sample-rate conversion and the cue's lifecycle), not
  against a real input.
- **Slide shows:** *Cue ▸ Add ▸ Slide show…* takes any number of images (chosen
  together, put in name order) and makes one cue that crossfades through them.
  Set one *Fade* (how long each slide takes to fade in over the last) and one
  *Duration* (how long each is shown in full before the next starts) for the
  whole show, and give any slide its own: change its fade or duration in the
  slide list (it turns yellow; Reset puts it back to the show's). The same list
  adds images, removes them and puts them in order, and says how long the
  show runs. A slide show is a group of image cues whose start times and
  fades are worked out from those settings, so each slide is still an
  ordinary image cue to give a position, size or colour (its timing is the
  slide show's). It plays once and the last slide fades out (in a slide show
  that advances on GO, below, the last slide stays until something else replaces it).
  *Advance* chooses what starts the next slide: **Automatically** (the above)
  or **On GO**, for a speaker stepping through a presentation: the first GO
  shows slide 1, each GO after it shows the next slide (crossfading as set),
  Back (the Left arrow, or a remote's back button) shows the one before, and
  after the last slide GO goes on to the next cue. Nothing advances by itself,
  and the durations aren't used. While such a slide show is on, GO and the
  remote step through its slides instead of firing the next cue; Esc, as ever,
  fades everything out.
- **Slides from a PowerPoint (or Keynote, OpenDocument, PDF) file:** *Cue ▸ Add ▸
  Slides from a presentation…* (or *From PowerPoint…* in a slide show's
  inspector) turns the file's slides into pictures. **A PDF needs nothing
  else:** NewQ draws its pages itself, so on a computer with no office program
  save the presentation as a PDF (every presentation program can) and choose
  that. A `.pptx` is drawn by the program on this computer: PowerPoint on
  Windows, PowerPoint or Keynote on a Mac, else LibreOffice (whose PDF NewQ
  draws). The pictures go in a new folder beside the file for each import
  (`<name> slides/import-…`) and become a slide show named after it. For a
  `.pptx` the file's own timing is used: a slide's *advance after* time becomes
  its duration, its transition time its fade, and hidden slides are left out;
  a deck with no *advance after* times in it (a speaker's deck) comes in
  with *Advance* set to **On GO**, one with times comes in **Automatically**.
  Animations, video and sound inside a slide are not kept, only the picture.
  Set `NEWQ_SLIDE_EXPORTER` to a program of your own (it is given the file and
  the folder) to use another. The office program may flash a window while it
  works. PDF drawing and the LibreOffice route are tested (and compared with
  poppler's pictures: the same); the PowerPoint and Keynote routes have not
  been tried on Windows or a Mac. A PDF has no timing, so it comes in as a
  slide show that advances on GO.
- **Windows "protected your PC" warning:** the Windows download isn't signed
  yet, so SmartScreen says *Unknown publisher* the first time. Right-click the
  downloaded zip ▸ *Properties* ▸ tick *Unblock* ▸ OK, then unzip, or choose
  *More info ▸ Run anyway*. The release workflow signs `newq.exe` when the
  repository has the secrets `WINDOWS_CERT_PFX` (the certificate as base64) and
  `WINDOWS_CERT_PASSWORD`; a new certificate still shows the warning until
  it builds reputation, unless it is an EV certificate.
- **New interface (optional):** *Window ▸ Interface ▸ New* (or *Settings ▸ Interface ▸ Classic
  or new*) swaps the panes for three
  views under one strip (the strip has GO and what it will fire, Stop all,
  Pause and *Show mode*; *Classic interface* switches back).
  The same show and settings, kept per computer, and nothing in the show file
  changes. *Editor* is the cue list with the inspector under it and what is
  playing beside it. *Operator* is for running a show: the next cue large, a big
  GO, the cues after it, what is running with the time left (with Fade out and
  Stop), the picture of the display, and lights for FFmpeg, the audio output,
  each display's monitor and each audio device. *Patch* draws cues, displays
  and audio outputs and the gear they reach (the monitor and any stream or
  recording of a display, the audio device with its level); drag from the dot on
  a cue to a display (a picture) or an audio output (its sound) to route it, and
  click a cue to select it. With timelines set to open as a workspace, a fourth
  view, *Timeline*, is the timeline workspace (see *Timelines*). *Settings* opens in place of the view. Routes
  can't be changed in show mode. The *Window* menu says the same things: in the new interface
  it lists the views (*Editor*, *Operator*, *Patch*, and *Timeline* when it is a view), the
  *Timeline* editor under the cue list, the *Cart*, *New preview window* and any windows of
  their own that are open (untick one to close it); the classic interface's pane ticks and
  *Reset layout* aren't there, since there is no dock to put a pane in. *Window ▸ Cart* puts
  the cart along the foot of the Operator view, under the GO (two rows of buttons fit at
  1280×760, more scroll; drag its top edge to make it taller), and *Cart ▸ Pop out* gives it a
  window of its own. Checked by drawing every view in tests, a test that drags routes, one that
  clicks every Window item and sees the window change, and one that fires and stops a cue from
  the Operator view's cart; the views by eye on Windows, the Window menu and the cart under
  Xvfb; not on a Mac.
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
- **Frame rate readout:** the top of the *Preview* pane says how many pictures a second
  are being drawn, on the display's own window if it has one (the number to
  trust for what the audience sees) and otherwise in the main window. *Settings ▸
  Interface ▸ Previews ▸ Frame rate on the display* writes the same figure in the top left
  corner of each display's picture, only while something is playing and never on an idle
  display; it is off by default and kept per computer, so don't leave it on in a show.
- **Statistics window:** *Tools ▸ Statistics* opens the live figures Test Mode shows while
  it runs, for an ordinary show: memory, processor, FFmpeg's processes and processor, the slowest
  frame, late frames, audio load, sound dropouts, cues and layers playing, and the frame rate of each display.
  It measures every 2 seconds while it is open and not at all when it is closed, and can also
  write a log file (`newq-stats.csv` in the log folder, the same file `NEWQ_STATS` makes).
- **Output windows on a Mac** are borderless windows above the menu bar, on every Space, so the
  menu bar and Dock are hidden on the output display and not on the display with the controls,
  and opening Finder or a swipe up for Mission Control does not bring them back over a show.
- **Closing a laptop's lid mid-show** (the built-in display goes away) leaves the display where it
  is. The system moves the main window onto the remaining display, which used to make NewQ treat
  that display as the main window's and drop the display from it; now the display stays, the main
  window stays behind it, and the window goes back to its own display when the lid opens.
- **Output windows go back over their display (Mac):** NewQ checks every pass that each output
  window still exactly covers its display, and puts it back if the system has moved it, which is
  what happens when a laptop's lid closes and opens (the external display's place changes). The
  system's own "displays rearranged" notice makes the check immediate, and a window the system moves is
  put back the moment it moves (at most four times a second). Output windows are backed with black, so
  the gap while the system brings a display back shows black, not the desktop. `newq.log` says when a window
  was put back, and when waiting for a frame on a display took over 100 ms (the system
  reconfiguring displays holds the picture for about half a second when a lid closes: that part
  is the system's).
- **Awake while showing:** while any output window is open NewQ holds the computer and its displays
  awake (screen saver, dimming and sleep can't interrupt a long black hold), shown as "☀ awake" in
  the status bar and turned off in *Settings ▸ This computer ▸ Power*. Editing a show with
  only the preview doesn't hold it. If the computer sleeps anyway, `newq.log` says so.
- **On battery:** a laptop with its lid closed keeps showing on an external display only while
  plugged in; on battery it sleeps, and nothing can prevent that. While outputs are open on battery
  the status bar and the Settings sidebar (beside *This computer*) warn, and the log says when the power source changes.
- **A frozen program is looked at (Mac):** if the main loop is silent for 1.5 seconds while a display
  is showing, NewQ runs macOS's `sample` on itself and saves what every thread was doing as
  `newq-freeze-<time>.txt` in the log folder (at most five a run); Test Mode copies the ones made
  during a test into its folder and lists them in the report. Send them with the log when a show
  freezes: they say which call the program was stuck in.
- **The pointer stays hidden on a display** when a lid is closed or opened: it is hidden again each
  time the system says the displays were rearranged.
- **Stalls are explained in the log:** when the main loop stops for more than 100 ms,
  `newq.log` says which parts of the loop took the time (*event loop stalled for 4815 ms:
  drawing the control window 4790 ms…*) and how much of it was spent outside NewQ's own
  code, how long the main thread itself worked, whether a thread that only sleeps was held up
  too (then the whole program was), inside the show engine which step and cue took the time,
  and the slowest picture made; a cue that takes more than 100 ms to start says so. A sound
  device's dropout is logged with its buffer size and whether NewQ's mixing was slow or the
  system called for sound late.
- **Settings:** every setting is in one place, *Settings ▸ Open settings…*
  (**Ctrl+,** to open and close it). It takes the place of the cue list, so GO,
  Stop All and the live preview stay where they are and a setting can be tried
  while the show runs; *Close settings* returns. A sidebar groups the pages, and
  says which are kept where:
  - Saved in the show file: **Show** (*Show*: its name, how long Stop All takes
    to fade, layers; *Media*: whether every file can be found, and the folders
    every computer keeps; *Show network*), **Picture and sound** (*Displays*,
    *Sub displays*, *3D stage*, *Audio outputs* and *Sync*) and **Remote
    control** (*OSC*, *MIDI* with MIDI Time Code, and the *Web remote*).
  - Kept on this computer, whatever show is open: **This computer**
    (*Interface*: where a timeline opens and how its view follows, the classic or
    the new interface, the previews; *This computer*: keeping it awake, hardware
    decoding, whether a show may run a streaming server or programs here, the
    log folder and how much the log says). A show opened on another computer
    finds these as that computer has them.

  Beside a page the sidebar shows what needs attention or is on: a red warning
  when a display's monitor is missing, the 3D stage's venue won't load, files can't be
  found, a listener failed to start or the computer is on battery, a green *on*
  for a listener that is working, and counts of displays and audio outputs. The
  *Settings* menu lists the same pages, in the same groups with the same two lines
  under them, and jumps straight to one.
- **The menus.** *File* (new, open, save, export the cue sheet), *Edit* (undo, copy and
  paste, find, renumber, show mode), *Cue* (*Add ▸* every kind of cue, grouped by what it
  plays; new lists and timelines; arming, grouping and moving the selected cue), *Window*
  (classic or new interface; then the classic interface's panes, new preview and reset, or the
  new interface's views, timeline, cart and new preview window; where a timeline opens),
  *Tools* (*Media manager…*, *Test Mode…*, *Statistics*), *Settings* (every page) and
  *Help* (*About NewQ…*, *Keyboard shortcuts…*, which is the table under Keyboard below,
  and *Open the log folder*). Both interfaces have the same menus; only the *Window* menu's
  middle differs, showing what the interface in use can show.
- **Cue list with a GO button.** Space (or 0) fires the cue at the playhead and the
  playhead moves on. Cues can have pre-waits and can **auto-continue** (fire the
  next cue at the same time) or **auto-follow** (fire the next cue when this one
  finishes).
- **Cue types:** Video, Image, Audio, Text, Fade, Stop, Wait and Group.
- **Built-in transitions on every video/image cue:** no extra cues needed to
  change what's on screen. Each cue chooses how it enters its display:
  - **Crossfade** (the default for new cues): fades in while everything else on
    that display fades out and stops.
  - **Dip to black** / **Dip to white**: the display fades to the colour, the new
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
  anywhere, it does nothing until *Settings ▸ This computer ▸ Allowed here ▸ Let send
  cues run programs* is ticked on this computer (a per-computer setting, not stored in
  the show).
- **OSC feedback.** *Settings → OSC → Feedback*: give a `host:port` and NewQ sends
  the show's state there as OSC: `/newq/playhead <number> <name>`, `/newq/paused`,
  `/newq/running`, `/newq/cue/<number>/status` (idle, prewait, playing) and
  `/newq/cue/<number>/progress` (0 to 1), `/newq/go <what> <then>` (what GO will do
  now, as the transport says it: "Continue Act 1", or "Act 1 is running" and "GO does
  nothing until it pauses"), and for each timeline list (and each timeline group
  while it runs, pauses or fades out) `/newq/timeline/<list name|group's cue
  number>/state` (`stopped`, `stopping`, `at_end`, `cueing`, `running`, `looping`,
  `last_pass`, `paused` or `parked`) and `/newq/timeline/<…>/position` (seconds, in
  tenths); spaces in a name become `_`, and a name that another timeline has too
  (two lists called the same, or a list named like a group's number) is replaced by
  `list:<id>` or `cue_id:<id>`, so no two timelines share an address. At most ten times a second and only what
  changed, with everything repeated every five seconds so a controller started late
  catches up. For TouchOSC pages, tally lights and other desks. (Only tested here
  against a loopback listener, not a real controller.)
- **Fade cues** are still available for dimming or ramping another cue's
  opacity and volume mid-cue.
- **Stop All / Panic** (Esc) fades everything out; press it again to stop instantly.
- **Displays and sub displays:** cues are sent to a **display** (*Display 1*,
  a name you choose such as *Stage TV*) or to a **sub display**, an area of
  one. Each display is assigned by hand to a monitor on the *Displays* page of the settings,
  so moving a show to another venue means reassigning each display once. *+ Add ▸
  Display* at the top of that page makes one and scrolls to it. A display's
  *Shows* says what it shows: *Its own cues*, *Part of* another display's
  picture (blended projectors, below), or *The 3D stage, from* a saved
  viewpoint (or *all of it*; see The 3D stage). A display that shows part of
  another's picture can't be given the stage: the menu greys it and says why.
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
  The preview keeps the display's own shape at any size of its pane (a sub display
  that shows part of another's picture: that part's shape), as large as fits, with
  black bars beside it or above and below. The pane's header has three modes:
  *One display* (chosen in the menu), *Multiview* and *3D stage* (the show's
  3D stage). *Multiview* shows every display at once in a grid, each under its
  name and in its own shape, as many columns as the pane's width allows (up to
  sixteen displays); *One display* goes back to the one chosen. While a display's
  corners are being edited the first Preview pane shows only that display, and
  goes back to its mode afterwards.
  There can be several Preview panes: *+* in a pane's header, or Window ▸ *New
  preview*, opens another beside the first, each with its own mode and its own
  display, and each can be popped out to a window of its own (double-click its
  tab) and closed again with its tab's ×. The modes, the displays chosen and
  where the panes are are kept per computer, with the window layout, not in the
  show; the multiview's arrangement is the show's, and every pane in
  *Multiview* shows the same one. In the new interface the Operator view's
  preview has the same modes, and *+* or Window ▸ *New preview window* opens a preview
  in a window of its own. There is no set limit on how many: each one is drawing
  work for the computer, and like every preview they make way for the displays
  (below).
  *Layout…* in the header (in Multiview) arranges the multiview yourself: from 2 to 16 tiles as a
  grid (2 × 1, 3 × 1, 2 × 2, 3 × 2, 4 × 2, 3 × 3, 4 × 3 or 4 × 4) or one large
  tile with three, four or five small ones along the bottom, and which display
  or sub display each tile shows (the same one may be in two tiles; a tile can
  be left empty). In the Preview itself, drag a tile onto another to swap them,
  and click a tile's name or right-click its picture to choose what it shows.
  The layout is saved with the show (*Automatic* goes back to every display in
  turn), so like other edits it can't be changed in show mode; a display that is
  deleted leaves its tiles empty. A show with no
  layout of its own shows every display, as before.
  The previews come second to the displays, but they are drawn as well as the
  displays leave room for. They give way only when a display really misses pictures
  people would see: a frame that was late *and* had a new picture to show (a
  film's next frame, a fade, a moving or animated layer, a clock, the 3D stage). A
  missed refresh while the picture stays the same doesn't count: a 25 fps film on
  a 100 Hz display shows each picture for four refreshes. What counts is the share
  of a display's new pictures that came late over the last three seconds, and the
  second after a cue starts or a timeline jumps isn't counted (pausing the previews
  couldn't spare the displays that). When it is too much while something plays (or
  NewQ's main loop is held up), every preview (single, multiview, 3D, the timeline
  workspace's monitor, in every Preview pane, in the main window or popped out)
  draws less, a step at most every three seconds while it goes on: 60, 30, 15, then
  5 pictures a second, then at half and a quarter of its size, then paused. Each
  keeps showing its last picture in between, so none goes black, and a display
  that only the previews show has its pictures copied to the graphics card no
  more often than that. The Preview pane says so ("Preview at 15 fps to keep the
  displays smooth", "Previews paused to protect the displays"). Two smooth seconds
  later they step back up a level, waiting longer (up to 20 seconds) each time
  the trouble comes straight back. Some displays miss pictures whatever the
  previews do (a system hiccup, a laptop lid, a screen without its graphics
  driver): when the misses go on at the same rate with the previews paused for a
  few seconds, the previews weren't the cause, so they come straight back
  ("Previews back: the displays' missed pictures aren't theirs") and that rate is
  let be; clearly more trouble steps them down again, and if it rises as soon as
  they are back they were the cause after all.
  Settings ▸ Interface ▸ *Previews* (this computer's own):
  - *Previews* On or Off. Off draws no preview pictures at all; each preview
    says so, with a button that turns them back on. The displays don't change.
  - *Frame rate*: Automatic, or at most 60, 30, 15 or 5 a second.
  - *Resolution*: Full, Half or Quarter: the largest size every preview (the 3D
    view too) is drawn at; giving way never goes above it.
  - *Give way*: **Relaxed** (the default) when about 5% of a display's new
    pictures come late, **Strict** at about 1%, or **Off**: the previews never
    give way, and on a busy computer the displays may stutter.
  *Test pattern* shows a picture for lining a display up, with a dropdown of
  fourteen beside it and a *Number* checkbox that turns the display's number
  on the patterns off (greyed for the patterns with no room for one; choosing a
  pattern turns it on; it works the same on sub displays, and the choice is saved
  with the show). The first, and the one a display starts with, is *SMPTE colour
  bars (HD)*, the HD bars of SMPTE RP 219 (ARIB STD-B28) that broadcast monitors
  and cameras show: from the top, the 75% bars; 75% white with 100% cyan and blue at
  its ends (do full colours clip?); a grey ramp from black to white with 100%
  yellow and red at its ends (banding, and crushed blacks or whites); and 100%
  white beside the black-level (PLUGE) strips, with the display's number over
  them. *SMPTE colour bars, classic* is the 1978 pattern (the reversed blue row and
  the -I, white and +Q blocks). Both are 4:3 in the middle of any display, with
  grey filling the width either side (wider on 16:9 than on 16:10, wider still on
  an ultrawide or an edge-blended pair, where the HD bars' coloured ends stay
  16:9's width); a 4:3 display, or a narrower one, is all bars. They are drawn for
  a computer's full-range picture, where black is 0 and nothing can be below it:
  turn the display's brightness down until the +2% strip (HD) or the +4% strip
  (classic) only just shows against the black beside it. *Colour bars and
  number* is the one NewQ had before, for telling which projector is which.
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
  of it. *Settings ▸ Displays ▸ + Add ▸ Blended projectors* sets up a pair: a *Wide picture*
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
  too, so a picture can go out without taking a screen. Destinations:
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
  - *Streaming server (MediaMTX)*: publishes the picture, as H.264 with no
    sound, to a streaming server such as [MediaMTX](https://github.com/bluenviron/mediamtx),
    which then serves any number of viewers (VLC, OBS, a browser through
    WebRTC, a media player). Choose *RTSP*, *RTMP* or *SRT*, the server's
    address (the port follows the protocol: 8554, 1935, 8890) and a stream
    name; viewers then open, for RTSP, `rtsp://server:8554/name`. It encodes
    with libx264 if your FFmpeg has it (NewQ's bundled Windows and Linux ones
    don't: libx264 is GPL), otherwise the computer's own hardware encoder
    (VideoToolbox on a Mac, then NVENC, Quick Sync, AMF) or OpenH264, found by
    trying each; if none works the status bar says so, and a computer with
    none of them needs a Custom output or an FFmpeg with libx264 (`NEWQ_FFMPEG_DIR`).
    *Encoder* on that section picks one by hand (Automatic uses the first that
    works). If viewers get no picture, try *OpenH264 (software)*: a hardware
    encoder can pass NewQ's quick test and still send a stream nobody can show.
    (AMD AMF did exactly that until its *low latency* mode was dropped: it sent one
    keyframe and never another, so anyone who joined the stream late, or whose
    connection dropped and came back, saw nothing.)
    The log (`newq.log`) names the encoder used.
    NewQ's Mac FFmpeg can do RTSP and RTMP but not SRT (that needs libsrt);
    whether the Windows and Linux ones have libsrt isn't checked here. A very
    wide picture is sent at most 4096 wide. No password for the server (use a
    Custom output for one).

    **The server comes with NewQ.** Tick *Run the server in NewQ* and NewQ
    starts the MediaMTX in its download (a `mediamtx` program next to NewQ;
    `NEWQ_MEDIAMTX_DIR` points elsewhere) when the stream is on, and stops it
    with the stream, so there is nothing to install or set up. It serves the
    stream by RTSP, RTMP, SRT, HLS and a **web player** (WebRTC, port 8889 by
    default) that works in any phone's browser. If it can't start (a port is in
    use, or two of its ports are set the same) the section says why and NewQ
    tries again every ten seconds. Without the tick, NewQ publishes to a
    MediaMTX you run yourself at the address you give.

    Anyone on the network can watch the stream, but only this computer can send
    one to the server, and a second sender can't take the stream over from the
    first. The server stops when NewQ closes, when it crashes (on Windows and
    Linux) or, failing that, at the next start of NewQ. It opens network ports,
    so a show can't start one on a computer that hasn't been allowed: ticking
    *Run the server in NewQ* allows it (so does *Allow this computer to run a
    server* on that section, and *Settings ▸ This computer ▸ Allowed here*
    turns it off again). What the server said is added to `mediamtx.log` in a
    `mediamtx` folder inside NewQ's log folder.

    One server serves every display that runs its own: the first such display's
    adapter and ports are used for all of them. A change to a port is acted on
    once you have stopped changing it for a second.

    **Network adapter:** choose which of the computer's network connections
    the stream is served on (*Automatic* is all of them). A server run by NewQ
    then listens on that adapter only, and the player link and QR code carry
    its address; a server on this computer is also reached by it. If the adapter
    you chose isn't there (the network is down, or the show was made on another
    computer) the server does not start, rather than listening on every
    adapter. NewQ can't
    make FFmpeg send from a given adapter to a server elsewhere (its RTSP, RTMP
    and SRT have no such option): to a remote server the adapter only decides
    the address in the links.

    **Player link and QR code:** the section shows the link to the web player
    (`http://address:8889/name`) and to the protocol itself (`rtsp://…`), each
    with a *Copy* button, and *Show QR code* draws the player link as a QR code
    that a phone's camera opens, so nobody types the address. If the server is
    on this computer and no adapter is chosen the link uses the computer's
    address on the local network. *Test server* tries the server's ports
    (SRT is UDP and can't be tried) and says which answered.

    Checked here by publishing to FFmpeg's own RTSP and RTMP listeners and
    decoding what arrived, and by starting the real MediaMTX through NewQ,
    publishing through FFmpeg, loading the web player page and playing the
    stream back over RTSP (Linux, libx264), and on Windows with AMD AMF through the
    bundled MediaMTX (a viewer joining late, five viewers over RTSP, RTMP and HLS for
    three minutes with flat memory); **not tried on a Mac, nor with NVENC or Quick Sync,
    nor a phone scanning the code**.
  - *Record to a file*: writes the display's picture to a new
    file each time it starts, named by the day and time (`NewQ 2026-11-09
    14.05.09.mov`). Tick *Record sound* to put a copy of what one of the show's
    audio outputs plays into the file as AAC (the two channels you choose of
    that device, at the volume heard, so Mute All mutes it too); it is picture
    only otherwise. Picture and sound are both timed by the clock as they arrive, so a frame the
    computer skipped doesn't shorten the picture against the sound; they start
    together to within a fraction of a second. Choose the *Folder* (the Downloads folder if empty), the
    *Codec* (H.264 or H.265), the *Container* (MOV, MKV or MP4) and the
    *Bitrate*; the picture's size is the height set for the send-out, and
    *Record only part of the picture* keeps a rectangle of it (left, top,
    width and height as a percentage of the picture). Stopping closes the file,
    which can take a few seconds, and NewQ waits up to a minute for it (also when
    you quit). A name that is taken gets (2), (3) after it, so two recordings that
    start together keep their own files, and changing a setting while recording
    starts a new file. **MKV is the default because it survives NewQ or the
    computer stopping suddenly; a MOV or MP4 cut off before it is closed can be
    unplayable** (the panel warns of this when either is chosen). H.265 needs an FFmpeg
    with libx265 or a working hardware encoder (NewQ's Windows and Linux ones
    have only the hardware ones). A display has one send-out, so it can record
    or stream, not both. Checked here with H.264 and H.265 on Windows (AMD
    AMF and software), files read back by FFmpeg at the size asked, a cropped
    area included, and a tone recorded as sound in MKV and MOV (not yet from
    a real audio device); not tried on a Mac, nor with NVENC or Quick Sync.
  - *Blackmagic output*: sends the picture to an UltraStudio or DeckLink
    output such as an **Express Monitor** or **Mini Monitor**. These are
    video interfaces, not displays, so they never appear as a second screen;
    NewQ sends the picture to the device itself (SDI or HDMI), as other show
    software does. Like Blackmagic input it goes through FFmpeg's `decklink`
    output, which NewQ's own FFmpeg doesn't have (Blackmagic's SDK terms), so
    it needs an FFmpeg built with `--enable-decklink` in the folder named by
    `NEWQ_FFMPEG_DIR`, and Blackmagic's Desktop Video installed. *Find* lists
    the devices; give a size (e.g. `1920x1080`) if the display's own size
    isn't one the device has. The display's frame rate must also be one of the
    device's modes, and *29.97 / 59.94 style* divides it by 1.001 for modes such
    as 1080p59.94. Picture only, no sound. The Mini Recorder and Express Recorder
    are inputs, and use the Blackmagic camera support above. **Written from
    FFmpeg's documented output and not tried with any Blackmagic hardware.**
  - *Custom FFmpeg output*: the arguments that follow the input (the picture
    arrives as raw RGBA), for anything else FFmpeg can send to: a streaming
    service, a file.
  Up to two displays can be sent out at once. A change to a *Send out* setting
  (typing an address, say) takes effect once you have stopped changing it for a
  second. A stream that can't start or that stops (the device is busy, the
  address can't be reached) is reported in the status bar and is not retried
  until its settings are changed, except a *Streaming server* one: it starts
  again by itself after 2, 4, 8, 15 and then every 30 seconds, so a server that
  restarts or a network that drops during a show comes back without you.
  Stopping a stream waits a few seconds for FFmpeg to say goodbye to the server and
  then ends it (a Custom output that may be writing a file is given a minute), so a
  server that has stopped answering can't leave an FFmpeg running, or hold up
  the next start. On Windows every stream's FFmpeg also ends with NewQ, however NewQ ends.
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
  to move that cue in time; its tracks open in the timeline editor, see
  *Timelines* below); *Play in order* fires each child when
  the one before it finishes. The group is running until all of its children
  have finished, so auto-follow after a group waits for the last child, and
  stopping or fading a group (or a Fade/Stop cue aimed at it) stops or fades its
  children. The playhead moves over a whole group at a time, and groups can be
  collapsed in the cue list. Adding a cue while a group is selected puts it
  inside; *Wrap in group*, *Move into group* and *Move out of group* (cue
  menu or right-click) move cues in and out, and Ctrl+↑/↓ moves a cue within or
  out of its group. Groups don't nest. In a *Play in order* group, a picture or
  video that crossfades in starts that many seconds before the one before it ends,
  and the two fade across each other (the earlier one doesn't fade to black
  first); with a cut, a fade-in or a dip it follows the old way, after the one
  before it. This is for pictures and videos on the same display and layer; a
  sound, or a picture on another layer, still fades out by itself, and a child
  with a pre-wait of its own comes in after the one before it. The group keeps
  its timing: a picture (or text, a shape, a camera) that starts early to
  crossfade is held that much longer, so it ends when it would have. A video
  can't play past its end, so each crossfade into a video makes the group that
  much shorter (the length the cue list shows counts it). In a *Start
  all together* group, the cues that start with it share the display without
  replacing each other; one with a pre-wait comes in later by itself and replaces
  what is on its layer, as any cue does, and so does a child you fire on its own
  while the group plays. (In a show from before layers were kept apart, see
  *Layers* below, they replace each other as they used to.)
- **Selecting and dragging cues:** Ctrl-click (Cmd on a Mac) adds a cue to the
  selection or takes it out, and Shift-click takes every row from the selected one
  to the one clicked. Delete, Duplicate, Ctrl+↑/↓ and arm/disarm then work on all
  of them (a group counts with its children). Drag a cue, or the whole selection by
  any one of it, to move it: a line shows where it will land, before or after a row,
  and dropping on the middle of a group puts it inside at the end (a child dropped
  by another child joins that group; a group can't go into one). The list scrolls
  when you drag near its top or bottom. Rows can't be dragged in show mode or while
  the Find box is filtering the list. Right-click one of the selected cues (the
  selection stays) and choose *Group the N selected cues* to wrap them in one new
  group where the first one is, or *Move N cues into group* to put them in a group
  that exists. A group among them is left out, and one that was inside another
  group comes out of it.
- **Shapes:** *Cue ▸ Add ▸ Shape* draws a solid **square, rectangle, circle,
  oval or star** in the middle of a display, in a colour you pick (the colour's
  opacity makes it see-through). A square, circle or star has one size and a
  rectangle or oval a width and a height, all as a share of the display's
  height; a star has 3 to 12 points. It plays like an image cue, with the same
  display, layer, transition, fades, hold time and geometry (move and scale it
  there), and the edges are smooth.
- **Editing several cues at once:** pick cues (Ctrl-click or Shift-click) and
  change a setting in the inspector: it is changed in all of them, whatever it
  is (duration, pre-wait, fades, layer, transition, volume, effects, arm…). A
  setting a cue does not have is skipped there, so a fade time reaches the
  videos and pictures and passes a wait by. Names, numbers, notes, hotkeys, files
  and text are each cue's own and never copied. A cue's position, colour
  correction or motion that is not set is the default: changing one number of it
  changes only that number in the others, not their own settings. A list (effects,
  audio routing) changes item by item, and only where the others have the same
  item. The playback settings reach text, shape and camera cues as well as videos,
  pictures and sounds.
- **Text cues:** titles, surtitles and lower-thirds. Type the text (several
  lines are fine), then set its size (a share of the display's height), colour,
  alignment, where it sits (top, middle or bottom) and where it wraps. It's
  drawn at the display's own resolution and then plays like an image cue, so it
  gets the same transitions, fades, layers, hold time and geometry, and a Fade
  or Stop cue can target it. There's one built-in font (a light weight, so
  *Shadow* and *Bold* help over video), and editing a text cue takes effect
  the next time it's fired.
- **Clock and speaker timer cues:** *Cue ▸ Add ▸ Clock* and *Timer*
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
  the device must support), then set the display, layer, transition, fades and
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
  **Capture cards:** a card's top modes (4K, or 60 frames a second) are often
  offered in only some formats, so a camera cue has a *Format* (Automatic,
  NV12, YUY2, UYVY, Motion-JPEG, H.264): if a mode won't open, try another, or
  press *Modes…* to ask the device what it offers (sizes, frame rates and
  formats; Video4Linux gives no frame rates) and click one to set the cue to it.
  Pictures are scaled down to the size of the display they go to, so a 4K card
  on a 1080p display doesn't send 4K frames to the screen (FFmpeg still has
  to take in and scale the 4K picture, which is heavy: 4K at 60 is more than
  a laptop may manage). On a Mac only the raw formats can be asked for. The
  Elgato Game Capture 4K S is a standard (UVC) capture device, and the Neo and
  4K X should be too; none of these has been tried with the cards themselves.
  **Blackmagic UltraStudio and DeckLink cards** aren't standard cameras: they
  are reached through Blackmagic's DeckLink interface, which FFmpeg reads with
  its `decklink` input. NewQ's own FFmpeg doesn't have it (Blackmagic's SDK
  terms don't let it be shipped), so these cards only appear in the camera
  list, as *Blackmagic · name*, when the FFmpeg in use was built with
  `--enable-decklink` (you need Blackmagic's SDK to build one) and Blackmagic's
  Desktop Video is installed. Put that FFmpeg and its `ffprobe` in a folder and
  set `NEWQ_FFMPEG_DIR` to it before starting NewQ. A DeckLink card is asked
  for its mode by name (*Modes…* lists them, and the size and rate follow the
  one you pick) or left to detect what it is sent; the format setting doesn't
  apply. The picture isn't deinterlaced, so an interlaced signal will show
  combing, and there's no sound from a camera cue (add an audio cue). All of
  this is written from FFmpeg's documented output and has not been tried with a
  card.
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
  `X-Pin` if set). Each timeline list, and a timeline group while it plays, has a
  card of its own under the GO button: its name, clock and length, what it is
  doing ("Running", "Paused · GO continues", "Looping · GO finishes this pass",
  "Stopping · fading out", "At its end"), Run, Pause and Stop buttons (Stop fades
  out over 3 s, as a Stop cue does), *Leave loop* while it loops, and a button for
  each marker that jumps there; the line above GO says what GO will do, and that it
  does nothing while a timeline list runs. Programs can use
  `POST /api/timeline/<timeline>/run|go|pause|resume|stop|hardstop|leave_loop` and
  `…/jump/<seconds or marker>` (also `…/run/<place>` and `…/stop/<fade seconds>`),
  where the timeline is a timeline list's name, a timeline group's cue number, or
  the `ref` the state gives each one (`list:<id>`, `#<cue id>`; `cue_id:<id>` works
  too), as over OSC.
- **Hotkeys, MIDI notes and the cart:** give a cue its own key in its
  inspector (click *Hotkey*, then press a letter, digit or F-key; P and A are
  taken by the transport). Pressing the key fires just that cue, like the
  *Fire* button, and leaves the playhead where it is. A key fires one cue:
  giving it to another cue moves it. A *MIDI note* does the same from a
  note-on (with MIDI control on); a note with no cue bound to it still fires
  the cue whose number is the note, as before. Tick *On cart* for the cues you
  want as buttons, then *Window ▸ Cart* for a grid of them (a pane in the classic
  interface, along the foot of the Operator view in the new one): click to fire,
  right-click to stop or fade out. Buttons show the hotkey, fill as the cue
  plays, and have a green dot when preloaded. Hotkeys work in the output windows
  too, and don't fire while you're typing in a text box. Copies of a cue don't inherit its hotkey or note.
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
  display. Position is a share of the display's size, rotation is clockwise about
  the middle of the picture, and edits apply live while the cue plays. Parts
  that leave the display are clipped. Two videos can share one display this way,
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
    but won't show its state; *OSC feedback* below sends NewQ's own.)
  - *Timelines over OSC*: `/timeline/<timeline>/…`, where the timeline is a
    timeline list's name (case doesn't count, and `_` stands for a space), a
    timeline group's cue number, or a reference that can't be mistaken:
    `list:<id>` or `cue_id:<id>` (the group's own id). Where two timelines answer
    to one name, a list wins over a group, the first tab over a later one, and a
    group in the list being worked over one in the main list over any other; the
    status bar says so each time, with the reference to send instead. Commands:

    | Address | Does |
    |---|---|
    | `…/run` (or `start`) | runs it from its playhead (a group from its start), or goes on from a pause; with a place, runs from there |
    | `…/go` | what GO would do to it: run, continue at a pause, leave a loop; nothing while it runs, and nothing to a list at its end |
    | `…/pause` | stops its clock and holds what plays, as the transport's Pause (live inputs stay live) |
    | `…/resume` (or `continue`) | goes on from a pause |
    | `…/stop` | fades it out over 3 s, as a Stop timeline cue does (a number argument is the fade in seconds; a second stop cuts) |
    | `…/hardStop` (or `panic`) | stops it at once |
    | `…/jump` (or `seek`, `marker`) | moves its playhead to a place: running, it plays on from there, else it parks there with its picture |
    | `…/leaveLoop` (or `devamp`) | leaves the loop it is playing, as GO would |

    A place is the first argument (a number of seconds, or a string: `75`,
    `1:15.5`, or a marker's name) or one more part of the address, for senders
    that can't send an argument: `/timeline/Act_1/jump/Interval`. A timeline or
    marker that isn't there is said in the log and nothing is done.
  - *MIDI input*: pick a port and, if you like, a channel. A note-on fires the
    cue whose number is the note number; a program change selects the cue
    with that number; and MIDI Show Control GO (with or without a cue
    number), STOP, RESUME, LOAD (gets the cue ready and puts the playhead on
    it), ALL OFF and RESET drive the transport. On the *MIDI* page *Show control
    device* limits that to messages sent to one device id (and to everyone, 127),
    for when several machines share a MIDI line. Timelines: MSC GO goes on with a
    timeline waiting at its pause as the GO button does, MSC RESUME with a timeline
    group's number continues it, and a note (or an MSC GO with its number) on a
    Run, Pause, Stop or Jump timeline control cue does what that cue does.
- **Audio outputs and channel routing:** on the *Audio outputs* page of the settings,
  add named outputs and put each on any audio device (or the system
  default). Each cue then picks an audio output and the first device channel its
  stereo pair plays on, so one show can feed a PA, a monitor desk and a
  multichannel interface at once. The first audio output is the default for cues
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
- **Layers and fit modes:** fit (letterbox), fill (crop) and stretch; per-cue
  opacity and volume can be adjusted live. Each picture cue (video, image, text,
  camera) has a **Layer** number in the inspector, from -1000 to 1000: layer 1 is
  drawn on top of layer 0, 2 on top of 0 and 1, and a negative layer goes under
  0. The cue list shows a layer other than 0 after the display's name. A cue
  that comes in with a cut or a crossfade replaces only what is on its own layer
  of that display; cues on the other layers keep playing (a dip to black or white
  still covers, and clears, the whole display). On the same layer the newest is on
  top. A show made before this (opened from an older NewQ) keeps its old way:
  a cut or crossfade there replaces everything on the display, as it did, until
  *Settings ▸ Show ▸ Layers* is turned on. New shows have it on.
- **Changes apply while a cue plays:** edit a cue in the inspector while it is
  on screen and it follows, with nothing to stop and fire again: layer, position,
  scale, rotation and crop, colour correction, effects, opacity, volume, fit, the
  display it is on, how long a picture, text, shape or camera is held, and a
  shape's or text's own settings (shape, size, colour, points, the words, the
  size of the letters), which are drawn again. Things that are fixed when a cue
  starts, such as the file, the in and out points, looping, speed, the audio
  routing and a transition's fade-in time, take effect the next time it is fired.
- **Copy and paste:** Ctrl+C, Ctrl+X and Ctrl+V (or *Edit*, or right-click) copy,
  cut and paste the selected cues, a group with its children. A paste goes after
  the selected cue, or at the end of a selected group, and the pasted cues are
  selected. They get new ids and numbers; a hotkey or MIDI note is left off, as
  with Duplicate. A fade, stop or control cue copied with the cue it aims at aims
  at the copy; copied without it, it keeps its aim in the same show and loses it
  in another. Cut and paste puts the cues back with their old ids (once), so what
  aimed at them still does. The copies stay for the whole session, so open another show
  and paste there (a display the other show doesn't have becomes its first one).
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
  temporary folder). If NewQ doesn't close properly, the next start offers, in
  the window, to restore them. Saving, or choosing not to save when closing, deletes the copy.
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
- **Lining sound up with picture:** *Settings ▸ Sync* sets how long to hold
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
  armed, hotkey, MIDI note, timecode and notes. A timeline's clips also have
  their *Start* on its clock, their *Track* ("V2 · Layer 1", "A1 · Main",
  "Cues") and the other displays a picture is *Also on* (their pre-wait column is
  left empty, since on a timeline it is the start); pauses and loops say what they
  do in the *Target* column, and the timeline's markers follow its last clip as
  rows of their own ("Marker", the name, its time).
- **Media manager:** *Tools ▸ Media manager…* lists every file the show uses and
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
  the show file, so a show folder can move between machines. Each file says
  which NewQ saved it. A show saved by a newer NewQ still opens: a strip over
  the window says so, cues of kinds this NewQ doesn't know are kept and marked
  *made in a newer NewQ: can't play here* (GO on one only moves on), settings it
  doesn't know are kept or fall back (a transition fades in, a fit letterboxes,
  an effect does nothing), and the first *Save* asks for a new name so the
  newer file isn't replaced by a copy that may lack some of it.
- Drag and drop media files onto the window to create cues.
- **Graphics device recovery:** if the graphics driver resets or the card is
  taken away mid-show, NewQ notices, says so in the status bar, and starts the
  main window and every output window again on a new device, usually in well
  under a second. Cues keep playing; a picture may blink out for that moment.
  NewQ always draws with a graphics card when there is one. If the only thing
  offered after a reset is the system's software renderer (on Windows
  *Microsoft Basic Render Driver*, which draws a picture every few seconds),
  NewQ uses it only to keep going, says so in red in the status bar (and, on a
  display computer, on the primary's row for it and in the network test's
  report), keeps looking for the card every few seconds, and moves back onto it
  as soon as it is offered again. A NewQ that started in software (with no card
  lost since) looks for a card for its first quarter of an hour, then stops
  (restart NewQ to look again). A display change in the middle of a frame no
  longer stops an output window's drawing.
- **A screen without its graphics driver (Windows):** when the driver for the
  graphics chip a screen is wired to is missing or has failed (Device Manager
  shows the chip with a warning, often *Code 31* or *Code 43*), Windows runs
  that screen through its *Microsoft Basic Display Adapter*, and every picture
  on it is drawn a few times a second. NewQ says so for that screen, plainly:
  *Screen 1 (B173HAN04.9) is run by Windows' basic display driver: install the
  graphics driver for this computer*: in the status bar (and in *Displays*
  beside the monitor), on the primary's row for a display computer, and as a
  problem in Test Mode's report. Install the graphics driver from the
  computer's or the chip's maker (on a laptop, often both the built-in and the
  separate graphics card need theirs), then restart the computer. Macs and
  Linux say nothing new.

## The 3D stage

See the show in the room before you are in it: a model of the venue, the
screens placed in it, and each screen showing what its display is showing now,
test patterns and cues alike. It is for planning: projectors can be stood in
the room to plan a rig and their light drawn (below); a projector ticked
*Mapped* lets the 3D model drive its display (tried only in simulation).

- **Where it is.** *Settings ▸ 3D stage* has a 3D view that stays at the top
  while you work, and under it a tab for each part of the stage: *Venue*,
  *Screens*, *Projectors*, *Viewpoints* and *Displays* (the displays that show
  the stage). Only the tab scrolls, so the view keeps showing each change as you
  make it; drag the line under the view to make it taller or shorter. The page
  uses the whole width of the window. Where the settings have little height
  (the classic window, under the transport) the view stands to the left of the
  tabs instead, and its handle makes it wider; in a very small pane all of it
  scrolls. Picking a screen or a projector in a view opens its tab. A Preview
  pane in its *3D stage* mode shows the stage too (several panes can, each from its
  own angle).
- **Start.** *Load a venue model…*, *Use a sample venue* (an arena, a
  proscenium theatre, a thrust stage, a black box, a club, a festival main stage
  or a ballroom, with its usual screens and viewpoints if you like), or *Add a
  screen*. A venue is optional: screens can be placed on their own, on a floor
  grid in metres. *Export as .glb…* saves a sample as a model to change in
  Blender or another 3D program; load it back with *Load a model…*.
- **Venue models.** glTF (`.glb`, or `.gltf` with its files beside it) and OBJ
  (`.obj`, with its `.mtl` and pictures). glTF is in metres with Y up; for an
  OBJ choose what its numbers are in (metres, centimetres, millimetres, inches,
  feet) and whether Z is up (3ds Max, SketchUp and CAD programs). **FBX can't be
  read yet:** export the model as glTF (`.glb`) from your 3D program (Blender,
  Maya, 3ds Max, Cinema 4D and SketchUp all can), or as OBJ. *Brightness* dims
  a bright model so the screens stand out; the model is read on a thread of its
  own, and a very large one is said in the status bar (one too big for the
  graphics card in one piece is drawn in pieces). A model reads only files in
  its own folder (and folders within it): a name with `..`, a drive or a full
  path is refused, as is anything that isn't a plain file.
  A screen made by a newer NewQ, of a shape this one doesn't know, is kept as
  it is (and saved so) but not drawn.
- **Screens.** *+ Add a screen*: a flat 16:9 screen, a curved wall (part of a
  cylinder, its width measured along the curve), a jumbotron (four screens
  round a hung box, and one underneath) or an angled LED wall (panels in
  columns, each turned a little). Each has a name, a position and a turn (yaw,
  pitch, roll), its size, and what it **shows**: a display's picture, or only
  part of it (*Only part of the picture*: left, top, width, height), for a
  screen that shows its share of a wider picture. A projector of a blended pair
  shows the whole picture the pair makes, as the audience sees it.
  *Duplicate* and *Delete* are beside the name. A new screen shows the first
  display.
- **Looking round.** Drag to turn round the stage; right-drag, middle-drag or
  Shift-drag to slide; the wheel to come closer; double-click (or **F**) to see
  all of it. Click a screen or a projector to pick it: its settings open on
  its tab.
  *Viewpoint ▸ Save this view…* keeps where you are looking from, with a name;
  *Viewpoint* goes back to it. *Grid* shows a floor grid in metres; *X-ray*
  makes the venue's walls see-through, so the screens show through them.
- **A display can show the stage.** On *Settings ▸ Displays*, a display's
  *Shows* can be *The 3D stage, from* a saved viewpoint (or *all of it*)
  instead of its cues: on its monitor, in any stream or recording it sends, and on the
  projectors that share its picture. It is drawn at the display's size every
  time something changes (a picture on one of the screens too, a test pattern
  or a still, though nothing plays), before any preview, never held back as a
  preview is. A display that shows the stage can't also be a picture on one of
  the stage's screens (it would show itself): that screen is left blank. A
  projector that shows part of another display's picture shows that display's
  stage, and a sub display can't show the stage. Renaming a viewpoint keeps the
  displays that show the stage from it. The 3D stage page's *Displays* tab
  lists them, each with *Open on the Displays page*, which goes to that display
  there.
- **The previews still give way.** The 3D views are previews: while a display
  misses pictures they are drawn less often, smaller, then held, as every
  preview is, and never above the *Resolution* set for the previews. A display that shows the stage is not (unless only the previews
  show it: no window and no stream). With no 3D view open and no display
  showing the stage for half a minute, what the stage held on the graphics
  card is let go.
- **In a show network** the stage is part of the show, so every computer has
  it, and the venue's model goes to the other computers with the media, with
  the files it reads beside it (a glTF's data and pictures, an OBJ's `.mtl` and
  pictures). Until they have come, a computer draws the screens without the
  room and says the venue is still arriving; the room appears once it is here.

### Projectors in the room

Will the picture fill the cyc from the balcony rail, and how bright will it be?
Stand a projector in the 3D stage and see, before hiring anything.

**For one projector on one flat screen you don't need any of this:** the four
corners on the *Displays* page line its picture up with less work. Projectors
here are for planning a rig: where to hang one, which lens, how big, sharp and
bright the picture is, on any screen, curved wall or set.

- **Add one.** *Settings ▸ 3D stage ▸ Projectors ▸ + Add a projector*:
  *Aimed at* a screen stands it square in front of that screen, where its
  picture fills it (a standard zoom, 10,000 lumens, the first display not
  already a projector); *From My projectors* adds one you kept; *Blank* puts one
  10 m out, pointing at the stage.
- **Its lens.** Pick *Ultra short throw (0.25)*, *Short throw (0.5)*,
  *Standard zoom (1.2 to 2.0)*, *Long zoom (2.0 to 4.0)* or *Very long zoom (4.0
  to 7.0)*, or type the **throw ratio**: how far away it stands for each metre
  of picture width (a throw of 2 means 10 m away for a 5 m picture). *Zooms*
  gives the range a zoom lens goes between, and a slider for where it is set.
  **Lens shift** moves the picture up or right without tilting the projector,
  as a share of the picture: 50% up puts the picture's bottom edge level with
  the lens. Data sheets give shift in different ways, so check yours.
- **Its brightness** in lumens, as its data sheet says (they tend to be
  generous), and its **pixels**: its display's where NewQ knows them, else
  chosen (1280 × 800 up to 4096 × 2160).
- **Where it stands** (*Position*, in the stage's units) and **which way it
  points** (*Turn*: yaw, pitch, roll). *Point at* turns it, where it stands,
  so its picture's middle (lens shift and all) lands on a screen's middle.
- **What it gives**, under its settings, about the screen its picture's middle
  lands on: "Picture on Screen 1: 5.8 m by 3.3 m, fills 96% of it", what
  spills over which edge, "about 520 lux there, about 165 nits to the audience
  (if nothing is in the way)", how far the lens is, and "One pixel is about 3
  mm there". A lens set past its zoom's end says so.
- **Fit to a screen**: *Move it to fill* a screen (square on, at the distance
  where its picture fills it without spilling, with its lens shift), or, for a
  zoom lens, *Zoom to fill it from here*. **Which lens from here?** says the
  throw ratio that fills the screen it points at from where it stands, and the
  usual lens that does it; *Use that lens* sets it.
- **In the 3D views** each projector is a small grey box (click it to pick
  it), with its light drawn as lines: its cone from the lens, and its picture's
  outline where it lands on the screens and the venue (the picked one's in
  yellow, the others' in blue). The lines show through walls. *Viewpoint ▸
  Through Projector 1* (or *Look through* on its row) shows the room as its
  lens sees it, lens shift and all, with the rest shaded; drag, the wheel or
  **Esc** leaves it. With the pointer over a screen or the venue the view says
  what lights it there: "Projector 1 + Projector 2 · about 410 lux · about 130
  nits · 1 pixel ≈ 3 mm" (the light of each projector that reaches it with
  nothing in the way, added up).
- **My projectors.** *Keep in My projectors* keeps a projector's lens,
  brightness and pixels on this computer (not in the show), to add again in
  any show. No list of makes and models is built in: wrong numbers would be
  worse than none.
- **How the light is worked out**, simply: the lumens spread evenly over the
  picture, falling off with the square of the distance and with the angle the
  light meets the surface at; nits are lux times the screen's gain over π (a
  matt white screen). 10,000 lumens on a 16:9 picture 6 m wide is about 494
  lux, 157 nits. Left out: lenses being dimmer at the edges, a screen's gain
  falling off to the side, black not being black, and light bouncing round the
  room. Treat the numbers as a guide.
- **Light.** Tick *Light* over a 3D view (the Preview pane's *3D stage* mode or
  the view on *Settings ▸ 3D stage*) and each projector throws its display's
  picture into the room: onto the screens and the venue, fading with distance
  and angle, with shadows where something is in the way, and adding up where
  projectors overlap. What it throws is what its display sends, its corners,
  mesh, part of a wide picture and edge blend included, so you can see your
  blend at work: set the blend widths and the bright band in the overlap
  fades. A projector with no display throws plain white. A screen no
  projector reaches (an LED wall) keeps its own picture. *Show* chooses what
  you see:
  - *Picture*: the light as it lands. *Auto exposure* shows the brightest
    projector's light on its screen as full white; untick it to set the nits
    shown as white yourself.
  - *Brightness*: how bright each place is at full white, in colours (under
    50, 50 to 100, 100 to 200, 200 to 400, 400 nits or more). Edge blends
    aren't counted here (an overlap shows as the sum of both): *Picture* shows
    them.
  - *Overlap*: how many projectors light each place (one, two, three or more).
  - *Pixel size*: how big a pixel is where it lands (under 1 mm up to 10 mm or
    more; the finest projector's where they overlap).

  The colours have a key in the view's corner. The light is the view's own,
  like the grid: each view has its own, nothing is saved in the show. A view
  lights at most 16 projectors (it says so if there are more). It is preview
  work: it gives way to the displays like every preview, drawing smaller
  pictures and shadows when they need the computer. A display that shows the
  stage shows it without light. A graphics card that can't draw the light
  shows the view without it, and says so.
- **Where a moving screen is.** A screen a *Move a screen* cue names is drawn where it is now in
  the 3D views, in their light and through mapped projectors (left out of their baked picture and
  drawn live, hidden behind what stands in front of it). Its row on the Screens tab says where it
  is *Now at*, and *Put at ▸* puts it at home or where one of its cues leaves it, at once, without
  changing the show: for rehearsals, and after a restart mid-show. Making a screen movable redraws
  the mapped projectors' pictures once; a GO never does. NewQ moves the picture as the cue says;
  it doesn't know where the scenery is. Tried only in simulation.
- **A screen's kind.** Each screen has a *Kind* on the Screens tab:
  *Projection, front*, *Projection, rear*, *LED or monitor* (it makes its own
  light: the light leaves its picture as it is and no mapped projector maps
  onto it), or *Works it out* (the default): a screen a projector's beam
  reaches, where everything stands, is projection, any other is LED. The row
  says what it came to ("Works it out: projection, front"). It is worked out
  from where things stand, not as they move, so a screen never changes kind
  while it travels through a beam.
- **Mapped** (tried only in simulation). Tick *Mapped* on a projector and its
  display shows the stage's screens as that projector sees them, instead of a
  picture of its own: each of its pixels shows the content of the spot on a
  screen it lands on. The content is on **canvases** (below). The projector's
  display then takes no cues; on the Displays page its *Shows* reads "The
  stage's screens, through Projector 1", its corners, warp and part of a
  picture are greyed (and kept, for when *Mapped* is switched off), and cues
  still sent to it are counted and not seen. A projector shows at most four
  canvases; past that its row says which are left black. The tick is greyed
  while the projector has no display, or while a screen shows its display
  (it would show itself): *Give Screen 1 a new canvas, and map it* does both
  in one click. For one flat screen, the four corners on the Displays page
  still do the same job with less work.
- **Line up** (tried only in simulation). *Line up…* on a projector's row
  finds where it really is from points: the page becomes two panes. On the
  left, the model through the projector's lens: click a spot you can find in
  the room (a click snaps to the nearest corner of a screen or facet; hold
  Alt to place it freely). A numbered cross appears on the projector and in
  the right pane, the projector's picture: drag the number, or click it and
  use the arrow keys (one pixel, Shift ten, Alt a tenth) while you watch the
  wall, until the cross sits on that spot. Do it for four spots or more.
  After every nudge NewQ works out where the projector is and says so in
  words under the panes ("Lined up to within about 2 pixels", which lens
  numbers it worked out and why not the others, a point that disagrees with
  the rest by name); the model's outline as that answer throws it is drawn
  in the picture pane and on the projector. Four points find where it stands
  and its aim with the lens as entered; six or more, not all on one flat
  surface, the throw too; eight or more, the lens shift. *Done* moves the
  projector there and keeps the points (one step to undo); *Cancel* leaves
  it as it was; *Back to before line-up* (in the panes, and on the row any
  time after) puts it back where it stood before it was first lined up. A
  projector with a display asks first, since its display shows black and the
  crosses meanwhile (or, ticked, its picture dimmed under them). Points on
  one flat screen that only fit a picture flipped left to right are refused.
  A line-up in progress is kept in `lineup-draft.json` in the node folder, so
  after a restart the page offers to carry on. It isn't possible in Show
  mode (one in progress is put away and offered again after), on a computer
  that follows a primary, or while Test Mode runs. From the primary the
  crosses go to the computer that drives the projector.
- Screens can move on a cue (*Screens that move*, below); line up with them at
  home. The points go on screens only (not on the
  venue model's own corners). The read-outs, the
  light and mapping are sums: nothing here has met a real projector, a light
  meter or a real graphics card (only a software one).
- A show with projectors opened in 0.25 plays as before; saving it there
  drops the projectors.

### Canvases

A **canvas** is a display with no monitor that a screen of the 3D stage shows.
You send cues to it as to any display; the screens show it, and the mapped
projectors that light those screens show each part of it where it lands. A
canvas is worked out, never set: a display becomes one when a screen shows it
and stops being one when it gets a monitor or no screen shows it. A display
with a monitor that a screen also shows (an LED wall's) stays a display and
says "Also on the stage: LED wall".

- **Make one.** *Settings ▸ Displays ▸ + Add ▸ Canvas* adds a display with no
  monitor and a picture size of its own (1920 × 1080 to start with); its row
  offers *Put it on a screen*. Or, on a screen's row (*Settings ▸ 3D stage ▸
  Screens*), *Shows ▸ New canvas…* makes one and points the screen at it.
- **Where it shows.** Its row on the Displays page has the tag *Canvas*,
  "Shown on: Cyc, Portal left" (each a button to that screen) and "No monitor
  (a canvas for the stage's screens)". Every menu of displays names it "Cyc
  content (canvas)" (a cue's *Display*, the Preview pane, the multiview, a
  projector's *Is display*), and a mapped projector's display "Projector 1
  (shows the stage's screens)", greyed in a cue's *Display* menu. In the new
  interface's Patch view a canvas's node says "canvas → Cyc", and a mapped
  projector's display takes no cue dropped on it. The 3D stage's *Displays*
  tab lists the canvases and what they feed.
- A canvas's delay (*Settings ▸ Sync*) is the one that counts for what is on
  it, for every projector that shows it.

### Screens that move

*Tried only in simulation: no real scenery, winch or projector yet.* A
**Move a screen** cue (*Cue ▸ Add ▸ Move a screen*) takes a screen of the 3D
stage from where it is to a place over a time: a wall that tracks downstage,
a flat that flies in, a revolve that turns. The projectors, the 3D views and
the light go with it.

**NewQ moves the picture as the cue says. It doesn't know where the scenery
really is.** If the scenery is half a second late, the picture is half a
second ahead of it. A screen with a few resting places (most theatre) is
exactly right; a picture that has to stay stuck to scenery while the audience
watches it travel needs tracking, which NewQ doesn't do yet.

- **The cue's page.** *Screen* (which one), *Takes* (seconds; 0 jumps) and the
  *Curve*. Under *To*, each of *Across (X)*, *Up (Y)*, *Towards the audience
  (Z)*, *Turn*, *Tilt* and *Roll* is ticked to move it there or left where it
  is, in metres and degrees as on the Screens tab. A turn keeps counting: 720°
  is twice round. *Use where it is now* takes every number from where the
  screen is, so you move it (or put it) where you want and press that; *Put
  the screen there now* jumps it to where this cue leaves it, for rehearsals.
  The page says where the screen is now.
- **Where it stays.** A screen stays where its last cue left it after the cue
  is over. *Stop all* and *Panic* don't send it back or stop it on its way:
  stopping the sound doesn't stop a winch. A cue moves only the numbers it
  gives, from where the screen is, so one cue can move a wall while another
  turns it. Opening a show puts every screen at home (its place on the
  Screens tab), and so does restarting NewQ: after a restart mid-show, *Put
  at* (on the Screens tab, and *Put the screen there now* on a cue) puts it
  where it was. Where screens are is never saved in the show.
- **On a timeline** a Move a screen clip moves with the timeline's clock and
  waits at its pauses. A seek or jump doesn't move screens (not yet): after
  one, *Put at* puts a screen where it should be.
- **On a show network** every computer moves the screen at the same shared
  time, so two projectors on two computers keep the picture on it together. A
  computer that joins late, or a backup that takes control, learns where the
  screens are from the primary; one that finds it disagrees with the primary
  eases to it over half a second rather than jump. A NewQ before this one keeps
  the cue in the show and doesn't play it.
- A screen a Move a screen cue names is drawn live by its projectors; it is
  left out of their lens maps, so moving it never makes the maps again.

## Show network: several computers

*New, and not yet tried on a real show network: try it between your own
computers before a show, and run the network test (below).* Several computers
run one show together. You run the show from one of them (the *primary*); the
others (*display computers*, and a *backup*) show pictures and play sound in step
with it. Each computer plays from its own copy of the show and of the media, so
no picture travels over the network while the show runs: only small messages (GO,
a seek, an edit) and a shared clock. A display computer keeps playing if the
primary goes away.

**What you need.** NewQ (the same version) on every computer, and a network
between them. A wired network (an Ethernet switch, ideally one just for the show)
is strongly recommended; Wi-Fi works, but its timing is looser and NewQ says so
where it matters. Nothing has to be set up on the network itself.

### The quickest way to make a display computer: NewQ Display

Every download has a second program beside NewQ: **NewQ Display** (*NewQ
Display.app* on a Mac, *NewQ Display.exe* on Windows, `newq-display` on Linux).
Open it on a computer with projectors or screens and that is all you do there:

- It asks nothing. There is no control window and no start-up picture: every
  monitor goes full screen at once with a quiet grey card saying the
  computer's name and which of its screens it is (*Assistant · Screen 1*,
  *Screen 2*…, numbered as the primary's Displays page and Test Mode number that
  computer's monitors, even when there is only one), its address and *Waiting
  for a NewQ primary on this network*. A monitor plugged in later gets a card
  too. The card is for start-up, so you can see every projector and screen is
  on and working: it stays until the primary plays its first cue, and from then
  on, for as long as NewQ runs, a screen is black whenever nothing plays on it.
  The card never comes back between cues (not when a cue ends, not if the
  computer drops out of step for a moment). No window is shown before it has
  drawn its first picture full screen, so nothing white or half-sized flashes up
  while NewQ starts.
- If no primary has reached it after about 20 seconds, the card says so: *Not
  reached by a primary yet*. If the primary lists the computer but can't add it,
  the firewall on the display computer may be blocking NewQ (on Windows its
  "allow access" question can hide behind the full-screen card: Alt+Tab).
- On the computer you run the show from, it appears under *Waiting to join* on
  *Settings ▸ Show network*: *Add to the show* and it is a display computer of
  your show, with nothing more to choose. It finds its primary again by itself
  after either of them restarts (no click on either).
- **Only NewQ Display opens display mode.** Opening NewQ itself on a display
  computer shows its normal window (locked: the show is run from the primary),
  with a line saying it is a display computer of the primary's show and two
  buttons: *Open as a display computer* (display mode now) and *Leave the show
  network*.
- **A show remembers its computers.** If you open your show on a different
  primary (a spare laptop, a new booth computer), the display computers the show
  uses that are waiting are added by themselves: the show vouches for them. A
  computer still in another primary's show network is never taken; it shows as
  *in another show network* until that primary releases it.
- **To quit it:** Ctrl+Q (Command-Q on a Mac), as its card says. Alt+F4 on
  Windows does the same.
- Keep NewQ Display in the same folder as NewQ (on a Mac, put both in
  *Applications*). It finds NewQ there and starts it as `newq --display`; if it
  can't find NewQ it says so.
- **On Windows, stop Windows Update changing the graphics driver during a
  show.** Windows can install or update the graphics driver by itself, in the
  middle of a show: it happened in a test, the screens blinked and NewQ had to
  recover its pictures. Before a show, pause updates (*Settings ▸ Windows
  Update ▸ Pause updates*). On Windows Pro you can also keep drivers out of
  Windows Update for good: in the Group Policy editor (`gpedit.msc`),
  *Computer Configuration ▸ Administrative Templates ▸ Windows Components ▸
  Windows Update ▸ Manage updates offered from Windows Update*, turn on *Do not
  include drivers with Windows Updates*. Do this on the primary too.

### Setting it up, step by step

1. **Install NewQ on every computer** and open it once.
2. **On each display computer** (the ones with projectors or screens): open
   *NewQ Display* (above), or in NewQ *Settings ▸ Show network*, tick *This
   computer is part of a show network*. With NewQ Display that is all you ever
   do there; joined from NewQ's own window, it keeps that window (locked) once
   added, until you press *Open as a display computer* on its banner or open it
   with NewQ Display from then on. NewQ asks the system to let it use the network the first time
   (allow it). To have it come back by itself after a power cut, tick *Starts
   at login* on its row on the primary (or on the computer itself), so a
   computer up in a truss never needs a keyboard again.
3. **On the computer you run the show from:** open your show, then *Settings ▸
   Show network ▸ Run the show network from this computer*.
4. **Add the computers:** they appear under *Waiting to join*; *Add to the show*
   on each. Give each its role from its row's menu: *Display*, or *Backup* for a
   second control computer. *Identify* shows a computer's name, which of its
   screens each one is (*Screen 1*, *Screen 2*…) and its address, big on black,
   on each of its screens for a few seconds, if you aren't sure which is which.
5. **Displays:** in *Displays*, give each display of the show a monitor: the menu
   offers every computer's monitors beside this one's.
6. **Sound:** in *Settings ▸ Audio outputs*, tick *Plays on* for the computer (or
   computers) each audio output should play on, and pick its device there.
7. **Media:** nothing to copy. The show's files go to every computer by
   themselves (below). For content you may use later, add its folder under
   *Settings ▸ Media ▸ Folders to keep on every computer* at load-in.
8. **Wait for "In step"** on every row of the Show network page, then run the
   network test (below) once. Then rehearse as usual: everything you do on the
   primary happens on every computer.

### The computers

- **Off unless you turn it on.** A NewQ that isn't part of a show network opens
  nothing on the network (no firewall question).
- **The computer you run the show from:** *Settings ▸ Show network ▸ Run the
  show network from this computer*. It becomes the *primary*.
- **Every other computer:** tick *This computer is part of a show network* on
  the same page (or start it with `newq --join`, handy for setting up several).
  Added as a display, NewQ opened this way keeps its control window (locked);
  its screens become the display computer's only through NewQ Display, start at
  login, or the banner's *Open as a display computer*. (`newq --primary` does
  the primary's button.)
  `newq --display` (what NewQ Display starts) switches it on and makes it a
  display computer from the first moment, before any primary has added it. It
  never undoes a computer's place in a show network: one already added stays
  with its primary, the backup stays the backup (with its controls), and a
  computer running a show network stays its primary (switch its network off
  first). `--display` and `--primary` together do neither.
- **Adding them.** On the primary they appear under *Waiting to join*; *Add to
  the show* makes one a display computer of this show. It then follows only
  this show network until it is released, and remembers it after a restart.
  The primary tries every address the computer was heard on (wired, Wi-Fi, a
  VPN), best first. If it still can't connect after about 10 seconds, the row
  says so, with each address tried, the kind of network and why it failed
  (*Can't reach WNO-2: tried 192.168.1.22:47820 (wired): connection refused.*);
  for a Windows computer it adds that Windows Firewall may be blocking NewQ
  there (its question can hide behind the full-screen card). It keeps trying.
- **One NewQ per computer.** Opening NewQ while it is already open (another
  copy, another download, NewQ Display) brings the open one to the front, hands
  it the show you opened, if any (it asks about unsaved changes first, as
  *File ▸ Open* does; never while something plays, a test runs or the computer
  follows a primary: the status bar says so), and the second copy closes with a
  short message. Two
  copies would share the computer's place in a show network. If two NewQs with
  one identity are heard anyway (one copied from another computer's settings,
  or an older NewQ left open), every page says so in red, and the other
  computers keep to the one they are connected to.
- **A row per computer** on the primary: online or not, its role (*Display* or
  *Backup*, chosen from the menu and written on that computer at once), its
  name (*Rename*), NewQ version (a different one is said plainly), wired or
  Wi-Fi where it can tell, free disk, its monitors and sound devices.
  *Identify* shows the computer's name, the screen's number and its address big
  on black on each of its screens for a few seconds, then what was there comes
  back;
  *Release* sends it back to waiting. On the other computer the page and the
  status bar say which primary it follows.
- **Addresses.** Each computer's row shows the address it is connected on and
  the kind of network (*Wired · 192.168.1.20*; the port only when it isn't the
  usual 47820); one offline shows its last known address. The page shows this
  computer's own addresses, one per network connection, so you can read them
  out, and a follower's page names its primary with its address.
- **The local network, not a VPN.** A computer with a VPN on (or Wi-Fi as well
  as a cable) is heard on several addresses; NewQ uses the best: the same wired
  network as this computer first, then Wi-Fi, a VPN last, and never a VPN while
  a local way exists. The shared clock and the media use the same address. The
  page says when a VPN is on, here or on another computer (*A VPN is on: it can
  slow the show network. NewQ is using the wired network (192.168.1.10).*), and
  when a computer is connected through one. A VPN that inspects local traffic
  can still slow the local network itself: turn it off for the show if the
  round trips on the page are more than a few milliseconds.
- **Displays and sound on other computers.** On the primary, *Displays* offers
  each computer's monitors beside its own: a display given another computer's
  monitor opens no window on this one (its picture is still in the preview).
  An *Audio output* can be ticked for any of the computers (the primary
  included, several at once: separate rooms, a booth monitor), with a device
  from that computer's list; one given only to other computers opens no
  device here. The show file keeps which computer each is on; a show that
  uses none of this is saved exactly as before.
- **Finding each other.** Computers announce themselves once a second on every
  network connection (UDP port 47820 and the next few; several NewQs on one
  computer each take the next free one). Where a network drops these
  announcements, type the other computer's address under *Add by address*.
- **Passcode (optional).** Only for a network that carries more than one show:
  set one on the primary and the computers of the show obey no primary that
  doesn't have it (the passcode itself never crosses the network). A computer
  waiting to join can be given it too, and then only that show takes it.
- **Without a passcode the show network trusts your network.** Anything on the
  same network that speaks NewQ's language and says it is your primary can run
  your display computers' cues and replace their show. Set a passcode when the
  network isn't only yours (a venue's, a festival's, one with guests on it):
  then a computer obeys only a primary that proves it knows the passcode, and a
  primary sends the show only to computers that prove it or that it added
  itself. Even then, keep the show network on a network you control: the
  passcode guards commands, it doesn't hide them, and someone already on the
  network can still get in the way while your primary is away.

### Media on every computer

Every computer of the show network gets every file of the show by itself: you
never copy media by hand, and nothing waits for it.

- **It just goes.** As soon as a computer is in the show, the primary sends it
  the show's files, the ones nearest the playhead first (then those of a
  running timeline from its clock, then the other cue lists, then everything
  behind the playheads). Jump the playhead about and the order follows it,
  about once a second, without starting anything again: a file under way
  finishes, and a file put aside goes on from where it stopped.
- **Rehearsal never waits.** Each computer plays what it has. Where a cue's
  file hasn't reached a computer yet, its display shows a plain card in the
  cue's place: *Media transferring*, the file's name, how far along it is and
  about how long is left. The cue keeps its place and its length (the primary
  tells the others how long each file is), so cue lists and timelines keep
  their timing. When the file arrives while the cue is still up, it takes over
  at the right moment of the cue. A file isn't played until all of it is there
  and checked. A sound on its way is silent until it arrives.
- **Media not found.** A file the primary itself hasn't (it was moved, a disk
  isn't mounted) can't reach anyone: its displays say *Media not found* and the
  primary's media manager lists it (*Tools ▸ Media manager…*), where it is found as ever.
- **Cards on display computers' screens.** *Settings ▸ Show network ▸ Cards on
  display computers' screens* has the two cards side by side, both kept in the
  show so every computer follows them:
  - *Show the start-up card*: the card is a display computer's name, address
    and how it stands, so you can see its screens are on and find it. *From
    start-up until the first cue* (the default) shows it until the primary
    plays its first cue; *Until the first cue, and on screens the show doesn't
    use* also keeps it on screens nothing is ever played on; *Never (black)*
    never shows it once the computer has been added to a show (a computer no
    show has added yet still shows it, or nobody could find it). Once the first
    cue has played, a screen with nothing playing is black for the rest of the
    run whatever is chosen: the card never comes back between cues, and a
    display computer that restarts mid-show comes up black as soon as it hears
    the primary. *Identify* always shows the name, screen and address.
  - *Show transfer cards on the displays*: off, a display shows black where
    *Media transferring* would be, for a show in front of an audience.
- **The library.** *Settings ▸ Media ▸ Folders to keep on every computer*: add a
  folder of content you might use (at load-in, all of it). It goes to every
  computer after the files the show uses, so a file is already everywhere when
  you reach for it, and its files are offered in the timeline workspace's
  *Files* shelf. A folder is read once: NewQ remembers what it read, so a big
  folder isn't read again after a restart.
- **Progress.** On the primary, *Settings ▸ Show network* has every computer's
  progress by bytes (percent, what is left, the rate, about how long), files no
  computer has, its free disk and what the next copy needs, and all the
  computers together at the top. While anything is copying the primary's status
  bar says so: playback may be less smooth meanwhile (it is a notice, never a
  lock). A computer whose file hasn't arrived is marked in the cue list (⏳,
  red when it is the computer that shows that cue's display; hover for which
  computers and how far along). Each computer's own page and status bar show
  its own progress.
- **Versions.** Change a file on the primary and the others get the new one.
  By default each computer keeps the last five versions of the show's media, so
  going back to an older show needs no copying. A computer short of room can be
  set from the primary to *Replace* (only the newest is kept), or to replace
  only very large files; the setting is kept on that computer. *Verify* reads
  every file a computer holds again and copies a damaged one again.
- Where it is kept: the `media` folder beside the log on each computer.
### Playing together

- **One clock.** Each computer keeps the primary's clock, asking it for the time
  several times a second. The primary's page says how far each computer's clock
  may be off (*clock ±0.05 ms* on a good wired network; on one computer, as when
  trying it out, it is the noise of the computer itself). Until a computer's
  clock has settled (a second or so) it starts nothing new, and says so; while
  it isn't settled the page says *not locked* rather than a figure. A computer
  asks the primary for the time at the port the primary's connection gives,
  whatever else answers with its name. If a computer's clock turns out to be a
  long way off the primary's (a quarter of a second or more, before it has
  settled), it is put right at once rather than run fast or slow for minutes to
  catch up, and what plays on that computer carries on from where it is.
- **Everything at the same moment.** Whatever you do on the primary that changes
  what plays (GO, firing, stopping or fading a cue, Stop All, Panic, pause, a
  timeline's run, pause, stop and seek, a slide show's step, a hotkey, OSC, MIDI,
  the web remote, a clock trigger) is done on every computer at the same moment
  by the shared clock, a little after you press: 50 ms on a wired show network,
  up to 400 ms on a slow one or Wi-Fi. The primary does it at that moment too, so
  they stay together. Each computer gets ready what it will start when it hears
  of it. The trial on simulated networks found every computer within one frame
  of the others (at 60 frames a second) through starts, seeks and pauses; what
  each display shows can still be up to one refresh apart, since displays don't
  refresh together without genlock hardware.
- **The show.** Every change made on the primary goes to the other computers a
  moment after you make it (the whole show, so nothing is missed), and to any
  computer that joins. What plays there carries on through it. A follower keeps
  the last show it was given and starts with it.
- **Catching up.** Several times a second the primary says what it is playing.
  A computer that differs (one switched on late, restarted, or that missed a
  message) puts itself right: it starts what it lacks part way in, at the right
  place, stops what the primary isn't playing, and moves a timeline's clock that
  is off by more than a frame. It waits to see a difference twice before acting,
  so a moment's delay is never mistaken for one. If the primary can't be
  reached, the others play on by their own clocks and pick up again when it is
  back. A group that plays its cues in order is caught up as what is playing,
  not as a group: what it plays next reaches the late computer the same way.
- **Only the primary sends, listens and records:** send cues (OSC, MIDI,
  projectors, network messages, programs), MIDI timecode out, OSC/MIDI/web
  remote input, clock triggers, Send out (streams and recordings). Nothing fires
  twice.
- **Sound** plays on the computers its audio output is given to; one given to
  no computer plays on the primary only. Two computers playing the same sound in
  one room will not be sample-locked.
- **Cameras.** A computer plays a camera cue only if it has that camera (a
  capture card in each display computer); otherwise it shows nothing for it.
- **Display computers** have no control window. Every monitor they have becomes
  an output window at start-up. Ctrl+Q (Command-Q on a Mac) on one quits NewQ there. From start-up until the
  primary plays its first cue, a quiet grey card says its name, its address, who
  runs the show and how it is getting on; after that a monitor with nothing
  playing is black. *Identify* from the primary shows the name, screen number
  and address on black on every monitor for a few seconds. A change
  of role takes effect at once (the control window closes or opens again);
  *Restart* on the primary's row starts NewQ again on that computer.
- **Backup computers** follow like any other computer and show the show, the
  playheads and what is playing, with their controls locked. Give a computer the
  *Backup* role from the primary's row (several can be backups). Its window has
  a **Take control** button beside *Leave the show network*, and says when the
  primary has gone quiet ("The primary (Booth) has not been heard for 12 s").
  *Take control* (and one more click, *Yes, take control*) makes the backup the
  primary: what plays carries on without a jump (the clock the computers share
  carries on with it), every computer follows it, and the old primary, if it is
  still there or comes back, becomes the backup and locks. Sends, OSC, MIDI,
  timecode, the web remote, clock triggers and recordings move to the new
  primary; so do sound outputs and displays given to no computer in particular
  (one given to a named computer stays with that computer). A sound output
  given by name to the old primary stays with it too (it may still be there,
  with the sound system plugged into it): the new primary's status bar and its
  *Show network* page say so, and *Play it here instead* gives it to the new
  primary in the old one's place (the other computers given it keep it). The
  page also says when an audio output is set to a sound device the new primary hasn't
  (the old primary's): it plays nothing there until one is chosen under
  *Settings ▸ Audio outputs*. The new primary
  sends the media from its own copy: a file it hadn't finished copying shows a
  *Media not found* card on it (its page says how many), while the computers
  that have it play it and send it on. The show it runs is the one the old
  primary last sent: it isn't saved on the backup, so *Save* asks where to keep
  it. Taking control is never automatic.
- **A primary remembers its computers** (with their addresses) and finds them
  again after a restart, even where the network drops announcements. *Forget*
  on an offline computer's row stops looking for one that is gone for good.
- **Start at login.** Never switched on by itself: the primary's row has a
  *Starts at login* tick for every computer, and the computer itself has one
  (on its locked window's banner, or *Settings ▸ Show network*). Ticked, NewQ
  starts when the computer starts, a display computer as one (`newq --display`),
  so one in a truss comes back after a power cut; unticked, the entry is
  removed. Where it is, to see or turn it off there: on Windows, Task Manager ▸
  *Startup apps*, listed as *NewQ Display* (*NewQ* for another computer); on a
  Mac, *System Settings ▸ General ▸ Login Items*, under *Allow in the
  Background*; on Linux, `~/.config/autostart/newq.desktop`. NewQ 0.24.0 to
  0.24.2 switched it on by themselves for a display computer (and 0.24.3 kept
  it): the first time a later NewQ opens on a computer it removes any entry an
  older one left, once, and says so for a while (*Start at login was switched
  off (an older NewQ had switched it on by itself); tick it to have it back*).
  A tick set from then on is kept. (Written by NewQ itself: a LaunchAgent on a
  Mac, the Run key on Windows, an autostart entry on Linux. Only tried on Linux
  so far.)
- **The other computers' controls are locked**, with a line saying who runs the
  show; *Leave the show network* takes one out.
- **Blended pictures.** The primary warns when a computer that shares a blended
  picture (an edge blend, or a part of a picture another computer shows) is on
  Wi-Fi or its clock may be more than half a frame off.
- Test Mode is off while a computer is in a show network, except for the
  network test below.

### Limits, plainly

- **Displays refresh on their own.** NewQ chooses the same frame of a film on
  every computer for the same moment, but each display shows it at its own next
  refresh. Without genlock (hardware that makes displays refresh together) two
  projectors can be up to one refresh apart (1/60 s at 60 Hz). Film the Sync
  flash (below) to see it.
- **Sound on two computers is not sample-locked.** Each computer's sound card
  keeps its own time. Sound from two computers is for separate rooms or separate
  speakers (a booth monitor, a foyer), not for one sound shared between them.
- **The firewall** asks the first time a computer is switched on: allow NewQ on
  private networks. NewQ uses UDP and TCP ports 47820 to 47827 (each NewQ takes
  the first free one), and each computer sends media from a port the system
  chooses, so allow the program rather than single ports. On a network that
  blocks broadcasts, add computers by address on the Show network page.
- **Only the primary sends** (above): a send cue or MIDI out fires once, from the
  primary.

### Every test on every computer

On a primary with computers in step, every test of *Tools ▸ Test Mode* (the *Cue
list* and *Timeline* parts and *More ▸ Check every feature*) runs across the
show network: the test show and its media go to every computer first (the window
shows each one's progress), then the primary plays it and every computer plays
along, each measuring itself as a computer alone would be measured (memory,
processor, FFmpeg processes, the frame rate of each display, stalls, sound
dropouts) and how it keeps to the primary (clock, round trip, each GO's lead, how
far apart, corrections). While it runs the window has a line per computer: in
step or not, each display's frames a second, and anything wrong. The result has a
part per computer and one verdict; a display that draws only a few frames a
second, a computer that stalls, and a computer whose measurements never came are
each a problem, never a silent pass. Your show comes back on every computer in
every ending (done, Stop, Cancel, NewQ closed, a computer dropping out and
returning).

- **Show on** lists every screen of this computer and of every
  connected computer, grouped by computer (*All* and *None* where a computer has
  more than one). Every
  ticked screen is tested at once and shows the same picture, each in its own
  window on its own computer (so each really draws). A display computer's
  screens that aren't ticked stay black. The ticks are remembered on this
  computer; a computer or monitor that has gone is skipped until it is back.
- **On the primary only**: the debug test's recording, streaming server and send
  cues (in a show network only the primary records, streams and sends), as the
  report says. *Muted* mutes every computer.
- **The hardware decoding comparison** is about one computer's own decoding, so
  it runs on a computer alone: switch the show network off for a moment on the
  computer you want to compare.
- **Time limits**: a computer that stops answering while the test waits for it
  (30 s), all the media there but a computer not in step (90 s), media that never
  arrives (20 minutes) each end the wait with a sentence saying what was waited
  for; your show is put back.
- **What the test adds** on each computer: the screens ticked for it, a look at
  memory and processor every 2 s on a thread of its own, and a short report over
  the network every second. Nothing is drawn on the displays for the test, so the
  frame rates are what a show would get.
- A computer that follows (a display or backup computer) can't start a test: its
  window says tests are started from the computer that runs the show.

### The network test

Before a show, run the network test from the primary: *Tools ▸ Test Mode*, tick
*Network* and press *Start*. It plays one lap at *Short* (about 2 minutes), three
at *Medium* and ten at *Long* (about 20 minutes; this was the *Network soak*).
The tick is offered on a primary with at least one computer *In step*; elsewhere
it is greyed, with the reason under it. Tick *Cue list* or *Timeline* too and they
run first, as one test. No command line.

- **What it does.** It puts your show aside and plays a short show of its own on
  the screens ticked under *Show on* (with a display computer's monitor the show
  doesn't use yet given a display of its own): a Sync flash, a film with sound, a
  timeline with a seek, a pause, Stop All. First its media goes to every computer
  through the show network, and the window shows each computer's progress until
  all of it is there, and how fast it came.
- **What it measures**, on each computer: how far its clock may be off and the
  round trip to the primary (and for how long its clock was not locked to the
  primary's), how early each GO arrived (or how late), every correction it made
  to keep in step, how far what its screens show was from the primary's, its
  own stalls and late frames, and whether it is wired or on Wi-Fi. *Apart on
  the screens* compares each cue's time now by the computer's own clock with
  the primary's as its heartbeat says (allowing for the shortest recent round
  trip, and taking the least of the last five seconds' heartbeats, so a message
  that waited on its way isn't read as the screens apart), so a clock that is
  wrong shows there; *began* (also in `network.csv`) compares when cues began, which a wrong clock
  can't upset. Frames are counted at the computer's slowest screen: a
  difference shorter than one of its refreshes can't be seen on it. A
  difference seen at one heartbeat alone (a loop going round between the two
  looks) isn't counted. A computer not in step for more than three seconds at
  a stretch (or comparing nothing while the primary played, as after a lost
  command) is a problem.
- **The answer**, per computer, in plain words: *In step: worst 0.4 frames apart
  over 2 min*, or what went wrong (*Late commands: 3 of 40 arrived after their
  time; use a wire*, *Clock error up to 6 ms: Wi-Fi*, *clock not locked to the
  primary's for about 2 min*, *dropped out of the network for 12 s*), and one
  verdict for all. Each problem is said once. *Use a wire* is said only to a
  computer that isn't wired. It says too what software can't see (the
  displays' refresh, sound between computers).
- **Your show comes back** on every computer at the end, after *Stop*, and to a
  computer that dropped out once it is back.
- **Send the zip.** The test makes one zip (*Show the zip in its folder*): the
  report, `network.csv` (every computer's measurements twice a second),
  `events.log`, this computer's log, and **each computer's own log** since the
  test began, as `computers/<name>/newq.log` (one per computer for the whole
  test, however many parts; two computers of one name get a short id after
  it; the report names it, and the window says whose did not come back). A display computer makes no
  zip of its own: everything comes back to the primary's, so that one file is
  all there is to send.
- **The Sync flash: see it with your own eyes.** The test starts with it, and
  shows it again later: a big frame number, a yellow bar stepping along a row of
  sixty slots, one slot a frame, and a white flash at the start of every second,
  on every display at once. Film two displays together with a phone in
  slow motion. In any one frame of the video, the two numbers should be the same,
  or one apart (that is the refresh, above); the bars should be in the same slot
  or the next. Two or more apart is worth sending with the zip.
- **Load the network on purpose.** *More ▸ Load the network while testing* (Off,
  *Light* about 50 Mbit/s, *Heavy* about 400 Mbit/s, or *As fast as it goes*)
  streams made-up bytes both ways between the primary and each computer, on a
  connection of their own, for as long as the test plays: the same strain as a
  big file copy over the network during a show, but nothing is downloaded or
  saved. It applies to every test that runs across the show network, so run
  once with it and once without and compare the round trips and late commands.
  The running window shows the rate with each computer now; the report says
  whether it was on and how much went each way (MB and the average Mbit/s per
  computer), or that a computer never connected for it (an older NewQ, a
  firewall). Greyed, with the reason, without another computer in the network;
  remembered on this computer, off unless chosen. Any trouble just stops it,
  with a line in the log.

## Timelines

A *Start all together* group is a **timeline**: each cue in it (a *clip*) starts at
its pre-wait on one clock, and the group can be paused, run on, stopped and moved
about on that clock.

**Making one.** Make a group and set it to *Start all together*, or, with the
Timeline pane open and no timeline chosen, click **New timeline group**. Add cues
to it as to any group (a cue added while the group is selected goes inside). A
group made this way has *Clips at opacity 0 wait for their fade-in* and
*Overlapping clips crossfade* ticked (see below); a group from an older show has
them off and plays as it always did.

**A timeline cue list.** A whole cue list can be a timeline, with a tab of its
own: *Cue ▸ New timeline*, or **+ ▸ New timeline** at the end of the list tabs.
Right-click another list's tab for *Make this list a timeline* (a list with groups
in it can't be one; its cues become clips at their pre-waits) and *Make it a plain
cue list* to go back. The main list is always a plain cue list.

- Its tab (🎞) shows the tracks editor in the cue list's place, full size, in both
  interfaces. *Cue rows* above it shows the same cues as rows, to number, name and
  arm them (with the Timeline pane open, the tracks are then under the rows, and
  the tab opens on its rows); *Tracks* goes back.
- Cues added while it is the tab you are working on become clips at its playhead:
  files dropped on the window (on a track: where they are let go), cues from the
  Cue menu, and cues pasted or moved into it from another list (as far apart as
  their pre-waits were). Several files go one
  after the other where their lengths are known. A picture goes on the top picture
  track with nothing on it at the playhead, or on a new layer above the others, so
  nothing is replaced; a sound on the first sound track; fades, stops and control
  cues on the Cues track. A still, a text or a shape is held for 5 seconds (trim it
  to the length you want). Groups don't go into a timeline list (moving, pasting or
  making one there is refused).
- **GO** on it runs the timeline from its playhead, carries it on at a pause cue,
  and does nothing while it runs or once it has played to its end; it never fires
  anything in another list. The transport says so: *Run …*, *RUNNING · next pause
  in 0:12.0* (or *ends in …*, or *loops back in …*) with *GO does nothing until it
  pauses*, *Continue …* and what comes in next, *Finish this pass and continue …*
  while a section loops, *… is at its end* (until a clip is put after the playhead
  or the playhead is moved). The web remote says the same. The new interface's header says the
  same, and its Operator view adds the run's state and clock. Pause (P) and Stop
  All work as everywhere; Stop All leaves the playhead where it was.
- In a show with *A picture replaces only what is on its own layer* off (one from
  NewQ 0.18.0 or older), the tab says that the tracks don't keep pictures over each
  other and offers to turn it on; nothing changes until you click it.

**The tracks editor.** Double-click a timeline group in the cue list (or click
*Open in timeline editor* in its inspector, or *Window ▸ Timeline*; while its tracks
are in sight the inspector says so instead, and a Timeline pane hidden behind another
tab is brought to the front by the button) to see its tracks: a row per display and layer ("V1 · Layer 0"), a row per audio output
("A1"), and a *Cues* row for the control, fade and stop cues. It is a pane under
the cue list (it can be popped out like the others); in the new interface it is a
panel above the inspector (*Close timeline* puts it away). Each clip is a bar from
its start to its end, with ramps for its fade-in and fade-out, and badges: 🔁 it
loops, ⏩ its speed is keyframed, 🔊 a video with sound, ∞ it has no end of its
own, +2 it is also shown on two more displays, ⚠ see *If a clip runs out* below
(and the warnings further down). A dimmed bar is disarmed. A sound clip shows its
waveform. Where a clip comes in over an earlier one on its track, the overlap is
drawn as a crossfade (an X, "crossfades in 2 s").

**Pane or workspace.** The tracks can open in a **workspace** of their own
instead: *Settings ▸ Interface ▸ Timeline editor*, the *Workspace* button at the end
of the tracks' toolbar, or *Window ▸ Timeline opens ▸ as a pane / as a workspace*
(a choice of this computer, in both interfaces). The workspace takes the place of
the cue list and the panes (in the new interface it is the *Timeline* view, next to
Editor, Operator and Patch) with the tracks across the whole width, the picture of
a display and the bin on the left, and the inspector of the picked clip on the
right, its timeline rows (*If it runs out*…) at the top. **◀ Back to cue list**
puts it away; *Show as a pane* goes back to the pane for good. Double-clicking a
timeline group, *Window ▸ Timeline* and going to a timeline list's tab open it
(the tab then keeps its cue rows, and its *Tracks* button opens the workspace). The
**bin** under the picture has three shelves. *New* has new clips (a file to choose,
text, shape, camera, live sound, fade, send, pause): click one to put it at the
playhead, or drag it onto a track to put it where you let go (a still, a text or a
shape is held for 5 seconds, wherever on a timeline it is made). *Files* are the
files the timeline plays (drag one onto a track for another clip of it), and *Clips*
its clips in time order (click to pick one). Drag the line between the picture and
the bin to give either more room. In
show mode it edits nothing, as elsewhere; the transport works. While the settings
are open in the classic interface they take its place.

- Drag a clip to move it in time, or up or down onto another track of its kind
  (a picture onto another layer or display, a sound onto another output); while it
  moves, an outline stays where it was. Ctrl-click and Shift-click pick several, as
  in the cue list, or drag a **rubber band** over the empty part of the tracks
  (Ctrl or Shift held adds to what is picked); dragging one of the picked clips
  moves them all. Drag a clip's end to trim it (with several picked, their ends
  move as far). Drag the small square at a clip's top corner to set how long it
  fades in or out (a picture that cut in then fades in). Moves snap to clip edges,
  fade-outs, markers and the playhead (the *Snap* button, or hold Alt while
  dragging to place freely). Each move or trim is one undo step.
- **Split:** Ctrl+K (Cmd+K on a Mac), the toolbar's *✂ Split* or the clip's menu
  cuts the picked clips (with none picked, every clip under the playhead) in two at
  the playhead; *Split here* in the menu cuts at the spot you right-clicked. The
  second half takes up where the first stops (its content, speed and keyframes); a
  fade cue stays aimed at the first half, and the second starts visible. A sound
  split in two may click or leave a tiny gap at the join, since each half starts on
  its own (the status bar says so). A looping clip isn't split.
- **Delete:** Delete (or the clip's menu) takes the picked clips off and leaves
  their gap; **Shift+Delete**, *Ripple delete*, or Delete with the toolbar's
  *Ripple* on, moves the later clips on their tracks up to close it (and the
  markers too when every track rippled). The keys act on the editor you last
  clicked in, or the one under the pointer; with cues that aren't clips of the
  timeline picked, Delete deletes cues as in the cue list.
- **Copy, cut, paste, duplicate:** Ctrl+C (Cmd+C on a Mac) copies the picked
  clips; Ctrl+X cuts them (Shift, or *Ripple*, closes the gap); Ctrl+V pastes the
  copied clips at the playhead, each on the track it came from and as far apart as
  they were (a fade, stop or control cue among them aims at the copy of its clip);
  Ctrl+D puts a copy of the picked clips just after them. Each is one undo step.
  As with Delete, the keys are the editor's when it holds them and what is picked
  is its clips (or nothing, for a paste).
- **Adding clips:** right-click an empty spot of a track for a new clip there: a
  video, image or sound file (a file dialog), a text, a shape, a camera or live
  sound. Files dropped on a track become clips where they are let go (a sound
  dropped on a picture track goes to a sound track). The *Cues* track's **+** adds
  a pause, a Stop timeline, a fade, a send or a wait cue at the playhead; its
  right-click menu adds them where you click.
- **Tracks:** *+ Track* in the toolbar adds an empty picture track (a layer above
  the top one) or sound track. Right-click a track's name (or an empty spot on it)
  to remove it when it is empty, or to move a picture track up or down: its clips
  and the next track's swap layers, so they go on top or underneath.
- **Speed and opacity lanes:** tick *Speed lane* or *Opacity lane* in a clip's
  menu for a lane under it: the keyframe line across the clip, with the value at
  the playhead (and for speed, the clip's length) in the lane's name. Double-click
  to add a key, drag a key to move it (the clip's bar follows as you drag: faster
  is shorter; keys at 0 before the opacity rises are its lead-in), right-click a
  key to take it out; the last one going leaves a steady value. The × closes the
  lane. Both are drawn on the timeline's own time, but they follow different
  things: **speed keys follow the clip's content** (through a free-run pause the
  content plays on, and so do they), **opacity keys follow the timeline's clock**
  (at a pause they stand still, so a ramp after a pause cue waits for GO). A clip's
  own fade-in and fade-out stay the slopes at its ends.
- **A clip on several displays** (its *Also on* displays) lives on its home
  display's track with a "+N" badge (hover for the displays), and a hatched ghost
  of it is drawn on the same layer's track of each other display; clicking a ghost
  picks the clip. Dragging the clip onto another display's track makes that its
  home.
- **Warnings (⚠):** a clip left to end if a pause in it outlasts it (*If a clip
  runs out*); a clip at opacity 0 that no fade cue raises ("waits for a fade cue":
  it never starts); and a clip that waits for its fade-in and enters by a Cut over
  another clip on its track ("cuts the clip under it": the one under goes at once
  and this one fades up from nothing; give it a fade-in transition or a higher
  track).
- Ctrl+wheel (Cmd on a Mac) zooms around the pointer; the wheel scrolls; *Fit*
  shows the whole timeline. **While the timeline runs the view follows the
  playhead:** when it passes nine tenths of the way across, or is out of sight (a
  jump, a loop going back, or you scrolled away), the view turns the page so it is
  a tenth in from the left. You can still scroll and zoom while it runs; nothing is
  moved while you hold a drag (a clip, an edge, a key, a marker or the scroll bar),
  and it catches up when you let go. Stopped, paused or parked, the view stays where
  you put it. Settings ▸ Interface ▸ *Timeline editor* (at the top of the page)
  can instead keep the playhead in the middle while it runs, the tracks creeping under it (near the start and the end
  the view stops and the playhead moves off the middle; a scroll of yours while it
  runs is undone, a zoom works around the playhead). The choice is kept for this
  computer. A thin bar at the right shows there are more tracks
  than fit; a lane you open is scrolled into view. On a narrow pane the toolbar
  takes two rows. The pane (or the new interface's panel) can be made taller by
  dragging its edge.
- **Markers:** *+ Marker* adds one at the playhead. Drag its flag to move it,
  double-click it to rename it, right-click for *Add Pause cue here*, *Jump here*
  and *Remove marker*. A flag's name is cut short ("…") where the next flag is
  close, and hovering it (or clicking it) shows it whole. Markers at the same time,
  or too close to tell apart, are one flag with a count ("×3 Chorus"): its tooltip
  lists them, and each click picks the next one, which the flag then shows and a drag
  moves.
- **The Cues track zoomed out:** each cue's name gets the room up to the next cue
  and is cut short with "…" to fit, or left out where there is too little room (the
  marker stays). Hover a cue's marker for its whole name, number, time and what it
  does; the cue under the pointer and the picked ones show their names whole, on
  top. Cues closer together than a marker's width (or at the very same time) are
  one marker with a second one peeping out behind it and "×2" before the name: its
  tooltip gives a short line for each (number, name, time and what it does; past
  eight, the rest are counted), and each click on it picks the next of them (zoom in to
  part them, unless they are at the same time). A marker flag of several on the
  ruler lists its markers the same way.
- Right-click a clip for *Pause before this clip comes in*, *Pause before this clip
  fades out*, *Split at the playhead*, *Split here*, *Delete*, *Ripple delete*, its
  lanes, *Freeze clip* and *Resume clip*.
- Editing is locked in show mode and while a test runs; Run, Pause and Stop still
  work.

**Running it.** GO on the group runs the timeline from the top, and the toolbar's
*Run* from where its playhead is (**Space** plays and pauses the timeline whose tab
you are on, or whose tracks editor you last clicked in: running, it pauses as *Pause*
does; anything else, it does what GO does to that timeline, so at a pause cue it
carries on, in a loop it leaves the loop, and stopped it runs; **0** is always GO); the red playhead follows the clock and the toolbar says *Running · next pause
in …*. The toolbar's *Pause* holds every clip where it is (live cameras and live
sound stay live) and *Run* carries on; *Stop* fades out what the timeline plays over
3 seconds (picture and sound) and leaves the playhead where it is: press it again
during the fade to cut at once, and GO or *Run* during the fade starts the timeline
again over the fading picture. While a timeline waits for GO (at a pause cue, or
parked), the main transport says what GO will do, "Continue …" and what comes in
next on it, instead of naming the next cue in the list; so do the new interface's
header and Operator view and the web remote.

**Pause cues.** A *Pause timeline* control cue on the timeline stops its clock when
the clock reaches it: nothing later starts until **GO**, which runs the timeline on
from there (the toolbar says what GO will bring in, "GO: crossfade to …"). What a
pause does to what is already playing is its *Freeze* choice:

- *Nothing: content runs on* (a **free-run** pause): only the clock stops; clips go
  on playing or looping. A transition timed after the pause (a crossfade, a fade
  cue, a clip's fade-out, a keyframe ramp) waits for the GO and then takes its full
  time. A fade already under way when the clock stops (a fade cue, a clip's
  fade-in, a crossfade, a dip, a fade-out as a clip runs out, a fade cue's matrix
  crosspoint levels) waits too, where it is, and finishes after the GO. A fade you
  start by hand (the active panel's fade-out or stop) or by a cue outside the
  timeline still runs. A clip frozen by its own *Pause* keeps following its
  opacity, position and effect keyframes with the timeline's clock.
- *Everything (pause on a frame)*: every clip holds its frame and its sound stops.
- *These clips*: only the ticked clips hold; the rest run on.
- *Live inputs freeze too* (for the last two): a camera or NDI source holds its
  last frame and live sound goes quiet; off (the default) they stay live, so a
  frozen picture can have a live camera over it.

**Play a section again until GO.** Tick *Loop: Play a section again until GO* on a
pause cue and choose where the section starts (*Section from*: a time, a marker or
a clip). When the clock reaches the cue it jumps back there and plays the section
again, round and round, everything in it together, until **GO**: GO lets the pass
that is playing finish and the timeline carries straight on past the cue (the
intro, a looping middle, the ending). Tick *On GO: Leave the loop at once* to go
on the moment GO is pressed instead. The toolbar says *Looping · GO continues*,
then *Looping · last pass, then on*. The jump back is not sample-accurate (the
clips are started again, or jumped, at the section's start): for a bed of sound
that must not break, set the sound clip itself to loop. A loop pause is blue on the
Cues track with 🔁, and its section is shaded on the ruler and the Cues track, with an
arrow back to where it starts; since the clock never stops there, its inspector
doesn't ask what it holds.

The other timeline control cues: **Run timeline** (from its playhead, or from a
point you choose), **Stop timeline** (fades out what it plays over its *Fade out*
time, 3 seconds unless you set another, 0 for a cut, leaving the playhead; a second
Stop during the fade cuts at once), **Jump in timeline** (moves the playhead to a time, a marker or the start
of a clip). Each acts on its own timeline, or on another you pick. A clip's own
*Pause* and *Resume* control cues (and *Freeze clip*) hold just that clip while the
clock runs on.

**If a clip runs out** before the timeline moves on (a free-run pause inside it
lasts longer than the clip): *If it runs out* in the clip's inspector chooses
**End** (the default: it fades out over its own fade-out, or the 2-second default
fade if it has none, or the time you set; 0 is a cut), **Loop** (round again from
its in point, pass after pass, until the timeline ends it; the next pass is got
ready ahead so it follows on) or **Hold** (its last frame; a sound goes silent).
The editor marks such a clip left on End with ⚠. (A loop-section pause doesn't
count: the clock never stands there.)

**Overlapping clips crossfade.** On a timeline with this ticked (new timelines
have it), a clip that comes in while an earlier clip on its track (a picture on
the same layer of a display they share, a sound on the same output) is still
playing crossfades over it for as long as they overlap, picture and sound, and
the earlier one doesn't fade out on its own as well. A clip wholly inside another
doesn't, and one that dips keeps its dip. Clips that butt up against each other
on a track follow on without a frame of black between them: the first is kept up
until the next has its first frame.

**Clips at opacity 0 wait for their fade-in.** On a timeline with this ticked, a
clip that is invisible when the clock reaches it (opacity 0, or opacity keyframes
that start at 0; a sound at silence) doesn't start its content until it begins to
fade in, by its own keyframes or by a fade cue, so the viewer sees it from its
first frame. The editor hatches the wait before it; a clip waiting for a fade cue
that never comes says so. A clip the run is waiting on right now is hatched in
blue. To have a clip rolling unseen from its start instead, tick *Already running
when it fades in* in its inspector.

**Moving the playhead.** Click or drag on the ruler: where you let go, the timeline
is **parked** there, with the picture at that moment on the displays and the sound
ready, and the toolbar says *Parked*. The picture changes all at once: what was up
stays (a video on the same clip is jumped where it is) until everything at the new
place is ready. A Jump cue does the same, and what it lands in is got ready ahead
of it. *Run* or GO plays on from there. (A click at
the very start shows the first frame.) If something else moves, runs, stops or pauses
the timeline while you hold the playhead (a remote, a key, a cue, GO), your drag gives
way: its playhead goes, and letting go does nothing. In show mode the ruler of a timeline group
is locked, since parking puts its picture on the displays; the ruler of the timeline
list you are working on stays free, as the operator's way to cue it.

**Keyframes on a timeline.** A clip's opacity, position and effect keyframes follow
the timeline's clock, so they stand still at a pause and go on from the GO, even
while the clip's content plays on. Its speed keyframes follow its content.

**A picture on several displays.** *Also on* in the inspector of a picture, video,
text, shape or camera cue (with more than one display) ticks the other displays it
is shown on too, besides its own *Display*: the same picture, effects, position and
keyframes, on the same layer. The cue list shows it as "Display 1 +2". Picking
several cues and ticking a display gives them all the same choice (a cue's own
display is left out of it). Each display is its own: a cue that comes in on one of
them replaces (or crossfades) it there only, and it stays on the others; it ends
when it is gone from all of them. Its sound plays until it has gone from the last
display it is fully on, then fades with it. A fade or Stop cue acts on it
everywhere. Text and shapes are drawn at the size of its own display and stretched
on the others.

**From a remote:** timelines take OSC (`/timeline/<name>/run`, `pause`, `stop`,
`jump` and more), the web remote has a transport for each, and OSC feedback tells
a controller each timeline's state and clock and what GO will do (see *Remote
control* and *OSC feedback*).

A show that uses the timeline control cues (Run, Pause, Stop, Jump timeline) does
not open in NewQ 0.20.3 or older.

## If NewQ crashes or won't start

NewQ keeps a log. It is in `%LOCALAPPDATA%\NewQ\newq.log` on Windows
(`~/Library/Logs/NewQ` on a Mac, `~/.local/state/newq` on Linux, where it also
goes to the terminal); the log of the run before is kept as
`newq.previous.log`. It records the displays and graphics card it found. If
NewQ crashes it writes what went wrong, where, and a backtrace to
`newq-crash.log` in the same folder, and on Windows shows a message with that
path. Please send that file (and `newq.log`) when you report a crash.

**How much the log says.** *Settings ▸ This computer ▸ Logs and recovery ▸ Log detail* chooses
it for this computer (it is not part of the show), and the change is used at once, with
no restart:

| Level | What is written |
|---|---|
| Errors only | what went wrong |
| Warnings | and what looked wrong: a stall of the main loop, a cue that failed, a late start, a device or stream lost |
| Normal (the default) | and what the show did: each GO, remote commands, output windows opening and closing, devices plugged in and pulled out, streams and the streaming server starting and stopping |
| Detailed | and each cue starting, playing, fading and ending, every FFmpeg started (with its whole command line and how long it took to start) |
| Everything | and the small steps: each cue being got ready and let go, each FFmpeg gone, running timecode |

Every line has the time to the millisecond (this computer's own time), the level, the
thread and the part of NewQ it came from:
`2026-11-11 19:14:03.250 WARN  [output-render] render: waiting for a frame took 140 ms`.
Use *Detailed* when you are looking for a fault and put it back afterwards: it writes a
great deal during a show. For a single run, `NEWQ_LOG=debug` in the environment does the
same without touching the setting (`error`, `warn`, `info`, `debug`, `trace`; parts can
be given their own, as in `NEWQ_LOG=info,newq::engine=trace,wgpu=debug`). The graphics
and window libraries NewQ is built on are kept a step quieter than NewQ itself. The log
can't grow without limit: at 20 MB it is put aside as `newq.earlier.log` and begun
again, so a show that runs for a week uses at most 40 MB (and the last run's log is
still there beside it). It is written by a thread of
its own, so a slow disk never holds the show up; if lines ever come faster than the disk
takes them they are dropped and the log says how many.

**Nothing is left running behind NewQ.** On a Mac and on Linux a small watcher
(`newq --reap`, a second copy of NewQ that does nothing else) is told each FFmpeg and
streaming server NewQ starts. If NewQ ends in any way, a crash or a forced quit
included, the watcher ends whatever is still running, so nothing is left holding a
camera or a processor. Should the watcher go at the same moment (a logout, say), the
next NewQ you start ends what was left. Only programs NewQ itself started are ever
ended: an FFmpeg you or another program started is never touched.

**A fault in one part does not end the show.** If a part of NewQ meets
something it did not expect while a show is running (a panic, in the
programmer's word), that part's work is dropped for the moment and everything
else carries on: the main loop comes round again, the cues keep playing and
the displays keep their pictures. A display whose drawing stopped is started
again by itself within two seconds, a cue whose sound or picture reader failed
ends with a message instead of hanging, a stream that stopped is retried, and
the controls let go of what was selected and draw again. The status bar says
what happened (*Drawing a window met an error and carried on: …*) and the
report is in `newq-crash.log` as before; the same fault happening again and
again is written out twice and then only counted, so it cannot fill the disk.
Only a failure to start at all (no graphics card, no main window) closes NewQ,
with a message saying why.

**The splash screen.** While the main window is being made ready (the graphics card is
set up first, which takes a moment) a small window with the logo and the version is
shown in the middle of the main display. It stays about two and a half seconds, longer
only if the main window is slow, and goes by itself; a click or a key sends it away
sooner. It is drawn on the processor, so it doesn't wait for the graphics card. Set
`NEWQ_NO_SPLASH=1` in the environment to start without it.

If the displays are what stops NewQ starting, run it with `--main-window-only`
(or set `NEWQ_MAIN_WINDOW_ONLY=1`): every display then stays in the main window
and no output windows are opened, so you can open the app and set up the
displays from the *Displays* page of the settings.

## Test Mode: try this computer before the show

*Tools ▸ Test Mode…* plays a show of its own to find out how this computer copes, then puts
your show back exactly as it was. Nothing to type. The window asks, top to bottom:

- **Length**: *Short* (a few minutes), *Medium* (about an hour) or *Long* (about 8 hours,
  overnight). Each item says how long it takes with what is ticked, and the line under the
  ticks says what will run (*Cue list, then timeline, then the network check: about 4
  minutes*). *What does this do?* opens a few words on each part.
- **Test**: *Cue list*, *Timeline*, *3D stage*, *Network*, any of them. Ticked together they
  run one after another (in that order) as one test. *3D stage* (below) is ticked by itself
  when your show has a projector or a display that shows the stage, and not otherwise; it isn't
  remembered. *Network* can be ticked only on the computer
  that runs a show network, with another computer in step; otherwise it is greyed and the
  line under it says why. In a show network the cue list and timeline play on every
  computer too, and the window says so.
- **Show on**: a tick per monitor (never the one the main window is on), grouped under each
  computer's name in a show network. Opening the window with none ticked ticks them all;
  *Start* needs at least one. With only the main window's monitor, the test shows in its
  Preview pane.
- **Sound**: the device the test plays on (your show's own audio outputs unless you choose
  one) and *Muted*.

Then *Start*. *More* holds what is rarely needed: *Download real films and pictures* (on
unless you untick it; *Where from?* names the sources), *Heavier load for the graphics card*,
*Load the network while testing* (in a show network: the network test, above),
*Check every feature* (the debug test, below) and *Compare hardware decoding* (below). The
window remembers the length, the ticks, the monitors and the sound on this computer.

At *Medium* and *Long* the time is the whole run's: the network part plays 3 laps (Medium) or
10 (Long), and the cue list and timeline parts share what is left of the hour or of the eight
hours.

**Several parts, one test.** While it plays the window says *Part 2 of 3: Timeline*. The result
gives the overall verdict first, then a section per part: its verdict, each computer's line
(its figures folded under it) and what it found (folded). Your show is put aside once, before
the first part, and back once, after the last: also when you stop it, when a part can't go on
and when NewQ is closed. A part stopped or failed ends the run; the parts after it are listed
as not run. The one zip has one `report.txt` and one `events.log` with a section per part; a
file two parts both wrote has the part's name in front (`cue-list-stats.csv`,
`timeline-stats.csv`). Across a show network each computer's own log is in it once, for the
whole test: `computers/<name>/newq.log`.

### Check every feature (the debug test), and one file to send

Under *More*. About two minutes. It cuts its clips and pictures from the same free film and NASA
pictures the longer tests use (the Sintel trailer and three Apollo photographs, about 12 MB,
downloaded once if *Download real films and pictures* is ticked; otherwise it makes plain ones
on this computer with FFmpeg), plays a show that uses every kind
of cue, one part at a time, and checks that each part *did what it should*: not only
that NewQ didn't fall over.

| Kind | What is played | What is checked |
|---|---|---|
| Video | a clip with sound; a loop of three; double speed; Hap (if this FFmpeg can make a Hap clip) | it started, its picture was handed to its display at full opacity, new frames kept coming, the mixer took its sound, it lasted its length and then ended; the loop went round three times; the double-speed cue runs half as long |
| Image, Text, Shape | a picture faded in and held; a title; a star | started, on its display, held its time, ended |
| Live text | a clock and a timer | both drawn, and drawn again as the seconds passed |
| Camera | the built-in `NewQ test pattern` device | live frames kept coming, and it ended when its time was up |
| Audio | a tone through a routing matrix, a fade cue (level and a crosspoint), a stop cue | the mixer took its sound; -6 dB before the fade and -30 dB after it; still playing until the stop cue, gone after it |
| Effects | a clip through blur (keyframed), vignette, sharpen, pixelate, negative; a tone through EQ, filter, compressor, echo, reverb | played through and ended |
| Slide show | three slides by themselves; three slides on GO | each slide came after the last and crossed over it; on GO each stayed until the next GO |
| Group | three pictures in order with crossfades; a clip, a shape and a text started together | order and overlap; all three on the display at once in the order of their layers |
| Transitions | crossfade, dip to black, cut | each new picture took the place of the last at the right moment |
| Fade | opacity, position and scale; then fade out and stop | 30% opacity and a quarter across after the fade; gone after the second |
| Control | load, start, pause, resume, go to, disarm, arm, reset, devamp, memo | the picture was got ready ahead; the clip stood still while paused and went on when resumed; the playhead moved; the cue was disarmed and armed again; reset stopped the clip; the endless loop ended after devamp |
| GO, wait and stop | GO on a picture that continues into a wait, followed after a pre-wait by a stop of everything | the wait lasted its second, the picture was there until the stop and gone after |
| Send | one OSC message to a socket of its own on 127.0.0.1 | it arrived |
| Timecode | a timecode cue with no MIDI output | it ran its two seconds and ended |
| Timeline | a timeline run by a Run cue: titles on one track crossfading where they overlap, a looping clip on another, a pause the clip plays on through, GO, a title crossfading in after it, a Stop cue's fade; then a timeline with the test-pattern camera, a pause that holds live inputs too, GO, a Jump cue to a marker past a title, a title waiting for its fade-in, a Stop cue that cuts | the overlap and GO's crossfade happened, both tracks were up together, the clock stood at each pause while the clip still moved and the camera's picture held, the camera was live again after GO, the jump never played what it passed, the waiting title came in 0.8 s after the other, the fade lasted and the cut didn't |
| Send out | the first display recorded to a file in the test's folder; the streaming server run on 127.0.0.1 only with the display streamed to it | the recording is read back with FFmpeg and is long enough; the server is asked over RTSP whether the stream is there |
| Load | four clips with sound and a text at once, twice round | all started and were on screen together; and the usual measurements (stalls, memory, late frames) |

Each part's clock starts when its first cue has its first picture or sound, so a slow
start of FFmpeg does not spoil the checks; the first cue not playing within five seconds
is itself a failure. Anything NewQ reports as a problem during a part fails that part.
It also checks the 3D stage: a sample venue saved as .glb and read back, the screens showing
their displays' pictures, a display showing the stage while a clip plays, and the projectors'
light (a card that refuses the light passes, and says so). Without a graphics card the last two
say "not checked".
*Live audio* is listed as not run (it would open a microphone). The sound parts are not
run on a computer with no sound device, and say so. The server part is not run unless
MediaMTX is with this copy of NewQ and this computer has been allowed to run a server
(*Settings ▸ This computer ▸ Allowed here*): the test changes no settings. The show it plays has layers kept
apart, as new shows have, whatever your show's own setting is.

**Nothing leaves the computer.** The check's show is a copy of yours with the cues taken
out and with its streams and OSC feedback switched off, so nothing of yours is sent
anywhere while it runs; the only network use is that one OSC message to 127.0.0.1 and
the server on 127.0.0.1. No projector is spoken to, no program is run, no MIDI is sent.
Your show is put aside and put back exactly as for the other tests.

**One zip.** At the end (also when you stop it part way, or it could not go on) NewQ
packs `NewQ-test-DATE_TIME_full.zip` in the test runs folder, and the window shows where
it is, with *Show the zip in its folder* (Explorer or the Finder with the file selected;
its folder elsewhere) and *Copy its path*. In it: `report.txt` (the verdict, what each
kind of cue did, every check of every part, the measurements), `events.log`,
`stats.csv`, `system.txt` (NewQ's version, the system, processor, graphics card,
displays, sound, FFmpeg and MediaMTX versions), `settings.txt` (this computer's settings,
the log level, the `NEWQ_`/`WGPU_` environment), `full-check.newq` (the show that was
played), `newq.log` at the level chosen in Settings (set *Detailed* first if you are
chasing a fault), the last run's `newq.previous.log`, any part of either put aside
for size (`newq.earlier.log`, `newq.previous.earlier.log`), `newq-crash.log` if there is one, and on a Mac the
samples taken while the program was frozen. The clips it made and the recording are not
in it. **It is not sent anywhere**: attach it yourself to a message if you want someone
to look. Short, Medium, Long and the hardware decoding comparison make the same kind of
zip. Once the zip holds them, the run's loose files are removed, so the zip is the one
thing a test leaves (if the zip can't be made, the folder stays and the window offers it).

**With no window:** `newq --self-test [--out FOLDER]` plays the same check through the
show engine and FFmpeg with no window at all, against a mixer that throws the sound away
(so it runs on a computer with no sound device), prints the report, writes the same zip
and ends with 0 (passed), 1 (a part failed) or 2 (it could not run). The recording and
the server need the app's graphics and are listed as not run. CI runs it on Linux,
Windows and macOS and keeps the zip.

### The cue list part

| Length | What it plays |
|---|---|
| Short | One lap of everything (about a minute): clips at 24, 30 and 60 fps, a slide show of large pictures, layers with effects and a moving picture, a 5.1 trailer with a downmix. |
| Medium | The same lap with a second trailer through sound effects, round and round: the run that finds a slow leak. |
| Long | A longer lap with H.265, a 30 MB 10-second clip, six large pictures, more effects and a burst of twenty quick cues, for an overnight run. |

### The timeline part

The *Timeline* tick plays timelines instead of a run of cues, with
the same downloads (less the 5.1 trailer), displays, sound, mute, memory guard, report
and zip. The show is a timeline list called *Programme*, which the window's cue list shows
as its tracks while it plays, and from Medium on a timeline group, *Interval*, in the main
list after it. Nobody needs to touch anything: where a pause waits for GO, the test
presses GO after 5 seconds (through the same GO the transport has); it leaves a loop
after three passes, just after the loop has gone back so every lap is the same length.

| Length | A lap | What it plays |
|---|---|---|
| Short | about a minute, once | A title over *Big Buck Bunny* (60 fps) butted to *Sintel* (24 fps), *Jellyfish* (29.97 fps) crossfading over the overlap, the Apollo 11 picture under a caption, the Blue Marble brought up from unseen by a fade cue, a pause GO ends, a closing title and a Stop that fades it all out; the Sintel trailer's sound under it all. |
| Medium | about 2½ minutes, round and round | All that, plus a third track (a picture in the picture with speed and opacity keyframes), the Sintel trailer split in two (held over the join) and frozen by a pause that holds everything, Curiosity on Mars also on the second display (with two displays ticked; the second shows Earthrise under it), the Jellyfish looping until GO, a Jump to a marker past a part, then the *Interval* group (a still pushing in under a title, a pause, a film). |
| Long | about 3 minutes, round and round | All that, plus a 25 Mbit/s film under four pictures at once (two H.265) each with an effect, Mars blurring, the trailer's sound through EQ and reverb, Mars and Andromeda in the group, and four seeks a lap while it runs (to a marker and back, twice). |

With two or more displays ticked, the timeline part gives each display its own picture
instead of mirroring the first. What they measure, on top of the usual figures:

| Finding | Problem when | Tracked when |
|---|---|---|
| The timeline's clock against real time, over each stretch it runs on its own (no pause, seek, loop or sleep) | more than 100 ms apart | more than 40 ms |
| Clips the clock reaches: how long after their place their first frame (or sound) came | more than 0.5 s, or never | more than 0.2 s |
| Getting clips ready after a start or seek (the clock stands meanwhile) | it ran out of the 2 s NewQ gives it | it took more than 1 s |
| Lap lengths | a lap more than 2 s (or a fiftieth, if more) off the others | |

The report has a *Timelines* part (how often GO was pressed, loops left, seeks; the
clock, the clips and the cueing in figures), and `timeline.csv` in the zip has each
measurement. A clip on a sound device that isn't there isn't judged (the report notes
there was no working audio output).

**Stalls and sound dropouts, in every test.** Each time the main loop is held up for 100 ms
or more (every display's picture stands still meanwhile) and each time the sound device
reports a dropout (a buffer underrun: the sound broke up for a moment), the test writes it
in `events.log` with its time and, for a stall, what the loop was doing (the show engine,
making the pictures, the test's own measuring, drawing the window), how much of it the main
thread spent working and whether the whole program was held up. The window counts both as
the test runs, `stats.csv` has an `audio_errors` column, and the report gives a line for each
(*No audio dropouts on …* when there were none). A dropout that came with the system calling
for sound late while NewQ's mixing was quick is said so (another program or the sound
device's power saving held it up; a larger buffer may help), not put down to NewQ's mixing:

| Finding | Problem when | Tracked when |
|---|---|---|
| Stalls of the main loop of 100 ms or more | one of 250 ms or more, or more than 12 an hour (100 ms is ten frames at 100 Hz; more than one every five minutes is several an act) | any |
| Sound dropouts | more than 4 an hour, or 3 within 10 s | any |

A run shorter than a quarter of an hour is counted as a quarter of an hour. The stall
finding says how many were the test's own doing, and when most began just after a whole
minute by the clock, says that something done once a minute is behind them, not the show.
The test measures memory and processor use on threads of its own, so it doesn't hold the
show up itself.

**The previews and the 3D stage, in every cue list and timeline test.** The previews in the
main window give way when the displays start missing pictures, and the report has a
*Previews* part that says how far they had to: this computer's preview settings, the Preview
panes open when the test began (and their mode; a 3D view says whether its light was on), the
seconds the previews spent at each level (full rate, 60, 30, 15 or 5 a second, half or a
quarter of the size, paused), how often they gave way and came back, anything the previews
said on the way (*not the previews' doing, so they went back to full rate*), each display
window's new pictures and how many came late, and what the 3D stage drew (views for the
previews and for displays that show the stage, the processor time a view took on average and
at the slowest, screens drawn without their picture, views drawn without the light, restarts,
the venue read). It ends with one sentence (*the previews drew at full rate throughout and the
displays stayed smooth*). `previews.csv` has a row each time the previews changed level (when,
from and to, the worst display window's late and new pictures over the last 3 s, stalls,
what was said), and each change is a line in `events.log`. `stats.csv` ends with three
columns: `preview_level` (0 full rate … 7 paused, as they were drawn: under this computer's
own limit, paused while its previews are off), `seen_missed_permille` (the worst display
window's late pictures since the row before, in thousandths of its new ones; blank when none showed 30) and `stage_ms`
(the slowest 3D view since the row before, to 1, 2, 4, 8, 16, 33, 66 or 133 ms, and 134 for
slower than that; blank when none was drawn). What it finds:

| Finding | Problem when | Worth knowing when |
|---|---|---|
| The previews gave way | | below full rate for a second or more (the displays smooth or not, said which) |
| Giving way didn't help the displays (the previews came back) | | the previews said so |
| The displays missed pictures with the previews paused or off | more than this computer's line (5% Relaxed, 1% Strict) of a window's new pictures: the previews are not the cause | |
| The displays missed pictures and the previews may never give way | | over 5% |

The previews at full rate throughout with smooth displays says nothing. The debug test and
the decode comparison have the three columns in their `stats` files but not yet the
*Previews* part; the network test has neither yet.

**A test keeps your 3D stage.** Its screens and projectors show the test's displays in place of
yours, so an open 3D view shows the test as it plays (with fewer monitors ticked than your show
has displays, the extra projectors throw no picture), and the report says what the stage was
pointed at. With no monitor ticked, no display shows the stage during the test, so the test's
pictures are seen. Across a show network a test leaves your venue model file out (unless *3D
stage* is ticked), so the other computers don't fetch it first.

### The 3D stage part: does 3D cost the displays here?

Tick *3D stage*. It plays the cue list's Short lap round and round in legs of the same length
(45 s each at *Short*, about four minutes in all; at *Medium* and *Long* the legs go round
for the part's share of the time), and compares them:

1. **Previews off**: every preview held off, the baseline.
2. **The 3D view**: a view of the stage in the Test Mode window, turning slowly, drawn as any
   preview is (it gives way to the displays too).
3. **Its light on**: the projectors' light in that view (no such leg when no projector throws
   a picture).
4. **A display showing the stage**: with two or more monitors ticked, the last one shows the
   stage in place of the test's pictures (a projector on it throws nothing then); with one,
   a display with no window does, drawn for the previews only, at their pace. Across a show
   network there is no such leg: changing a display mid-test would send the show to every
   computer again.
5. **A mapped projector**: the lap is also shown on a canvas that the screens show (from the
   first leg on, so every leg draws the same pictures), and in this leg the last ticked
   monitor's display is a mapped projector's, showing the screens as that projector sees them
   (its own projector, or one added for the test if none is on it). A mapped picture is drawn
   for every frame and never gives way, so it is judged by the same line as the other legs but
   in its own words; a graphics card that won't make the lens map is a problem (that display
   stays black here). No such leg across a show network, with no monitor ticked, or when the
   projector reaches no screen. Tried only in simulation.
6. **A screen moving**: the same mapped projector, with the first screen it maps made movable
   for the leg alone and swung to and fro (a quarter of a metre each way, turned 8°), so it is
   drawn live over the mapped picture, hidden where the room is in front of it. Judged by the
   same line, in its own words; a screen that never moved is said. Tried only in simulation.

With no 3D stage in your show it uses the *Proscenium* sample with its usual screens and a
projector for each ticked monitor (two make an overlapping pair), on the test's copy of your
show only. The previews' governor starts each leg afresh, and the first 4 s of each leg aren't
measured. The report's *3D stage* part says, leg by leg, how many of the displays' new
pictures came late, how often the 3D stage was drawn and the processor time it took, how far
the previews gave way and, for a display showing the stage, how many pictures a second it
drew; `stage.csv` has a row a leg. Then one sentence (*With the 3D view open and its light on
the displays missed no more pictures than without it (0.1% against 0.1%). The previews drew at
full rate throughout.*). The holding off ends with the leg however the test ends.

| Finding | Problem when | Worth knowing when |
|---|---|---|
| A 3D leg's displays missed more than the baseline's | by this computer's own line or more (1% Strict, 5% Relaxed, 5% when the previews never give way) | |
| The graphics card won't make a mapped projector's lens map | always (a mapped display stays black on this computer) | |
| The previews gave way in the 3D legs | never | once or more, or below full rate for a second |
| The displays already missed pictures with every preview off | | over the line: that much is the computer's, not 3D's |
| The light was refused by the graphics card, projectors past the 16 the light shows | | any |
| The display showing the stage drew under half its monitor's refresh | | any |
| The 3D drawing stopped and was started again | any | |
| The venue took 10 s or more to load | | any |
| Nothing to compare (no display window showed 30 pictures a leg), or a leg in which the stage was never drawn (the previews off on this computer) | | said, so a pass is never taken for a measurement |

Never run on a real graphics card, a Mac or Windows, or with real monitors: only headless under
Xvfb with a software card, where every figure is the software card's.

**The media.** The first time, Test Mode downloads free films and pictures (the
Blender Foundation's *Big Buck Bunny* and *Sintel* and the *Jellyfish* clip, under
CC-BY, from test-videos.co.uk and download.blender.org, and NASA pictures, which are
public domain) into `test-mode/media` in the log folder. The window says how much
first (about 20 MB for Short, about 100 MB for Long) and you can untick the box. Each file is checked
against a known checksum and by decoding a few frames; a file that is missing, damaged
or can't be fetched is made on this computer instead (colour bars, tones, test
pictures), so with no internet the test still runs, just with plainer media. The
window shows each file's progress, and a note when it fell back. The credits are saved
in every report.

**Tracking, and what the window showed.** While a test runs the window shows what it has
found so far. Something that moved but is usually harmless (memory stepping up while the
show warms up) is shown as *Tracking*, in blue, with gentle words, and does not count
against the result; only a rise that goes on for the last part of a long run, and doesn't
level off, is called a problem. Everything the window shows is also written to
`events.log` when it appears and when it goes, and the report lists what came and went
(*From 31:00 to 51:00 the window was showing this, and it cleared by itself*), so what
someone saw on screen is always in the files. `stats.csv` also records the memory NewQ has
asked the graphics card for (`gpu_mb`) and, on Windows, its committed memory
(`commit_mb`), which a leak in the graphics driver moves when the usual memory figure doesn't.

**It keeps the computer awake.** While a test runs NewQ asks the system not to sleep or
switch the display off (`caffeinate` on a Mac, the system's execution-state call on Windows,
`systemd-inhibit` on Linux), and says in `events.log` whether that worked. If the computer
sleeps anyway (a lid closed, a forced sleep) the report says so as a problem, with when and for
how long: the show stops while the computer sleeps, so the laps it fell in are left out of the
lap checks and the figures around then aren't a fair measure. (On a Mac, an hour-long test took
69 minutes by the wall clock because the Mac slept and woke briefly, and one lap took twice as
long.)

**Which display.** The ticked monitors all show the same picture, in place of your show's own
displays while the test runs (in a show network, see
[Every test on every computer](#every-test-on-every-computer)). The comparison below uses them
too, and `system.txt` of each run says where the test was shown.

**Which sound.** The chosen device takes the place of your show's own audio outputs while the
test runs (your show is not changed). *Muted* plays it without a sound: everything is still
decoded, mixed and put through its effects, so it costs what it would with sound. When the run is
long enough to judge memory and nothing is rising, the window and the report say *Memory and
cache use steady during this test*.

**Heavier load.** *More ▸ Heavier load for the graphics card* adds three more
films, each with an effect, to the cue list part's layered lap, so up to seven films decode at
once. Late frames then show where the computer's limit is, and are a note, not a problem.

**Does hardware decoding help here?** *More ▸ Compare hardware decoding* (on a computer
alone) plays a wall of four 4K H.264 clips twice, about two minutes in all, with
hardware decoding off and then on. It makes the 4K clip from the downloaded film with
an H.264 encoder this computer has (x264, else the system's), measures the processor
use of NewQ and FFmpeg, late frames and the longest stall of each trial, and says
*Turn it on*, *Leave it off* or *Little difference*, with a button to apply the advice.
It also shows what FFmpeg said about the hardware decoder it chose. Your show and the
setting are put back as they were, and the numbers are in `report.txt` of its folder.

**Your show is not touched.** It is put aside in memory while the test runs and put
back exactly as it was, with its file, undo history, selection and playhead; nothing is
saved over it, and its autosave copy is left alone. A show with unsaved changes can't
start a test (save first), and the test show can't be edited or saved. If you close
NewQ mid-test it ends properly first.

**The log.** Each run has a folder `test-mode/runs/DATE_TIME_PARTS` in the log folder
(and a zip of it beside it, as above) holding `report.txt` (verdict, findings, laps, measurements, media and credits),
`stats.csv` (a row every 2 to 15 seconds), `laps.csv`, `events.log` (what the engine
said, with times), `system.txt` (OS, processor, graphics, displays, sound, FFmpeg) and
an excerpt of `newq.log`. The window shows the path while it runs and at the end, with
buttons to open the folder and copy the path. The verdict is *Passed*, *Passed, with
notes* or *Problems found*: memory, FFmpeg processes, open files or threads that keep
rising for hours, main-loop stalls, late frames, heavy audio load, laps that run late or early,
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

`newq --soak --mapped` (with `--short` too, if you like) plays the same show on a canvas that
one curved screen in the Proscenium sample venue shows, through two mapped projectors sharing
it. Each projector's display sends its picture to a stream that keeps nothing, so the mapped
pictures are drawn all night on any computer; give the two displays monitors in *Settings ▸
Displays* to watch them. Tried only in simulation.


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

*Help ▸ Keyboard shortcuts…* shows this table in a window.

| Key                   | Action                                          |
|-----------------------|-------------------------------------------------|
| Space                 | GO on a cue list; on a timeline you are working on (a timeline list's tab, or the tracks editor you last clicked in or are pointing at): play / pause it — running, it pauses; paused, parked, at a pause cue or in a loop it does what GO does there; stopped, it runs |
| 0 (top row or keypad) | GO, everywhere (on a timeline, as GO does there: nothing while it runs) |
| Esc                   | Fade out everything (press twice: stop now)     |
| P                     | Pause / resume everything                       |
| A                     | Arm / disarm the selected cue                   |
| any cue's hotkey      | Fire that cue (letters, digits and F1–F12; a cue given 0 takes it from GO) |
| ↑ / ↓                 | Move selection and playhead                     |
| → / Page Down         | Show mode: GO (the next slide of a slide show that advances on GO) |
| ← / Page Up           | Show mode: the slide before in such a slide show; otherwise pause everything, and again to fade it out |
| Ctrl+↑ / Ctrl+↓       | Reorder the selected cue                        |
| ← ↑ → ↓ / Shift+arrow | Move the selected corner 1 px / 5 px (while editing) |
| ← ↑ → ↓ / Shift / Alt (line-up) | Move the picked line-up point 1 / 10 / 0.1 pixel (once it is clicked in the projector's picture) |
| Ctrl+D / Del          | Duplicate / delete cue (all selected)           |
| Ctrl+C / Ctrl+X / Ctrl+V | Copy / cut / paste cues                    |
| Ctrl-click / Shift-click | Add to the selection / select a run of cues  |
| Ctrl+Z / Ctrl+Y       | Undo / redo (Ctrl+Shift+Z also redoes)          |
| Ctrl+N / Ctrl+O / Ctrl+S | New / open / save show                       |
| Double-click a timeline group | Open it in the tracks editor            |
| Ctrl+wheel (tracks editor) | Zoom around the pointer                    |
| Alt while dragging (tracks editor) | Place freely, without snapping     |
| Ctrl+K (tracks editor) | Split the picked clips (or those under the playhead) at the playhead |
| Del / Shift+Del (tracks editor) | Delete the picked clips / ripple delete (close the gap) |
| Ctrl+C / Ctrl+X / Ctrl+V (tracks editor) | Copy / cut the picked clips / paste at the playhead on their tracks |
| Ctrl+D (tracks editor) | Duplicate the picked clips just after them     |
| Ctrl+Shift+S          | Save as                                         |
| Ctrl+Q                | Quit (asks first while cues play, in Show mode, or with unsaved changes) |
| Ctrl+F                | Find cues                                       |
| Ctrl+,                | Open / close the settings                       |
| Ctrl+Shift+L          | Show mode / edit mode (leaving show mode asks first) |
| F (pointing at a 3D view) | See all of the 3D stage (double-click does too) |
| Esc (in a 3D view looking through a projector) | Leave the look through its lens |

Space, 0, Esc and the cues' hotkeys also work, the same way, while an output window
has focus (a cue given 0 takes it from GO there too); in show mode, so do the arrows
and Page Up/Down.

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
| `NEWQ_TEST_PANIC` | For trying the above: `engine@10` makes that part fail once, 10 s after starting (`engine`, `ui`, `outputs`, `render`, `audio`, `uploads`: the thread that writes pictures to the graphics card) |
| `NEWQ_LOG`        | How much the log says for this run, whatever the setting: `error`, `warn`, `info`, `debug` or `trace`, and parts by name (`NEWQ_LOG=debug,wgpu=info`). `RUST_LOG` is read if this isn't set |
| `NEWQ_STATS`      | Write a CSV of memory, CPU, FFmpeg processes, late frames, audio load and main-loop stalls (a file, or `1` for `newq-stats.csv` in the log folder) |
| `NEWQ_STATS_SECS` | Seconds between rows of that file (default 10)                      |
| `NEWQ_AUTOSTART`  | Press GO this many seconds after starting (`1` = 5), once, for shows that run unattended |
| `NEWQ_NODE_DIR`   | Folder of this computer's show network identity (`node.json`; default the log folder), to run several NewQs on one computer |
| `NEWQ_NET_PORT`   | First port of the show network's range (default 47820)              |
| `NEWQ_MEDIA_RATE` | Bytes a second the primary sends media at most (for trying transfers; default as fast as it goes) |

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
only to say which control messages it understands. NewQ contains no QLab code, files or
text.

## License

NewQ is licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT),
at your option. FFmpeg is a separate program under its own licence (LGPL/GPL
depending on the build); if you distribute FFmpeg binaries alongside NewQ,
follow FFmpeg's licence terms for them.
