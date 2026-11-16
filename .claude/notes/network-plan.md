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

## Rulings, 2 (2026-11-16)

- R10. Sound on several computers is for separate rooms, a booth monitor, or local
  sound on the primary while editing. Make it flexible: any computer can be given any
  of the show's audio outputs, the primary included. No warning needed for the normal
  cases; no attempt at locking sound between computers.
- R11. Ease of use over security. Adoption needs no code by default. An optional
  passcode exists only for a network that carries more than one show. Most networks
  have one show and no internet. "This is for art": nothing an artist has to configure
  to make the ordinary case work.
- R12. Build agents are Opus where possible.

## Design for stages 2, 4 and 5 (2026-11-16; the owner said build all day to a release to test)

### D1. Turning it on
A NewQ that is not part of a show network opens no ports (no firewall prompt for an
artist who never uses this). One switch, Settings ▸ Show network: "This computer is
part of a show network". On: it listens and announces itself. Everything after that is
done from the primary (R3). The same page on the computer that will run the show has
"Run the show network from this computer" (makes it primary). `newq --join` on the
command line does the same as the switch, for setting up many machines.

### D2. Identity
`node.json` beside the log (per computer, never in the show): id (random, made once),
name (the host name until renamed, renamable from the primary), role (`Alone`,
`Display`, `Primary`, `Backup`), the network it belongs to (the primary's network id)
and the optional passcode's hash (R11), `term` (see D7).

### D3. Finding each other
UDP broadcast beacon once a second on every adapter (`if-addrs`), fixed port, plus
"add by address" for networks that drop broadcasts. Beacon: id, name, role, NewQ
version, protocol version, network id, TCP port, show version, term. No mDNS dependency.

### D4. Channels
TCP (nodelay) per peer for everything ordered: adoption, roles, show sync, commands,
state. UDP for the clock exchange only. Time-critical commands carry a shared-clock time
a lead ahead (from the sync trial), so TCP's delay is inside the lead.

### D5. What the show says
`DisplaySlot` and each audio output gain `computer` (a node id; absent = the primary,
unwritten, so old shows are unchanged). An audio output can name several computers
(R10). Monitors: each computer reports its monitors; the primary's Displays page assigns
Display N to computer X's monitor Y, exactly as it assigns local monitors now.

### D6. Who does what
Every computer runs the whole engine on the same show. A computer draws only the
displays it owns and opens only the audio outputs given to it. The primary alone sends
(send cues, scripts, projector control), listens (MIDI, OSC, timecode, web remote,
clock triggers) and records (R8); what those cause goes to the others as commands.
Replicated: operator inputs as commands at a shared time (GO, fire/stop/fade a cue,
Stop All, pause/resume, each timeline verb, seek, playhead and list changes), and the
show itself (whole JSON on each burst of edits, with a version number and hash; the
engine already copies live fields into playing cues). Authority: the primary's state
heartbeat (active cues with their shared start times, each run's clock anchor,
playheads). A follower that differs corrects itself: starts what it lacks at the right
place (timelines: `chase.rs`), ends what the primary doesn't have. The same mechanism
is "catch up after a restart".

### D7. Primary, backup, takeover
`term` is a counter. The primary's messages carry it. Take control on the backup raises
the term and announces it; every computer obeys the highest term it has seen, and an
old primary that comes back and sees a higher term becomes the backup (never two
primaries obeyed). Manual only (R7). The backup's window follows the primary (show,
playheads, what is playing) with its controls locked but for Take control.

### D8. Display computers
Role `Display`: no control window; every monitor is an output from start-up; before the
show is in step each shows a quiet card (name, address, "waiting for the primary",
media progress), black otherwise; "Identify" from the primary flashes the name. Start
at login is written by NewQ itself (Mac LaunchAgent, Windows Run key, Linux autostart
entry), so the primary can turn it on remotely once the computer has joined. Keep-awake
and crash survival exist already.

### D9. The primary's page (Settings ▸ Show network)
A row per computer: name (rename), role (menu), online, clock error, show in step,
media (progress, disk free, versions or replace), wired or Wi-Fi where it can be told,
NewQ version (a mismatch is said plainly), Identify, Release. Wi-Fi on a computer that
shares a blended picture is flagged (R2).

### D10. Order of work
2a identity, beacon, TCP session, adoption, roles, the page (no playback yet).
2b show sync + media store wired in (pull from the primary, readiness on cues).
4a the engine on the shared clock; commands and the heartbeat; display role window.
5  backup follow and take control.
6  Test Mode network test (several NewQs on one machine), README, CHANGELOG.
The release to test is a pre-release (`-rc1`).

## Rulings, 3 (2026-11-16): transfers never block rehearsal

- R13. A transfer never blocks rehearsal. Every computer plays what it has. Where a
  cue's file isn't on a computer yet its display shows a card in the cue's place:
  "Media transferring" (with the name and progress) when it is on its way, "Media not
  found" when no computer has it (missing on the primary too). When the file lands
  while the cue is still up, the cue picks it up at the right place.
- R14. Order: files nearest the playhead first (the active list's playhead and each
  running timeline's clock, forward first). The order follows the playhead as the user
  jumps around, without thrash: a transfer in flight is put aside (its `.part` kept)
  only for something now clearly nearer, never when it is nearly done, and the order
  is re-made at most about once a second.
- R15. After every file the show uses: the files in the media manager that are not in
  the show yet (the manifest's "library" entries: the show's media folder and any
  folder the user added to the manager).
- R16. The primary shows a warning while transfers run: playback may be less smooth
  until they finish. It is a notice, not a gate.
- (The owner: this is the worst part of Watchout. Get it right.)

## Rulings, 4 (2026-11-16)

- R17 (replaces R14's put-aside rule). The order is re-made when a file completes, or
  about once a second when many small files are completing. Transfers in flight carry
  on. One exception (ours, told to the owner, who may strike it): when the nearest
  file is not in flight, every slot is busy and none will free within about 30 s, the
  lowest-ranked transfer in flight is put aside (its `.part` kept) so the nearest
  starts now.
- R18. An accurate percentage and time estimate, by bytes, for each computer and for
  all of them together (on the primary's page and in its notice), and per file on the
  "Media transferring" card.

## Rulings, 5 (2026-11-16)

- R19. The media manager's library is a first-class use: at load-in a designer adds a
  huge folder of content they might use; it goes to every computer in the background
  so that using a file later needs no transfer.
- R20. **Never play a partial file.** A computer plays a file only when it is complete
  and verified. Until then the card is shown; the cue takes the file up only once it
  is whole. (This also means: no "start playing while it downloads".)
