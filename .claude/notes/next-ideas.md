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
