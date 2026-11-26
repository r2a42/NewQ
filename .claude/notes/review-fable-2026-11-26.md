# NewQ code review (Fable), 2026-11-26

Reviewed: `main` at 8033753 (release 0.27.1), from the code only. Read-only: nothing was built, run
or changed. Every finding below comes from reading the code and tracing a path through it; none was
reproduced on a running program. "Confirmed" means every step of the path is in the code as quoted;
"plausible" means one step depends on something I could not check here (a driver, a real network,
a Mac).

## 1. Summary

The code is in good health for live use, and better defended than most programs of its kind. The
classic ways a show program dies have been closed on purpose: there are almost no `unwrap`s in
production code (I found about twenty and opened most of them; each was guarded), locks are `parking_lot` so a panic cannot
poison them, `clamp`s and `Duration`s on show data are guarded, the audio callback catches a panic
per voice and takes no lock that can block, FFmpeg is started and reaped off the main thread, saves are atomic and
synced, and network input (OSC, the web remote, the media server, Hap frames, venue models) is
length-checked before it is trusted. I looked hard for crashes from bad files or bad packets and
did not find one.

What I did find is mostly where the operator or the outside world meets the program, not in the
decoding or drawing. I found no defect I would call Critical outright, but R1 is a total freeze.
The three things I would fix first:

1. **R1**: cues that fire each other with no wait (two "Start" control cues, or a looped section
   made only of instant cues) spin the main thread for ever. NewQ has to be killed. Timelines have
   a guard against exactly this; the cue list does not.
2. **R2**: quitting or closing the main window mid-show is unguarded. With a saved show it ends the
   show at once with no question; with unsaved changes it puts up a system dialog that stops the
   engine, so pictures freeze until it is answered.
3. **R3**: on the show network, any computer on the LAN can declare itself "welcomed" and be taken
   for the primary when the real one's connection is down. The passcode does not cover this path.

After those: R4 (a show from a newer NewQ loses data when saved in an older one), R5 (a display
whose drawing fails to restart is never tried again) and R6 (NDI's library is loaded into NewQ
itself, every frame, despite the helper built to keep it out).

One caution on the design, not a defect: `ui.rs` is 17,800 lines and `Engine::tick_inner` is one
function of about 800 lines with some thirty interacting flags. Both work and are heavily tested,
but they are where the next subtle bug will be hard to see in review. I did not read most of
either (section 4), so the absence of findings there is not a clean bill.

Counts: 0 Critical, 3 High, 6 Medium, 7 Low.

## 2. Findings

### R1. Cues that fire each other with no wait freeze NewQ for good
- **Severity:** High. (It is a total freeze, which would be Critical; I rate it High because it
  needs a show built with such a circle, and it shows itself the first time that cue is fired.)
- **Confidence:** confirmed by reading every step; not run.
- **Where:** `src/engine.rs:1858-1871` (`fire`), `1873-1900` (`fire_one`), `3460-3463` (a Control
  cue in `start_with`), `2060` (`ControlAction::Start => self.fire(show, target)`), `3535-3541`
  (auto-continue → `fire_next`).
- **What:** `fire` runs a queue until it is empty, on the main thread, and nothing limits how many
  cues one press may fire or notices a cue coming round again. Cues that take no time (Control,
  Send, Memo, an empty group) and lead back to each other keep the queue from ever emptying.
- **How it happens:**
  ```rust
  self.fire_queue.push_back(id);
  if self.firing {
      return;
  }
  self.firing = true;
  while let Some(id) = self.fire_queue.pop_front() {
      self.fire_one(show, id);
  }
  ```
  `fire_one` starts a cue with no pre-wait at once; a Control cue is "instantaneous; nothing stays
  active" and its Start action calls `self.fire(show, target)`, which (because `firing` is set)
  only adds to the queue. Two ways to build the circle:
  1. Cue A is "Start cue B" and cue B is "Start cue A", neither with a pre-wait.
  2. The ordinary way to loop a section: cues 5 to 9 are OSC/MIDI sends or control cues set to
     auto-continue, and cue 10 is "Start cue 5". If nothing in the circle has a pre-wait, it never
     ends.
  One GO, hotkey or remote command then spins the main thread for ever: the window stops
  answering, pictures hold their last frame, nothing can be stopped, and NewQ has to be killed (up
  to 30 s of edits since the last autosave go with it). In a show network the same command reaches
  every follower and freezes each of them. The inspector already keeps a Control cue from naming
  *itself* (`ui.rs:8491`, `.filter(|x| x.0 != cue_id)`), and timelines have exactly this guard
  (`engine/timeline.rs:83-85`, `MAX_JUMPS`: "more is a loop"); the cue list's own queue has none.
