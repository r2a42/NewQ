# NewQ code review (Opus, 2026-11-26)

Reviewed: `main` at 8033753 (0.27.1). This was a read-only review: no builds or tests were run here (the owner reports that the full suite passes). Every finding below was traced in the code. "Plausible" marks a step that couldn't be shown without running it.

## 1. Summary

For a program of this size (about 207,000 lines), the code is in good health for live use. In what I read, I found **no Critical or High defect**: nothing I could trace that crashes NewQ, loses a saved show, or gives a wrong picture or sound in ordinary use. That all-clear covers only what I read (section 3). The engine's fade and timeline logic, the compositor and render threads, the video readers and the network's play sync were sampled, not proved. The rules in CLAUDE.md are kept everywhere I searched for them. Clamps on numbers from a file are guarded against NaN and inverted bounds, my searches found no `Instant` subtracted from, there are almost no `unwrap`s outside tests in the files I scanned (`gpu.rs`, `render.rs`, `upload.rs`, `app.rs`, `media/audio.rs`, `media/video.rs`, `net/hub.rs`, `app/play.rs`, `engine/timeline.rs`, and `engine.rs` before its tests), and every GPU submit and configure goes through `Gpu`. Panics are caught at the right places: the main loop's parts, each window event, each audio voice, and the send-cue threads. The engine's network gate is reset after a panic (`net_settle`). Shows are saved by writing a temporary file, syncing it to disk and renaming it, and an autosave copy is kept every 30 s. The weak spots I found are at the edges: what the show network trusts, one dialog and one network lookup that run on threads that must never wait, and a safety feature (double-GO protection) that isn't there.

The three things I'd fix first:
1. **R2**: OSC and MIDI send cues share one thread that looks up host names at send time. One slow name can make a lighting or sound GO seconds late.
2. **R3**: there is no double-GO protection. A double tap or two operators fire two cues.
3. **R1**: on the show network, a follower believes whoever *says* they are the primary, even with a passcode. Bind the session to a verified identity.

R4 (a folder dialog that freezes every display while open) is a small fix and worth doing alongside.

## 2. Findings

Counts: Critical 0, High 0, Medium 4, Low 7.

### R1. On the show network, any computer on the LAN can pose as the primary to a follower, passcode or not

- **Severity**: Medium (needs someone on the show network acting on purpose; the effect is a wrong cue on a display computer)
- **Confidence**: confirmed by reading the code (not tried on a network)
- **Where**: `src/net/hub.rs:1322-1324` (`Msg::Welcome`), `src/net/hub.rs:1424-1432` (the catch-all), `src/net/hub.rs:1150` (`conn.peer = Some(h.id)`), `src/app/play.rs:312-314`
- **What**: a follower decides that a message comes from its primary only by the node id the other end *says* it has in its Hello. The passcode proof is checked only when that end also claims to be a primary. A session's messages are passed to the app once the *other end* has sent `Welcome`, which the follower takes from anyone.
- **How it happens**: the primary's node id is in every beacon it broadcasts (`proto::Beacon { id, .. }`). A program on the LAN connects to a follower's TCP port and sends a Hello with that id and `role: Display`. The follower stores `conn.peer = Some(h.id)` (hub.rs:1150) and sends `Prove`. `proved` checks the passcode only `if let Some(claim) = self.claim_of(conn)`, and `claim_of` returns None unless `h.role == Role::Primary`, so a non-primary hello passes without proof. The program then sends `Msg::Welcome`, which the follower accepts from anyone:
  ```rust
  Msg::Welcome => {
      conn.welcomed = true;
  ```
  After that every later message is forwarded:
  ```rust
  other => {
      if (obeying || conn.welcomed)
          && let Some(from) = from
      {
          let _ = self.out.send(NetEvent::Message { from, msg: other });
  ```
  `play_message` trusts it, because `from` is the claimed id:
  ```rust
  let primary =
      self.ui.net.me.primary == Some(from) && mode_of(&self.ui.net) == NetMode::Follower;
  match msg {
      Msg::Play(t) if primary => { ... self.play.follower.receive(*t, ...) }
      Msg::ShowPart(p) if primary => { ... self.take_show(hash, json); }
      Msg::Restart if primary => { self.play.restart = true; }
  ```
  `mode_of` is `Follower` whenever this computer is adopted (`v.me.adopted()`), and `me.primary` is its stored primary's id, whatever session is up. The media manager's `net.following` is built the same way (hub.rs:2296-2299: `self.me.primary.filter(|_| self.me.adopted())`). So the program can fire cues on that display, replace its whole show, or restart it. The rule "one session per computer" (hub.rs:1136-1146) blocks this only while the real primary's session is up. Once it drops (a cable, a primary restart, 6 s of silence), whoever says the id first holds the place, and the real primary is then the one turned away. The media messages check the same claimed id (`net.following.map(|f| f.id) == Some(from)`, manager.rs:405), so `MediaServer` could point a follower at another server too. The hash check in `fetch.rs` would still refuse wrong bytes.
