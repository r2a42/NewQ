# Changelog

What changed in each NewQ release, newest first, in words for the people who run
shows with it. The section for a version is put at the top of that version's
release page, and a release is refused if it has none.

## Unreleased

- **Timeline workspace.** A timeline can open in a workspace of its own: the tracks
  across the whole window, the picture, a bin of new clips and the timeline's files
  and clips, and the inspector with the clip's timeline settings at the top. Choose
  it, or the pane as before, in Settings ▸ General, the Window menu or the tracks'
  toolbar; in the new interface it is the Timeline view. *Back to cue list* puts it
  away.
- **New stills, texts and shapes on a timeline last 5 seconds** wherever they are
  made (the tracks' menus, a picture dropped on a track, the workspace's bin), as
  they already did in a timeline list, instead of holding for ever.
- **Timelines from a remote.** OSC can run, pause, continue, stop (with the Stop
  cue's 3 s fade, or at once), jump to a time or a marker, and leave a loop on a
  timeline, named by its list's name or its group's cue number:
  `/timeline/Act_1/jump/Interval`, `/timeline/5/stop`.
- **The web remote's timeline transport.** Each timeline list (and a timeline group
  while it plays) gets a card with its clock, what it is doing, Run, Pause and Stop,
  Leave loop while it loops, and a button for each marker. The line above GO says
  when GO will do nothing because a timeline list is running or at its end.
- **Timelines in OSC feedback.** A controller is told each timeline's state
  (running, paused, looping, stopping, at its end…) and its clock, and `/newq/go`
  says what GO will do now.
- **Timelines on the cue sheet.** A timeline's clips have their start time, track and
  other displays in columns of their own, and its markers are listed after it.
- **Test Mode's debug test plays timelines:** two tracks, an overlap crossfade, a
  pause GO ends, a camera held by a pause, a jump to a marker, a clip waiting for
  its fade-in, and Stop cues that fade and cut.

## 0.22.0

- **Changelog.** Each release page now starts with what changed in that version.

Timelines:

- **Timeline groups.** A *Start all together* group is now a timeline you can see
  and work on: double-click it (or *Window ▸ Timeline*) to open its tracks, one row
  per display layer and sound output, with the clips as bars. Drag clips to move
  them, drag their ends to trim, add markers, and click the ruler to park the show
  at any moment with its picture on the displays. Double-clicking a timeline group
  in the cue list now opens its tracks instead of firing it (GO and *Fire* still
  start it). See *Timelines* in the README.
- **Timeline cue lists.** *Cue ▸ New timeline* (or *+ ▸ New timeline* by the list
  tabs) makes a cue list that is a timeline, with a tab of its own showing its
  tracks in the cue list's place; right-click a list's tab to make it a timeline or
  a plain list again. Files dropped on it, cues added to it and cues pasted or
  moved into it become clips at its playhead. GO runs it, carries it on at a pause
  and does nothing while it runs or once it is at its end.
- **Editing in the tracks.** Ctrl+K (Cmd+K) or *✂ Split* cuts clips in two at the
  playhead (a sound split in two may click at the join; the status bar says so).
  Delete takes clips off, and Shift+Delete (or Delete with *Ripple* on) closes the
  gap they leave. Ctrl+C, X and V copy, cut and paste clips: they are pasted at the
  playhead on the tracks they came from, as far apart as they were. Ctrl+D puts a
  copy of the picked clips just after them. Pick clips with a rubber band, trim
  several at once, and drag the square at a clip's corner to set its fade-in or
  fade-out.
- **Adding to a timeline:** right-click an empty spot of a track for a new clip
  there, drop files on a track, use the cue track's + for a pause, stop, fade, send
  or wait cue at the playhead, and *+ Track* for an empty track. Right-click a
  track's name to remove an empty one or move a picture track above or below the
  next.
- **Speed and opacity lanes under a clip:** add, drag and remove keyframes where
  the clip is, and see its length follow its speed.
- **Pause cues.** A *Pause timeline* cue stops the timeline's clock until GO. It
  can let what is playing run on (and then the next crossfade, fade or keyframe
  ramp waits for your GO), hold everything on a frame, or hold only the clips you
  tick; cameras and live sound stay live unless you say otherwise. Fades already
  under way when the clock stops wait where they are and finish after GO; a fade
  you start by hand still runs. *Run*, *Stop* and *Jump in timeline* cues go with
  it, and a clip's own Pause and Resume cues still hold just that clip.
- **Play a section again until GO.** A pause cue can loop the timeline back to a
  marker or a time and play that section round until GO; GO lets the pass finish
  and carries on past the cue (or, if you choose, leaves at once). The jump back
  isn't sample-accurate: for a bed of sound that must not break, loop the sound
  clip itself.
- **The timeline Stop cue fades what the timeline is playing out**, over 3 seconds
  unless you set another time (0 cuts); pressing Stop again during the fade cuts at
  once. The editor's Stop button does the same.
- **Overlapping clips crossfade** on a timeline made in this version: a clip that
  comes in while an earlier one on its track is still playing crossfades over it
  for as long as they overlap, and the tracks draw the crossfade. Clips that butt
  up against each other follow on without a gap of black.
- **Moving the playhead, or a Jump cue, changes the picture all at once**: what was
  up stays until everything at the new place is ready.
- **A clip set to Loop goes round again** if a pause outlasts it; *Hold* keeps its
  last frame; *End* (the default) fades it out. The editor marks a clip left to
  end with ⚠.
- **Clips at opacity 0 wait for their fade-in** on a timeline made in this version,
  so a picture that fades up later is seen from its first frame. A clip can opt out
  (*Already running when it fades in*). Older shows play as they did.
- **On a timeline, opacity, position and effect keyframes follow the timeline's
  clock:** they stand still at a pause and go on from the GO. Speed keyframes
  follow the clip.
- **The transport says what GO will do on a timeline:** "Continue …" and what it
  brings in next, "RUNNING · next pause in 0:12.0", "Finish this pass and
  continue …" while a section loops, "… is at its end" (also in the new interface
  and on the web remote).
- **The tracks editor warns** of a clip that nothing fades up (it never starts)
  and of one that cuts in over the clip under it as it fades up; sound clips show
  their waveform.

Several displays:

- **A picture on several displays.** A picture, video, text, shape or camera cue
  has an *Also on* row in its inspector to show it on other displays too; the cue
  list shows "Display 1 +2" and the tracks a "+2" badge, with a ghost of the clip
  on each other display's track. What comes in on one of those displays replaces
  it there only; its sound plays until it has gone from the last of them. Text and
  shapes are drawn at the size of the cue's own display.

Also:

- **Duplicating a group** now aims the Pause, Fade and Stop cues inside it at the
  copied cues, not at the originals.
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
