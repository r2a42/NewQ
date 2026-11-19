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
