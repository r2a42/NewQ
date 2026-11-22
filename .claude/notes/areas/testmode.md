# Test Mode

Covers `src/testmode/`, `src/net/measure.rs`, `src/net/measure_local.rs`, `src/net/load.rs`, `src/stats.rs`, `src/soak.rs`.

Moved word for word from the Layout section of `CLAUDE.md` (commit 2589061). Read this before
working in these files, and keep it true when the code changes. A heading that says "Part of"
is a piece of a longer entry that covered several areas.

## Part of the `src/net/` entry: the network test (stage 6)

  Stage 6, the network test: `net/measure.rs` (pure) is `Msg::NetTest` (Begin{run,start}/End/Report/
  LogTail; marked block in proto.rs) and a follower's `Measurer` (rows every `ROW_EVERY` of clock error,
  rtt, `apart_ms` from `apart_between` at each heartbeat once `Follower::has_done(s)`, commands' `lead_ms`,
  corrections via `correction_ms`, stalls, late frames; a `Report` each `SEND_EVERY`; `log_tail` at End),
  fed by `app/play.rs`'s marked block (`net_test_message`, `net_test_pass` from `play_send_show`, which
  also moves `TestMode::net_in/net_out`; `NEWQ_NET_TEST_AUTOSTART=<n>` for developers). `testmode/
  network.rs`: `refusal` (a primary with a computer `in_step`), `build` (the user's displays and outputs,
  a display per bare follower monitor, `KEPT_LIST` of the user's media disarmed so one-version stores keep
  it, `SHOW_NAME`/`is_test_show`: a follower doesn't keep it), `NetPrep` (assets `MEDIA` and
  `syncflash::make`, frames drawn by `syncflash::frame`), `NetRun` (Stage WaitingForMedia via
  `media_state` and `MediaView.current`, Playing a `script` of `lap()`, Ending; `finish` restores, writes
  report.txt, network.csv, events.log, computers.txt, computer-<name>-newq.log, the zip). `testmode/
  net_judge.rs`: pure judging, constants, `csv`, `report`, `CANNOT_SEE`. Tests: `network_tests.rs` (a
  simulated primary and two followers through `Leader`/`Follower`: late, cancel, drop-out),
  `network_ui_tests.rs` (1280x760 both interfaces), hub_tests' loopback NetTest; `testmode/net_demo.rs`
  (cfg(test)) starts NewQs as children (`NEWQ_E2E_PROGRAM`, `NEWQ_E2E_DISPLAYS`, `NEWQ_E2E_DIR`) for the
  ignored `three_newqs_run_the_network_test_by_themselves`; `scripts/net_demo.sh` the same by hand. On
  one overloaded machine the test said "Problems found" (up to 9 frames apart around a pause and seek,
  174 main-loop stalls): unknown whether real computers do better. The soak's 10 laps were never run.

