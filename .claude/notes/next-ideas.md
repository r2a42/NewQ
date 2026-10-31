# Notes for later (not started)

Saved from the owner's list, 2026-10-31. Nothing here is built. The "to settle" lines are
questions to answer before building, not decisions.

## 1. FPS in the preview, optionally on the main display

The preview pane should show the FPS at the top, with a checkbox to show it on the main
display too.

To settle:
- Whose FPS: the rate the output windows are being drawn at (`render.rs::FrameTimer` already
  counts frames per window), the video's own frame rate, or the engine's tick rate? Probably
  the drawn rate of that output, perhaps with the playing video's rate beside it.
- "On the main display": an overlay burned into the output picture itself (so it shows on the
  projector or monitor), or on the main window's preview only? An overlay on a real output
  must be off by default and obvious, so nobody leaves it on in a show.
- Where it is kept: per computer (`Prefs`), not in the show.

## 2. An optional stats window, from Test Mode, usable in the main show

The live figures Test Mode shows while it runs (memory, processor, FFmpeg processes, slowest
frame, late frames, sparklines, the findings) as an optional window or pane that can be turned
on in an ordinary show, not only during a test.

To settle:
- A dock pane (`dock.rs`, `Pane`) that can be popped out, or a floating window like Test Mode's?
- Sampling cost and who owns it: `StatsLog::frame` refreshes every process each time. Outside a
  test it would want its own light sampler, off when the pane is closed.
- Whether the same "tracking" findings appear (`analysis.rs`) or only the numbers.
- Whether it can log to a file (the `NEWQ_STATS` CSV) from the window.

## 3. An audio output chosen for the test

Test Mode should let the user pick which audio output the test plays on, as it now lets them
pick displays.

To settle:
- The test show is built from the user's show, which often has no audio outputs (reports say
  "0 audio output(s)"), so the sound goes to the default device. The choice would replace the
  show's audio outputs while the test runs, the way `plan::use_displays` replaces displays.
- Which devices to list (`audio.rs` device list), and how a Dante or multi-channel device is
  shown. The 5.1 trailer section uses an 8-crosspoint matrix, so a stereo-only device needs
  the downmix, and a device with fewer than 6 channels says so.
- Remember the choice per computer (`Prefs`), like `test_displays`.
- Say which device was used in `system.txt` and the report.

## 4. Run a test muted

A Test Mode option to run the test with no sound out of the speakers.

To settle:
- There is already `NEWQ_MUTE=1` (`media/audio.rs`, where the stream is built): it keeps the audio
  clock running and the mixer working but outputs silence. A muted test should do the same, so
  the decoding, effects and the audio clock still cost what they would (a test that simply
  played nothing would measure less). That is read once, when the stream is built; a per-test
  switch needs the master gain to be something the app can change while it runs (an atomic),
  and to be put back as it was when the test ends.
- Audio findings: with no sound out, the "no working audio output" note and the late-audio
  figures need to say the test was muted on purpose, not that the device was missing.
- Whether it is remembered per computer, and whether it shows in `system.txt` and the report.

## 5. A harder test, for the processor and the graphics card

A way to make the test more intense: more opacity and blending, more effects, and an option to
download larger media.

To settle:
- What the tests do now: the Medium and Long shows already stack a few video layers (up to
  four), one at 0.7 opacity, blur / pixelate / negative / vignette / sharpen on layers, echo /
  EQ / reverb on the sound, and 1080p clips with H.265 and a 25 Mbit clip in Long. Each layer is
  small on screen except the background one, so the graphics card is lightly loaded.
- Ways to push harder: more full-screen layers at partial opacity (blending cost grows with
  covered pixels), effects on every layer and more of them at once, mesh warp and edge blend
  on the output, 4K layers (the decoding comparison already makes a 4K clip), more simultaneous
  decodes, and faster motion/rotation so nothing is cached.
- Larger media to download: the pinned catalog (`testmode/assets.rs`) has nothing above 1080p
  and the biggest clip is about 30 MB. A larger-media option needs new entries with sizes,
  hashes, credits and licences checked (as the others were), and a clear size warning before
  the download. Not yet looked for: free 4K sources with a licence that allows this.
- How it is chosen: a fourth level ("Heavy"), or an "intensity" setting on any level. It should
  say how big the download is and how long a lap is, and the analysis thresholds (late frames,
  stalls) should be judged against what a heavy test is expected to cost.
- A way to find the ceiling: step the load up until frames are late and report where the
  computer started to struggle, instead of pass/fail at one level.
