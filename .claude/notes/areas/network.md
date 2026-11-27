# The show network

Covers `src/net/`, `src/app/play.rs`, `src/engine/net.rs`, `src/engine/cards.rs`, `src/autostart.rs`, `src/net/load.rs`, `src/bin/newq-display.rs`, `src/ui/network.rs`, `src/ui/net_media.rs`.

Moved word for word from the Layout section of `CLAUDE.md` (commit 2589061). Read this before
working in these files, and keep it true when the code changes. A heading that says "Part of"
is a piece of a longer entry that covered several areas.

See also: the blocks after the real tests also hold notes for other areas: picture uploads and
`adapters::look_every` (`compositor-and-displays.md`), Test Mode across a network and its network load
(`testmode.md`), and the rule for a tagged release build, `NEWQ_REQUIRE_NOTARISED` (`packaging.md`).

- Line-up across the network (mapping plan 4.4, round 2 agent B; tried only on loopback hubs and
  pure state machines, never two real computers and a projector). `net/lineup.rs` (pure, `now` an
  argument): `Msg::LineUp(Box<LineUpMsg>)`, tagged `k`: `Show(Box<Overlay>)` (the whole state every
  time: session, seq, display, pixels, `Mark { n, at, on }` in raw monitor pixels, `active`, `outline`
  (None: keep the session's last), `Under { Black, Dimmed, Unknown }`; every field defaulted), `End
  { session }`, `Seen { session, seq, pixels }` back, `#[serde(other)] Unknown`; an older NewQ reads
  the whole as `Msg::Unknown`, which the hub passes by. `Overlay::sane` drops what can't be drawn
  (non-finite, over `MAX_MARKS`/`MAX_OUTLINE_*`, pixels 0 or over `MAX_PIXELS`: refused). `Shown` (the
  projector's computer): `take(source, msg, monitor, now)` (a display not on a monitor here, a
  seq not newer within the session: passed by; a new session replaces; returns the `Seen`), the
  **lease** `tick(now)` (`LEASE` 5 s since the last `Show`, `saturating_duration_since`), `keep_only(
  primary)` (crosses from a primary not followed now go at once; `Source::Here` kept), `stamp` for
  redraws. `Sender` (the primary): `due(want, outline, rev, now)` sends a change at once, the whole
  state every `REPEAT` (1 s), the outline at most every `OUTLINE_EVERY` (200 ms) and with each repeat;
  `heard` takes a `Seen` only from its `Target` for its session and a seq it sent; `link(connected,
  now)` -> `Link { Waiting, Seen { pixels }, Lost, Older }` (connected, no `Seen` after
  `SEEN_WITHIN` 3 s), `link_words` the panes' sentences (lost link, older NewQ with its version,
  monitor size not the points'). **Trust**, `route(msg, from_my_primary, leading)`: on top of the hub
  (only heeded sessions are handed on; `tell` only writes to heeded ones), `Show`/`End` only from the
  primary this computer follows (play_message's `primary`) and only for a display with a window here;
  `Seen` only on a primary; a primary takes no crosses. Without a passcode a stranger could blank a
  projector for five seconds at a time; it can already send it a show: no new hole. In the app,
  `app/lineup.rs`: `LineUpState { shown, sender, sprites, outline }`, `play_message`'s `Msg::LineUp`
  arm -> `line_up_message`; `line_up_pass` each pass after `play_send_show` (lease, `keep_only`, the
  draft `UiState::line_up` -> `Sender` for its display's computer, `End` when the draft goes or its
  session changes, the link in words on the draft); **one path**: a display on this computer is
  `Target::Here`, its messages go into this computer's own `Shown` and the `Seen` back
  (`no_window_here` when it has no window); `line_up_wake` (100 ms while either is held);
  `update_outputs`: `line_up_groups` (a group in raw monitor pixels: the outline picture, a sprite per
  mark from `stage3d::lineup_overlay`, textures made once) and `with_crosses` (black, or the list's
  opacities times `DIM`), drawn last, the window's blend off, its mapped frames dropped on black; a
  change of `stamp` is a redraw. *Done* is the panes' one show edit, sent by show sync. Tests:
  `net/lineup_tests.rs` (lease, late and stale, sane, outline, keep_only, the rule, the sender's
  pace, the link and words, alone as one loop, the wire), `net/hub_tests.rs::line_up` (crosses move
  on the follower with a passcode and `Seen` back; a stranger's line-up unheard with and without a
  passcode; a stranger's `Seen`; a dropped primary and the lease, then back; an older peer; *Done* as
  a show edit).

- `src/net/` (stage 1 of `.claude/notes/network-plan.md`; results and what remains in
  `network-sync-trial.md`): `clock.rs` a follower's shared clock from NTP-style exchanges (pure;
  weighted fit by round trip, drift, slews once locked, never back), `msg.rs` the wire format
  (clock ask/reply, commands at a shared time, ack, heartbeat), `sim.rs` and `trial.rs` (tests
  only) a simulated network and three `Engine`s; the hour-long report is the ignored
  `the_hour_long_report`. The app uses it through `net/play.rs` (below). `Engine::tick_shared(show, now)` ticks by
  a shared time: cues get `ActiveCue::anchor` and runs `Run::anchor`, clips `clock_offset`, and
  their times are then the clock's (`ActiveCue::pinned`), not sums of ticks or the sound device's
  count; `tick` alone (the app) is unchanged.
  `src/net/store/` is the media store (stage 3; rulings R4, R13-R20; used by `net/media/` below): `manifest.rs`
  (`media_paths`: the cues' files, then the 3D stage's venue model, whose own files `build` adds
  through `stage3d::load::venue_files`; `Manifest`: show path, `rel` to the show folder, size, SHA-256, `missing`, `library`; id = hash of
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
  `MAX_FRAME`; the first frame at most `HELLO_MAX`, 16 KB, by `read_frame_up_to`; a body grows as its
  bytes arrive, `read_body`, never reserved from the length), `PROTOCOL`, a lenient first `Hello` (`mismatch_sentence`), `Msg::Unknown` for messages a
  later NewQ adds, `Beacon`, `StatusReport` (monitors, sound devices, disk, `Link`; clock/show/media
  fields unfilled). `session.rs`: a reader and a writer thread per TCP connection, bounded queue (a full
  one drops the peer), 5 s write timeout. `discover.rs`: a UDP beacon each second to every port of
  `[NEWQ_NET_PORT or 47820, +8)` on each adapter's broadcast address, loopback and addresses added by
  hand; each NewQ binds the first free port (UDP and TCP the same number), so several on one machine
  find each other. `hub.rs`: the `net-hub` thread; **nothing is opened until switched on** (D1); the
  primary dials its computers and the waiting ones; Ping each second, 6 s of silence closes; followers
  report `Status` each second; connections accepted that haven't said hello: at most `UNNAMED_MAX` (16),
  `UNNAMED_PER_ADDRESS` (4) from one address (`admits`, pure; one more is shut with no threads), closed
  after `HELLO_WITHIN` (3 s). **Who is heard** (review 2026-11-26, Fable R3/Opus R1): `heeded(conn,
  role)` is the one gate for forwarding app messages (`NetEvent::Message`), `Status`, `tell`,
  `broadcast`, `send_to`, `order` and the `SetPasscode` key: a follower heeds only the session it
  `obeying` (after `judge`/`adopt`, the proof checked when it holds a key); a primary only a session
  `trusted` (latched: `Welcome`, counted only while this computer is a primary, plus the proof when it
  has a passcode, `latch_trust`; or this primary sent the key over it, `gave_key`: *Add to the show*, a
  new passcode). A peer's claimed id, role or `Welcome` alone never counts. One session per computer
  holds only for a heeded one: an unheeded session with an id doesn't keep a newcomer out; the first
  heeded closes the others with that id (`hold_only`; `closed` moves `Peer::conn` to one left). A primary
  with a passcode refuses (`Refusal::Passcode`) a session that claims its network and hasn't proved it
  within `PROVE_WITHIN` (10 s). `NetView` `connected`/`reached`: heeded, or a waiting computer that
  welcomed (`shown_connected`). Without a passcode the LAN is trusted (README says so): a claimed
  primary of the network with the term is obeyed as before. A proof is `MAC(verifier's nonce,
  prover's id)`, so whoever signs a nonce can be relayed: a computer with the key that isn't a primary
  proves it only once the peer's proof checked (`Conn::proof_withheld`; a primary goes first), so only a
  primary's own proof can be had, and only under its id. Known, not fixed (a wire change): that relay of
  the primary's proof while its session with a follower is down but it is up; *Add to the show* (and
  `add_vouched`) give the key to whoever answers as that waiting id; an unheeded session as a follower's
  id holds `Peer::conn` on the primary, so it isn't dialled (closed each `PROVE_WITHIN` when it claims
  the network, held while it claims none); unheeded sessions as the primary's id live on a follower as
  long as they ping; sessions past hello aren't capped in number; nothing is encrypted. `NetView` is published on a change with the app's `wake`;
  `Hub::send_to`/`broadcast`, messages the hub doesn't handle come out of `take_events` as
  `NetEvent::Message`, a role change as `NetEvent::RoleChanged`. Tests in `net/hub_tests.rs` (two and
  three hubs on loopback, `lan: false`). App: `App::net`, `pass_net`, `sync_audio_outputs`
  (`AudioOutputConfig::device_here` -> `AudioOut::sync_placed`; an output given only to other
  computers opens no device). Show (D5): `OutputConfig::computer` (`is_on(me)`; another computer's
  display opens no window here, `route_outputs`), `AudioOutputConfig::computers`/`devices_on`
  (`set_plays_on`), unwritten when unused. UI: `src/ui/network.rs` (Settings ▸ Show network, `badge`,
  the status bar, the Identify overlay), the Displays page's monitor menu with other computers' monitors
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

