# Network sync trial (stage 1), simulated

Everything here was **simulated**: no second computer, no real network, no real
display or sound device. The numbers say what the software would do given the
network and clocks described; they say nothing about real hardware. Made by
`src/net/trial.rs` (`cargo test net::trial -- --ignored --nocapture`, the
`the_hour_long_report` test; `NEWQ_TRIAL_OUT=<file>` writes it), deterministic
(seeded), about 8 s of processor time for all of it.

## The set-up

- Three computers, each a whole `Engine::for_tests()` with the same show: a
  primary, whose own monotonic clock *is* the shared clock, and two followers.
  Clocks: the followers run +25 and -25 ppm (50 ppm apart, 25 from the primary)
  and read 4.2 s behind and 7.9 s ahead of it.
- Each engine ticks at 60 frames a second **by its own clock**, at its own phase,
  each tick read up to 1 ms late (the main loop isn't punctual).
- The followers keep a shared clock (`net::clock::SharedClock`) from NTP-style
  exchanges with the primary, sent as real `net::msg` bytes through the simulated
  network: every 0.1 s until locked, every 0.5 s after.
- Commands are timed by the shared clock ("do X at shared time T", T = now +
  the lead) and sent to each follower again every 25 ms until acknowledged.
- The show, once both followers are locked: at T0 a cue is fired and a timeline
  group is run; at a third of the hour the timeline is sought to 100 s; at two
  thirds it is paused (freezing everything), and GO'd again 30 s later. One
  simulated hour after T0.
- The cue is a Wait cue standing in for a video: a video's frame is the last whose
  `pts <= t` of the cue's time `t` (`video.rs::VideoPlayer::update`), so the frame
  index is `floor(t * fps)` and is worked out at 30 and 60 fps from the cue's time.
  Two tests with real media check that a video cue's and a timeline clip's shown
  frames do follow those times on two engines (`a_warm_and_a_cold_engine_…`,
  `a_timeline_clip_on_two_engines_…`, `test_media/testsrc_10s.mp4`).
- Frames are compared every 37.1 ms of true time, two ways:
  - **clock at the instant**: the frame each computer's clock says at that true
    instant (what software can promise);
  - **as shown**: the frame chosen at each computer's last frame tick before the
    instant (60 Hz each, unaligned: what an unaligned refresh shows).
  "Differ" is the share of instants at which not all three had the same frame;
  "worst" the largest spread across the three. Samples taken just while some had
  made a seek and others not yet (over 0.5 s apart) are counted apart ("either
  side of a jump"); there were none with the shared clock.
- Two ways of playing are compared: **Synced** (the change below: commands at a
  shared time, engines ticked by the shared clock) and **As is** (each computer
  does a command when it arrives and ticks by its own clock, which is what
  NewQ would do today if a remote's GO were sent to several computers).
- Networks (`net::sim::Network`), each one-way delay drawn per packet:
  - Wired show LAN: 1 ms each way, triangular ±0.3 ms, 25 µs slower one way.
  - Poorer wired: 5 ms, ±2 ms, 0.25 ms of asymmetry, 0.2 % held up 20 ms, 0.1 % lost.
  - Wi-Fi: a 2 ms floor plus an exponential tail of mean 6 ms (about 8 ms, most
    within ±15 ms), 1 % held up ~100 ms, 2 % lost, 1 ms slower up than down.
  A steady asymmetry can't be seen by any clock exchange: it leaves an error of
  half of it (12.5 µs, 125 µs and 0.5 ms here), and that is most of the clock
  error below on the poorer networks.

## Results (one simulated hour per network and mode)

### Wired show LAN (1 ms, ±0.3 ms)

Clock (followers, after lock, 194072 samples): median 0.024 ms, p99 0.061 ms, worst 0.255 ms; largest error bound believed 1.255 ms; locked after 0.72 and 0.76 s.

Command delivery to both followers (200 000 simulated, resent every 25.000 ms): median 1.046 ms, p99 1.245 ms, p99.99 1.272 ms, worst 1.274 ms. Lead used: 10.000 ms; least to spare in the trial: 8.790 ms.

| Mode | What | Samples | Differ 30 fps | Differ 60 fps | Worst (frames 30/60) | Worst (ms) | Either side of a jump |
|---|---|---|---|---|---|---|---|
| Synced | Cue, start (first 2 s) | 53 | 0.00 % | 0.00 % | 0 / 0 | 0.253 | 0 |
| Synced | Cue, clock at the instant | 97035 | 0.05 % | 0.10 % | 1 / 1 | 0.409 | 0 |
| Synced | Cue, as shown (last frame tick) | 97035 | 22.55 % | 45.01 % | 1 / 1 | 17.470 | 0 |
| Synced | Timeline, clock at the instant | 97035 | 0.05 % | 0.10 % | 1 / 1 | 0.409 | 0 |
| Synced | Timeline, as shown | 97035 | 22.27 % | 44.45 % | 1 / 1 | 17.470 | 0 |
| Synced | Timeline, 2 s after the seek | 54 | 0.00 % | 0.00 % | 0 / 0 | 0.035 | 0 |
| Synced | Timeline, 2 s after GO from the pause | 54 | 0.00 % | 0.00 % | 0 / 0 | 0.022 | 0 |
| Synced | Timeline paused: spread of the clocks | | | | | 0.000 | |
| As is | Cue, start (first 2 s) | 53 | 11.32 % | 22.64 % | 1 / 1 | 3.600 | 0 |
| As is | Cue, clock at the instant | 97035 | 92.58 % | 97.11 % | 6 / 11 | 183.501 | 0 |
| As is | Cue, as shown (last frame tick) | 97035 | 93.81 % | 98.26 % | 6 / 12 | 200.214 | 0 |
| As is | Timeline, clock at the instant | 97035 | 81.64 % | 90.90 % | 4 / 8 | 117.298 | 0 |
| As is | Timeline, as shown | 97035 | 84.83 % | 95.31 % | 4 / 8 | 134.010 | 0 |
| As is | Timeline, 2 s after the seek | 54 | 9.26 % | 18.52 % | 1 / 1 | 3.217 | 0 |
| As is | Timeline, 2 s after GO from the pause | 54 | 100.00 % | 100.00 % | 2 / 4 | 58.897 | 0 |
| As is | Timeline paused: spread of the clocks | | | | | 66.652 | |

### Poorer wired (5 ms, ±2 ms)

Clock (followers, after lock, 194074 samples): median 0.250 ms, p99 0.749 ms, worst 1.026 ms; largest error bound believed 5.205 ms; locked after 0.72 and 0.97 s.

Command delivery to both followers (200 000 simulated, resent every 25.000 ms): median 5.225 ms, p99 6.621 ms, p99.99 34.625 ms, worst 42.967 ms. Lead used: 70.000 ms; least to spare in the trial: 64.185 ms.

| Mode | What | Samples | Differ 30 fps | Differ 60 fps | Worst (frames 30/60) | Worst (ms) | Either side of a jump |
|---|---|---|---|---|---|---|---|
| Synced | Cue, start (first 2 s) | 53 | 1.89 % | 5.66 % | 1 / 1 | 0.951 | 0 |
| Synced | Cue, clock at the instant | 97034 | 1.16 % | 2.34 % | 1 / 1 | 1.026 | 0 |
| Synced | Cue, as shown (last frame tick) | 97034 | 24.97 % | 49.87 % | 1 / 2 | 17.635 | 0 |
| Synced | Timeline, clock at the instant | 97034 | 1.14 % | 2.32 % | 1 / 1 | 1.026 | 0 |
| Synced | Timeline, as shown | 97034 | 24.74 % | 49.42 % | 1 / 2 | 17.635 | 0 |
| Synced | Timeline, 2 s after the seek | 54 | 1.85 % | 1.85 % | 1 / 1 | 0.480 | 0 |
| Synced | Timeline, 2 s after GO from the pause | 54 | 1.85 % | 3.70 % | 1 / 1 | 0.584 | 0 |
| Synced | Timeline paused: spread of the clocks | | | | | 0.000 | |
| As is | Cue, start (first 2 s) | 53 | 9.43 % | 18.87 % | 1 / 1 | 3.240 | 0 |
| As is | Cue, clock at the instant | 97036 | 92.40 % | 96.95 % | 6 / 11 | 183.143 | 0 |
| As is | Cue, as shown (last frame tick) | 97036 | 91.72 % | 97.58 % | 6 / 12 | 199.821 | 0 |
| As is | Timeline, clock at the instant | 97036 | 81.76 % | 91.01 % | 4 / 7 | 116.646 | 0 |
| As is | Timeline, as shown | 97036 | 85.78 % | 95.83 % | 4 / 8 | 133.323 | 0 |
| As is | Timeline, 2 s after the seek | 54 | 7.41 % | 14.81 % | 1 / 1 | 2.636 | 0 |
| As is | Timeline, 2 s after GO from the pause | 54 | 100.00 % | 100.00 % | 2 / 4 | 58.247 | 0 |
| As is | Timeline paused: spread of the clocks | | | | | 67.031 | |

### Wi-Fi (8 ms, ±15 ms, 100 ms spikes, 2 % loss)

Clock (followers, after lock, 194088 samples): median 0.488 ms, p99 1.090 ms, worst 2.086 ms; largest error bound believed 4.376 ms; locked after 0.81 and 4.05 s.

Command delivery to both followers (200 000 simulated, resent every 25.000 ms): median 9.332 ms, p99 108.522 ms, p99.99 167.407 ms, worst 191.082 ms. Lead used: 300.000 ms; least to spare in the trial: 272.950 ms.

| Mode | What | Samples | Differ 30 fps | Differ 60 fps | Worst (frames 30/60) | Worst (ms) | Either side of a jump |
|---|---|---|---|---|---|---|---|
| Synced | Cue, start (first 2 s) | 53 | 5.66 % | 9.43 % | 1 / 1 | 1.552 | 0 |
| Synced | Cue, clock at the instant | 97035 | 1.79 % | 3.58 % | 1 / 1 | 2.086 | 0 |
| Synced | Cue, as shown (last frame tick) | 97035 | 34.01 % | 68.02 % | 1 / 2 | 17.880 | 0 |
| Synced | Timeline, clock at the instant | 97035 | 1.77 % | 3.55 % | 1 / 1 | 2.086 | 0 |
| Synced | Timeline, as shown | 97035 | 33.72 % | 67.46 % | 1 / 2 | 17.880 | 0 |
| Synced | Timeline, 2 s after the seek | 54 | 1.85 % | 3.70 % | 1 / 1 | 0.710 | 0 |
| Synced | Timeline, 2 s after GO from the pause | 54 | 1.85 % | 5.56 % | 1 / 1 | 0.965 | 0 |
| Synced | Timeline paused: spread of the clocks | | | | | 0.000 | |
| As is | Cue, start (first 2 s) | 53 | 47.17 % | 96.23 % | 1 / 1 | 15.777 | 0 |
| As is | Cue, clock at the instant | 97043 | 87.96 % | 96.65 % | 6 / 11 | 167.536 | 0 |
| As is | Cue, as shown (last frame tick) | 97043 | 94.63 % | 98.80 % | 6 / 11 | 183.722 | 0 |
| As is | Timeline, clock at the instant | 97043 | 82.26 % | 91.44 % | 5 / 9 | 133.845 | 0 |
| As is | Timeline, as shown | 97043 | 89.67 % | 97.02 % | 5 / 9 | 150.031 | 0 |
| As is | Timeline, 2 s after the seek | 54 | 9.26 % | 18.52 % | 1 / 1 | 2.839 | 0 |
| As is | Timeline, 2 s after GO from the pause | 54 | 100.00 % | 100.00 % | 3 / 5 | 75.444 | 0 |
| As is | Timeline paused: spread of the clocks | | | | | 66.772 | |

Reading the tables:

- **Clock**: locked within 1 s on wired networks, 1 to 4 s on Wi-Fi. After lock
  the shared clock is within 0.06 ms (p99) on the show LAN, 0.75 ms on the poorer
  wired one and 1.1 ms on Wi-Fi, never worse than 2.1 ms. The bound each
  follower believes (half the best round trip plus what it can't rule out) is
  well above the real error, as a bound should be. The drift (50 ppm) is fitted
  away: without it the clocks would part by 180 ms an hour.
- **Synced, clock at the instant**: the three computers chose the same frame
  except within about the clock error of a frame boundary: 0.1 % of instants at
  60 fps on the show LAN, 2 to 4 % on the poorer networks, and **never more than
  one frame apart**, at the start, across the hour, after the seek and after GO
  from the pause. A paused timeline stood at exactly the same place on all three.
- **Synced, as shown**: with each computer's 60 Hz frames unaligned, what is on
  screen differs by up to one refresh (17.5 ms, one frame at 60 fps) about half
  the time at 60 fps. That is the refresh, not the software: no clock can remove
  it without the displays being locked together (genlock/framelock).
- **As is**: the start is a few ms apart on wired networks (arrival time plus
  each computer's next frame) and 16 ms on Wi-Fi, then the clocks' 50 ppm takes
  them apart: 6 frames at 30 fps (11 at 60) and 180 ms by the end of the hour.
  The paused timeline stood 67 ms apart on the three, and GO after it was 2 to 5
  frames apart.
- **Lead time**: a command must arrive before its time. With resending every
  25 ms the worst of 200 000 simulated deliveries to both followers was 1.3 ms
  (show LAN), 43 ms (poorer wired; a 20 ms hold plus a lost packet) and 191 ms
  (Wi-Fi; a 100 ms spike plus losses). Add the clock error (under 2 ms). In the
  trial the leads used were 10, 70 and 300 ms and every command arrived with time
  to spare. The real floor is not the network but getting the media ready (below).

## Where time comes from in NewQ (as found)

- `app.rs::pass_show` calls `Engine::tick(show, dt)` once per main-loop pass with
  `dt` from `Instant` (the computer's own monotonic clock). Every cue's
  `ActiveCue::elapsed` is a **sum of those dts** from the tick at which it
  became `started` (`engine.rs::tick`, the `retain_mut` over `active`).
- A video's frame is chosen by time, not by when frames arrive:
  `video.rs::VideoPlayer::update(t)` takes queued frames while `pts <= t`; the
  frames' `pts` are counted from where the decoders began (`seek_base`). So if
  the cue's time agrees, the frame agrees; a late decoder catches up by
  skipping, as far as its decoding is faster than real time.
- But `started` waits for the first frame (`video_start`; with sound, also for
  the sound, up to 3 s, then it goes without it), and a cue whose file hasn't
  been read waits for ffprobe (`PendingCue::reading`). Its time then **starts
  from 0 when it is ready**, not when GO was pressed: on this (cloud, simulated
  -device) machine a cold video's first frame came 160 to 260 ms after GO
  (ffprobe and FFmpeg start), a preloaded one's within 1 ms. That difference
  would be the offset between two computers for the whole cue.
- A video **with sound** takes its time from the sound: `elapsed =
  max(elapsed, seek_base + voice.clock())` and the picture is placed by
  `voice.clock_with_lead()` (`audio.rs::Voice::clock`, frames the device has
  consumed). That is the sound card's crystal (typically tens of ppm off), plus
  its buffer. Two computers would drift apart by it and no network clock steers it.
- A sound-only cue's time is the device's count too (`elapsed = seek_base +
  vo.clock()`).
- A timeline's `Run::pos` moves by `dt` in `timeline_step`; its clips start when
  `pos` crosses their time (`PendingCue::at`), within a tick, and then sum their
  own dts, so a clip's content is up to a tick off its clock. A run that is
  `Cueing` (after a seek, or a start with clips not ready) holds its clock until
  every clip has its first frame or `CUE_LIMIT` passes, by the wall clock: a slow
  computer would start later, and stay later.
- Pre-waits count down by `dt` and fire at the tick that crosses zero (up to one
  tick late, different on each computer).

## What limits frame accuracy as it is

1. Each computer's own clock (`Instant`) times everything: 50 ppm is 180 ms an hour.
2. A command acts when it arrives, at the next frame tick: network delay plus up
   to a frame.
3. Times start when the media is ready (`started`), not at GO: FFmpeg's start time
   is in the cue's time for good unless it was preloaded.
4. Pictures with sound follow the sound device's clock.
5. Timeline clocks wait for cueing by the wall clock; clips and pre-waits start on
   tick boundaries.
6. Displays refresh on their own schedule (up to one refresh apart even when
   everything above is fixed).

## What was changed (all off unless `Engine::tick_shared` is called; the app never calls it)

- `Engine::tick_shared(show, now)`: ticks by a shared time. `dt` is how far the
  shared clock moved (never back), and:
  - a cue started off a timeline gets `ActiveCue::anchor`, the shared time its
    own time was 0: fired between ticks, the earlier tick's time; started by a
    pre-wait, the moment it was due inside the tick; started after its file was
    read, the time it was fired (`Reading::shared`). Its time is then `now -
    anchor` at every tick (`ActiveCue::pinned`), so a late decoder catches up and
    a picture with sound follows the shared clock, not the device. A pause (the
    show's, or a control cue's hold) moves the anchor on; a scrub re-anchors it.
  - a running (or cueing) timeline gets `Run::anchor`: its clock is `now -
    anchor` (`timeline_step`), so cueing no longer makes it late: it catches up
    when its clips are ready. A seek anchors it at the seek's shared time; a
    jump made by the clock (a Jump cue, a loop going back) at the shared time the
    clock reached it (`Runs::stepping_at`); a pause drops the anchor and GO sets
    a new one (`Runs::anchor_runs`).
  - a timeline clip on an anchored run keeps its clock's time plus the offset it
    had when it was started or jumped exactly (`ActiveCue::clock_offset`; for a
    clip that waited for its file, where the clock was when it was due).
  - To start something at shared time T, tick to T and do it then (the trial's
    nodes do a tick at T, then the command, then the frame's own tick).
- `src/net/`: `clock.rs` (the estimator: weighted line through the offsets,
  weights by round trip above the shortest, drift fitted over 10 s or more,
  slews at most 500 ppm once locked, steps only forward before, never gives a
  lower time, takes three samples in a row that disagree beyond their round
  trip and the bound as a step of the primary's clock), `msg.rs` (clock ask and
  reply, timed commands, ack, heartbeat; 16 to 32 bytes for the clock
  exchange), `sim.rs` and `trial.rs` (tests only). A real-socket test runs the
  exchange over UDP on loopback with 1.5 ms each way and a 1234.567 s offset;
  it locks and is within 2 ms (on this machine well under).
- No show fields were added; the existing 1206 tests pass unchanged.

## What remains

- **Sound.** Anchoring makes a picture with sound follow the shared clock, but
  the sound itself plays at its device's rate from when it began: over an hour
  at 50 ppm that is 180 ms between picture and sound on one computer, and the
  same between computers. Fixing it means resampling each voice a little to
  follow the shared clock (the `Stretcher`/`SpeedPlan` machinery or a simple
  rate trim in `mix_voice`), which also absorbs the device's own latency
  differences only if they are measured. Two computers playing the same sound
  in one room will still comb without a shared audio clock (Dante, ruling R5:
  pick the computer that plays sound). Untested: no device here.
- **Media start.** Catching up works only as far as the decoder can run ahead
  of real time and the start isn't longer than the lead. A command should go out
  early enough to preload (`load_ahead`) before its time; today a cue is
  preloaded at the playhead, so a GO is normally warm, but a Fire of an
  unexpected cue or a seek to a new place is cold (160 to 260 ms here, more on
  a busy Mac). A late joiner, or a command that arrives after its time, needs the
  cue started part way (`StartAt::part_way`, as the timeline's chase does)
  rather than from 0; `tick_shared` anchors such a cue at its arrival.
- **Things still on the wall clock**: `CUE_LIMIT`, `AUDIO_START_TIMEOUT`,
  `REVEAL_TIMEOUT` (a dip's reveal), `PROBE_LIMIT`, the `Seek` swap-in. They
  decide *when* to give up, not the time a cue is at, but on two computers they
  can decide differently.
- **Not anchored**: fade ramps (`Param::step` sums `dt`; with shared `dt` they
  agree to a tick at their start), sound-only cues (device clock), live inputs,
  MIDI timecode output, a cue started by a follow/continue gets the tick it
  started at (within a tick, as today).
- **Displays.** Unaligned refresh: up to one refresh apart (17 ms at 60 Hz).
  Choosing the frame for the time it will be *presented* (the next vsync, from
  the swap chain's timing where the system reports it) would make each display
  correct for its own refresh but still not aligned with the other's.
- **Clock wander and sleep.** The simulation's drift is constant; real crystals
  wander with temperature (the fit's window is 5 minutes and it refits each
  sample). A computer that sleeps or is suspended jumps its local clock; the
  estimator would see a step and relock.
- **Real networks.** Switches with energy saving, Wi-Fi power save (delays of
  100+ ms in bursts) and asymmetric paths are worse than simulated. The
  asymmetry bias can only be measured with hardware timestamps (PTP) or a
  loopback measurement.

## Recommendations for stage 2 on

1. Keep the primary's own monotonic clock as the shared clock; run the exchange
   on its own thread on every follower (UDP, the `msg` format), 10 Hz until
   locked and 2 Hz after, and give the engine `SharedClock::now` once per frame
   through `tick_shared`. Show locked/error in the status panel; don't let a
   computer that isn't locked take timed commands without saying so.
2. Every GO, Fire, Stop, seek, pause and resume from the primary carries a
   shared time T = now + lead and is resent until acknowledged. Leads: 50 ms on a
   show LAN (the network needs under 2 ms; the rest is margin for a busy main
   loop), 100 ms on a poorer wired network, 300 to 400 ms on Wi-Fi. The primary
   plays to the same T itself. On receipt, preload what it will start
   (`load_ahead`), so the lead covers FFmpeg's start too; measure the cold start
   on real Macs and Windows machines and raise the lead if it is longer.
3. A command that arrives late, or a computer that joins late, starts the cue (or
   chases the timeline) part way, at `now - T`, rather than from 0.
4. Stage 2's heartbeat (cue times and timeline clocks by shared time) lets the
   primary show how far apart the computers are, measured, not assumed.
5. Make `tick_shared` the only tick on networked computers (primary included),
   and in the meantime keep `tick` exactly as it is for a single computer.
6. Sound (ruling R5): play it from one computer; if a second must, resample to
   the shared clock first (a later stage), and say plainly it will not be
   sample-locked.
7. Try it on two real computers on the owner's show LAN with a camera filming
   both screens and a timecode burn-in, before trusting any of this.