- **Fix**: bind identity to the session. Only accept `Welcome`/app messages on a session where this computer has verified a passcode proof for that id, or where the session is the one `obeying` (or dialled by us to the address the primary's beacons came from). With no passcode, at least refuse a second claimant of the primary's id while its last session is recent. Say in the README that without a passcode the show network trusts everyone on the LAN.


### R2. A send cue to a host *name* holds up every OSC and MIDI send behind it while the name is looked up

- **Severity**: Medium (a lighting or sound GO sent by NewQ can arrive seconds late)
- **Confidence**: confirmed for the code path; how long a lookup takes on a show LAN with no DNS is plausible, not measured
- **Where**: `src/output.rs:450-486` (one `send-cues` thread), `src/output.rs:758-765` (`SocketSet::send`)
- **What**: OSC and MIDI send cues share one worker thread. Each OSC message resolves its host with `to_socket_addrs()` at send time, every time. That is a blocking system lookup, not cached. Projectors, TCP and scripts got a thread per host so that "one that doesn't answer holds up nobody else". OSC and MIDI did not.
- **How it happens**: the host is a free text box in the send cue's inspector (`ui.rs:9742,9782`), so a name is as easy to type as an address. An OSC send cue names its target as `lx-desk.local` or `eos` instead of an IP address. On a closed show network the name is unknown, or reachable only by mDNS/LLMNR. Each lookup can then take from about one to several seconds before it fails or answers. The lookup runs inside `for job in rx { … sender.send(&job) }`:
  ```rust
  Outgoing::Osc { host, port, packet } => {
      let addr = (host.as_str(), *port)
          .to_socket_addrs()
  ```
  While it waits, a MIDI GO to the sound desk fired by the next cue, or by the same GO, waits in the unbounded queue behind it. The feedback sender knows the problem and looks up its address once, on a thread of its own (`feedback.rs:372-377`). The send cues don't.
- **Fix**: resolve each host once, on a thread of its own, and cache the address with a short lifetime. Or give OSC hosts a thread per host, like TCP and projectors. Don't put MIDI behind a lookup.


### R3. Nothing protects against a double GO

- **Severity**: Medium (a missing safety feature, not a bug; a nervous double tap fires the next cue too)
- **Confidence**: confirmed (it is absent)
- **Where**: `src/engine.rs:1685-1729` (`Engine::go`, `go_playhead`); `src/ui.rs:11407` (`Action::Go`); the space bar (`space_key`, ui.rs) and every remote input (`run_remote`: web, OSC, MIDI, MSC) reach the same `apply(Action::Go)`; none of these places has a guard
- **What**: each GO, from the space bar, the GO button, the web remote, OSC, MIDI or MSC, fires the cue at the playhead at once and moves the playhead on. There is no minimum time between two GOs. Key repeats are filtered (`ui.rs:1352`, only a fresh press counts), so this is only about two separate presses. A key bounce, a double click on the GO button, or two operators (the desk, plus a phone on the web remote) fire two cues. QLab ships a "double-GO protection" setting for this reason. The presenter-remote code here has its own guard (`clicker::BACK_GUARD`), but GO has none.
- **How it happens**: a GO is pressed twice within 100 ms. `go_playhead` fires cue 12, sets `self.playhead` to cue 13, and the second call fires cue 13.
- **Fix**: a per-show "ignore a second GO within N ms" setting (QLab has such a setting; a few hundred milliseconds is a sensible default), applied in `Engine::go` so every input gets it, with the ignored press shown in the status bar.


### R4. "Browse…" for a recording's folder opens a dialog that stops the main loop, so every display freezes while it is open

- **Severity**: Medium (only if someone does it while something plays, but then the pictures freeze, fades stop and auto-follows wait)
- **Confidence**: confirmed by reading the code (not run)
- **Where**: `src/ui.rs:6297-6302` (in `stream_editor`, `StreamKind::Record`)
- **What**: every other file and folder dialog is run on a `file-dialog` thread (`start_dialog_with`, ui.rs:2895-2910), and its answer is polled, because a dialog that waits on the main thread stops the show. This one waits on the main thread:
  ```rust
  if ui.button("Browse…").clicked()
      && let Some(p) = rfd::FileDialog::new()
          .set_title("Record into…")
          .pick_folder()
  ```
- **How it happens**: during a show, the operator opens a display's Send out settings to set up a recording and clicks Browse…. Until the folder dialog is closed, `about_to_wait` → `App::pass` doesn't run. So there is no `Engine::tick`, and no new video frames go to the displays (the windows show their last picture). Fades and pre-waits stop. On a primary, no network heartbeats or commands go out. Sound goes on playing, because the audio callback is on its own thread, so picture and sound part company. The same holds, by design, for the "Unsaved changes" question (`confirm_discard`, ui.rs:10871), which waits on the main thread when the show is being left.
- **Fix**: open it with `start_dialog_with` and a `DialogFor::RecordFolder(display)` like the others. Add a test or a lint that finds `rfd::` calls outside the dialog thread.


### R5. A picture cue's FFmpeg is waited on with no limit and is not given to the reaper

- **Severity**: Low (Medium if a show is run from a network share)
- **Confidence**: confirmed
- **Where**: `src/media/video.rs:860-900` (`load_image`); also `media/mod.rs:630` (thumbnails), `media/camera.rs:471,529,752,781`, `stream.rs:550`, `ffmpeg.rs:68,105`
- **What**: `load_image` runs FFmpeg with `cmd.output()` on a thread. There is no deadline, no cancel and no `reaper::watch`. This breaks CLAUDE.md's hard rule "**No program NewQ starts outlives it**: `reaper::watch(&child)` after each spawn". The missing time limit is my own point: CLAUDE.md asks for one only for a stream's FFmpeg, but the same reasoning applies here. `ffprobe` got a limit for exactly this case (the comment in `Ffmpeg::probe`: "a file on a network share that has gone away can hold ffprobe for ever"), but the still-image path did not.
- **How it happens**: a picture cue's file is on a share that stops answering. The preload calls `load_image` → `cmd.output()` blocks for good, and the engine's `ActiveKind::Image` waits on `img.loader` for ever: the cue is "playing" but shows nothing, with no error. If NewQ then crashes, that FFmpeg is not on the reaper's list. (I didn't trace whether preparing the cue again starts another one.)
- **Fix**: use `ffmpeg::output_until(&mut cmd, &cancel, Some(deadline))` (which already watches the child) for `load_image` and the device-listing calls, and turn a timeout into the cue's error.