- **Fix:** count the cues one outside call to `fire` starts (say 1,000, or each cue at most once
  per call); past the limit, clear the queue and tell the operator which cue went in a circle. A
  test: A starts B starts A returns, with a problem logged.

### R2. Quitting or closing the main window in the middle of a show is not guarded
- **Severity:** High
- **Confidence:** confirmed for File ▸ Quit and the main window's close button (all systems);
  plausible for Command-Q on a Mac (it goes through winit's default menu, which I could not run).
- **Where:** `src/app.rs:3677-3679`, `src/ui.rs:12401`, `src/app.rs:3267-3275`,
  `src/ui.rs:10857-10888` (`confirm_discard`), `src/ui.rs:11883-11895`.
- **What:** nothing asks "cues are playing, quit?". The only check before stopping everything and
  exiting is the unsaved-changes question, and that question is a system dialog shown on the main
  thread, which is the thread that runs the show.
- **How it happens:** the close button and File ▸ Quit both only set a flag:
  ```rust
  WindowEvent::CloseRequested => {
      self.ui.quit_requested = true;
  ```
  and the next frame does:
  ```rust
  if self.ui.quit_requested {
      self.ui.quit_requested = false;
      if ui::confirm_discard(&self.show, &mut self.ui) {
          ...
          self.engine.stop_all(0.0);
  ```
  `confirm_discard` returns `true` straight away when the show is saved (`if !st.dirty { ...
  return true; }`). So:
  1. *Show saved (the usual state on a show night):* a slip onto the close button, or Alt+F4 with
     the control window in front, stops every cue and exits. Show mode refuses New and Open but
     not Quit.
  2. *Unsaved changes:* `rfd::MessageDialog::new()...show()` blocks the main thread. The engine is
     ticked from that thread (`pass_show` → `play_tick`), and a video's next frame is only chosen
     in the tick (`v.player.update(v.picture_at)`, `engine.rs:3974`). While the box is up, pictures
     hold their frame and fades and auto-follows stand still; sound carries on from the audio
     thread; remote commands that arrive meanwhile wait in the queue and are all run in the first
     pass afterwards (`app.rs:1009-1021`). The same blocking
     question runs for New and Open with unsaved changes in edit mode. The code already knows the
     cost: `tell_save_failed` says "Waiting for the operator to read it would stop the show".
- **Fix:** when `engine.is_active()` or in Show mode, ask inside the egui window (non-blocking)
  before quitting; make the unsaved-changes question an in-window modal as well, so the engine
  keeps ticking.

### R3. Show network: a stranger on the LAN can pass for the primary; the passcode does not guard commands
- **Severity:** High (security; needs someone on the same network, and a moment when the real
  primary's connection to that computer is down)
- **Confidence:** confirmed by reading; not run.
- **Where:** `src/net/hub.rs:1322-1323`, `src/net/hub.rs:1427-1430`, `src/net/hub.rs:1151`,
  `src/app/play.rs:311-313`, `src/app/play.rs:116-130`.
- **What:** a connection becomes "welcomed" because the other side says so, with no proof and no
  check that this computer asked. Messages from a welcomed connection go to the app labelled with
  the id the peer wrote in its own Hello. The HMAC passcode is only checked on the adopt/obey path.
- **How it happens:**
  ```rust
  Msg::Welcome => {
      conn.welcomed = true;
  ```
  ```rust
  if (obeying || conn.welcomed)
      && let Some(from) = from
  {
      let _ = self.out.send(NetEvent::Message { from, msg: other });
  ```
  ```rust
  let primary =
      self.ui.net.me.primary == Some(from) && mode_of(&self.ui.net) == NetMode::Follower;
  ```
  `from` is `conn.peer = Some(h.id)` from the peer's Hello; ids are in the broadcast beacons. A
  program that connects to a follower, sends Hello with the primary's id, then `Welcome`, has its
  `Play` (fire cues), `ShowPart` (replace the show), `Restart` and `StartAtLogin` accepted.
  `conn.proof` is never consulted on this path, so a passcode changes nothing. The limit: the hub
  keeps one session per id (`hub.rs:1139-1149`), so it works only while the real primary's session
  to that follower is down (follower started first, primary restarting, a network blip); the
  impostor's session then also keeps the real primary out. Nothing drops a peer for never
  proving itself: the sweep at `hub.rs:1696-1715` only closes silent connections, and the passcode
  refusal runs only when the peer chooses to send `Prove`. On a primary, a stranger that names the
  network's id in its Hello and says `Welcome` is likewise sent the show and the play state:
  `tell` (`hub.rs:1912-1925`) sends to every peer of the network whose connection is `welcomed`.
- **Fix:** accept `Welcome` only on a connection this computer dialled as primary; forward app
  messages only when `obeying`, or when `welcomed` and (no passcode, or
  `conn.proof == Some(Some(true))`). Add a hub test: an unproven peer claiming the primary's id is
  not heard.

### R4. A show from a newer NewQ opens silently in an older one, and saving there drops what the older one doesn't know
- **Severity:** Medium (lost work)
- **Confidence:** confirmed by reading (by the absence of any check).
- **Where:** `src/show.rs:4028-4029` (`Show::load`), `src/show.rs:31-35` and `3730-3734` (`version`
  is only the 1→2 layout migration), `src/show.rs:4089-4104` (`save`).
- **What:** the rule "a later NewQ's kinds are read, not refused" is kept by reading leniently, but
  nothing in the file says which NewQ wrote it, so nothing warns. Unknown struct fields are not
  kept (no struct has a catch-all), and a kind read through `#[serde(other)]` or as the default is
  written back as that; only the two untagged variants in `stage3d/mod.rs:420,471` keep what they
  could not read.
- **How it happens:** `let mut show: Show = serde_json::from_str(&text)?;` ignores unknown fields
  and turns unknown kinds into `Unknown` or the default. The show is built on the main machine on
  0.27; the backup laptop still has 0.25; the operator opens the show there, nudges a level and
  saves. Every field 0.25 does not know is gone from the file, with no message; the recovery copy
  has the same loss.
- **Fix:** write the program's version into the show (a `saved_with` field, `#[serde(default)]` so
  old files load). On load, if it is newer than this program, say so and make the first Save a
  "Save as…". Test: a file with a higher `saved_with` raises the warning.

### R5. A display whose drawing fails to start again is never tried again
- **Severity:** Medium
- **Confidence:** the code path is confirmed; how often the restart fails on real hardware is
  plausible (surfaces are often unavailable for a moment right after a driver restart).
- **Where:** `src/app.rs:3468-3482` (`watch_render_threads`), `src/app.rs:1476-1485` (`watch_gpu`'s
  rebuild).
- **What:** when a render thread has died, or after a lost graphics device, each display's thread
  is started once. If that start fails, the display is left with no thread and nothing ever
  retries.
- **How it happens:**
  ```rust
  screen.thread = None;
  match start_output_thread(&gpu, &screen.window, screen.slot) {
      Ok((thread, size)) => { screen.thread = Some(thread); ... }
      Err(e) => said.push(e.replace("{display}", &(monitor + 1).to_string())),
  }
  ```
  The watchdog only looks at `screen.thread.as_ref().is_some_and(OutputThread::is_finished)`,
  which is false for `None`. No other code sets `screen.thread` for an existing window (I checked
  every use in `app.rs`). `start_output_thread` fails when `create_surface` fails or
  `surface_config` finds no format, both of which can be passing conditions while a monitor or
  driver comes back. The operator gets one line in the log and a dark display until that
  display's window is closed and opened again (the display switched off and on, the monitor
  replugged, or NewQ restarted): `screens` entries are only replaced when the routing changes
  (`app.rs:1800,1849`).
