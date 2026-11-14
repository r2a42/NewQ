# Changelog

What changed in each NewQ release, newest first, in words for the people who run
shows with it. The section for a version is put at the top of that version's
release page, and a release is refused if it has none.

## Unreleased

- **Changelog.** Each release page now starts with what changed in that version.
- **Timelines.** A *Start all together* group is now a timeline you can see and
  work on: double-click it (or *Window ▸ Timeline*) to open its tracks, one row per
  display layer and sound output, with the clips as bars. Drag clips to move them,
  drag their ends to trim, add markers, and click the ruler to park the show at
  any moment with its picture on the displays. See *Timelines* in the README.
- **Pause cues on a timeline.** A *Pause timeline* cue stops the timeline's clock
  until GO. It can let what is playing run on (and then the next crossfade, fade
  or keyframe ramp waits for your GO), or hold everything on a frame, or hold only
  the clips you tick; cameras and live sound stay live unless you say otherwise.
  *Run*, *Stop* and *Jump in timeline* cues go with it, and a clip's own Pause and
  Resume cues still hold just that clip. Fades already under way when the clock
  stops (a fade cue, a crossfade, a clip's fade-in or fade-out, matrix levels) wait
  where they are and finish after GO; a fade you start by hand still runs.
- **The timeline Stop cue fades what the timeline is playing out**, over 3 seconds
  unless you set another time (0 cuts); pressing Stop again during the fade cuts at
  once. The editor's Stop button does the same.
- **The transport says what GO will do on a timeline that waits:** "Continue …" and
  what it brings in next, instead of the next cue in the list (also in the new
  interface and on the web remote).
- **A clip that runs out during a pause** fades out by default; choose Loop or Hold
  for it in its inspector. The editor marks such clips with ⚠.
- **Clips at opacity 0 wait for their fade-in** on a timeline made in this version,
  so a picture that fades up later is seen from its first frame. A clip can opt out
  (*Already running when it fades in*). Older shows play as they did.
- **On a timeline, opacity, position and effect keyframes follow the timeline's
  clock:** they stand still at a pause and go on from the GO. Speed keyframes
  follow the clip.
- **Duplicating a group** now aims the Pause, Fade and Stop cues inside it at the
  copied cues, not at the originals.
- **Double-clicking a timeline group** in the cue list opens its tracks instead of
  firing it (GO and *Fire* still start it). Other cues still fire on a
  double-click.
- **Not yet:** timeline cue lists (a tab of their own), automatic crossfades where
  clips overlap, adding clips, splitting and tracks from the editor, and one clip on
  several displays.
- **A show that uses the new timeline cues (Run, Pause, Stop or Jump timeline)
  will not open in NewQ 0.20.3 or older.**

## 0.20.3

- **Nothing is left running behind NewQ (Mac and Linux).** If NewQ crashes or is
  force-quit, a small watcher ends the FFmpegs and streaming server it started. If
  the watcher goes at the same moment, the next NewQ you start ends what was left.
  Only programs NewQ itself started are ever ended.

## 0.20.2

- **Screen capture and capture devices no longer freeze on their first frame
  (Mac).** A camera cue showing the screen or an HDMI-to-USB device stood still;
  a webcam was fine. Fixed.

## 0.20.1

- **Recording to `.mkv` works on a Mac.** The bundled Mac FFmpeg could not write
  the default recording format.
- **The control window keeps drawing when an external display is switched off.**
  It could stop redrawing until the display came back.
- **Test Mode:** the full check is now the *Debug test*, chosen beside Short,
  Medium and Long. It plays a cut of a real film and pictures. Every test leaves
  one zip and nothing else.

## 0.20.0

- **A fault in one part no longer ends the show.** If a part of NewQ meets an
  error, that part is skipped for a moment, the show goes on, and the status bar
  says what happened. An output window whose drawing stops is restarted; a sound
  that fails is dropped without stopping the others.
- **Log detail is a setting** (Settings ▸ General ▸ Recovery and logs), from
  *Errors only* to *Everything*, applied at once. The log is kept to 20 MB.
- **Test Mode's full check:** one button plays every kind of cue and checks that
  each did what it should, then makes one zip to send. `newq --self-test` runs
  the same check with no window.

## 0.19.1

- The MediaMTX streaming server in the downloads now comes from a pinned copy
  kept with NewQ, checked by checksum.

## 0.19.0

- **NewQ can run the streaming server itself** (MediaMTX), with a link and QR
  code for players, a choice of network adapter and a server test.
- **Record a display's picture to a file**, with sound.
- **Blackmagic output** for a display's Send out (needs an FFmpeg with DeckLink).
- **Select, drag, copy and group several cues**, and edit several at once.
- **Layers:** cues on different layers of one display no longer replace each
  other (new shows; older shows play as they did).
- **Shape cues**, and an optional new interface with Editor, Operator and Patch
  views.
- **Group crossfades overlap** and the group keeps its length.

## 0.18.0

- **Send a display's picture to a streaming server** over RTSP, RTMP or SRT.
- **Capture cards:** ask a camera cue's device for a format, and see the modes it
  offers.
- **Blackmagic UltraStudio and DeckLink inputs** (needs an FFmpeg with DeckLink).

Earlier versions: see the commit history.