### R6. Before it has said who it is, a connection to the show network can make NewQ reserve 64 MB, and there is no limit on connections

- **Severity**: Low (needs someone on the LAN acting on purpose, and only while the show network is switched on)
- **Confidence**: confirmed for the code path; that it exhausts memory is plausible, not tried
- **Where**: `src/net/proto.rs:25` (`MAX_FRAME = 64 << 20`), `proto::read_frame`, `src/net/hub.rs:2432-2440` (`listen`), `hub.rs:1015` (`add_conn`), `session.rs:55-80`
- **What**: every accepted connection gets two threads and is read with `read_frame`, which trusts the 4-byte length up to 64 MB *before* the first frame (the hello) has been checked:
  ```rust
  let n = u32::from_be_bytes(head) as usize;
  if n > MAX_FRAME { return Err(FrameError::TooBig(n)); }
  let mut body = vec![0u8; n];
  ```
  The listener accepts any number of connections. A silent one is closed after `SILENT_FOR` (6 s). In those 6 s, a few hundred connections that each send a 64 MB length can claim tens of GB of address space, and on Windows commit charge. An allocation failure in Rust aborts the process: it is not a panic that `crash::survive` can catch.
- **Fix**: cap the first frame (a hello is a few hundred bytes) at, say, 64 KB, read the body in chunks as it arrives rather than reserving it up front, and cap connections that haven't yet said hello (per address and in all).


