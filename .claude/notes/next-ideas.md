# Notes for later

Built after 0.15.0 (on the session branch, not released): cue list opens in front; Mac output windows
cover their display above the menu bar (`macwin.rs`); FFmpeg is started by the reader threads;
the children of a following group are preloaded. All of it is untested on a Mac.

Still open:
- **Check on a Mac after the next release:** menu bar absent on the output display only and not coming back
  when Finder is opened from a browser; the faint 1 px border (a shadow was the guess: `setHasShadow(false)`);
  a 3-finger swipe up (Mission Control) not moving the output; the stalls gone from `newq.log`
  (`event loop stalled … the show engine`) in the heavy Medium test. If the stalls remain, `starting cue … took`
  names the cue.
- **4K media for the heavy test.** Not added: free 4K sources are big. The decoding comparison already makes a 4K
  clip from the 1080p film; a heavy test could do the same with no download.
- **Find the ceiling.** A mode that steps the load up until frames are late and says where.
- **Windows: 5.4 s freeze when Start is pressed** (system report and FFmpeg probes on the main thread).
- **Log noise:** "Referenced QT chapter track not found" for the 5.1 trailer, a warning every lap.
