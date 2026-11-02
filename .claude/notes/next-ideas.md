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

## Plan agreed after 0.16.0 (one at a time)

1. Slide shows that advance on GO, and the show-mode arrow keys (Right/Page Down forward, Left/Page Up back;
   a deck steps, otherwise Back pauses and again fades out): built on the session branch.
2. PDF import with no external program (today `pdftoppm` isn't shipped, so PDF import fails on a stock
   machine). Needs a PDF renderer crate or PDFium bundled: check licences (not AGPL) and quality first.
3. The presenter remote working with NewQ in the background: a PerfectCue is a plain USB keyboard (defaults
   Right, Left, B; `docs` from DSan in the session scratchpad), and a global hook can't tell which keyboard a key
   came from, so it needs Raw Input (Windows) / IOHIDManager (Mac). Needs a `newq --list-keyboards` run on the
   real receiver (PC-SYS-AS4) to see its name and IDs. Don't start the hook at launch (a Mac asks for Input
   Monitoring); `clicker::AVAILABLE` is the switch. DSan also sells a serial "COM Connect" path (KES-485-PC cable),
   data format not documented.
4. At startup, warn when the app (or its FFmpeg) runs from an external drive, and copy FFmpeg to the internal
   cache: a run from a JetDrive stalled 6-7 s, the same test from the internal drive had a 63 ms worst stall.
5. Windows signing: wants a cloud signing service (Azure Trusted Signing or similar); the `.pfx` step in
   `release.yml` can't work with new certificates (hardware-backed).
6. The release notes template still says the Mac app "isn't notarised yet".