### R7. The media server answers anyone on the LAN, with no passcode

- **Severity**: Low
- **Confidence**: confirmed
- **Where**: `src/net/media/manager.rs:728-737` (bind `0.0.0.0:0`), `src/net/store/http.rs:357-404` (`serve`)
- **What**: the primary (and every follower that serves its store) runs an HTTP server on all adapters. It answers `GET /manifest/current` (the show's file list, with paths and hashes) and `GET /object/<sha>` (the files) to anyone who connects. It never checks the show network's passcode. Its 32 connection places (`ServeLimits::connections`) can also be taken by slow readers: each write waits up to 30 s (`write_timeout`). That would keep followers from pulling media.
- **How it happens**: `serve` checks the method, the path shape and `is_sha`, and nothing else. The port is random, but a port scan finds it.
- **Fix**: give requests a token derived from the passcode (an HMAC of the path with the key), or accept only addresses of computers adopted in the network. Count connection places per address.


### R8. A save asked for while another is still being written is dropped without a word, Save As included

- **Severity**: Low
- **Confidence**: confirmed
- **Where**: `src/ui.rs:10956-10960` (`start_save`), `11902-11907` (`Action::Save`, `Action::SaveShowTo`)
- **What**: `start_save` returns at once, saying nothing, when `st.saving` is set:
  ```rust
  fn start_save(show: &Show, st: &mut UiState, path: PathBuf) {
      if st.saving.is_some() {
          return;
      }
  ```
  On a slow disk (a USB stick, a network share), a Ctrl+S, or a Save As to a *new* file, made while the previous save is still being written, does nothing. With Save As, the operator has chosen a file name and believes the show is there, and no file is written. The show stays marked as changed (`dirty` is worked out against the earlier snapshot), and the recovery copy goes on. That is why this is Low.
- **Fix**: queue the latest request (path and snapshot) and start it when the running save ends, or at least say "still saving, try again" in the status bar.

### R9. Two copies of the primary's show can be written to the same temporary file at once

- **Severity**: Low
- **Confidence**: confirmed (race by reading; consequences traced)
- **Where**: `src/app/play.rs:395-404` (`take_show`, thread `net-keep-show`)
- **What**: each show taken from the primary starts a thread that writes `synced-show.tmp` and renames it over `synced-show.newq`. Nothing stops two of these threads running together. The primary sends the show 150 ms after every edit, so while someone edits on the primary, a follower takes shows several times a second.
- **How it happens**: thread A runs `std::fs::write(&tmp, …)`, thread B truncates and rewrites the same `tmp`, then A renames whatever is in it. The kept file can then be a mix of the two, or the older show can land last. At the next start `play_load_kept` either fails to parse it (`let Ok(mut show) = … else { return false }`: the display starts with no show until the primary is reached) or opens the older show. The primary then corrects it, because the hashes differ.
- **Fix**: one writer thread fed by a channel that keeps only the newest, or a unique temporary name per write plus a sequence number checked before the rename.


### R10. A show from a newer NewQ that uses a new cue kind, transition or effect can't be read at all by an older one, on disk or over the network

- **Severity**: Low (the network page already warns when versions differ)
- **Confidence**: confirmed
- **Where**: `src/show.rs:1250` (`ContinueMode`), `1275` (`Fit`), `1319` (`Transition`), `1507` (`CueKind`), `1988` (`EffectKind`); `src/app/play.rs:375-385` (`take_show`)
- **What**: these enums have no `#[serde(other)]` or lenient fallback. One unknown value anywhere makes `serde_json::from_str::<Show>` fail for the whole show. On a follower, `take_show` then logs "the primary's show couldn't be read" and keeps whatever it had. The primary resends the whole show every `RESEND_SHOW` (3 s), and the follower fails again each time. CLAUDE.md's rule says "most" kinds in a show are read leniently, and these central ones are not.
- **Fix**: give `CueKind` an `Unknown(serde_json::Value)` variant that loads as a disabled, unplayable cue and is written back as it was. Make the small enums fall back to their defaults. Failing that, refuse to adopt a follower whose `VERSION` is older than the primary's, rather than letting it fail quietly.


### R11. Release builds use third-party actions pinned by moving tags

- **Severity**: Low
- **Confidence**: confirmed
- **Where**: `.github/workflows/release.yml:107,174` (`dtolnay/rust-toolchain@stable`), `ci.yml`, `warm-cache.yml`, `flaky-hunt.yml` (`Swatinem/rust-cache@v2`); `release.yml:27-28` (`permissions: contents: write` for the whole workflow)
- **What**: every action is pinned to a tag or a branch (`@v4`, `@v2`, `@stable`), not to a commit. `dtolnay/rust-toolchain@stable` is a branch. It installs the compiler that builds the release programs: in the `mac-half` jobs it builds the Mac halves that the `macos` job later signs and notarises with the Apple keys, and in the `build` job it runs before the step that signs `newq.exe` with `WINDOWS_CERT_PFX` on the same runner. A changed upstream action could therefore get code into a signed download, or leave something behind on the Windows runner for the signing step to meet. Every job also holds a token that can write releases. Downloads are well guarded: FFmpeg and MediaMTX are checked against pinned SHA-256s, and the release build is `--locked` and uncached.
- **Fix**: pin third-party actions to full commit SHAs (Dependabot can keep them current), and give `contents: write` only to the jobs that make and publish the release.


## 3. Areas reviewed and how deeply

| Area | Depth | What was read |
|---|---|---|
| Show network: trust, sessions, frames | deep | `net/hub.rs` (hello, prove, message, closed, add_conn, listen), `net/session.rs` (all), `net/proto.rs` (frames, hello, beacon, lenient kinds), `app/play.rs` (play_message, take_show, play_load_kept, play_send_show, net_test_flush), `net/play.rs` (ShowAssembly, show_parts), `net/media/manager.rs` (handle, server start) |
| Media store and its HTTP server | medium | `net/store/http.rs` (server and serve, the start of the client), `net/store/local.rs` (resolve, find_latest, gc, protected_now, the start of plan) |
| Web remote | deep | `web.rs` (all non-test code), `web_page.html` (how names are drawn) |
| OSC and MIDI input | deep | `remote.rs` (OSC parsing and bundles, first argument, MSC and MIDI parsing, the OSC listener) |
| Send cues, feedback, PJLink | medium | `output.rs` (Outbox threads, send_network, SocketSet::send, run_script), `feedback.rs` (address lookup), `projector.rs` (connect and timeouts) |
| Audio mixing and the callback | deep | `media/audio.rs` (the output callback, mix_each, mix_voice, fade_crosspoint, taps, sync_placed, retry_failed, silent mixer) |
| FFmpeg processes | medium | `media/ffmpeg.rs` (probe, output_until, has_encoder, command), `media/video.rs` (load_image), `media/playback.rs` (Children spawn/reap, by its outline), `media/camera.rs` (device listing), every `Command::new` against `reaper::watch` |
| Show files and saving | medium | `show.rs` (load, save, RecordArea::clamped, Animated::at, Effect::values, the main enums), `recovery.rs` (all non-test), `ui.rs` save, start_save, finish_save, confirm_discard, tell_save_failed |
| Engine (GO, groups) | skim | `engine.rs` go, go_playhead, playhead_top, switch_list, first_in_list, image loading in tick, the handling of deleted cues (by grep), `engine/net.rs` (net_settle, gated) |
| Hard-rule sweeps across `src/` | medium (by search) | `.clamp(` with bounds that aren't constants (listed by search; those in `show.rs`, `gpu.rs`, `audio.rs`, `fx.rs`, `speed.rs`, `engine.rs`, `stage3d/projector.rs`, `ui.rs` and `multiview` were opened), `Duration::from_secs_f32/f64(` outside tests, Instant arithmetic by pattern, `queue.submit` and `surface.configure` (only inside `Gpu`), `unwrap`/`expect` in the files named in section 1, `rfd::` dialogs, `TcpListener::bind`, `.output()` calls |
| GPU compositor and device loss | skim | `gpu.rs` (Gpu::new tail, submit, configure, try_configure), `app.rs` (part, pass, watch_software, the repaint pacing), the area notes |
| NDI and other FFI | medium | `ndi.rs` (find, receive loop, convert_frame), `net/store/space.rs`, `monitor.rs` (the Windows EDID read); the rest were only listed |
| Timelines | skim | `engine/timeline.rs` looked over for loops and panics only |
| Clock triggers | medium | `clock.rs` (follower) |
| CI and release | medium | `release.yml` (verify, build, signing steps), every workflow's `uses:` and `permissions:`, `package_release.sh` (FFmpeg and MediaMTX downloads and checksums) |

## 4. Not reviewed

- **Most of the engine's logic** (`engine.rs` fades, transitions, preload and layers; `engine/timeline.rs`), and `timeline/`. It is very large and very heavily tested (about 4,000 lines of timeline tests alone), so I sampled it for panics and loops rather than proving its logic.
- **The GPU compositor, render threads and the uploads thread** in depth (`gpu.rs`, `render.rs`, `upload.rs`), and the device-loss rebuild. I read only the submit and configure gate. These can only really be judged by running them.
- **The 3D stage** (`stage3d/`, `render3d/`, `stage_*`), **Test Mode** (`testmode/`), `soak.rs`, `bench.rs`: not used while a show plays (Test Mode is refused on a network), so they were left for last and not reached.
- **The UI** (`ui.rs` beyond saving and dialogs, `ui/`), menus, keys, the timeline editor.
- **The streaming paths** (`stream.rs`, `mediamtx.rs`, `streamlink.rs`), the video readers' frame timing (`video.rs` beyond `load_image`), Hap decoding, PDF and PowerPoint import.
- **Mac and Windows-only code** (`macwin.rs`, `clicker.rs`, `freeze.rs`, `monitor.rs`): read only where it is `unsafe`. None of it could be run here.
- **The other `unsafe` blocks**: `macwin.rs`, `clicker.rs`, `mediamtx.rs` (job objects), `autostart.rs` (registry), `awake.rs`, `power.rs`, `crumbs.rs`, `stats.rs`, and the Mac part of `monitor.rs` were listed, not read.
- **The network's clock and play sync** (`net/clock.rs`, `net/time.rs`, `Leader`/`Follower`, `Engine::net_correct`), the media fetch and pull (`net/store/fetch.rs`, `pull.rs`, so the "hash-checked" claim below rests on the code's own notes and `Store::resolve`), and `net/node.rs`'s judge and adopt rules beyond how `proved` calls them.
- **Show migration and backwards compatibility** (`migrate_legacy`, `normalize_*`, the serde defaults), beyond the enums in R10.
- **The audio reader thread and a voice's life** (`media/audio.rs` reader, `Voice`), `crash.rs` and `reaper.rs` themselves.
- **No lock-order (deadlock) audit and no leak audit** were done beyond the threads and processes named in the findings.
- **Hardware**: nothing here was tried on real sound devices, displays, MIDI, NDI or a real network, and no test was run.