## Part of the `src/net/` entry: Test Mode's window and several parts

  Test Mode's window and several parts in one test (0.24.2). `testmode/choice.rs` (pure): `Parts` (the Cue
  list / Timeline / Network ticks, kept in `Prefs::test_pick`; `PickFile` reads 0.24's flavour/debug), `Part`,
  `PartPlan`, `plan(level, parts, network_ok)` (Medium and Long are the whole run's time: network laps 1/3/10
  via `network_laps`, the rest shared, at least 20 minutes each), `total_secs`, `summary`, `level_item`,
  `network_reason`, `start_refusal`, `parse_parts` (`NEWQ_TEST_PARTS` for `NEWQ_NET_TEST_AUTOSTART`).
  `testmode/sequence.rs`: `TestMode::start_plan` runs several parts one after another (one part alone runs as
  before); `UiState::test_carry` (`TestCarry`): while `keep` is set `end_test` holds the stash and the next
  `begin_test` hands it back (`release_test`/`holds_test` are the safety net in `sequence_step`); while
  `parts_run` is set a part's `part_bundle` makes no zip; `Sequence::finish` builds one `Done` with `parts:
  Vec<PartDone>`; `gather` (on the zip's thread, `Bundle::start_after`) joins report.txt and events.log and
  moves the other files by `file_names` (a shared name gets the part's slug, except the part's own
  `<slug>.csv`). `network.rs`: `laps: u32` (was `soak`). `gui.rs`: `idle`/`more` (the debug test, the decode
  comparison, downloads, heavier load), `Facts`, `displays_section` (a tick per screen, by computer; all
  ticked on opening when none are; Start needs one), `part_section`, `computers_results`. Tests:
  `sequence.rs`, `network_tests.rs` (`Rig::start_combined`, `TestMode::launch_plan` with `sequence::Made`),
  `network_ui_tests.rs`. A combined Short run was tried headless with three NewQs on one machine; Medium and
  Long combined were never run to their end.

## Part of the `src/net/` entry: `src/testmode/fleet.rs`

  `src/testmode/fleet.rs`: every test across a show network. `session::across` -> `TestMode::spread`
  (`Phase::Spreading`, `Fleet::media_ready`, `waited_too_long`: `GONE_LIMIT` 30 s, `STEP_LIMIT` 90 s,
  `MEDIA_LIMIT` 20 min), then the usual `Run`/`FullRun`; ends through `Phase::Closing` (`ENDING_LIMIT`);
  `fold_in` adds each computer's verdict (`net_judge::judge_computer` plus `LocalNotes::findings`), files
  (`computers.csv`, `outputs.csv`) and report part. `NetRun` uses `Fleet`. Followers measure themselves:
  `net/measure_local.rs` `LocalMeter` (`NetTest::Local`, `NetTest::Mute`). Across computers the ticked
  screens are separate displays and `plan::spread_pictures` sets `also_on` (alone, mirrors as before);
  `DisplayChoice.computer`, `gui::remote_key`, `Fleet::live_lines`. The decode comparison is refused in a
  network; a follower can't start a test (`UiState::following_test` banner). Tests: `network_tests.rs`
  (`Rig` with `paced`, `launch_across`). Run headless with three NewQs on one machine (Short only).

## Part of the `src/net/` entry: the 3D stage part

  The 3D stage part (B5 of `.claude/notes/organise-and-test3d-plan.md`): `choice::Part::Stage3d`
  (`plan_with`/`level_item_with` take its tick, `TestMode::stage3d`, not in `Parts`/`Prefs`: the
  window ticks it on opening from `TestMode::stage_wanted` = `choice::stage_wanted`, set in
  `update_at` while idle; `parse_stage` for `NEWQ_TEST_PARTS`), `Flavour::Stage`, `plan::build_stage`
  (the Short cue lap made to go round, `round_again`; `stage_setup`: the user's screens kept, else
  `ui::stage_edit::use_sample` Proscenium plus a projector a test display; `stage::StageDisplay`
  swapped in by `Setup::apply`: the last ticked monitor's mirror, else a disabled windowless
  display; none across a network). `testmode/stage.rs`: `Leg` Baseline/View/Light/Display,
  `StageRun` (driven from `update_at`; each leg `PreviewGovernor::reset`; `SETTLE`; `LegResult` from
  `shown_totals`, `Tally::since`, `stage_render::totals`), `judge` (Problem only when a 3D leg's
  share exceeds the baseline's by `previews::Settings::line`, decision 11; and a stopped stage
  thread), `in_short`, `report_part`, `stage.csv`; replaces `analyse_previews` for that part.
  `TestMode::holds_previews` -> `App::pace_previews` -> `PreviewGovernor::set_held` each pass
  (`ask` returns paused; `previews::drawn_level` and `update_outputs` treat it as previews off),
  so the hold ends however the test does. The window's view is `gui::stage_running` calling
  `ui::stage_view::draw` with `StageRun::show` (a copy of the live test show: the window gets no
  show) as `stage::VIEW`. Decision 9: `plan::leave_venue_out` in `network::build`, `session::build`
  across and after `full::plan_for`, unless the run has the 3D part (`Prep::keep_venue`). With no monitor ticked `unshow_stage` clears
  every display's source. Tests: `stage.rs` (judge sentences, legs driven), `choice.rs`, `plan.rs`,
  `session.rs::a_3d_stage_part_…`; run headless under xvfb only (lavapipe).

- `src/testmode/`: Test Mode (Tools menu), no command line. `assets.rs` is the pinned
  catalog of free media (hashes, `.part` resume downloads with `ureq`, checks by
  decoding, generated fallbacks named `generated_*`; tested against a loopback HTTP
  server, and an ignored live test `real_downloads_match_the_catalog`), `plan.rs` builds
  the Short/Medium/Long shows from the user's show (keeping its outputs), `session.rs`
  runs one (`TestMode::update` each frame from `app.rs`; always restores via
  `UiState::begin_test/end_test`, writes the run folder, never lets autosave run while
  `test_locked`; nothing slow on the main thread: `MemoryWatch` looks on its own thread, `StatsLog` (also the
  Statistics window's and `NEWQ_STATS`) asks its `stats-sampler` thread for a row and never waits, `StatsLog::with`
  takes a stand-in `Looked`; a Windows hour had sysinfo's process list take up to 3 s), `analysis.rs` turns samples into findings (pure, tested; growth is judged on per-window
  peaks, middle third against last third by medians, so a warm-up step that levels off is only a
  `Severity::Watch` "tracking", which doesn't count against the verdict; `session::FindingLog`
  writes every finding the window shows, and when it goes, to `events.log` and the report), `decode.rs` is the
  hardware decoding comparison (a 4K clip made from the downloaded film, four looping
  tiles played with `video::set_hw_decode` off then on; a cue with a plain cut
  transition takes the place of the layers before it, so tiles use `FadeIn`),
  `gui.rs` is the window (the test chosen is `Prefs::test_pick`, chosen again on opening,
  `TestMode::choose_pick`; its "Show on" ticks are `Prefs::test_displays`;
  `plan::use_displays` replaces the show's outputs with them before the cues are made, the first
  display shows the picture and the others mirror it with a whole-area `part_of`; the
  main window's own monitor is never offered). The 3D stage is kept: `plan::carry_stage` points
  screens' feeds and projectors at the test's displays (the user's in order, extras to the first, a
  sub display its display's, regions kept, a display one projector at most) and, with nothing
  ticked, `unshow_stage` clears the first display's `source`; `use_displays_as` returns the report
  lines for `Builder::notes` (`full.rs` drops them). The Network part (`network::build`) leaves a
  venue model out (decision 9 of `organise-and-test3d-plan.md`; a sample stays; the cue-list and
  timeline parts across a network still send it); `begin_test` ends corner, area and
  multiview-layout edits. Timeline tests: `Flavour::Timeline` beside each `Level`
  (`Flavour::name`, "Timeline medium"; `assets::wanted_for` drops the 5.1 trailer); `plan::build_timeline` (same
  `Builder`, `list` set for a timeline list's cues; `use_displays_as(.., false)`, no mirror; `own_layers` on) makes a
  list "Programme" and from Medium a group "Interval" and a `timeline::Script` (legs, `TIMELINE_HOLD`, loop passes,
  Long's seeks); `timeline::Driver` plays it lap after lap through `go_plan`/`go` (switching the worked list), GO
  after the hold at a pause, leaving a loop just after it went back; `timeline::Watch` notes `Stretch`es (clock vs
  `Instant` while Running; the first tick from idle, dt 0 by `app.rs`'s rule, is skipped), `Arrival`s (a clip the
  running clock crossed, by `Engine::timeline_clips_up`'s `due`, per key) and `CueSpell`s; `analysis::
  analyse_timeline` judges them and lap spread; `timeline.csv`, the report's Timelines part. `session::Run::timeline`
  replaces the first-cue lap logic. `hitches.rs` (every test, the full check too) notes stalls and sound dropouts
  (`audio::stream_errors`, the cpal error callback; `Stats` keeps the last 64 buffers' work and gap for its log line)
  into events.log; `analysis::analyse_stalls`/`analyse_dropouts` judge them (`STALLS_PER_HOUR`, `STALL_PROBLEM_MS`,
  `DROPOUTS_PER_HOUR`, `DROPOUT_BURST`); stats.csv's columns end `audio_errors`, then `preview_level`, `seen_missed_permille`, `stage_ms` (`stats::Previews::now()` on each `Inputs`: the level `App::pace_previews` drew at via `stats::set_preview_level`, `render::shown_totals`, `stage_render::totals`; each `StatsLog` subtracts its own last row's, `row_since` -> `PreviewCells`; blank when not `measured`; `STAGE_MS_OVER` 134 past the last band; a follower's `LocalSample` doesn't carry them yet; the reader takes up each header in a file, and `create_with` writes one when the file's last differs). `previews.rs` (every cue list and timeline test, `session::Run::previews`): `Recorder::start/tick/finish` (the governor's `tally()` cloned at the start, its new steps and news each tick as `previews.csv` rows (`CSV_HEADER`) and `events.log` lines (`step_line`, `news_words`), the windows' pictures while `drawn_level` is Paused in `while_paused`; `finish` gives an `Account`: `Tally::since`, `shown`, `StageTotals::since`, `Settings::of(prefs)`, `open_previews`), `report_part` (the report's Previews part, after the findings) and `stage_lines`; `analysis::analyse_previews` (Note: gave way; not theirs; never allowed to give way and over 5%; Problem: over the line (`Settings::line`) while paused or off, which takes in a not-theirs; nothing at full rate and smooth) and `previews_sentence`. Judged by raw seen misses over the test (the grace after a GO included; `while_paused` leaves `Governor::GRACE` after each `Engine::jolts` change out, `tick` takes the jolts). The debug test and the decode comparison only sample the columns; the network test (`network.rs`) has neither. Engine test: Timeline short at 2x through an `Engine` (~30 s). The full check (`feature.rs`, chosen as "Debug test" beside
  Short/Medium/Long, `TestMode::debug`, `Phase::FullPrep`/`Full`, driven by `full.rs`): `make_media` makes a clip, tone, stills and a Hap
  clip in the run folder, cut from the catalog's Sintel trailer and NASA pictures (`feature::Sources`,
  fetched by `full.rs::real_media` when downloads are allowed, else only from the cache) or plain ones; `build` makes a show with every cue kind and a list of `Part`s (acts at times,
  `Check`s); `Runner` plays them one at a time, its clock starting when the part's `lead` cue is started,
  noting `Obs` every 20 ms (what a part fires later is got ready first, `Engine::load_ahead`, so FFmpeg's start
  time isn't in the part's timings); `judge` (pure, tested on made-up notes) gives each check's `Verdict`.
  Two "Timeline" parts (`timeline_parts`; `Obs::clocks` from `Engine::timeline_clock`, checks `ClockStill`,
  `Frozen`, `Live`, `Never`, `Later`) cover the four timeline actions, both pause kinds (the camera held
  with `freeze_live`), an overlap and a GO-timed crossfade, a jump to a marker and a lead-in; a test
  plays just them through an engine (about 25 s). `ControlAction::ALL` is every action (the
  timeline ones, `TIMELINE`, last; the inspector's menu draws a line before them); the plan test checks
  each is used. A problem
  the engine reports in a part (`Engine::take_problems`) fails it unless the part expects it. The copy has
  `own_layers` on (an old show's off makes cues in a together group replace each other: that is tested
  behaviour, not a fault), no streams, no OSC feedback. `full.rs` does what needs the app: `Ask::RecordOn`
  sets a `StreamKind::Record` on the first display and `judge_recording` probes the file once it has stopped
  growing; `Ask::ServeOn` streams to a server on `streamlink::LOOPBACK` (127.0.0.1, free ports) and asks it
  over RTSP whether the stream is there; skipped unless `mediamtx::locate()` and `Prefs::allow_hosting`.
  `bundle.rs` zips the run folder plus newq.log, newq.previous.log (and their `.earlier` parts) and newq-crash.log beside it
  (`NewQ-test-<run>.zip`, on a thread; every test's `Done::bundle`; with `bundle::TIDY`, off under
  `cfg(test)`, the run folder is removed once the zip holds it), and `reveal` shows it (`explorer
  /select,"…"` needs `raw_arg`; `open -R`; `xdg-open` of the folder). `selftest.rs` is `newq --self-test
  [--out dir]`: the same `Runner` on an `Engine` with `AudioOut::silent()` (a paced mixer thread, no
  device), exit 0/1/2; CI runs it on all three systems and uploads the zip. Engine-level only: "reached the
  screen" means the cue's layer with a frame was in `Engine::layers`, not that pixels were looked at. The
  window's run was tried under xvfb (recording and a real MediaMTX passed); the sound parts never met a
  real device. The full check has three "3D stage" parts (`feature.rs`): `venue_round_trip` (the Proscenium
  sample written as `media/stage.glb` and read back, done in `make_media`, `Check::VenueLoads`), `screens_fed`
  (`Check::ScreensFed`, pure), `Check::StageDrawn` and `Check::LightBuilt`, answered by the driver like the
  recording (`Context::waits_for`, `Runner::set_stage_drawn`/`set_light`). `build` gives the check's show a stage
  (`stage_for_check`: the user's if it has screens, else the sample; a fed screen and a projector on the first
  display either way) and `Plan::notes` (with `use_displays_as`'s) goes into the report. In the window's run
  `Ask::StageOn/Off` (`full.rs::show_the_stage`/`put_back`: the last display, or one added with no monitor) is
  judged by `stage_drawn_verdict` over `stage_render::totals().since`; the light, and in `--self-test` both, are
  drawn by `stage_trial` on a `Gpu::headless()` of its own (`start_stage_trial`, `STAGE_TRIAL_LIMIT`; no card or
  too slow is "not checked" unless `NEWQ_REQUIRE_GPU`); its views count into `stage_render::totals()`. Skipped
  across a show network: the display part. Cross-target clippy for Windows and Mac runs from Linux with
  `scripts/cross_clippy.sh` (a stand-in C compiler for `ring`, since nothing is linked); CI still builds and
  tests them.

- `src/stats.rs`: the `NEWQ_STATS` CSV log and `--stats-report`. `src/soak.rs`: builds the
  self-running soak show: `newq --soak` makes the media with our FFmpeg, runs the app on it
  and writes the report; `--make-soak-show` writes just the show. sysinfo
  lists Linux threads as processes (`thread_kind()`), skip them when summing children.
