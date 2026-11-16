# Networked NewQ: display, primary and backup computers

Plan and the owner's rulings (2026-11-16). Later sections win.

## The idea

Every computer runs the whole show itself, from its own copy of the show file and
media. The network carries only small messages (GO, seek, pause, edits) and a shared
clock. No picture is streamed between computers for playback. A display computer keeps
playing when the control computer goes away.

Roles, one per computer: **display** (no control window, full screen on every output
from start-up), **primary control**, **backup control**.

## Rulings

- R1. The goal is **frame accurate** between computers. The owner's show LAN pings at
  about 2 ms. Built for a purpose-built show network, not an IT-managed one: latency
  matters most.
- R2. Wired is strongly recommended, never required (a battery prop on Wi-Fi is a real
  use). NewQ says so where a computer is on Wi-Fi and shares a blended picture.
- R3. The primary assigns roles **from the primary**. A display computer may be out of
  reach (in a truss): nothing may need typing on it after it is installed and set to
  start at login.
- R4. All media goes to every computer. Media is **versioned**; a computer short of
  disk, or an exceptionally large file, can be set to **replace** instead of keeping
  versions.
- R5. The user picks which computer or computers play sound.
- R6. Video inputs: either streamed from one computer (low budget; NDI and the server
  streams exist) or a capture card in each display computer (large budget).
- R7. Backup takeover is manual first (agreed). Automatic may follow.
- R8. Send cues, scripts, MIDI/OSC/timecode input and recordings run on the primary
  only, so nothing fires twice.
- R9. Today the sync trial is simulated; latency is improved as real hardware is tried.

## What "frame accurate" can mean (to tell the owner plainly)

- The same content frame is chosen on every computer for the same shared-clock
  instant. This is what software can promise.
- Each display still refreshes on its own schedule: without genlock hardware two
  outputs can be up to one refresh apart (16.7 ms at 60 Hz). Watchout has the same
  limit without sync cards.
- Sound on two computers is never sample-locked without a shared audio clock (Dante):
  two computers playing the same sound into one room will comb or echo.

## What helps in the code as it is

- `video.rs` shows the frame whose `pts <= t` (the cue's time), so the frame is chosen
  by a clock, not by when FFmpeg happened to start. Sync reduces to every computer
  agreeing on the cue's time.
- `Engine::tick(show, dt)` adds up `dt`; a timeline `Run` has a clock `pos`, ramps
  follow it, and `chase.rs` works out what plays at a time (a late joiner).
- Preload (`update_preload`, `load_ahead`) gets a start ready ahead.
- `testmode/assets.rs` has hashes and `.part` resume; `web.rs` is HTTP on std sockets;
  `relink.rs` copies media; `Show::assign_displays`/`DisplaySlot` per display.

## Stages

1. **Sync trial (simulated).** `src/net/`: clock (offset and drift estimate from
   timestamped exchanges, pure), messages, a simulated network (latency, jitter,
   asymmetry, loss, clock offset and drift per node), and timestamped starts. Measure:
   clock error, and the difference in the frame two engines show at the same true
   instant, at start and across an hour, for a 2 ms wired LAN and for Wi-Fi. Report
   what in the engine stops frame accuracy and the change that removes it.
2. **Computers and roles.** Identity, discovery, adoption from the primary (R3), role
   kept on the computer, each display and sound output owned by a computer, a status
   panel.
3. **Media store.** Content-addressed files, a manifest per show version, pull with
   resume and verification, versions or replace (R4), progress, per-cue readiness.
4. **Display computers.** Start at login into full screen, last synced show, catch up
   after a restart.
5. **Backup control.** Mirrors edits and state; takes over on the operator's press.
6. **Network test in Test Mode**, README, docs.

## Open points

- Adoption without touching the computer means an unadopted NewQ accepts the first
  primary that claims it. Acceptable on a show LAN; after adoption it obeys only that
  network's key, and the primary can release it.
- A fresh computer still needs NewQ installed and set to start at login once, by hand.
