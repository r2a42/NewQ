# Notes for later

The seven ideas saved on 2026-10-31 were built in 0.15.0. What is left from them:

- **4K media for the heavy test.** Not added: the free 4K sources are big (Big Buck Bunny 4K is a
  630 MB zip, Sintel 4K does not have a stable small download). The decoding comparison already
  makes a 4K clip from the 1080p film; a heavy test could do the same.
- **Find the ceiling.** A mode that steps the load up until frames are late and reports where the
  computer struggled. Only the fixed "heavier load" exists.
- **The Mac stall.** 0.14.4 on a Mac with a 4K second display: three main-loop stalls of 4 to 5.5 s,
  each at lap offset ~49 s (the start of the Layers section), in 3 of 23 laps. 0.15.0 logs which part
  of the loop took the time (`event loop stalled for N ms: …` in `newq.log`) and `starting cue … took N ms`.
  Read those lines from the next Mac run before guessing.
- **The Mac menu bar.** Output windows now use `set_borderless_game(true)`, so simple fullscreen
  hides the menu bar and Dock instead of auto-hiding them. Not tried on a Mac. If the menu bar still
  appears, look at the app being activated again (the control window taking focus).
- **Windows: 5.4 s freeze when Start is pressed** (system report and FFmpeg probes on the main thread).

## Mac notes from 0.15.0 testing (2026-11-01, not started)

1. On opening the program the Settings tab is in focus instead of the cue list. The cue list should be the default.
2. The top menu bar should be hidden on the fullscreen output display but not on the display with the control window. (0.15.0's `set_borderless_game` hides it app-wide, which is the opposite of the second half. Presentation options are per application, so a per-display answer may need a different approach: e.g. a borderless window above the menu bar level on the output display, with the normal presentation options left alone.)
3. A faint 1 pixel white/grey border on the fullscreen output display, always there in Test Mode. Not looked at. Candidates: the simple-fullscreen window frame/shadow, a window border or `has_shadow`, the surface being 1 px smaller than the screen, or a layer in the test picture itself (check with a test pattern).
