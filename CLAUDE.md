# NewQ

A cue-based playback system for theatre and live events, in the spirit of QLab.
Rust, with FFmpeg run as subprocesses, wgpu for compositing and egui for the UI.
The README describes the features; this file is about working on the code.

## Layout

- `src/show.rs`: the show data model and `.newq` (de)serialisation, plus the
  group, arm, cue-list and audio-output structure helpers. Cue lists: `Cue::list`
  (0 = main) and `Show::lists`; the cues of a list are kept together in
  `Show::cues`, the main list first (`normalize_lists` repairs a loaded show), so
  `next_top`, `move_cue` and `insert_slot` stay inside a list. The engine's
  `playhead` is the active list's; the other lists' are in `list_playheads`.
- `src/engine.rs`: GO, pre-waits, continues, transitions, fades, groups, preload.
- `src/net/` (stage 1 of `.claude/notes/network-plan.md`; results and what remains in
  `network-sync-trial.md`): `clock.rs` a follower's shared clock from NTP-style exchanges (pure;
  weighted fit by round trip, drift, slews once locked, never back), `msg.rs` the wire format
  (clock ask/reply, commands at a shared time, ack, heartbeat), `sim.rs` and `trial.rs` (tests
  only) a simulated network and three `Engine`s; the hour-long report is the ignored
  `the_hour_long_report`. The app uses it through `net/play.rs` (below). `Engine::tick_shared(show, now)` ticks by
  a shared time: cues get `ActiveCue::anchor` and runs `Run::anchor`, clips `clock_offset`, and
  their times are then the clock's (`ActiveCue::pinned`), not sums of ticks or the sound device's
  count; `tick` alone (the app) is unchanged.
  `src/net/store/` is the media store (stage 3; rulings R4, R13-R20; not wired in yet): `manifest.rs`
  (`Manifest`: show path, `rel` to the show folder, size, SHA-256, `missing`, `library`; id = hash of
  the entries without times; built on a thread by `Hashing` from `Sources` with a `HashCache` keyed by
  path, size and mtime; `Lookup` matches a path exactly, then ignoring case, then by the longest `rel`
  tail, for paths from another system), `local.rs` (`Store`, a receiver's objects `objects/ab/<sha>.<ext>`;
  partials in `partial/`, never reachable by `resolve`: **a partial file is never handed out**; an object
  is indexed only after its hash matched and the rename; `Policy` keeps N versions or replaces; `plan`,
  `gc`, `verify`), `primary.rs` (`Primary` serves files where they are; `watch` re-hashes a changed one
  and publishes a new version), `http.rs` (`Server` on std sockets with Range and a rate cap,
  `transfers()` for the primary's notice; our own client with an idle timeout per read), `fetch.rs`
  (resume, verify while writing, next source on failure), `pull.rs` (`Puller`: follows the primary's
  current version in the caller's order, `set_order`; a new order is taken up when a transfer ends, at
  most once a second; one put-aside only when the top file waits behind full slots for `URGENT_WAIT`;
  the library after the used files; `availability` and `Progress` by bytes, `Progress::sum`, answered
  from memory). Tests over loopback with fake bad sources; no real network, multi-GB file or full disk.
  Computers and roles (stage 2a): `node.rs` this computer's `Identity` in `node.json` (log folder, or
  `NEWQ_NODE_DIR`; never in the show): `NodeId`, `Role` (Alone/Display/Primary/Backup), `on`, `network`,
  `primary`, the passcode's `Key` (HMAC proofs over a nonce) and `term`; a corrupt file is put aside. The
  pure rules: `judge` (first primary wins, own network only, passcode if held, highest term; a primary
  obeys only a newer one of its network and becomes `Backup`), `adopt`, `release`, `set_role`,
  `switch_off`; `--join`/`--primary` (`apply_startup`). `proto.rs`: frames (4-byte length + JSON `Msg`,
  `MAX_FRAME`), `PROTOCOL`, a lenient first `Hello` (`mismatch_sentence`), `Msg::Unknown` for messages a
  later NewQ adds, `Beacon`, `StatusReport` (monitors, sound devices, disk, `Link`; clock/show/media
  fields unfilled). `session.rs`: a reader and a writer thread per TCP connection, bounded queue (a full
  one drops the peer), 5 s write timeout. `discover.rs`: a UDP beacon each second to every port of
  `[NEWQ_NET_PORT or 47820, +8)` on each adapter's broadcast address, loopback and addresses added by
  hand; each NewQ binds the first free port (UDP and TCP the same number), so several on one machine
  find each other. `hub.rs`: the `net-hub` thread; **nothing is opened until switched on** (D1); the
  primary dials its computers and the waiting ones; Ping each second, 6 s of silence closes; followers
  report `Status` each second; `NetView` is published on a change with the app's `wake`;
  `Hub::send_to`/`broadcast`, messages the hub doesn't handle come out of `take_events` as
  `NetEvent::Message`, a role change as `NetEvent::RoleChanged`. Tests in `net/hub_tests.rs` (two and
  three hubs on loopback, `lan: false`). App: `App::net`, `pass_net`, `sync_audio_outputs`
  (`AudioOutputConfig::device_here` -> `AudioOut::sync_placed`; an output given only to other
  computers opens no device). Show (D5): `OutputConfig::computer` (`is_on(me)`; another computer's
  display opens no window here, `route_outputs`), `AudioOutputConfig::computers`/`devices_on`
  (`set_plays_on`), unwritten when unused. UI: `src/ui/network.rs` (Settings ▸ Show network, `badge`,
  the status bar, the Identify overlay), the Displays menu's other computers' monitors
  (`remote_display_status`), the audio output's "Plays on"; tests in `ui/network_tests.rs`. Only
  loopback and xvfb have been tried.
  `src/net/media/` is the media manager (stage 2b media; rulings R4, R13-R20). `mod.rs`:
  `local_path(&Path) -> Local {Here|Transferring(Transfer)|NotFound}`, the one way the engine, the probe
  cache and the waveform reach a file; with no finder it is one atomic load (alone, primary).
  `set_finder`/`finder()` hold a follower's `StoreFinder`, which answers from the `Puller` and only ever
  hands out `Store::resolve`; `StoreServes` serves a follower's store to the others. `manager.rs`:
  `Manager` (`App::pass_media`, its own block called from `pass_net`) decides Off/Primary/Follower from
  the `NetView`. Primary: hashes the show's used files and `Show::library_folders` on a thread
  (`media-hashes.json`, FFprobe `Facts` per digest in `media-facts.json`, re-made once the files have
  rested `settle`), publishes, `Server` (`NEWQ_MEDIA_RATE`), `Primary::watch`, sends
  `Msg::MediaServer{port, sources}` and `MediaOrder` to each computer, builds `MediaView.lacking`/`sum`
  from the reports. Follower: `Store` at `<node dir>/media` opened on a thread, its own server, a
  `Puller` on the primary plus complete followers, `MediaPolicy` kept in `media-policy.json`
  (`PolicySetting.folder` is the hook for choosing the store folder), `MediaVerify`, `MediaReport` in
  `StatusReport.media_progress` (via `Hub::set_media`). `order.rs`: `order` and `signature` (R17), pure.
  `words.rs`: the card text, progress, policy and notice wording, pure. `card.rs`: `draw` and
  `info_from(facts)`. Manifest entries carry `facts` (`Facts::of`), written only when known. Engine:
  `engine/cards.rs`: in `prepare_from` a file not `Here` gives `PreparedKind::Card` (a picture; black
  with `Show::no_transfer_cards`) or `Silent` (`ActiveKind::Wait`), with the length from the facts;
  `ActiveCue::card` (`CardState`) is redrawn about once a second (`tend_cards`); once the file is
  `Here`, `take_up` starts decoders `TAKE_UP_LEAD` ahead as a `Seek { keep_time }` that `apply_seek`
  turns into Video or Audio (**never a partial file**, R20). Preloads skip files that aren't here;
  `ProbeCache` asks through the finder (`via`, `through_store`, `read_here`). Tests:
  `net/media/tests.rs` (loopback hubs), `engine/cards_tests.rs` (real media, fake finder), media rows in
  `ui/network_tests.rs`. UI: `ui/net_media.rs` (the Show network page's media rows and policy, the
  primary's notice, `cue_mark`, the library on Settings ▸ Media, the missing-media note), and
  `MediaView.library_files` in the timeline bin's Files shelf. Tried headless with two NewQs on one
  machine; no real network, no take-up of a timeline clip or a cue on several displays.
  Playing together (stage 4a; D6): `src/net/play.rs` and `src/engine/net.rs`. The engine's entry points
  are a gate (`Engine::gated`, `NetGate`, `NetMode` Off/Primary/Follower): alone nothing is caught; on a
  primary an input from outside the engine becomes a `play::Command` (`net_take`); on a follower it is
  dropped; calls the engine makes to itself are never caught (`enter`/`leave` around `tick` and
  `net_apply`; `net_settle` each pass). `Leader` times commands `lead_for(rtt, wifi)` ahead (50-400 ms;
  at least 300 on Wi-Fi), never before an earlier one, does its own at the same shared time (`step`:
  `tick_shared(at)`, `net_apply`, `tick_shared(now)`), resends unacknowledged ones every `RESEND` until
  `GIVE_UP`, and sends a `PlayState` heartbeat every `BEAT` and after each command (`Engine::net_state`:
  cues not on a timeline with their shared anchors, runs, playheads, the show's hash). `Follower`:
  `receive` gets the cue ready (`load_ahead`) and acks; `step` does what is due by the shared clock,
  backdating a late command (`net_apply(late)`), and holds commands while its show hash isn't the
  primary's; it compares with a heartbeat only when no command at or below the heartbeat's `seq` is
  waiting and none above it has been done; `Engine::net_correct` acts only on a difference seen at two
  heartbeats in a row (`NetCorrection`), never on a cue within `SETTLING`: it starts missing cues part
  way, stops extras (a carded cue of unknown length ends this way), re-anchors (or seeks past
  `JUMP_PAST`) and puts runs right (`RunFix`). `net/time.rs`: `local_now()` is monotonic, counted from
  the wall clock at start; the primary answers `ClockAsk` on its beacon socket (`discover.rs`); a
  follower's `Keeper` thread (`keep_clock`, `Hub::clock()`) asks at 10 Hz until locked, 2 Hz after.
  `proto::Msg::{Play, PlayAck, PlayState, ShowPart, Restart, StartAtLogin}`; `Hub::tell(to, msg, urgent)`
  (`Session::send_urgent`, written ahead of other frames); `Hub::set_sync(SyncFacts)` fills the
  `StatusReport`'s `clock_locked`, `rtt_ms`, `show_hash`, `in_step`, `standing`, `start_at_login`.
  `src/app/play.rs`: `play_tick` replaces `engine.tick` (`mode_of(NetView)`: a primary with no adopted
  computer is Off, so alone it is `tick` as before); `play_send_show` sends the whole show JSON 150 ms
  after a change (`show_parts`, 256 KB pieces; FNV `show_hash`) and again to a computer whose reported
  hash differs; `take_show`/`UiState::took_synced_show` install it, kept as `synced-show.newq` in the
  node folder and opened at start-up; `sends_here`: remote input, feedback, clock triggers, streams,
  send cues and timecode are the primary's alone (R8), `camera_missing` for cameras. Display computers
  (D8): `is_display_computer` hides the control window (`keep_control_hidden`), gives every monitor an
  output window (`reconcile_outputs`) and draws `card`/`card_text` per monitor until in step; Identify
  flashes the name. `src/autostart.rs` writes start at login (pure contents tested; only Linux's was
  run). `AudioOutputConfig::device_here`: no computers named = the primary only. `ui/network.rs`:
  `locked`/`locked_banner` (a follower's actions are filtered to `Action::Net` and an edit is put back
  within 0.25 s; greying alone doesn't reach every panel), `step_words`, `blend_warning`, Restart,
  Starts at login. Test Mode is refused in a network. Tests: `net/play_tests.rs` (simulated, 2-3
  engines; its `World` is the harness to grow), `net/hub_tests.rs` (loopback clock and show sync),
  `engine/cards_tests.rs`, `ui/network_tests.rs`. Tried headless with two NewQs on one machine (output
  windows did not go full screen there: no window manager); never on two computers. Not done: an
  ordered group on a late joiner, timeline jumps got ready on arrival, sound resampled to the clock.
  NewQ Display: `src/bin/newq-display.rs` (std only; `default-run = "newq"`) finds NewQ (`find`, pure:
  `NEWQ_PROGRAM`, beside itself, on a Mac `NewQ.app` beside its bundle, /Applications, ~/Applications,
  else `open -b`) and starts `newq --display`. `node::Startup::Display`/`display_startup`/`Applied`: on and
  waiting, an adopted display or backup kept, a running primary refused (one switched off released), with
  `--primary` neither. `Playing::display` -> `play::display_computer` (waiting counts) and
  `display_windows`: no splash, no recovery offer, Ctrl/Cmd+Q or closing an output quits (`quits`); the
  card's `hint`. `autostart::args(display)` writes `--display`. `play::vouched_for`/`add_vouched`: a
  primary adds, once a run, a waiting computer its open show names (never one with a network).
  Packaged as `NewQ Display.exe`, `newq-display` (+ `.desktop`), `NewQ Display.app` (signed, stapled).
  Tried under Xvfb with openbox (full screen); without a window manager it stays 960x540 (winit's X11
  fullscreen needs one). Mac and Windows parts only read and cross-clippied.
  Take control (stage 5; D7, R7): `node::take_control` (Backup -> Primary, term above any seen),
  `remember`/`forget` and `Identity::known` (a primary's computers with their last address, dialled at
  start by `Hub` `recall_known` even without beacons; `keep_known` each tick; `NetAction::Forget`);
  `judge` breaks a tie of terms by the higher id. `hub.rs`: `NetAction::TakeControl`
  (`State::take_control`: carries the follower's `ClockHandle::continued()` into the hub's
  `time::PrimaryClock`, which the beacon thread answers clock asks with (`time::answer`) and
  `Hub::primary_now` gives the app; closes every session so the new primary dials), an old primary that
  judges a newer one steps down and closes its sessions; followers keep their shared clock when the
  primary changes within one network (`clock_network`); `Following::quiet_secs` (`QUIET_AFTER`);
  `NetView::took_control_from`; a peer heard again after a silence is dialled at once.
  `app/play.rs::play_switch`: Follower->Primary and Primary->Follower skip the pass through Off (anchors
  kept); the new primary lets the old primary's queued commands go (`Follower::take_queue`), the one
  stepping down does its own at once (a send fires once); `UiState::took_control` (unsaved, `show_path`
  None: Save asks). Media: a primary keeps a follower's store when it takes control (and reopens
  `<node dir>/media` when `me.term >= 2`): `store::Primary::set_store`/`inherit` (entries missing on disk
  taken from `Store::find_latest`, served from the object, not re-hashed by the watch),
  `media::PrimaryFinder` (store object, else "not found" if the old primary had it, else the path),
  `MediaView::lacking_here`/`from_store`. UI: `network::locked_banner` second row (`quiet_words`,
  `take_control_button` with a confirm kept in egui memory), *Forget* on an offline row, the took-control
  note. Tests: `net/play_tests.rs` (`World` has `primary`, `dead`, `Ev::TakeOver/StepDown/Return`,
  `worst_gap`), `net/hub_tests.rs` (takeover, the old primary back, a restarted primary re-finding its
  computers), `net/media/tests.rs::after_taking_control_the_media_is_served_from_the_store`,
  `ui/network_tests.rs`. Tried headless with three NewQs on one machine (takeover, the old primary back
  as backup, Restart from the primary); never on separate computers. Known: a backup with no clock lock
  at takeover starts from its own clock; the old primary, now backup, copies the media into its store; an
  output that names the old primary stays with it.
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
  After the first test on two real computers (0.24.2). Sound in a network: an `ActiveKind::Audio` cue is
  anchored like any cue; `Voice::keep_to(at)` passes over (`skip_left`, applied by `mix_voice`, the lead
  first) what a voice is more than `audio::SOUND_SLACK` (40 ms) behind, and never holds it back;
  `ActiveCue::sound_zero` -> `CueNow::sound` -> `measure::sound_apart_between` (`sound_apart_ms`). By a
  shared clock the picture's time is `elapsed - seek_base - video_lag` (no audio lead). Test:
  `engine/net_sound_tests.rs` (`AudioOut::with_manual_device`; 0.24.1 gave 123 ms "apart" with identical
  picture times: the real test's 118 ms was the sound cue, not the pictures). **R21 (owner's ruling): the
  network is a reference; a picture never waits for or jumps at the primary.** `net_correct` leaves a cue
  under `LEAVE_UNDER` (1.5 frames) alone, acts only after `STEADY_BEATS` (4) heartbeats within
  `STEADY_SPREAD`, at most once per instance (`NetCorrection::fixed`/`said`); `Follower::step` with no
  shared clock ticks on by its own. Tests: `play_tests.rs` R21 block (`World::state_offset/state_jitter/
  state_delay/silence`, `jumps`). Drawing rates: `render::FrameTimer` counts idle time between frames
  (4 fps had read as no late frames); `WaitLog` (one line per 5 s per window; `stuck` -> Mailbox where
  offered; `NEWQ_PRESENT` overrides); `MonitorReport::drawing_fps/late_permille` (`proto::slow_below`),
  `measure::MonitorRate`, `net_judge::drawing_findings` (a Problem under half the refresh), `ui/network::
  slow_drawing`; `Gpu::new` logs every adapter. Unproven: why a Windows laptop's built-in panel drew 4 fps
  (the wait is `get_current_texture`; guess: hybrid graphics) and whether Mailbox cures it.
  `src/net/paths.rs`: which address reaches a computer: `kind_of(adapter name)` (Wired/WiFi/Vpn/Other),
  `rank`/`best` (our wired subnet, Wi-Fi, private, other; a VPN or 100.64/10 last; link-local last). The
  hub keeps every address a beacon came from (`Peer::heard_from`) and uses the best for sessions, the
  clock and media; no beacons on VPN adapters; a session on a worse path is logged (`say_worse_paths`),
  not cut. `StatusReport.path`/`vpn_on`, `NetView.reached`/`adapters`; `ui/network::{address_text,
  peer_address, own_addresses, vpn_words}`. Never tried with a real VPN. `Show::wait_cards` (`WaitCards`
  UntilReady/AlsoUnused/Never; `play::{still_ready, card_shows}`): `Playing::ready` latches once in step
  with the media here, so a screen with nothing playing stays black between cues (the card used to flash
  whenever `in_step` flickered); set on the Show network page beside `no_transfer_cards`.
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
- Timelines (plan and rulings: `.claude/notes/timeline-plan.md` and `timeline-plan-delta-1.md`, later
  sections win; section U is I2's). A timeline is a "start all together" group that is not a slide show
  (`TimelineKey::Group`), or a list with `CueList::timeline` (`List(n)`, never list 0). `src/timeline/`:
  `mod.rs` the file's types (`TimelineSettings` with `start_on_fade_in`/`crossfade_overlaps`, both on in
  `for_editor()`, off for an old group; `Marker`, `TimelinePoint`, `RunOut`, `PauseFreeze`, `TrackId`;
  all `#[serde(default)]`, unwritten at their default, so old shows are unchanged; the four
  `ControlAction::Timeline*` variants are what 0.20.3 can't read); `model.rs` the pure read model
  (`clips` with each clip's lead-in and `clocked` (a free-run pause or its own Pause inside it; a loop
  pause doesn't count), `tracks`, `ghosts`, `cuts_over`, `lead_in`, `fade_wake`, `key_of`; `members` is
  structural only; `Dur` is the engine's `cue_duration`, and only its Group arm adds the lead-in);
  `edit.rs` pure edits that say what they changed (move, trim, split, ripple delete, add clip/control/
  pause-before/track/marker, `set_fade`, `set_opacity`, `move_track` (swaps the layers of two picture
  tracks), `place_at`, `paste_clips`, `duplicate_clips`; `split` leaves fade cues on the first half and
  makes the second visible, `starts_visible`; a move onto a display in `also_on` takes it out of the
  list; `add_clip` gives a new still, text, shape, clock or timer `STILL_LEN`, wherever it is made); `snap.rs`; `chase.rs` works out what plays at a time (seek, parking, Jump; `overlap` and
  `crossed_by` for overlap crossfades, per display), pure, its own tests.
  `src/engine/timeline.rs`: a `Run` per timeline in `Runs.map` (clock `pos`, `RunState`
  Stopped/Cueing/Running/Paused, `RunView` for the UI, `pub use`d by `engine.rs`); waiting clips are
  `PendingCue`s tagged with the run; at equal times control cues go first (`model::due_order`), so a
  pause stops the clock before anything after it; a clocked clip's end and fade-out come from the clock
  (`ActiveCue::ends_at`, `run_out` when its content runs out first; a detached clip set to Loop goes
  round, `timeline_go_round`); a seek cues for up to `CUE_LIMIT` while running, else parks (paused,
  picture on the outputs; a seek to exactly 0 parks with nothing up, so the editor seeks to
  `engine::FIRST_FRAME`, as a remote's jump does; `Run::parked`, `RunView.parked`); the picture changes all at once
  (`Run::outgoing`, `timeline_unseen`, `timeline_settle`; clips alive on both sides are jumped in place,
  `timeline_carry`); jumps are got ready ahead (`timeline_jump_wanted`, `Prepared::fits`); overlaps
  crossfade (`timeline_overlap`, `ActiveCue::crossed_by`) and abutting pictures are held over the join
  (`timeline_joins`, `JOIN_GAP`, `ActiveCue::joined`); a loop pause (`ControlCue::loop_from`,
  `leave_at_once`, `is_loop()`) is `Run::looping` (`timeline_loop`, `timeline_leave_loop`);
  `timeline_waiting`/`timeline_detached` feed the editor's hatching. The active panels show a
  clocked clip set to Hold or Loop on the clock (`ActiveCue::panel_times`); one set to End by its own time. A clip's keys read at
  `ActiveCue::key_time()` (its run's clock less `clip_zero`; `elapsed` off a timeline). Ramps a run
  starts are tagged (`Ramp.run`, `Param::follow`) and move only as far as its clock does
  (`Runs::steps`); a run's fade cue keeps its clock's time (`fade_from`), and its matrix crosspoints are
  held in the mixer while the clock stands (`timeline_hold_crosspoints` -> `Voice::hold_crosspoints`,
  an atomic); untagged (by hand, off the timeline, a Stop's fade) move with show time. The Stop cue
  fades the run out over `ControlCue::fade` (`stop_fade()`: None = `TIMELINE_STOP_FADE`, 3 s; 0
  cuts; a second Stop cuts; `RunView.fading` meanwhile, "Stopping · fading out"). What GO does is
  `Engine::go_plan` (`GoPlan`, `TimelineGo`: Start, Resume, Play, Running, Cueing, LeaveLoop, AtEnd (a
  list stopped at its end, `Run::at_end`, until a clip is after the playhead); `go` and `timeline_go`
  follow the same `timeline_go_for`), which the transports, the new interface and the web remote's `go`
  read out (`GoPlan::describe`, words from `model::on_go_text`). Tests in `engine/timeline/tests.rs`
  play real media (`color_red.png`, the test-pattern camera); live sound under a freezing pause is
  untested (no device).
  `src/engine/displays.rs`: a picture on several displays (`MediaCue::also_on`; read only through
  `MediaCue::displays(show)`, the home first). `ActiveCue::screens` holds per display `{ output, exit,
  gone }`, kept in step with the show each tick (`displays::sync`); `Engine::layers(output)` uses
  `level_on(output)`; `apply_entry` replaces or crossfades a cue only on the entering cue's displays
  (`cut_from`, `crossfade_from`), so it stays on the others; gone from all, it ends
  (`gone_everywhere`); its one voice plays until the last full display goes (`leave_sound`). Fades, Stop
  cues and a timeline Stop act on the whole cue. The decode size is the home's or the largest open window
  among `also_on` (`picture_size`); text and shapes are drawn at the home's size. Tests in
  `engine/displays/tests.rs` (text cues, no media); no real displays tried.
  `src/ui/timeline.rs` and `src/ui/timeline/`: the tracks editor (`editor`, hosted as `Pane::Timeline`
  in the dock, as a bottom panel of the new interface's Editor view, as a timeline list's tab, and as
  the workspace; `Host` salts its ids); it takes `&Show` and asks for `TimelineAction`s that `apply` makes after the
  frame (edits refused in show mode and tests by `edits_the_show`); a drag lives in `EditorState` and is
  asked for once on release (one undo step). `tracks.rs` clips, rows, drags, ghosts, the overlap's
  crossfade mark, a loop pause's section (`loop_sections`, shaded on the Cues track and the ruler) and the
  vertical-overflow bar (a lane just opened is scrolled into view,
  `EditorState::reveal`; fade handles only on the hovered or picked clip; a moving run's playhead is
  kept in view by `tracks::follow` (`Prefs::timeline_follow`: `Page`, or `Centre`, which overrides a user
  scroll; `TimelineAction::SetFollow`, Settings ▸ General), not while a drag or the scroll bar is held; the Cues
  track's no-time cues are `tracks::chips`, narrowed to the next, those nearer than `MIN_CHIP` one chip
  whose click picks each in turn, names `fitted` to the next and drawn whole on top when hovered or
  picked, `cue_labels`; the ruler's flags are cut to the next the same way); `ruler.rs` ticks, markers (grouped
  into one "×N" flag by the Cues track's `tracks::chips`, generic over the id; a click picks each in
  turn, `EditorState::marker_pick`),
  the playhead (a ruler tap or drag seeks on release; a drag is cancelled when the run's
  `RunView::mark` changes meanwhile, `Drag::Playhead { pos: None }`; locked in show mode for a group); `toolbar.rs`
  transport and read-outs (`state_text` from `RunState` and `RunView.parked`; two rows under
  `ONE_ROW_W`; the state text comes last on its row so the buttons don't move); `control.rs` the
  inspector rows (`inspector_rows` at the end of the inspector, `settings_fields`); `menus.rs` the clip,
  track and cue-track "+" menus; `lanes.rs` the speed and opacity keyframe lanes under a clip
  (`EditorState::lanes`; a key drag is drawn from a copy of the show with the keys in, `lanes::preview`,
  so the bar's length and lead-in follow; both on the timeline's time axis, speed keys following the
  content and opacity keys the clock, ruling L2); `keys.rs` Ctrl/Cmd+K (split at the playhead), dropped
  files and the new-clip file dialog. `ui::keyboard` runs before anything is drawn, so the editor that
  holds the keys (`EditorState::focus`, last clicked, or hovered) swaps the `Action::Delete`, `Cut` and
  `Duplicate` it already made for its own (`TimelineAction::Delete`/`Cut`/`Duplicate`) when every picked
  cue is one of its clips, and `Paste` for `TimelineAction::Paste` (at the playhead, `paste_into`) when
  the clipboard holds no group and nothing outside the timeline is picked; Space is `Action::Space` (the
  hovered editor's timeline), `ui::space_key`: `timeline::space_target` (the focused editor still showing
  its timeline, `EditorState::drawn`, else the hovered one, else `list_tab::active`) gets
  `space_action` (Pause while running outside a loop or cueing, else `TimelineAction::Go` =
  `timeline_go_one`), none is GO; `0` is GO (`ui::go_key`; output windows too); and an `Action::AddFiles` for
  `AddClip`s when the files were let go over a track (exactly one handles a drop). A clip on several
  displays has a "+N" badge and ghosts. Double-clicking a timeline group's row opens it rather than
  firing it. `list_tab.rs`: a timeline list's tab: `cue_list` calls `list_tab::draw` after the tabs, which
  draws the editor (`Host::ListTab`, it doesn't pin) in place of the rows unless
  `EditorState::list_rows` (set on going to the tab while the classic Timeline pane is open, so the pane
  shows the tracks); the pane otherwise says the tracks are in the tab. `add_cue`/`AddFiles` on the active
  timeline list go to `list_tab::add_clips` (at the run's `pos`, in turn where lengths are known,
  `fitting_track`); a paste there or a `MoveToList` into a timeline
  list lands at its playhead (`edit::place_at`); groups are refused there (`NO_GROUPS`: MoveToList,
  GroupCue, Paste, slides, `NewTimelineGroup`). `ui::go_readout` is `list_tab::go_readout` (`go_plan` plus
  the run's next pause or loop); GO on a timeline list keeps the selection; `modern::next_cue` is None
  there and the Operator view adds `operator_state`. Tabs: `+` menu and the tab's right-click
  (`SetListTimeline`, which then `sync_lists`). Layout B, `workspace.rs` (`Host::Workspace`): chosen by
  `Prefs::timeline_layout` (`workspace::chosen`; `TimelineAction::SetLayout` saves it and moves an open
  editor; reached from Settings ▸ General (its first section, two rows: the classic window's settings are
  short while a Timeline pane is open), the Window menu under Timeline (with the follow mode too) and
  the toolbar's `Workspace`); classic `ui::draw` draws it in `dock_area`'s place while `st.timeline.open` and no
  docked Settings (`replaces_panes`); the new interface's `View::Timeline` (in the strip only for B;
  leaving it is `Close`); `Open` goes to it (and `SelectList` into a timeline list does), the tab then
  keeps its rows (`shows_tracks` false) and its Tracks button opens it; a Timeline pane under B only
  points to it (`workspace::pointer`). `workspace::showing` (the inspector's "tracks are open" instead
  of its Open button) counts a classic Timeline pane only in front of its tabs (`dock::Layout::in_front`,
  read through `EditorState::pane_in_front` while the dock is drawn). Bar (Back = `Close`, `Show as a pane`), tracks across the
  bottom (`TRACKS_SHARE`), left column the monitor (its own paint of `PreviewOutput`, a resizable top panel,
  `monitor_height`) over `bin.rs` (shelves `Shelf` New/Files/Clips, one shown: new clips click to the
  playhead or drag a `BinItem` onto a track: `bin::dropped`, called by `tracks.rs` next to
  `keys::dropped`; the timeline's files; its clips), the inspector in the
  middle with `inspector_rows` first (`ROWS_ABOVE` skips them inside; edits spread as
  `ui::inspector`'s, `user_touched`). The inspector's two columns need about 940 pt, which is why
  the monitor isn't in the middle. Tests in `timeline/tests/workspace.rs` (1280×760, both
  interfaces). The inspector's "Also on" row writes `MediaCue::also_on`
  (home never in it); `bulk_edit::WHOLE` spreads it as one value and `tidy` drops the target's own
  display. Tests drive it with the mouse (`TimelineRects`, `Draw::Full` for the whole window and undo,
  the list tab's `Win`; cfg(test) notes such as `timeline_clip_text` and `timeline_state`); tried
  headless under xvfb in both interfaces.
- `src/media/`: `ffmpeg.rs` (find and probe), `video.rs`, `audio.rs` (devices,
  routing, mixer), `playback.rs` (looping passes).
- `src/gpu.rs`, `src/render.rs`: the compositor and the per-window render threads.
  Every `queue.submit` goes through `Gpu::submit` and every `surface.configure` through `Gpu::configure`
  (`Gpu::gate`, a RwLock): configuring waits for the device to go idle and fails if another thread submits
  meanwhile; a Mac whose external display went off left the control window undrawn for 34 s.
  Texture uploads happen on the main thread (`app.rs::update_outputs`) and wgpu holds each
  one's staging copy until the next `queue.submit`; a Mac stops presenting a background
  window, so nothing else submits: `Gpu::flush_uploads` after each frame's uploads (this
  was 81 GB in a Medium test run in the background).
- `src/awake.rs`: `KeepAwake` holds the computer awake (caffeinate, `SetThreadExecutionState`,
  systemd-inhibit) while a test runs; `SleepWatch` notices a sleep by the wall clock running ahead of
  `Instant`, which stops in sleep on a Mac and Linux (Test Mode reports it and leaves the laps it
  fell in out of the lap checks).
- `src/splash.rs`: the startup splash window, drawn on the processor with `softbuffer` (`compose`
  makes the picture and is tested; the window code is in `Splash`). `app.rs::resumed` opens it,
  and the main window's set-up (`open_main_window`, the slow part) begins after the splash's first
  redraw event so it is on screen first; `about_to_wait` closes it (`splash::due`).
  `NEWQ_NO_SPLASH=1` skips it. Not tried on a real Mac or Windows desktop.
- `src/macwin.rs` (Mac only): an output window covers its display as a borderless window above the
  menu bar's level, on all Spaces, stationary, no shadow (winit's simple fullscreen hid the menu bar
  for the whole app, only auto-hid it, and left the window to Mission Control; it stays the fallback).
  Checked for the Mac target with `scripts/cross_clippy.sh mac`, not tried on a Mac.
- Starting video and audio: FFmpeg is spawned by the reader thread (`video.rs::start`, `spawn_hap`,
  `audio.rs`), not the main loop: a process start took 3 to 4 s now and then on a Mac. A start that
  fails shows as `error()` and no frames. `Engine::update_preload` also preloads the children of
  the group that an auto-follow leads to (`MAX_PRELOAD_CHILDREN`), so a group of layers starts warm.
  `crumbs.rs` leaves a note after each part of the main loop; a stall over 100 ms is logged with them, the main
  thread's CPU time (`thread_cpu`), a sleeping thread's lateness (`pulse_late`: whole program held or not), the
  engine's own crumbs (`Engine::crumbs`, `tick_breakdown`, slowest cue) and the slowest picture made, and kept for
  Test Mode (`note_stall`/`take_stalls`, with `past_minute`). `Children::close` warns if it takes 30 ms.
- Display loss: `app.rs::control_for_outputs` treats the main window as being on no display while its
  remembered display (`control_home`) is disconnected (a lid closed), so an output stays on its display
  even though the system moved the window there. `HomeTracker` (app.rs) makes a display the window's home
  only after it has stayed there 10 s and never while the home is disconnected: a closing lid moves the
  window to the other display a moment *before* the system reports the first gone.
  An output window is kept when its display only changes number (`renumbered`; the lid closing makes the
  external display 0): closing and recreating it showed the desktop. Its render slot is its own (`ScreenWindow::slot`).
- Output windows are kept over their display (`app.rs::pin_outputs`, each pass, Mac): `macwin::frames`
  compares the window's frame with its screen's (both global points) and `monitor::should_pin` decides;
  `macwin::watch_screens` (NSApplicationDidChangeScreenParametersNotification) sets a flag and wakes
  the loop so the look at the displays is prompt; the 2 s poll is the safety net. `pin_outputs` runs before
  and after `scan_monitors` (which the system can block for 200 ms while rearranging), `pin_output(m)` runs
  on the output window's `Moved`/`Resized`, at most once per `monitor::PIN_SPACING` per window. The window
  is backed with black (`NSWindow::setBackgroundColor`). The 0.5 to 0.7 s freeze at a lid change is the
  system holding the next drawable (`nextDrawable` blocks, wgpu has no timeout); nothing to do about it.
  Not tried on a Mac (checked by `scripts/cross_clippy.sh` and CI).
- `src/freeze.rs` (Mac): `about_to_wait` beats; a watchdog thread runs `/usr/bin/sample <pid> 2 -file
  newq-freeze-<time>.txt` in the log folder when the beat stops for 1.5 s while outputs are open (at most 5
  a run); Test Mode copies them into its run folder (`copy_samples`). `macwin::hide_pointer`
  (`NSCursor::setHiddenUntilMouseMoves`) is called when the system says the displays were rearranged and
  after each put-back, since the pointer shows on the output when a lid closes.
- `src/power.rs`: on battery or not (pmset, GetSystemPowerStatus, /sys/class/power_supply) on a thread of
  its own; `app.rs::keep_awake_for_outputs` holds `KeepAwake` while an output window is open
  (`Prefs::let_computer_sleep` turns it off), logs sleeps (`SleepWatch`) and the power source, and sets
  `UiState::keeping_awake/on_battery` for the status bar and the General page's badge.
- Panics are lived through, not fatal (`crash.rs`): `survive(what, f)` is a `catch_unwind` that the main
  loop wraps each pass in (`about_to_wait` -> `App::pass`, with the parts the rest can do without, the
  engine tick and the outputs each in their own `App::part`) and each window event (`after_window_panic`
  -> `UiState::after_panic` lets go of the selection and open dialogs). The hook writes a report for the
  first two panics at one place and 40 a run, then only counts; a panic that is survived or on a worker
  thread shows no message box and becomes a status-bar notice (`take_notices`). Only the main thread
  outside `survive` is fatal; `open_main_window` reports a failure to start itself and exits. Worker
  threads say they died with `crash::OnPanic` (the video/audio readers set their cue's error, a stream
  reports a failure and is retried); `watch_render_threads` restarts an output or control render thread
  that ended (at most one restart every 2 s); `mix_each` drops a voice that panics in the audio callback
  (a panic leaving a CoreAudio callback aborts; the MIDI input callback catches its own for the same reason);
  `output.rs::lived_through` keeps the send-cues threads alive; `update_hosting` replaces a dead MediaMTX runner.
  `NEWQ_TEST_PANIC=<engine|ui|outputs|render|audio>@<seconds>` makes one happen. Never take time from an
  `Instant` (`Instant::now() - d` panics soon after boot): use `Option<Instant>` or `checked_sub`.
  A `clamp` whose bounds come from a show or a file panics when they are inverted or NaN (`ui.rs::within`).
- `src/reaper.rs`: no program NewQ starts outlives it. `reaper::start` (main.rs) runs a copy of NewQ as a
  watcher (`newq --reap <pid>`) with a pipe; `reaper::watch(&child)` after each spawn (playback, stream,
  waveform, probes, MediaMTX) gives it the pid; the watcher notes it only if it is running as a child of that
  NewQ and named ffmpeg/ffprobe/mediamtx/newq, with its start time; when the pipe closes (NewQ ended in
  any way) it ends the noted ones still running with the same pid and start time, and nothing else. It also
  writes them to `<log folder>/started/<NewQ's pid>.txt` (first line the NewQ itself); `sweep`, at the next
  start, ends what the records of NewQs no longer running list, by the same rule, and removes those records
  (for when NewQ and its watcher went together). Unix only (a Mac has no
  `PR_SET_PDEATHSIG`; FFmpegs waiting on a capture permission were left after a crash). Tried on Linux
  with `kill -9`; not on a Mac.
- `src/logging.rs`: the log. `LogLevel` (`Prefs::log_level`, Settings ▸ General ▸ Recovery and logs;
  `set_level` applies it at once) or `NEWQ_LOG` (`Spec`: a level and `part=level` pairs; it wins, and the
  setting is greyed). Always to `newq.log` in the log folder (and stderr except on a Windows release),
  through a bounded queue to a `log-writer` thread (`write_loop`; a full queue drops and counts), put
  aside as `newq.earlier.log` at 20 MB (`Rotating`; the last run's stays `newq.previous.log`). Libraries (targets not `newq::`) are a step quieter
  (`for_libraries`). Levels: `Engine::problem` (warn) for what went wrong and `Engine::log` (info) both
  show in the status bar; debug is each cue's start/playing/out/end (`Engine::log_cue_changes`) and each
  FFmpeg's command line; trace is preloads and FFmpeg exits. The panic hook flushes the log.
- `src/app.rs`: windows and wiring. `src/ui.rs`: the control UI.
  Group crossfade: in a "play in order" group a child whose next sibling crossfades in (`crossfade_into_next`) is not faded out by itself, and `advance_groups` starts the next one that long before it ends; a held next cue (`holds`: picture, text, shape, camera) gets the overlap added to its hold (`Engine::started_early` -> `ActiveCue::extra_hold`, which the per-tick hold update adds back), so the group keeps its timing; a video can't, so `cue_duration` subtracts a crossfade into a video, and a next child with a pre-wait doesn't overlap. Shape cues: `show::ShapeCue` (`ShapeKind`), drawn by `shape::render` at the display's size on a thread (`Engine::prepare_shape`, which hands the picture over as `PreparedKind::Text`, like a text cue). Editing several cues: `inspector` wraps `inspector_one` and `spread_edit` makes in the other picked cues what changed (`bulk_edit::spread` compares the cue as JSON before and after, skipping the fields that are a cue's own, `target` and `device` included). A list missing from the file is empty, so a list that appears or empties is a length change and isn't spread; an item with a `kind` (an effect) is addressed as `<index>#<kind>` and changes only the same kind; `start`/`end` reach only the same cue type (`SAME_KIND_ONLY`); `inspector` spreads only on a frame with user input. `Engine::tick` copies the show's layer, geometry, colour, effects, fit, output and hold time into the cues playing (the block under `if let Some(m) = media`), and a shape or typed text is drawn again when `drawing_key` changes (`ImageState::redraw`); what is fixed at start (file, in/out, loop, speed, audio routing, fade-in) is not.
  Selection state: `multi` only counts while `selected == multi_for` (`sync_selection` at the start of `draw` and `apply`, and `forget_multi` in `load_into`, `restore` and the test stash), so an old selection never returns when ids repeat in another show. The clipboard remembers the show it came from (`show_epoch`) and whether it was cut: `paste_cues` remaps a fade/stop/control cue's target to the copy, clears targets from another show, and gives a cut its old ids back. The right-click menu's Fire/Fade out/Stop name the clicked cue (`FireCue` etc.), not `selected`. Copying also writes a short text to the system clipboard, since egui-winit sends Paste only when it holds some text. In a group, `crossfade_into_next` applies only to pictures on the same display and layer, and the automatic fade-out waits for the crossfade only while it is coming or running (`running` set in `tick`).
  Layers: `MediaCue::layer` (higher on top; `Engine::layers` sorts by it); with `Show::own_layers` (new shows; a file without it, from 0.18.0 and before, has it off and replaces everything on the display as it did) `apply_entry` replaces or fades only the cues on the entering cue's own layer and display (`layer_no`, `ActiveCue::own_layer`), a dip's matte replaces all. The children a "start all together" group fires share the display (`together`): the group records them in `Engine::together_for`, read at each start attempt (a start can wait for its file and come back) and removed when the start ends, so a child fired on its own later replaces as usual. Copy and paste: `UiState::clipboard`, `Show::copy_cues` and `Show::paste_cues` (new ids, display/audio output mapped to the pasting show's); Ctrl+C/X/V are `Event::Copy/Cut/Paste`, handled in `keyboard` after the typing check.
  The cue list picks several cues (`UiState::multi`, only while `selected` is still `multi_for`, so any other way of choosing ends it; read it with `UiState::selection`): Ctrl/Cmd-click toggles, Shift-click takes a run of visible rows. Rows drag with egui's drag and drop (`DraggedCues` payload); the drop spot is worked out per row (top half before, lower half after, a group's middle into it) and `Show::move_cues` (`DropAt`) does the move, keeping groups whole and children in their groups. Delete, Duplicate, Move and arm act on the whole selection; so do `GroupCue` (`Show::group_cues`) and `MoveIntoGroup` (`move_cues` with `DropAt::Into`) from the cue menu, which keeps the selection when you right-click one of its cues (`cue_context_menu`'s `picked`). Tests drive the list with the mouse (`ListMouse`, rows recorded under `cfg(test)`); not looked at on a real screen beyond that.
  `src/ui/modern.rs`: the optional new interface (`Prefs::new_layout`, Window menu; `Action::SetNewLayout`): `draw` replaces the transport and dock when it is on, with `View`s Editor (cue list, inspector, active), Operator and Patch (painter-drawn signal flow; a drag from a cue port sets `MediaCue::output` or `audio_output`), reusing `cue_list`, `inspector`, `active_panel` and `output_previews`. The settings take the view's place while open. Tests draw each view and drag routes; `PatchRects` is test-only.
  `src/ui/settings.rs`: the settings page (a sidebar of `SettingsPage`s in
  groups, with badges; it replaces the cue list while open). A setting goes on
  the page it belongs to; the displays, audio and sub display panels are
  functions in `ui.rs` that the pages call.
- `src/dock.rs`: the pane layout (an `egui_dock` tree plus the popped-out panes),
  saved per computer as `layout.json` in the log folder, not in the show.
  `ui.rs::dock_area` draws the tiles; each pop-out is an OS window in
  `app.rs` (`PopoutWindow`) with its own egui context and render thread, drawn by
  `ui::draw_popout`. Opening a surface while other windows draw can fail to
  configure, so `Gpu::configure_checked` retries. A lost device drops the
  pop-outs and `reconcile_popouts` opens them again from the layout.
- `src/ndi.rs`: NDI in and out. The runtime (NDI's own library, not shipped) is
  loaded with `libloading` only inside a helper that is NewQ's own program run as
  `newq --ndi-list|--ndi-receive|--ndi-send` (see `main.rs`), so its faults can't
  take the show down; pictures cross over stdin/stdout. An NDI source is a camera
  cue's device (`ndi:` + the source's name), and `StreamKind::Ndi` is the output.
  The structures follow NDI's SDK 6 headers (not vendored: their licence);
  tests build a fake runtime in C with `cc` (skipped without one) and the pure
  converters are tested, but no real NDI has been tried.
- `src/media/hap.rs`: Hap frames (section header, Snappy, chunked); FFmpeg only
  demuxes (`-c copy`, `video.rs::spawn_hap`) and the texture data goes to
  `LayerTexture::new_bc/upload_bc` (BC1/BC3, Hap Q's YCoCg is shader mode 2). Needs
  `TEXTURE_COMPRESSION_BC` (`video::plays_as_hap`; `Gpu::new` sets the flag); a
  speed is in the frames' times. `media/convert.rs` makes Hap copies; `bench.rs` is
  `newq --bench-video` (headless; `Gpu::headless`). Tests compare our decode to
  FFmpeg's own for every format.
- `src/speed.rs`: keyframed playback speed. `SpeedMap` integrates a cue's speed
  (`MediaCue::rate`, an `Animated`) into file time and back; `Stretcher` is a
  WSOLA time stretcher the audio reader runs along a `SpeedPlan`; the video
  reader (`video.rs::read_at_speed`) picks frames by the same plan. A steady
  speed still uses FFmpeg's `atempo`/`setpts`. Loops follow the plan too: the readers
  see the passes as one run of frames, and `ActiveCue::pass_len` is then in the
  file's seconds (`pass_time` maps the cue's time into them).
- Animated geometry: `show::Motion` (six optional `Animated` tracks on
  `MediaCue::motion`, evaluated in `Engine::tick`); a fade cue's `geometry` ramps an
  `ActiveCue::live_transform` (`LiveTransform`) that then wins over the tracks.
  Fade cue `matrix` targets set `VoiceShared::xp_target/xp_rate` (up to
  `audio::MAX_FADEABLE` crosspoints, kept in `Matrix::keys` order); `mix_voice`
  moves the gains per buffer. Silent crosspoints are kept in the matrix for this.
- `src/feedback.rs`: OSC feedback to the address in `RemoteSettings::osc_feedback`
  (`snapshot`/`changes`/`repeat` are pure and tested; `Feedback` paces and sends). Timelines:
  `timelines_to_tell` (every timeline list, plus `Engine::timelines`: runs not stopped or still fading),
  `state_word`/`state_text` (the web remote uses them too), `/newq/go` from `go_text` (`GoPlan::describe`).
- `timecode::Generator` makes MTC messages; `output::MtcRunner` is its paced thread
  (`ActiveKind::Timecode`, dropped to stop). Send cues' `Program` target runs only
  when `Prefs::allow_scripts` (per computer; `output::set_scripts_allowed`).
- `src/testmode/`: Test Mode (Help menu), no command line. `assets.rs` is the pinned
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
  `TestMode::choose_pick`; its "Show the test on" ticks are `Prefs::test_displays`;
  `plan::use_displays` replaces the show's outputs with them before the cues are made, the first
  display shows the picture and the others mirror it with a whole-area `part_of`; the
  main window's own monitor is never offered). Timeline tests: `Flavour::Timeline` beside each `Level`
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
  `DROPOUTS_PER_HOUR`, `DROPOUT_BURST`); stats.csv's last column is `audio_errors`. Engine test: Timeline short at 2x through an `Engine` (~30 s). The full check (`feature.rs`, chosen as "Debug test" beside
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
  real device. Cross-target clippy for Windows and Mac runs from Linux with
  `scripts/cross_clippy.sh` (a stand-in C compiler for `ring`, since nothing is linked); CI still builds and
  tests them.
- `src/clicker.rs`: presenter remote. `decide` is the pure GO/Back logic (Back pauses, Back again
  fades out and stops, GO resumes; `BACK_GUARD` ignores a double press); `Shared` is the learned keys
  and learning flow; a Windows low-level keyboard hook and a Mac `CGEventTap` (Input Monitoring) feed
  it with `Command::Clicker`, the in-window `KeyboardInput` is the fallback. Keys are per computer in
  `Prefs`. Not tried on a real Mac or Windows machine. **Switched off** (`clicker::AVAILABLE`): it
  started the global capture at every launch (a Mac would ask for Input Monitoring) and can't tell the
  remote from the laptop keyboard; see `.claude/notes/next-ideas.md` for the device-aware plan.
- `src/pdf.rs`: draws a PDF's pages as 1920-pixel PNGs with `hayro` (pure Rust, no unsafe, MIT/Apache; the
  PDF's own fonts, Foxit substitutes for the standard 14; no encrypted PDFs). A page that panics is an error
  naming it, not an abort. Compared against poppler on a LibreOffice-made deck: the same pictures.
- `src/pptx.rs` and `src/slides_export.rs`: slides from a presentation. `pptx.rs` reads the slide
  order, hidden slides and first transition (advance and fade) from the XML (pure, tested);
  `slides_export.rs` makes pictures: a PDF through `pdf.rs` (no program needed), PowerPoint (PowerShell
  COM), PowerPoint/Keynote (AppleScript), LibreOffice (makes a PDF that `pdf.rs` draws), or
  `NEWQ_SLIDE_EXPORTER`; `match_timings` pairs pictures with slides.
  `ui.rs` (`start_deck_dialog`, `start_deck_import`, `poll_deck`) runs it on a thread with Cancel and
  `Action::ImportSlides` -> `Show::add_slides_timed`. Not tried against a real office program.
  A deck with no advance times imports with `SlideShow::advance = Advance::OnGo`: the group fires only
  its first slide (`GroupState::deck`), `Engine::deck_step` fires the next or previous child (a fresh
  instance, which crossfades over the older ones: `apply_entry` only fades out layers started before it),
  and `Engine::go` steps it before it fires anything else; past the last slide the deck lets go
  (`DeckStep::AtEnd`). `ui::remote_button` is the one handler for the presenter remote's buttons and for the
  show-mode arrow keys (`keyboard`, and `app.rs::handle_output_key` for the output windows, which
  maps the winit key to egui's and calls `ui::output_key`; hotkeys in both through `ui::hotkey_cue`).
- Mac signing: `release.yml` imports a Developer ID certificate into a temporary keychain when the
  secrets `MACOS_CERT_P12`, `MACOS_CERT_PASSWORD` (and, to notarise, `APPLE_API_KEY_P8`,
  `APPLE_API_KEY_ID`, `APPLE_API_ISSUER_ID`) exist; `package_release.sh` then signs ffmpeg, ffprobe
  and the app (hardened runtime, `assets/macos/entitlements.plist`), notarises with `notarytool`,
  staples and zips. Without the secrets it ad-hoc signs as before. A step's own `env` can't be used in
  its `if`, hence the `signing` step's outputs. Works: used for every release since 0.16.0 (the
  notarisation wait is about 30 minutes).
  When Apple's servers are down, dispatch `release.yml` itself with `tag` and `mac_unsigned: true`:
  the Mac download is ad-hoc signed and `scripts/release_notes.sh` (used by `prepare` and `publish`)
  puts the matching macOS paragraph on the page (`<!--mac-signed-->`/`<!--mac-adhoc-->` blocks in
  `.github/release_notes.md`). 0.24.2 was released that way.
- Windows signing: the `Sign newq.exe` step in `release.yml` runs only with the secrets
  `WINDOWS_CERT_PFX` and `WINDOWS_CERT_PASSWORD`. Untested, and a placeholder: new certificates
  are hardware-backed and can't be exported as a .pfx (SignPath or a cloud service is the way).
- `src/stats.rs`: the `NEWQ_STATS` CSV log and `--stats-report`. `src/soak.rs`: builds the
  self-running soak show: `newq --soak` makes the media with our FFmpeg, runs the app on it
  and writes the report; `--make-soak-show` writes just the show. sysinfo
  lists Linux threads as processes (`thread_kind()`), skip them when summing children.
- `src/fx.rs`: cue effects. `show::Effect` (kind and up to four `Animated`
  numbers, each a value or keyframes) is on `MediaCue::effects`. `fx::VideoFx::at`
  turns the picture effects into the numbers in the compositor shader (blur,
  sharpen, pixelate, negative, vignette in `gpu.rs`, one pass, a test renders them);
  the sound effects (`AudioChain`: biquad EQ/filter, compressor, echo, freeverb)
  are built when a voice is, kept in step with the show by `FxShared` atomics
  that `Engine::tick` sets from the cue's time, and run per frame in
  `audio.rs::mix_voice`. `render.rs::FrameTimer` and the mixer callback's load
  feed the status-bar warnings.
- `src/remote.rs`: OSC and MIDI. Timelines: `/timeline/<name>/<verb>[/<place>]` parses to
  `Command::Timeline { timeline, does: TimelineDo }` (`timeline_command`, shared with the web
  remote's paths; a place is `Place::Time`/`Marker`, from the first argument, `OscValue`, or a fourth
  part); `run_timeline` (called by `ui::run_remote`) finds it (`find_timeline`: a reference `list:<n>`/`#<id>`/
  `cue_id:<id>` first, then of `timelines_named` a list (first tab) before a group (worked list, main,
  any); a shared name is said in the status bar; feedback's `timeline_address` falls back to the
  reference where a name isn't one timeline's alone) and calls the engine
  (`timeline_go_one`, `timeline_continue`, `timeline_leave`, `timeline_run_from`, `timeline_seek`, a
  stop with `TIMELINE_STOP_FADE`). `remote::fixtures` is a timeline show for tests (here, feedback,
  web, ui). `src/text.rs`: text cue rendering.
  `src/history.rs`: undo (JSON snapshots of the show, bursts merged).
  `src/timecode.rs`: MIDI Time Code decoding and the trigger follower.
  `src/output.rs`: send cues' OSC and MIDI messages and the sender thread.
  `src/projector.rs`: PJLink (commands, MD5 digest, the conversation; tested
  against a fake projector on loopback). Projector and TCP sends run on a
  thread per host from `output.rs`.
  `src/export.rs`: the cue sheet as CSV or a printable page (a timeline clip's Start, Track and Also
  on columns, its pre-wait left empty; marker rows, `Row::marker`, after the timeline's last clip).
  `src/clock.rs`: time-of-day triggers (local time via `jiff`).
  `src/crash.rs`: the log file and crash reports (Windows has no console).
  Reports go to the log folder (`%LOCALAPPDATA%\NewQ` etc.); ask for
  `newq-crash.log` and `newq.log` when a user reports a crash.
  `src/web.rs` and `web_page.html`: the web remote (HTTP on plain `std`
  sockets, handlers tested as functions and over loopback). `/api/timeline/<ref>/<verb>[/<place>]`
  (`timeline_command_for`) and the state's `timelines` (`timelines_json`: ref, state, clock, length,
  next pause, markers), drawn as a card each by the page's `renderTimelines`.
  Displays: the show has `DisplaySlot`s (Display 1, 2…) each assigned to a
  monitor by the user; `show.rs::slot_state` finds the monitor and never picks
  another, `app.rs::route_outputs` places outputs, and `Show::assign_displays`
  gives older shows (Auto/Display placements) their displays once.
  `assets/logo/`: the logo (SVGs, PNGs, `newq.ico`, `newq.icns`, the raw 128 px
  window icon `src/branding.rs` embeds, and the 1280x640 GitHub social preview).
  Change the artwork in `scripts/make_logo_assets.py` and re-run it (it needs
  fontTools, node with playwright and ImageMagick; see its header), then commit
  what it writes. `build.rs` puts `newq.ico` in the Windows executable; the
  packaging script puts `newq.icns` in the Mac bundle and the icon and a
  `.desktop` file (`assets/linux/`) in the Linux download.
  `src/mesh.rs`: the mesh warp (a Hermite spline through a grid of points);
  the vertex shader in `gpu.rs` does the same sums, cutting each layer into
  a 48x48 grid of cells, and a test renders it and compares.
  `src/blend.rs`: the edge blend maths (test-only reference for the blend
  shader in `gpu.rs`). A display can show a part of another's picture
  (`OutputConfig::part_of`): `app.rs::update_outputs` draws the other
  display's layers through that part; windows drawing the same layers need
  different `Slot::Window`s or their uniforms overwrite each other.
  `src/stream.rs`: sending a display's picture out through an FFmpeg process
  (argument building, `Sink`); `render.rs::CaptureThread` draws the picture off
  screen at the stream's rate and feeds it. A test streams to a file through
  real FFmpeg; the Linux virtual camera itself can't be tested here.
  `StreamKind::Server` (RTSP/RTMP/SRT to MediaMTX and the like) takes its H.264 encoder from
  `stream::encoder` (libx264, then VideoToolbox, NVENC, QSV, AMF: listed *and* proven by a short
  test encode, once per run), and `missing_for` checks the muxer and protocol (the Mac bundle has
  no libsrt, so no SRT). The Mac FFmpeg is built with network on and the `mpegts,rtsp,rtp,flv`
  muxers for this; `package_release.sh::verify_ffmpeg` checks them. Tests publish to FFmpeg's own
  RTSP/RTMP listeners. A hardware encoder must be checked for its keyframe spacing, not just that it
  encodes: AMF's `lowlatency` usage sent one keyframe ever (`ffprobe -show_entries packet=flags`).
  A stream's FFmpeg must never be waited on without a limit: `Sink::finish` kills it after
  `finish_patience(kind)` (3 s; a Custom output gets 60 s for closing a file), `CaptureThread::stop`
  ends it through a `Killer` when the thread is stuck writing to it (a hung server fills the pipe),
  and on Windows it is in a kill-on-close job (`mediamtx::tie_to_us`); on Linux and a Mac the
  watcher in `reaper.rs` ends it after a crash. The stress tests (`mediamtx/stress.rs`, `gpu.rs::stress_stop_…`) are
  `#[ignore]`d; run them with `NEWQ_MEDIAMTX_DIR` and `NEWQ_FFMPEG_DIR` set and `--ignored --nocapture`.
  sysinfo only fills `Process::cmd()` when asked (`ProcessRefreshKind::with_cmd`); without it a
  "no FFmpeg left" check passes for the wrong reason.
  `src/mediamtx.rs`: NewQ runs MediaMTX itself (`StreamKind::Server::host_here`): `locate` (next to the
  executable, `NEWQ_MEDIAMTX_DIR`, `third_party/mediamtx`, PATH), `Hosting::config` writes its YAML (listen
  address of the chosen adapter, per-protocol ports; publishing only from loopback and the adapter, read for all,
  `overridePublisher: no`), `Hosting::conflict` finds two ways in on one port, `Hosted` is the child process
  (killed on drop; Linux `PR_SET_PDEATHSIG`, a Windows job object, so a crash leaves none; its last lines say why it
  stopped; output appended to `<log folder>/mediamtx/mediamtx.log`). Each run has a folder named for its pid with
  `mediamtx.yml` and `server.pid`; `stop_leftovers` kills a server whose NewQ is gone (or is us) and only if the pid
  is still the MediaMTX exe. `Runner` is the worker thread (start/stop/retry every 10 s; status in `Status`).
  `plan`/`align` make every display that runs its own server agree on one adapter and port set (first wins).
  `app.rs::update_hosting` works out the plan each frame, settles a change for 1 s, refuses a missing adapter and a
  computer without `Prefs::allow_hosting`, and sets `UiState::hosting`. Real-server tests need
  `NEWQ_MEDIAMTX_DIR` (they pick their own ports; WebRTC's and RTSP-over-UDP's are settings for that).
  `src/streamlink.rs`: network adapters (`if-addrs`), the player/protocol links, the QR modules (`qrcode`),
  and `probe_server` (TCP connects; SRT is UDP so unchecked). The UI is `ui.rs::server_share_panel`.
  An adapter can't make FFmpeg bind its outgoing socket (RTSP/RTMP/SRT have no such option), so it picks the
  listen address, the publish address for a local server, and the address in the links. Tests: the real
  server and a real publish/play-back run when `NEWQ_MEDIAMTX_DIR` (or PATH) has `mediamtx`, else skip.
  `scripts/mediamtx_pins.txt` pins the bundled version, its mirror release and SHA-256s; `package_release.sh::bundle_mediamtx`
  fetches it from this repository's mirror (a universal `lipo` binary on the Mac, signed with the others).
  `src/pattern.rs`: the twelve test patterns (`show::PatternKind`), drawn on the
  CPU; `app.rs` caches one per display by kind, number and size.
  `src/live_text.rs`: clock and speaker timer text (the engine redraws the
  picture as the shown time changes; the renderer re-uploads on `version`).
  `src/relink.rs`: finding moved media and copying it into one folder.
  `src/gpu.rs` also notices a lost graphics device (`DeviceLost`) and
  `app.rs::watch_gpu` rebuilds every window's device, surface and render thread;
  `NEWQ_LOSE_GPU_AFTER=<seconds>` destroys the device on purpose to test it.
  `src/color.rs`: colour adjustment and chroma key maths; the shader in
  `gpu.rs` does the same per pixel (a test renders it on a real GPU if there
  is one; `NEWQ_REQUIRE_GPU=1` makes having none a failure).
  `src/monitor.rs`: what a display is called (a Mac's name for it) and where a
  window starting on it goes. winit's Mac `name()` is "Monitor #" and a number,
  and its `position()` is in that display's own pixels, so convert to points.
  `src/recovery.rs`: autosave copies. `src/media/camera.rs`: camera cues'
  device lists and ffmpeg input: `CaptureFormat` (asked for as `-pixel_format`, `-vcodec` or `-input_format`
  per system), the device's modes (`list_modes`, run on a thread by `Engine::query_camera_modes`; parsers for
  dshow, v4l2, avfoundation and decklink output, tested on sample text only) and Blackmagic `decklink:` devices
  (listed only when the FFmpeg in use has the `decklink` input: NewQ's bundled one can't, the SDK terms stop us
  shipping one; `CameraCue::mode_code` is the `-format_code`). Automatic names the format the device offers (`resolve_format`), never waiting: `modes_known` answers from its cache and asks on a thread, the answer used from the next preload (it is in the camera's `PrepSig`). None of it has met a real capture card. A display's Send out can also go to a Blackmagic device (`StreamKind::DeckLink`: `-pix_fmt uyvy422 [-s WxH] [-r N000/1001] -f decklink <device>`, devices from `camera::list_decklink_outputs`; same FFmpeg-build limit, also untried on hardware). A display's Send out can record to a file (`StreamKind::Record`: `record_path`, `crop_filter`, `hevc_args`; H.265 encoder found by `hevc_encoder`; closing the file gets the Custom-sized `finish_patience`, and `App::exiting` stops every stream so a quit closes it; the file is made first by `reserve_record_file`, so names never collide; inputs are stamped with `-use_wallclock_as_timestamps` and the output is `-fps_mode cfr`). Its sound is a tap on an audio device (`AudioOut::tap`: `TapSlot` is try-locked in the audio callback, never blocking it; each tap has an id so a recording ending cannot clear its replacement, and a device that comes back at the same rate keeps it) that `stream::AudioPump` hands to FFmpeg over a local TCP socket as f32le (FFmpeg connects), encoded to AAC; silence fills a quiet device so FFmpeg never waits. `src/media/waveform.rs`: peaks for the
  inspector's waveform.
  The `NewQ test pattern` device stands in for a camera in tests.

## Build and test

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings      # CI fails on any warning
NEWQ_REQUIRE_MEDIA=1 cargo test                 # over 700 tests, about 3 minutes once built
```

- The tests decode real media with FFmpeg: `scripts/make_test_media.sh` makes
  `test_media/*.mp4|png|wav` (git-ignored). `NEWQ_REQUIRE_MEDIA=1` makes a
  missing ffmpeg or clip fail the tests instead of skipping them.
- Linux needs `libasound2-dev libudev-dev libxkbcommon-dev pkg-config ffmpeg`.
  In cloud sessions the SessionStart hook (`.claude/hooks/session-start.sh`)
  installs these, fetches the crates and makes the test media.
- Use `git grep`, not `grep -r`: `test_media/` and `target/` are big.
- Hardware can't be tested here (audio devices, MIDI ports and timecode sources,
  cameras, real displays). The
  logic is tested without it: `Engine::for_tests()`, the fake `Device::for_test`
  in `audio.rs`, and the pure parsers in `remote.rs`. Say so when something was
  only tested that way.

## Conventions

- **Show files stay backwards compatible.** A new field is `#[serde(default)]` and
  is left out of the file when it has its default value, with a test that old
  files load and that the default isn't written.
- New features get tests, a README entry (and a keyboard-table row if they have
  a key), and are checked by running the app where the UI changed.
- **`CHANGELOG.md`** gets a line under `## Unreleased` with each change a user would notice, in
  words for people who run shows. A release moves them under `## X.Y.Z`; `scripts/check_release_tag.sh`
  (the dry run) refuses a version with no section, and the Release workflow puts the section at the
  top of the release page (`scripts/changelog_section.sh`).
- Commits are one feature each. PRs are merged with a **rebase merge** to keep
  `main` linear. After a merge, delete the session branch on GitHub (or let the repository's
  "Automatically delete head branches" do it) and restart it from `main`: a rebase merge gives the
  commits new ids, so a branch left behind holds old copies and the next push needs a force-push.

## Running the app headless (cloud sessions)

```sh
sudo apt-get install -y xvfb xdotool imagemagick libxkbcommon-x11-0 libegl1 \
  libgl1-mesa-dri mesa-vulkan-drivers libvulkan1
WGPU_BACKEND=vulkan NEWQ_MUTE=1 xvfb-run -a -s "-screen 0 1500x900x24" \
  sh -c "target/debug/newq show.newq & sleep 9; xdotool mousemove 85 70 click 1; \
         sleep 2; import -window root shot.png"
```

There is no window manager, so keystrokes go nowhere until the window has focus.
Clicking the GO button (85, 70) fires a cue; for keys, focus the window and send
them to it: `W=$(xdotool search --name NewQ | head -1); xdotool windowfocus $W;
xdotool key --window $W space`.
Don't record audio with an ALSA `file` plugin: the null device isn't paced and
writes gigabytes in seconds.

## CI and releases

- CI runs fmt, clippy, tests and the real-media tests on Ubuntu, Windows and
  macOS, once per change: on pull requests (a newer push cancels the older run)
  and on `main` after a merge. A rebase merge of a pull request that CI passed on
  has the same tree, so its `main` run skips the builds (the `gate` job) and still
  counts for `Cut release`; a merge after `main` moved on is tested in full.
  Slow steps are capped (`apt` once stalled for 40 minutes). A branch has no CI until it has a pull request. Changes
  that only touch `*.md` or `LICENSE-*` skip CI; `scripts/check_main_ci.sh` (used by
  `Cut release`) accepts such commits on top of a commit CI passed on. Occasionally the macOS runner never starts and the job is cancelled
  after 15 minutes with no steps run; that's not a failure of the change. Re-run it.
- **Releasing:** follow `.claude/skills/release/SKILL.md` (the whole sequence, the traps,
  and how to keep going without being poked). In short: bump `version` in `Cargo.toml` (and `Cargo.lock`) in a PR, merge,
  and wait for CI to pass on `main`. Then, as the repository owner, comment
  `/release vX.Y.Z dry-run` and then `/release vX.Y.Z` on any issue or PR. The
  `Cut release` workflow checks the tag against `Cargo.toml`, tags `main` and
  starts the `Release` workflow, which builds and publishes the downloads.
  (It can also be run from the Actions tab, or by the owner's token.)
  A suffix (`v0.3.0-rc1`) makes it a pre-release.
- **FFmpeg for Windows and Linux is pinned to one build** (`scripts/ffmpeg_pins.txt`):
  a dated BtbN build, copied into a release of this repo (`ffmpeg-<version>`,
  not a NewQ release) so it can't change or vanish upstream, and checked against
  its SHA-256 while packaging. Nothing moves it but a deliberate change: to get a
  newer FFmpeg, run `scripts/update_ffmpeg_pins.sh <BtbN autobuild tag>`, commit,
  merge, run the **Mirror FFmpeg** workflow (owner only), then a release dry run.
- **MediaMTX is mirrored the same way** (`scripts/mediamtx_pins.txt`): MediaMTX's own release builds,
  copied into a release of this repo (`mediamtx-<version>`) by the **Mirror MediaMTX** workflow
  (`scripts/mirror_mediamtx.sh`, owner only), which checks each file against the pins and the pins
  against MediaMTX's `checksums.sha256`; packaging downloads from the mirror. To move to a newer one:
  change `version`, `release` and the SHA-256s, commit, merge, run Mirror MediaMTX, then a release dry
  run. `scripts/mirror_mediamtx.sh --check` verifies without uploading.
- Cloud sessions can't push tags (the git proxy only accepts branches), which
  is why releases go through that comment.
- **Claude can't post the comment, but can run the workflow.** The GitHub
  comment tool rewrites slash commands in comments Claude posts (`/release`
  becomes `·/·r·elease`), so the workflow skips them. That looks deliberate, so
  don't work around it. The same workflow also has a manual trigger
  (`workflow_dispatch`, inputs `tag` and `dry_run`) that accepts only the
  repository owner's account. A session with the owner's token (set up with
  `/web-setup`) can start it with `actions_run_trigger` `run_workflow` on
  `cut-release.yml`. Do the dry run first, then the real one, and check the
  result: the `Release` workflow publishes the downloads.
- Large tool outputs: page GitHub list calls (`per_page` of a few).
