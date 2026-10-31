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