- **Fix:** treat `thread.is_none()` on an open output window as "needs starting" and retry with a
  back-off (the 2 s `render_restart` pacing is already there); keep saying so in the status bar
  until it works.

### R6. NDI's library is loaded into NewQ itself, on the main thread, on every frame of two panels
- **Severity:** Medium
- **Confidence:** confirmed by reading; not run (no NDI runtime here).
- **Where:** `src/ndi.rs:189-195` (`runtime_installed`), called from `src/ui.rs:10177`
  (`camera_inspector`), `src/ui.rs:6662` (`stream_editor`) and `src/stream.rs:926`.
- **What:** the design, and `areas/media-and-streams.md`, say NDI's library "is loaded with
  `libloading` only inside a helper ... so its faults can't take the show down". The check for
  whether it is installed loads it in the main program.
- **How it happens:**
  ```rust
  pub fn runtime_installed() -> bool {
      library_candidates()
          .iter()
          .any(|p| unsafe { Library::new(p) }.is_ok())
  }
  ```
  `Library::new` is `dlopen`/`LoadLibrary`: the library's start-up code runs inside NewQ, and
  dropping the result unloads it. The two UI callers are immediate-mode drawing code with no
  cache, so while a camera cue's inspector (for any non-NDI camera) or an NDI stream's editor is
  on screen this runs every frame. On a machine with NDI installed, a closed-source library is
  loaded and unloaded in the show's process many times a second, which is what the helper exists
  to prevent; and each call is loader and file-system work (up to four paths) on the thread that
  ticks the engine. The last candidate is a bare file name (`found.push(PathBuf::from(name))`), so
  the system's search order applies and a DLL of that name in the working folder would be loaded.
