# Fable review before 0.24.4 (2026-11-19)

Three read-only Fable reviewers over everything since v0.24.2. Findings to fix before the release.

## C. Test Mode, measurement, media, release

C1. **The user's show is lost when a network test fails to launch after spreading** (confirmed by
   reading; also in 0.24.2). `session.rs` `Phase::Spreading` `Then::Level`/`Then::Full` arms and
   `launch`/`full::launch` take the stash by value and `?` on `create_dir_all`/`StatsLog::create`, dropping
   it; test show stays, `test_locked` stays, fleet dropped without `End`. Fix: on error `end_test` with the
   stash (or return it in the Err), `f.end(now)` and drain `f.out` before dropping the fleet.
C2. **A follower that misses `End` stays muted and measuring up to 3 h** (`MEASURE_AT_MOST`): session blip
   at the end, C1, or the primary quitting (`End` pushed to `net_out` then `exit()`). Fix: follower ends
   measuring (unmute, drop load) when the primary's session closes or a non-test show arrives; primary
   sends `End{run}` to any computer still reporting for a finished run; flush `End` on quit.
C3. **`shown_apart` subtracts half the worst round trip of the last 20 s** (`play.rs:327` ->
   `measure::seen_offset`, `time.rs` `worst_rtt`): one 70 ms ask biases every heartbeat by -35 ms for 20 s
   -> false "Out of step"; and can hide a real 30 ms offset. Fix: the best (smallest) recent round trip or
   the fitted sample's.
C4. **Long parts over 3 h get a false "dropped out" Problem** at 3 h/6 h: `MEASURE_AT_MOST` (3 h) is shorter
   than a Long part (480 min). Fix: refresh `measuring_since` on each heartbeat/re-Begin, or raise the cap
   above the longest part.
C5. **A computer that compared nothing can pass** (rows `compared == 0` / `!in_step` for the rest of the
   test after a lost command). Fix: a Problem when rows after settling are `!in_step` or compared 0 while
   the primary played, for more than a few seconds.
C6. **Release notes say "signed and notarised" whenever `mac_unsigned` isn't true**, even when the cert or
   API key is missing (ad-hoc or signed-not-notarised builds). Fix: a tagged build without `mac_unsigned`
   fails the macOS job if identity or key is missing; or record what was built and choose the paragraph
   from that.
C7. **A lost clock reads "Clock error up to 1000 ms"**: `report_ms` caps at 1000 and `clock_kept` accepts
   `<= 1000`. Fix: `<` or a cap above the lost threshold.
C8. **`MediaServer` can reach a new computer before any `MediaOrder`** (`manager.rs` ~1032-1047). Fix: hold
   `MediaServer` for a new peer until its order went, or send the order whenever new.
C9. Minor: `fleet.rs` `file_name` collisions (non-ASCII -> `_`, two computers of one name share
   `computers/<name>/`); `gui::logs_inside` claims every log is inside when one is missing; `log_since` after
   a rotation reads the old offset in the new file; a 4 MB `LogTail` frame could delay pings past 6 s.

## B. Graphics, windows, UI, display computers

B1. **A hard cut now shows a gap with neither picture** (confirmed by trace; `app.rs` ~2386-2389, also
   ~2336, 2364, 2601-2612, 2423). The new cue's layer is left out until `is_filled`, but the cue it replaces
   is already gone on that tick, and the job is still sent: black (or what's underneath) until the next pass,
   ~1 cut in 4 at 60 Hz, and as long as the uploads thread is held up (the very 100-180 ms Mac waits it was
   built for). Same for a video texture re-made after a seek to another size, a still at another size, a
   display computer's card (each text change makes a new texture: the waiting card blinks), the fps label.
   Fix: write a new texture's first picture on the main thread where it is created and use the thread for
   later frames; keep the old card/label texture until the new one is filled.
B2. **A restarted uploads thread loses first pictures for good** (`app.rs` ~2206-2218, `upload.rs` ~139-156):
   pending jobs die with the thread but the main thread recorded them as sent; never filled, never shown,
   `uploads_waiting` stays true (4 ms loop). No restart backoff; `NEWQ_TEST_PANIC` has no uploads kind.
   Fix: on restart clear `textures`, `patterns`, `fps_labels`, `play.cards`; restart at most every 1-2 s; add
   an `uploads` panic kind and a test.
B3. **Card and fps-label textures from the old device survive a rebuild** (`app.rs` ~1253-1256): `watch_gpu`
   clears `textures`/`patterns` but not `fps_labels`/`play.cards` -> wgpu validation errors per frame, card
   not drawn until its text changes; now hit on purpose by the software->hardware switch. Fix: clear both.
B4. **Only the uploads thread polls for a lost device** (`app.rs` ~1207, `upload.rs` ~253-262): stuck in
   `write_texture` on a dying driver, nobody polls. Fix: uploader stamps an atomic per poll; the main thread
   polls itself when the stamp is over ~1 s old.
B5. Minor: `watch_software` on a machine with no card makes a fresh wgpu instance every 30 s all run (slow
   down/stop after a while when no loss happened this run); `Gpu::new` sets `GPU_NAME`/`SOFTWARE` before
   `request_device` succeeds (stale status on failure); `App::new` doesn't keep `start_at_login_cleaned` if
   prefs can't be saved, so every start removes an entry the user ticked since.

## A. Show network and the engine's network paths

A1. **A sound that follows another cue can lose its first 40-250 ms in a show network** (`engine.rs`
   `ENDED_LATE_MAX` 0.25, `chain_backdate`, `net_backdate`, `keep_to`): what follows is anchored up to 250 ms
   back, and an anchored sound more than `SOUND_SLACK` (40 ms) behind skips forward; a picture's first frames
   are never shown. Fix: cap the chain backdate at ~2-3 frames (0.05 s), or `SOUND_SLACK` when the cue has a
   voice; a larger overrun starts at the tick as in 0.24.2.
A2. **A several-part test with a 0.24.2 follower drops its session in a loop**: `NetTest::Whole` is sent to
   any peer; 0.24.2 has no catch-all. Fix: send `Whole` only to peers on 0.24.3+ (`PeerView.version`).
   (Test Mode agent.)
A3. **The 4 MB log frame can get the follower dropped on a slow link** (< ~5.5 Mbit/s): `last_rx` is set only
   per whole message. Fix: send the log in <= 256 KB pieces, or refresh `last_rx` on bytes read. (Same as
   C9's last point; Test Mode agent.)
A4. **Under network load, "screens apart" measures heartbeat delay** (`seen_offset`): any wait in the TCP
   session or main loop counts as clock error. Fix: while locked, bound the heartbeat-derived offset by the
   clock's own error, or report it as heartbeat delay. (Joins C3; Test Mode agent.)
A5. **Forward-compatibility gaps**: `paths::AdapterKind`, `timeline::TimelineKey`, `timeline::PauseFreeze` are
   reachable from `Msg` with no `#[serde(other)]`. Fix: catch-alls or lenient decoding.
A6. **A restarted display computer can end with a modal dialog and no NewQ** (`main.rs` 91-147) if the old
   copy takes > ~28 s to exit. Fix: with `RESTARTING_ENV` or `--display`, keep waiting and never show the
   dialog.
A7. Small: `net_shift_shared` doesn't shift `net_ended`; the load server never prunes ended connections
   (fd leak per reconnect); a cloned `node.json` on two machines (same id, same port) isn't detected as a
   twin (a beacon with our id from an address not ours).
   Also: a CHANGELOG line that the first start of this version removes start-at-login entries once (re-tick
   display computers before a show).