## 5. What's good

- **Panics are survived where it matters**: each part of the main loop, each window event, each audio voice (`mix_each` drops only the voice that panicked and silences that one buffer), and the send-cue threads. After a panic the engine's network gate is put back each pass (`net_settle`), so a panic can't silently stop a primary sending to its followers.
- **The audio callback is careful**: the mix buffer and voice list are reserved up front (they can still grow past 16,384 frames or 64 voices), finished voices are freed on a janitor thread, the recording tap is only `try_lock`ed, and output that isn't a number becomes silence.
- **Numbers from files and the network are distrusted consistently**: NaN and inverted-bound clamps are guarded, OSC bundles are depth-limited and bounds-checked, web requests have a deadline, a size limit, a PIN lockout, Host checks against DNS rebinding and POST-only commands, and the media store hands out only objects in its index at their full size (`Store::resolve`); the hash check itself is in `fetch.rs`, which I didn't read.
- **Saving is safe**: a temporary file, `sync_all`, then a rename, plus a recovery copy every 30 s that can't come back after a save.
- **Processes and threads are bounded**: probes have deadlines, playback FFmpegs are watched by the reaper and killed on drop, and the web and media servers cap their connections.
- **Release supply chain**: FFmpeg and MediaMTX are checked against pinned SHA-256s, the release build is `--locked` and uncached, and only the owner or the cut-release bot may start a release.
- **Version skew on the wire is handled**: network kinds read unknown values leniently, and the Show network page warns when versions differ.