- **Fix:** answer "is it installed" with `Path::exists` on the fixed candidates, or ask the helper
  once and cache the answer for the run. Never call `Library::new` outside the helper.

### R7. "Browse…" for a recording's folder opens the system's folder picker on the main thread
- **Severity:** Medium
- **Confidence:** confirmed by reading.
- **Where:** `src/ui.rs:6297-6302` (in `stream_editor`).
- **What:** every other file dialog runs on its own thread (`start_dialog_with`,
  `start_slide_dialog`, `timeline/keys.rs::pick_file`) so the main loop never waits. This one does
  not.
- **How it happens:**
  ```rust
  if ui.button("Browse…").clicked()
      && let Some(p) = rfd::FileDialog::new()
          .set_title("Record into…")
          .pick_folder()
  ```
  The call blocks inside the drawing code. While the picker is open the engine does not tick, with
  the effects described in R2 case 2. Setting up a recording while something is already on the
  displays is an ordinary thing to do.
- **Fix:** route it through `start_dialog` with a new `DialogFor::RecordFolder`.

### R8. A video with sound is paced by the sound device's counter, with no fallback when the device goes quiet without an error
- **Severity:** Medium
- **Confidence:** plausible. The code path is confirmed; whether a device stalls without reporting
  an error depends on the driver, and I could not test hardware.
- **Where:** `src/engine.rs:3941-3959`, `src/media/audio.rs:1524-1530` and `143-149`.
- **What:** while a video's sound is not `finished()`, the cue's time and the picture's frame come
  only from the number of frames the audio callback has mixed. Nothing compares that with real
  time.
- **How it happens:**
  ```rust
  Some(vo) if !vo.finished() => {
      a.elapsed = a.elapsed.max(a.seek_base + vo.clock());
  }
  ```
  `finished()` turns true for a lost device only when cpal's error callback reports one of the
  kinds in `ends_stream`. If the device just stops asking for sound (a Bluetooth or HDMI sink that
  hangs, a stalled driver) there is no error, `played_frames` stops, the picture freezes on its
  frame, the cue never ends and the auto-follows after it never fire.
- **Fix:** a watchdog in the tick: if a started, unpaused voice's `played_frames` has not moved for
  about half a second, mark the device lost (the existing `Lost` path then lets the picture run on
  its own time) and tell the operator.

### R9. Show network: no limit on incoming connections, or on what an unproven one may send
- **Severity:** Medium
- **Confidence:** plausible (read, not run).
- **Where:** `src/net/hub.rs:2426-2445` (`listen`), `src/net/session.rs:50-77`,
  `src/net/proto.rs:349-360`, `src/net/hub.rs:317-318`.
- **What:** with the show network on, every accepted connection gets two threads and may ask for a
  64 MB buffer before it has said Hello or proved anything; nothing caps how many there are.
- **How it happens:** `Event::Accepted(stream)` → `add_conn` → `Session::start` spawns `net-write`
  and `net-read`; `read_frame` does `let mut body = vec![0u8; n];` for any `n` up to `MAX_FRAME`
  (`64 << 20`). A silent connection lives 6 s; one that trickles bytes lives longer. A host that
  opens connections in a loop costs threads and memory on the show machine; the hub's event queues
  are unbounded too.
- **Fix:** cap sessions that have not said Hello (say 16), cap the first frame at a few KB, and
  allow large frames only from a proven or obeyed peer.

