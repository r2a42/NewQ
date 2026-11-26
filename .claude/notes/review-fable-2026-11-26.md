# NewQ code review (Fable), 2026-11-26

Reviewed: `main` at 8033753 (release 0.27.1). Read-only; nothing was built or run.

STATUS: in progress. Findings are added as they are confirmed.

## 1. Summary

(to be written last)

## 2. Findings

(working notes; sorted and renumbered at the end)

### N1. Show network: any computer on the LAN can make itself "welcomed" and pass for the primary; the passcode does not guard commands
- Severity: High. Confidence: confirmed by reading (not run).
- Where: `src/net/hub.rs:1322-1323` (`Msg::Welcome => { conn.welcomed = true; ...`), `src/net/hub.rs:1427` (`if (obeying || conn.welcomed) && let Some(from) = from` forwards the message to the app), `src/app/play.rs:311-313` (`let primary = self.ui.net.me.primary == Some(from) && ...`), `src/net/hub.rs:1151` (`conn.peer = Some(h.id)` from the peer's own Hello).
- What: `welcomed` is set because the *other side says so*, with no proof and no check that this computer asked for it. `from` is the id the peer wrote in its own Hello. So a program that connects, says "Hello, I am <the primary's id>" (ids are in the broadcast beacons) and then "Welcome", has its `Play`, `ShowPart`, `Restart`, `StartAtLogin` messages accepted as the primary's. The HMAC passcode is only checked on the adopt/obey path (`proved`, `judge`); this path never looks at `conn.proof`.
- Limit: the hub keeps one session per id (`hub.rs:1139-1149`), so it only works while the real primary's session to that follower is down (follower started first, primary restarting, a network blip). Then the impostor's session also blocks the real primary's until it is dropped.
- Effect: a follower's show replaced, cues fired, NewQ restarted, on a network the owner believed was protected by a passcode. Also, on a primary, `broadcast_frame` (`hub.rs:782`) sends the whole show to any "welcomed" stranger.
- Fix: only accept `Welcome` on a connection this computer dialled as primary; forward app messages only when `obeying`, or when `welcomed` **and** (no passcode, or `conn.proof == Some(Some(true))`).

### N2. Show network: no limit on incoming connections or on what an unproven one may send
- Severity: Medium. Confidence: plausible (code read, not run).
- Where: `src/net/hub.rs:2426-2445` (`listen`, every accept becomes a session), `src/net/session.rs:50-77` (two threads per connection), `src/net/proto.rs:349-360` (`let mut body = vec![0u8; n];` for any `n` up to `MAX_FRAME` = 64 MB, before Hello or proof), `src/net/hub.rs:317-318` (unbounded event queues).
- What: with the show network switched on, a host on the LAN can open connections as fast as it likes; each costs two threads and may reserve a 64 MB buffer, and lives 6 s if silent (longer if it trickles pings). Nothing caps the count. A machine short of memory or threads during a show is a stutter or worse.
- Fix: cap sessions without a Hello (say 16), cap the first frame at a few KB, and only allow big frames from a proven/obeyed peer.

### N3. NDI's library is loaded into NewQ itself, on the main thread, on every drawn frame of two panels
- Severity: Medium. Confidence: confirmed by reading (not run; no NDI runtime here).
- Where: `src/ndi.rs:189-195` (`runtime_installed`), called from `src/ui.rs:10177` (`camera_inspector`), `src/ui.rs:6662` (`stream_editor`), `src/stream.rs:926`.
- What: the design (and `areas/media-and-streams.md`) says NDI's library "is loaded with `libloading` only inside a helper ... so its faults can't take the show down". But the check for whether it is installed does load it, in the main program:
  ```rust
  pub fn runtime_installed() -> bool {
      library_candidates().iter()
          .any(|p| unsafe { Library::new(p) }.is_ok())
  }
  ```
  `Library::new` is `dlopen`/`LoadLibrary`: it runs the library's start-up code inside NewQ, and dropping the result unloads it again. The two UI callers are immediate-mode drawing code, so this happens on every frame while a camera cue's inspector (any non-NDI camera) or an NDI stream's editor is on screen, with no cache.
- How it hurts: (a) on a machine with NDI installed, a closed-source library is loaded and unloaded in the show's process many times a second; a library that starts threads or is not written to be unloaded can crash or leak, which is exactly what the helper was built to keep out. (b) Each call is file-system and loader work on the main thread (up to four candidate paths; on Windows a bare DLL name is searched along the DLL search path), on the thread that also ticks the engine. (c) The bare-name load (`found.push(PathBuf::from(name))`) follows the system search order, so a DLL of that name in the working folder would be loaded into NewQ.
- Fix: answer "is it installed" by `Path::exists` on the candidates, or ask the helper once (`newq --ndi-list` style) and cache the answer for the run; never `Library::new` outside the helper.

### N4. A video with sound is paced by the sound device's counter, with no fallback when the device goes quiet without an error
- Severity: Medium. Confidence: plausible (the code path is confirmed; whether a given device stalls silently depends on the driver).
- Where: `src/engine.rs:3941-3959`, `src/media/audio.rs:1524-1530`.
- What: while a video's voice is not `finished()`, the cue's time and the picture's frame come only from frames the audio callback has mixed:
  ```rust
  Some(vo) if !vo.finished() => {
      a.elapsed = a.elapsed.max(a.seek_base + vo.clock());
  }
  ...
  Some(vo) if !vo.finished() => Some(vo.clock_with_lead()),
  ```
  `finished()` becomes true for a lost device only when cpal's error callback fires with one of the kinds in `ends_stream`. If the device simply stops asking for sound (a Bluetooth or HDMI sink that hangs, a driver that stalls, a sleeping USB interface) there is no error, `played_frames` stops, and the picture freezes on that frame; the cue never ends, so auto-follows after it never fire. Nothing compares the voice's clock with wall time.
- Fix: a watchdog in the tick: if a started voice's `played_frames` has not moved for, say, 0.5 s while not paused, treat the device as lost (set `Lost`), log it, and let the picture run on its own time, as it already does after a reported loss.

### N5. Third-party GitHub Actions are referenced by tag or branch, in the job that builds what gets signed
- Severity: Low. Confidence: confirmed.
- Where: `.github/workflows/release.yml:107,174` (`dtolnay/rust-toolchain@stable`), `ci.yml:83,133` (`Swatinem/rust-cache@v2`), `actions/*@v4` throughout.
- What: `@stable` is a moving branch and `@v2`/`@v4` are movable tags. Whoever controls those repositories (or steals a maintainer's token) can change what runs in the release build before the signing step. The rest of the release chain is careful (FFmpeg and MediaMTX pinned by SHA-256, `--locked`, owner-only triggers, `permissions: {}` by default), so this is the one soft spot.
- Fix: pin each `uses:` to a full commit SHA (with the version in a comment), and pin the Rust toolchain version in `rust-toolchain.toml` so a release is reproducible.

### N6. The media store's HTTP server and the web remote's state are open to the whole LAN
- Severity: Low. Confidence: confirmed.
- Where: `src/net/store/http.rs:321-420` (no check of who asks), `src/web.rs:505` (`TcpListener::bind(("0.0.0.0", port))`), `src/remote.rs:648` (OSC on `0.0.0.0`).
- What: with the show network on, any host that can reach the port can `GET /manifest/current` (every media file's path, size and hash) and `GET /object/<sha>` (the files themselves). The web remote with an empty PIN (the default) and OSC accept GO/stop from any host; that is how OSC works, but the web remote could say so. The remote settings travel in the show file, so opening someone else's show opens the listeners it names.
- Fix: say in the UI when the web remote has no PIN; consider binding the media server to the show network's adapter and requiring the passcode proof (a header) when a passcode is set.

### N7. OSC feedback: an old address lookup can overwrite a newer one
- Severity: Low. Confidence: confirmed by reading.
- Where: `src/feedback.rs:371-377`.
- What: each change of the feedback address starts a lookup thread that writes into the same shared slot (`let (slot, name) = (self.addr.clone(), ...)` ... `*slot.lock() = found;`). If the address is changed twice quickly and the first lookup (a slow DNS name) answers last, feedback goes to the old address until the setting is changed again.
- Fix: give each lookup a generation number and drop an answer that is not the latest.

### N8. `waveform.rs` and a few helpers use `std::thread::spawn`, and a waveform's FFmpeg has no time limit
- Severity: Low. Confidence: confirmed by reading.
- Where: `src/media/waveform.rs:131-154`.
- What: `std::thread::spawn` panics if the system refuses a thread (the rest of the code uses `Builder::spawn` and handles the error). The read loop returns early on a read error (`.map_err(|e| e.to_string())?`) without `child.kill()`/`wait()`, leaving a zombie until NewQ exits; and a file on a hung network share keeps the waveform thread and its FFmpeg for ever. None of this is on the main thread.
- Fix: `Builder::spawn`, and kill + wait the child on every exit path (a small guard type).

### N9. Quitting or closing the main window in the middle of a show is not guarded: it either ends the show at once or freezes the pictures behind a question
- Severity: High. Confidence: confirmed by reading for File ▸ Quit and the window's close button (all systems); plausible for Command-Q on a Mac (winit's default menu; not run here).
- Where: `src/app.rs:3677-3679` (`WindowEvent::CloseRequested => { self.ui.quit_requested = true; ...`), `src/ui.rs:12401` (`Action::Quit => st.quit_requested = true`), `src/app.rs:3267-3275`, `src/ui.rs:10857-10888` (`confirm_discard`).
- What: the only thing between a quit request and `engine.stop_all(0.0)` + exit is `confirm_discard`, which asks nothing when the show is saved:
  ```rust
  if !st.dirty {
      recovery::clear(st.show_path.as_deref());
      return true;
  }
  let answer = rfd::MessageDialog::new()
      .set_title("Unsaved changes") ...
      .show();
  ```
  Two cases, both bad in a live show:
  1. **Show saved (the normal state on a show night)**: a slip onto the close button, Alt+F4 with the control window in front, or File ▸ Quit, stops every cue and exits with no "something is playing, quit?" question. Show mode refuses New and Open but not Quit.
  2. **Show has unsaved changes**: the native question is shown *on the main thread* (`.show()` blocks). The main thread is the one that ticks the engine (`pass_show` → `play_tick`), and a video's next frame is only chosen in the tick (`v.player.update(v.picture_at)`). So while the box is up, pictures hold their frame, fades and auto-follows stand still; sound carries on from the audio thread, and a video with sound jumps forward to catch up when the box is answered. The code knows this about another dialog: `tell_save_failed` says "Waiting for the operator to read it would stop the show".
  The same blocking question runs for Ctrl+N / Open with unsaved changes in edit mode (`ui.rs:11883-11895`).
- Fix: when `engine.is_active()` (or in Show mode), ask inside the egui window ("Cues are playing. Quit?") with nothing blocking, and only then go on; make the unsaved-changes question an in-window modal too, so the engine keeps ticking.

### N10. "Browse…" for a recording's folder opens the system's folder picker on the main thread
- Severity: Medium. Confidence: confirmed by reading.
- Where: `src/ui.rs:6297-6302` (in `stream_editor`).
- What: every other file dialog is started on its own thread (`start_dialog_with`, `start_slide_dialog`, `timeline/keys.rs::pick_file`), with the comment that the main loop must not wait. This one is called straight from the drawing code:
  ```rust
  if ui.button("Browse…").clicked()
      && let Some(p) = rfd::FileDialog::new()
          .set_title("Record into…")
          .pick_folder()
  ```
  While the picker is open the engine does not tick (see N9, case 2). Setting up a recording while something is already on the displays is an ordinary thing to do.
- Fix: route it through `start_dialog` with a new `DialogFor::RecordFolder`.

### N11. A show from a newer NewQ opens without a word in an older one, and saving it there silently drops what the older one doesn't know
- Severity: Medium (lost work). Confidence: confirmed by reading (by the absence of any check).
- Where: `src/show.rs:4028-4029` (`Show::load`: `serde_json::from_str(&text)`), `src/show.rs:31-35` (`version` is only the 1→2 layout migration, `show.rs:3730-3734`), `src/show.rs:4089-4104` (`save` writes the in-memory show).
- What: the hard rule "a later NewQ's kinds are read, not refused" is kept by reading leniently: unknown fields are ignored by serde (no struct keeps them), unknown kinds become `Unknown`/the default. Nothing in the file says which NewQ wrote it, so nothing can warn. Scenario: the show is built on the main machine on 0.27; the backup laptop (or last season's machine) still has 0.25; the operator opens the show there, nudges a level, presses Save. Every field 0.25 does not know (newer cue options, 3D stage parts, timeline kinds) is gone from the file, with no message, and the autosave/recovery copy has the same loss.
- Fix: write the program's version into the show (`saved_with`, `#[serde(default)]`); on load, if it is newer than this program, say so in the status bar and make the first Save a "Save as…" (or refuse to overwrite without a yes). A test: load a file with `saved_with` above the current version and check the warning.

### N12. Some FFmpeg runs cannot be watched, limited or cancelled (`Command::output()`), against the "no program outlives NewQ" rule
- Severity: Low. Confidence: confirmed by reading.
- Where: `src/media/convert.rs:80-88` (`to_hap`: a whole-file Hap conversion), `src/media/video.rs:883` (`load_image`: every picture cue's decode), `src/media/camera.rs:471,529,752,781`, `src/media/ffmpeg.rs:68,105`, `src/stream.rs:550`, `src/ndi.rs:657`.
- What: CLAUDE.md's rule is "`reaper::watch(&child)` after each spawn". `Command::output()` gives no `Child` to watch, no time limit and no way to stop. Most of these are one-second queries. Two matter: `to_hap` can run for many minutes at full CPU and goes on after NewQ quits or crashes (on Windows nothing ends it; the half-written file stays); `load_image` is in the play path, so a picture on a stalled network drive leaves an FFmpeg and a thread waiting for ever, and stopping the cue does not end them.
- Fix: one helper `run_limited(cmd, deadline, cancel)` built on `spawn` + `reaper::watch` + `try_wait` (as `ffmpeg.rs:184-226` already does for probes), used everywhere `output()` is now.

### N13. Closing a display's window switches that display off in the show itself
- Severity: Low (design risk). Confidence: confirmed by reading.
- Where: `src/app.rs:3727-3735`.
- What: `WindowEvent::CloseRequested` on an output window sets `cfg.enabled = false` on the show's displays that were on it:
  ```rust
  let on_it = self.screens[&m].outputs.clone();
  for cfg in &mut self.show.outputs {
      if on_it.contains(&cfg.id) { cfg.enabled = false; }
  }
  ```
  Alt+F4 (or the system closing the window) with an output in front takes the picture off that display for the rest of the show, and the change is in the show data: the next Save or autosave keeps the display off for the next performance too. In Show mode "nothing in the show can be changed by accident", but this can.
- Fix: in Show mode (or while anything plays) ignore the close and say so in the status bar; otherwise close the window without editing the show, or ask.


## 3. Areas reviewed and how deeply

(to be filled)

## 4. Not reviewed, and why

(to be filled)

## 5. What's good

(to be filled)