## Part of the `src/net/` entry: after the real tests and the 0.24.4 review

  After the first test on two real computers (0.24.2). Sound in a network: an `ActiveKind::Audio` cue is
  anchored like any cue; `Voice::keep_to(at)` passes over (`skip_left`, applied by `mix_voice`, the lead
  first) what a voice is more than `audio::SOUND_SLACK` (40 ms) behind, and never holds it back;
  `ActiveCue::sound_zero` -> `CueNow::sound` -> `measure::sound_apart_between` (`sound_apart_ms`). By a
  shared clock the picture's time is `elapsed - seek_base - video_lag` (no audio lead). Test:
  `engine/net_sound_tests.rs` (`AudioOut::with_manual_device`; 0.24.1 gave 123 ms "apart" with identical
  picture times: the real test's 118 ms was the sound cue, not the pictures). **R21 (owner's ruling): the
  network is a reference; a picture never waits for or jumps at the primary.** `net_correct` leaves a cue
  under `LEAVE_UNDER` (1.5 frames) alone, acts only after `STEADY_BEATS` (4) heartbeats within
  `STEADY_SPREAD`, at most once per instance (`NetCorrection::fixed`/`said`), and a missing cue is started once per primary instance (`NetCorrection::started`, by (cue, nth) and the primary's anchor, followed while paused: one gone again isn't restarted until a new one there). A sound (or live sound) whose output `is_elsewhere` is `PreparedKind::Elsewhere` (`ActiveKind::Wait`, its length, no card; a seek moves its time), so it is playing here, not missing; a film's sound elsewhere is left out quietly (item 32: 0.24.4 restarted it each heartbeat); heartbeats leave out `over_by_clock` cues (`ActiveCue::listed`); `Follower::step` with no
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
  UntilReady/AlsoUnused/Never, the names kept; `play::{show_started, card_shows}`): the card is for
  start-up only. `Playing::started` latches for the run once the show has started: `Follower::saw_start`
  (a command that `starts()`, or a heartbeat that `shows_start()`: `PlayState::started`, the primary's
  `started_total() > 0`, serde default, or cues/runs in it) or the engine started a cue here; after that a
  screen with nothing playing is black (AlsoUnused keeps it on unused screens; not adopted always shows
  it). Identify (`identify_text`: name, "Screen N", `card_address_of` the best adapter) replaces the
  screen's layers, on black (`app.rs::put_card`: the card goes last, over a display showing the 3D stage
  too). Output windows start hidden on Windows (`OUTPUTS_START_HIDDEN`: winit shows a
  window before making it full screen, with no background brush, so it was a white box) and are shown by
  `show_drawn_outputs` once `OutputThread::presented_size` is the display's (`output_ready_to_show`, or
  after `FIRST_FRAME_WAIT`); Mac and X11 as before. Set on the Show network page beside
  `no_transfer_cards`.
  After the second real test (0.24.3; list in `.claude/notes/second-test-fixes.md`). **One NewQ per
  node folder**, every app mode: `node::lock_node_dir` holds `<node dir>/node.lock` (std
  `File::try_lock`, released by the OS however NewQ ends), taken in `main.rs::hold_node_folder`
  before the log and `node.json`; helper modes exit before it. A second copy hands its show over
  (`node::hand_over`, `<node dir>/handoff/*.json`) and exits with code 3; the open one's
  `App::take_handoffs` comes forward and opens it (refused by `handed_show_refused` while something
  plays, a test runs or it follows). Every self-restart sets `node::RESTARTING_ENV` (the new copy
  waits up to 20 s). Several NewQs on one machine need separate `NEWQ_NODE_DIR`s. Hub: `Hello.tcp_port`
  and `Beacon.system` (serde default); while a session holds, a peer's beacons from another port are
  ignored (`pinned_port`); `keep_clock` aims at `(p.addr, hello.tcp_port)` and logs every re-aim
  (`NetView.clock_with`); one id switching between two ports (`twin_ports`) or this computer's own id
  from another port (`twin_self`) is `PeerView.twin`/`NetView.twin`. `dial` tries every heard address
  best first (`paths::by_rank`, `way_words`); after `ADOPT_PATIENCE` the row says why
  (`unreached_sentence`, Windows Firewall named only for a Windows beacon). Clock: unlocked, a step
  back over `Settings::big_step` is taken at once; `SharedNow::jumps` -> `Engine::net_shift_shared`
  moves anchors so what plays carries on (R21); `SharedNow::report_ms` is None while unlocked, capped
  at `REPORT_CAP_MS`. Display computers: display mode (`play::display_computer`) only for a start with
  `--display` (NewQ Display, start at login); NewQ started plainly on an adopted Display
  (`play::paired_display`) shows its control window, locked, and `network::locked_banner` offers
  *Open as a display computer* (`App::open_as_display`, `restart_adds_display`), *Leave the show
  network* and a start-at-login tick. Start at login is never set by itself: the primary's row
  (`Msg::StartAtLogin`) or the computer's own tick; Windows value `NewQ Display` for a display
  computer, else `NewQ` (`autostart::name`/`run_values`, `where_words`, `fix_old_entry` for 0.24's).
  Cards: `screen_name` "Name · Screen N" (N = index in `ui.monitors` + 1); `NOT_REACHED_AFTER` (20 s)
  with no peer connected adds `not_reached_words`. Network test: `Row::shown_apart_ms` (each cue's
  time on the screens by the follower's clock against the heartbeat, `seen_offset`) is what the
  verdict uses; `began_apart_ms` (old `apart_ms`) can't see a wrong clock; a key counts only when two
  heartbeats in a row differ; `net_judge::CLOCK_LOST_MS`, `frame_hz` (slowest screen that drew), no
  "use a wire" on a wire; each computer's whole log since the test began (`LogMark`, `log_since`,
  `NetTest::Whole`) is `computers/<name>/newq.log` in the primary's zip (`fleet::log_path`,
  `sequence::gather_logs`); `ClipUp::waiting` gives timeline.csv's "(waited: …)"; measure rows and
  reports reset their due time when the shared clock is put back.
  `ActiveCue::outgoing_of`: the run a seek or a loop going round let go of a clip from (kept up in
  `Run::outgoing`, `run` None); `plays_by_itself()` (no run, not outgoing) is what `net_state` lists
  and what `net_correct` counts as its own; `net_correct` never starts a cue `model::key_of` places on
  a timeline. Tests: `engine/timeline/tests.rs` (`NEVER_CUED`, cfg(test) in `run_cued`).
  After the third real test (0.24.4; `.claude/notes/second-test-fixes.md` items 20-29). Cues that follow
  one another: `engine.rs::ended_at` records, ticking by a shared clock, the shared time a cue ended in
  `Engine::net_ended` (`ENDED_KEPT`); auto-follow, a group's next child (a crossfade `fade_in` earlier,
  never before that child began) and a group's end are placed there through `chain_backdate` (at most
  `ENDED_LATE_MAX`, 0.05 s, back, or `SOUND_SLACK` when what it starts has a sound: `net_started_sound`; later, it starts at the tick from its beginning; by a shared clock a sound or film the clock takes past its length is over there, `ActiveCue::over_by_clock` (a timeline's clip too, unless its clock keeps its end), while its device plays the tail out, and one that ran out up to `SHORT_OF_CLOCK_MAX` (1 s) short of it waits for the clock (`short_of_clock`; issue 33: a display computer without the sound ended a trailer group when its picture ran out; `net_sound_tests.rs::the_timeline_group_after_a_film_…`, `play_tests.rs::trailers_in_timeline_groups_…` with `World::uncompared`; the primary logs each command it times, a follower one done there it never had, `Follower::heard`), and a group whose last child ended this tick ends in that tick, a second `advance_groups` pass in `tick`); `net_mark`/`net_backdate` shared with `net_apply`'s late commands
  (each computer had anchored at the tick that saw the end: 0.24.3's -26 ms corrections). Tests:
  `play_tests.rs::cues_that_follow_one_another_start_together_at_any_frame_rate` (`Node::hz`),
  `net_sound_tests.rs::a_chain_of_films_and_sounds_…`, `engine/net_chain_tests.rs`. Network load in Test
  Mode: `src/net/load.rs` streams a repeated 64 KB made-up buffer both ways between the primary and each
  computer while a test across the network plays (`LoadLevel` Off/Light 50/Heavy 400 Mbit/s/Full;
  `Prefs::test_load`, Test Mode ▸ More; `NEWQ_TEST_LOAD`); `LoadServer` (token + node id first, counts
  both ways, `keep_only`), `LoadClient`, paced writer (`Pacer`) and draining reader threads; carried as
  `NetTest::Begin { load }` (serde default); `Fleet::start_load`/`stop_load`, `load_lines` in the reports,
  `app/play.rs::net_load`. `NetTest`, `play::Command`, `RunWord` and `Link` read an unknown kind as
  `Unknown` (`#[serde(other)]`), so a later NewQ's new kinds never close a session (`Hub::broadcast_json`,
  cfg(test)). Media: `Puller::start_ordered` takes the order at start and the primary sends `MediaOrder`
  ahead of `MediaServer` (the puller could start the version's first file in its own order). Start at
  login: `autostart::at_start` (pure: `Leave`/`Remove`/`Rewrite`), `look_at_start` in `App::new`; the first
  start of 0.24.4+ removes any entry once (`Prefs::start_at_login_cleaned`, set only when the removal
  worked), note `autostart::SWITCHED_OFF` via `network::login_note` (`LOGIN_NOTE_FOR`). Sound after a
  takeover: an output named for the old primary stays with it; `NetView::took_control_from_id`,
  `UiState::took_control_from()`, `network::sound_after_takeover` (`SoundLeft::OnOld`/`NoDevice`), *Play it
  here instead* = `Action::PlaySoundHere` -> `AudioOutputConfig::move_to` (allowed in show mode); the id is
  lost if the new primary restarts. Test Mode's timeline `Watch`: a clock put elsewhere between two looks
  resets `from` to `pos - wall` (a seek released at once was never seen `Cueing`).
  After the 0.24.4 review (`.claude/notes/review-0.24.4.md`). Pictures: `upload::first` writes a new
  texture's first picture on the main thread where `app.rs::picture_layers` (or `renew_rgba`, in place at
  the same size, for the fps label and cards) makes it, so a cut never has a pass with neither picture;
  `Uploader::busy` keeps windows drawing until in-place writes land; a stopped uploads thread:
  `App::forget_pictures` (also on a device rebuild), main-thread writes, restart at most every
  `UPLOADS_RESTART_EVERY`; `polled_within`/`UPLOADER_POLL_PATIENCE`; hooks `Uploader::hold`/`fail`;
  `adapters::look_every(on_for, lost_this_run)` stops after `LOOK_FOR_UNLOST`; `autostart::skip_clean_up`
  when prefs can't be saved. Network: `NetTest`, `play::Command`, `RunWord`, `Link`, `AdapterKind`,
  `Refusal` take an unknown kind as `Unknown`/`Other`; `PauseFreeze` (to `Nothing`) and `TimelineKey` (to
  `TimelineKey::UNKNOWN`, `List(u32::MAX)`) deserialise leniently, shows too; `Command::Control.action`
  unknown reads as `Memo`; `ControlAction`/`TimelinePoint` in show files stay strict. `net_shift_shared`
  moves `net_ended`; `LoadServer` removes ended connections; a beacon with our id and port from an address
  not ours (`hub::from_here`) is `twin_self` (a cloned `node.json`). `node::when_held`
  (`Ask`/`HandOverOrWait`/`Wait`), `wait_for_node_dir`: a restart or a `--display` start waits for the lock
  however long, no dialog. Test Mode: `session::NotLaunched` carries the user's show back from `launch` and
  `full::launch` (`TestMode::not_launched` puts it back and ends the fleet); a follower stops measuring
  without `End` (`App::stop_measuring`: a non-test show arrives, `network::is_any_test_show`;
  `PRIMARY_AWAY_AT_MOST` 20 s unreached; `MEASURE_AT_MOST` since the last `Begin`, which the fleet resends
  every `BEGIN_AGAIN`); `End` only for the run measured; `TestMode::end_again`; `App::net_test_flush` on
  quit; `Whole` only to 0.24.3+ (`fleet::takes_whole`); `seen_offset` uses `ClockHandle::best_rtt` and
  `Measurer::least_seen` (`SEEN_WINDOW` 5 s); `Row::uncompared` and the not-in-step Problem
  (`NOT_IN_STEP_PROBLEM_SECS`); `CLOCK_LOST_MS` exclusive; `LogTail` in `log_pieces` (`LOG_PIECE` 256 KB,
  `more`), `LogAt` (length + head hash) survives a rotation; `Fleet::log_path(id)` unique folders;
  `ComputerVerdict::log` -> `gui::logs_inside`; `Fleet::all_in` waits `LOG_LIMIT` for a log arriving.
  Media: `manager::sends` (pure), `served`: the port never goes before the order. Release: a tagged build
  without `mac_unsigned` needs all five Mac secrets (`signing` step fails) and sets
  `NEWQ_REQUIRE_NOTARISED` (package_release.sh refuses ad-hoc or un-notarised).