### R10. Some FFmpeg runs cannot be watched, limited or cancelled
- **Severity:** Low
- **Confidence:** confirmed by reading.
- **Where:** `src/media/convert.rs:80-88` (`to_hap`), `src/media/video.rs:883` (`load_image`),
  `src/media/camera.rs:471,529,752,781`, `src/media/ffmpeg.rs:68,105`, `src/stream.rs:550`,
  `src/ndi.rs:657`.
- **What:** CLAUDE.md's rule is "`reaper::watch(&child)` after each spawn". These use
  `Command::output()`, which gives no child to watch, no time limit and no way to stop.
- **How it happens:** most are one-second queries. Two matter. `to_hap` converts a whole file
  (minutes, all cores) and can carry on after NewQ quits or crashes (most likely on Windows, where
  nothing ends it), leaving a half-written file. `load_image` is in the play path: a picture on a
  stalled network drive leaves an FFmpeg and a thread waiting for ever, and stopping the cue does
  not end them.
- **Fix:** one helper `run_limited(cmd, deadline, cancel)` on `spawn` + `reaper::watch` +
  `try_wait`, as `ffmpeg.rs:184-226` already does for probes, used wherever `output()` is now.

### R11. Closing a display's window switches that display off in the show itself
- **Severity:** Low (design risk)
- **Confidence:** confirmed by reading.
- **Where:** `src/app.rs:3727-3735`.
- **What:** closing an output window edits the show.
- **How it happens:**
  ```rust
  let on_it = self.screens[&m].outputs.clone();
  for cfg in &mut self.show.outputs {
      if on_it.contains(&cfg.id) {
          cfg.enabled = false;
      }
  }
  ```
  Alt+F4 with an output in front (or the system closing the window) takes the picture off that
  display for the rest of the show, and the next save or autosave keeps it off for the next
  performance. Show mode promises "nothing in the show can be changed by accident"; this can.
- **Fix:** in Show mode, or while anything plays, ignore the close and say so in the status bar;
  otherwise ask.

### R12. Third-party GitHub Actions are referenced by tag or branch in the job that builds what gets signed
- **Severity:** Low
- **Confidence:** confirmed.
- **Where:** `.github/workflows/release.yml:107,174` and `ci.yml:83` (`dtolnay/rust-toolchain@stable`),
  `ci.yml:133` (`Swatinem/rust-cache@v2`), `actions/*@v4` throughout.
- **What:** `@stable` is a moving branch and `@v2`/`@v4` are movable tags.
- **How it happens:** whoever controls those repositories, or steals a maintainer's token, can
  change what runs in the release build before the signing step. The Rust toolchain itself is
  "whatever stable is today". The rest of the chain is careful (FFmpeg and MediaMTX pinned by
  SHA-256, `--locked`, owner-only triggers, `permissions: {}` by default), so this is the soft
  spot.
- **Fix:** pin each `uses:` to a full commit SHA with the version in a comment, and pin the
  toolchain in `rust-toolchain.toml`.

### R13. The media server and the remote controls answer the whole LAN
- **Severity:** Low
- **Confidence:** confirmed.
- **Where:** `src/net/store/http.rs:321-420`, `src/web.rs:505`, `src/remote.rs:648`.
- **What:** the media store's HTTP server checks nothing about who asks; the web remote and OSC
  listen on every adapter.
- **How it happens:** with the show network on, any host that reaches the port can
  `GET /manifest/current` (every media file's path, size and hash) and `GET /object/<sha>` (the
  files). The web remote with an empty PIN accepts GO and stop from anyone, as OSC always does. The
  remote settings travel in the show file, so opening someone else's show opens the listeners it
  names. (The web remote itself is well made: PIN lock-out, Host check, POST-only with a custom
  header, size and time limits, a connection cap.)
- **Fix:** say plainly in the UI when the web remote has no PIN; bind the media server to the show
  network's adapter and ask for the passcode proof when a passcode is set.

### R14. OSC feedback: an old address lookup can overwrite a newer one
- **Severity:** Low
- **Confidence:** confirmed by reading.
- **Where:** `src/feedback.rs:371-377`.
- **What:** each change of the feedback address starts a lookup thread that writes into one shared
  slot.
- **How it happens:** `let (slot, name) = (self.addr.clone(), wanted.to_string());` then
  `*slot.lock() = found;`. Change the address twice quickly; if the first lookup (a slow name)
  answers last, feedback goes to the old address until the setting is touched again.
- **Fix:** give each lookup a generation number and drop an answer that is not the latest.

### R15. The waveform reader: `std::thread::spawn`, no time limit, and a child left unwaited on an error
- **Severity:** Low
- **Confidence:** confirmed by reading.
- **Where:** `src/media/waveform.rs:131-154`.
- **What:** small departures from the care taken elsewhere.
- **How it happens:** `std::thread::spawn(move || stderr.map(...))` panics if the system refuses a
  thread (elsewhere `Builder::spawn` is used and the error handled). The read loop's
  `stdout.read(&mut buf).map_err(|e| e.to_string())?` returns without `kill`/`wait`, leaving a
  zombie until NewQ exits. A file on a hung share keeps the thread and its FFmpeg for ever. None
  of this is on the main thread.
- **Fix:** `Builder::spawn`, and a small guard that kills and waits the child on every way out.

### R16. Edits made just before a Test Mode run are not on disk for the whole run
- **Severity:** Low
- **Confidence:** confirmed by reading.
- **Where:** `src/ui.rs:586-617` (`begin_test`), `src/app.rs:3238`.
- **What:** a test puts the operator's show aside in memory only, and the autosave is switched off
  while it runs.
- **How it happens:** `begin_test` does `show: std::mem::replace(show, test_show)` and keeps
  `dirty` in the stash; the autosave is `if !self.ui.test_locked && ...`. If the last autosave was
  up to 30 s before the test began, those edits exist only in memory for as long as the test runs
  (a Full test or a soak can be hours), and a test is when a crash, a hang or a lost graphics
  driver is most likely. The older recovery copy survives; the last half minute does not.
- **Fix:** in `begin_test`, when the show is dirty, write the recovery copy (and wait for it)
  before swapping the show.

## 3. Areas reviewed and how deeply

| Area | Depth | What I read |
|---|---|---|
| Panics, `unwrap`, `clamp`, `Instant`, `Duration` on outside data | deep (whole-tree sweeps) | every production `unwrap`/`expect`/`panic!`; every `clamp` with non-literal bounds; every `Duration::from_secs_f*`; `Instant` subtraction; lock kinds; `queue.submit`/`surface.configure` outside `Gpu` |
| Audio mixing and the callback | deep | `media/audio.rs` 1340-1375, 1480-1531, 1690-2050 |
| Web remote, OSC | deep | `web.rs` 36-130, 283-328, 490-653; `remote.rs` 145-245, 640-690 |
| Show network: hub, sessions, wire format | deep for the entry points | `net/hub.rs` 730-800, 1005-1050, 1085-1440, 2420-2464; `net/session.rs`; `net/proto.rs` 15-372; `app/play.rs` 116-130, 300-440 |
| Show network: play sync | medium | `net/play.rs` 560-748; `engine/net.rs` 120-190 |
| Show network: media store | medium | `net/store/http.rs` 321-700; `net/store/local.rs` 226-345 and its deletions; `manifest.rs` object keys and `is_sound`; `paths.rs::extension`; `net/media/manager.rs::handle` |
| Show files, saving, autosave | medium | `show.rs` 3985-4135, the `version` field and migration; `recovery.rs` whole; `ui.rs` 10857-11010; `app.rs` 3220-3275 |
| Video decoding and FFmpeg processes | medium | `media/video.rs` 55-186, 551-700, 800-905; `media/playback.rs` 195-384; `reaper.rs` 1-150; every `.wait()`/`.output()`/`.spawn()` site |
| Engine | medium: GO and firing, control cues, the tick's start and its video branch, every loop; skim elsewhere | `engine.rs` outline, 1685-1736, 1856-1916, 2019-2108, 2560-2582, 2971-3060, 3440-3546, 3679-4040, 4755-4790; every `loop`/`while` in `engine.rs`, `engine/`, `timeline/`, `speed.rs`, `playback.rs` |
| Compositor, render threads, device loss | medium | `gpu.rs` 18-62, 276-318; `app.rs` 177-201, 1462-1500, 3448-3520, 4405-4472 |
| Main loop and blocking | medium | `app.rs` 982-1022, 3644-3682, 3722-3735, 3745-3800, 3830-3960; `ui/menus.rs` 566-575; every `rfd::` call site |
| `unsafe` and FFI | medium | `ndi.rs` 123-212, 436-470; `monitor.rs` all `unsafe` blocks; counts for the rest |
| Hap | deep | `media/hap.rs` 60-190 |
| 3D stage | skim | `stage3d/load.rs` outline and its caps |
| Platform (crash, reaper) | medium | `crash.rs` 1-250; `reaper.rs` 1-150 |
| CI and release | medium | `release.yml` 1-60, 160-235, 296-340; `cut-release.yml` 36-130; all `uses:`/secrets/triggers (by grep); `ffmpeg_pins.txt`; checksum lines of the packaging scripts |
| Timelines | skim | `timeline.md` (first part); the `MAX_JUMPS` guard in `engine/timeline.rs`; `timeline/mod.rs` 52-70 |
| Test Mode | one probe | `ui.rs` 585-640 (`begin_test`/`end_test`), the autosave gate |
| Area notes | read | `show-and-engine.md`, `remote.md`, `media-and-streams.md`, `compositor-and-displays.md`, `network.md` to line 140, `timeline.md` to line 40; `platform.md`, `previews-and-ui.md`, `packaging.md` only searched for quit/close rulings (none found) |

## 4. Not reviewed, and why

I am one reviewer and this is about 177,000 lines of production Rust; I chose the paths where
outside data or the operator can hurt a show.

- **Timelines** (`timeline/`, `engine/timeline.rs`, `ui/timeline/`): checked only for loops that
  cannot end (there is a guard) and where the engine tick touches them. This is the largest
  unreviewed piece of show-critical logic.
- **Much of the engine**: groups and `advance_groups`, crossfades, `apply_entry`, most of
  `start_with`, `seek`, preload, fades (`Param`), `stop_all`/`panic`, and the network corrections
  (`net_correct`). Their logic can only be checked by reading all 5,000 lines with the tests, and
  I did not.
- **Test Mode** (`testmode/`, about 30,000 lines), `stats.rs`, `soak.rs`: one probe only (R16); not
  a show-time path.
- **`ui.rs` and `ui/`** apart from dialogs, save and the two NDI call sites.
- **3D stage rendering** (`render3d/`, `stage_render.rs`, `stage_live.rs`), `preview_pace.rs`,
  `multiview.rs`, `dock.rs`.
- **`upload.rs`**, most of `render.rs` and `gpu.rs` (shaders, the capture thread), `stream.rs`,
  `mediamtx.rs`: only their process handling was swept.
- **Net clock** (`net/clock.rs`, `time.rs`), `discover.rs`, `net/store/pull.rs` and `fetch.rs`,
  `node.rs`'s role rules.
- **MIDI, timecode, PJLink, clicker, clock triggers, PDF/PPTX import, relink, export.**
- **Mac and Windows code paths** (`macwin.rs`, `awake.rs`, `power.rs`, `autostart.rs`,
  `mediamtx::tie_to_us`): read only where listed; none can be run here.
- **Nothing was run.** No race was reproduced; timing-dependent claims rest on reading.
- **Dependencies** (`Cargo.lock`) were not audited.

## 5. What's good

Only what bears on risk:

- **Panics are designed for, not hoped against.** `crash::survive` around each part of the main
  loop, `catch_unwind` per voice in the audio callback, `OnPanic` on worker threads, report
  throttling so a per-frame fault cannot fill the disk, and `net_settle` to undo state a panic
  left behind.
- **No lock poisoning.** `parking_lot` almost everywhere; the `std::sync::Mutex` in `recovery.rs`
  recovers with `into_inner`.
- **The audio callback is clean**: no locks that can block (the recording tap is `try_lock`),
  buffers sized up front, dead voices freed on another thread, NaN turned to silence.
- **Saving is done properly**: write beside, `sync_all`, rename; autosave on a thread with a
  counter so a cleared copy cannot come back; a declined recovery copy is kept aside.
- **Outside input is bounded where I looked**: OSC bundles (depth and length), HTTP heads, Hap
  sections and Snappy lengths, venue models (triangles, textures, file sizes), manifest ids and
  object names (no path can leave the store).
- **FFmpeg handling is disciplined**: started and reaped off the main thread, a kill-then-wait
  thread per child, a watcher process that cleans up after a crash without touching anyone else's
  FFmpeg.
- **The release chain is careful**: owner-only triggers, checked tags, `--locked`, pinned and
  hash-checked FFmpeg and MediaMTX, a draft release that is only published when every build is in.
- **The notes are honest about what was never tried on real hardware.** That list (real NDI,
  capture cards, two real computers on a network, Mac and Windows-only code) is the real residual
  risk, more than anything in this report.
