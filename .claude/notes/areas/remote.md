# Remote control, sends and feedback

Covers `src/remote.rs`, `src/feedback.rs`, `src/timecode.rs`, `src/output.rs`, `src/projector.rs` (PJLink), `src/clock.rs`, `src/web.rs`, `src/web_page.html`, `src/clicker.rs`.

Moved word for word from the Layout section of `CLAUDE.md` (commit 2589061). Read this before
working in these files, and keep it true when the code changes. A heading that says "Part of"
is a piece of a longer entry that covered several areas.

- `src/feedback.rs`: OSC feedback to the address in `RemoteSettings::osc_feedback`
  (`snapshot`/`changes`/`repeat` are pure and tested; `Feedback` paces and sends). Timelines:
  `timelines_to_tell` (every timeline list, plus `Engine::timelines`: runs not stopped or still fading),
  `state_word`/`state_text` (the web remote uses them too), `/newq/go` from `go_text` (`GoPlan::describe`).
- `timecode::Generator` makes MTC messages; `output::MtcRunner` is its paced thread
  (`ActiveKind::Timecode`, dropped to stop). Send cues' `Program` target runs only
  when `Prefs::allow_scripts` (per computer; `output::set_scripts_allowed`).

- `src/clicker.rs`: presenter remote. `decide` is the pure GO/Back logic (Back pauses, Back again
  fades out and stops, GO resumes; `BACK_GUARD` ignores a double press); `Shared` is the learned keys
  and learning flow; a Windows low-level keyboard hook and a Mac `CGEventTap` (Input Monitoring) feed
  it with `Command::Clicker`, the in-window `KeyboardInput` is the fallback. Keys are per computer in
  `Prefs`. Not tried on a real Mac or Windows machine. **Switched off** (`clicker::AVAILABLE`): it
  started the global capture at every launch (a Mac would ask for Input Monitoring) and can't tell the
  remote from the laptop keyboard; see `.claude/notes/next-ideas.md` for the device-aware plan.

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

## Part of the `src/remote.rs` entry (a run of other files' notes): timecode, sends, PJLink

  `src/timecode.rs`: MIDI Time Code decoding and the trigger follower.
  `src/output.rs`: send cues' OSC and MIDI messages and the sender thread.
  `src/projector.rs`: PJLink (commands, MD5 digest, the conversation; tested
  against a fake projector on loopback). Projector and TCP sends run on a
  thread per host from `output.rs`.

  **No OSC or MIDI send waits on a name lookup** (R2 of the 2026-11-26 review). The
  `send-cues` thread sends MIDI and OSC/UDP to a literal address (`literal_ip`) itself,
  with no lookup. OSC and UDP (`NetworkOut` without TCP is an `Outgoing::Osc` too) to a
  host *name* go to a thread for that name (`start_name_sender`, keyed by the lowercased
  name, so every port of it keeps one order). While its first lookup runs, that name's
  messages queue (`NAME_QUEUE` = 64; past that one is not sent, with an error); a message
  that waited over `NAME_HOLD` (3 s) is not sent and says so, since a GO seconds late is
  worse than one the operator is told was lost. A failed lookup fails what waited for it
  in one error, and the next message looks again. A found address lasts `NAME_FRESH`
  (30 s), then is looked up again in the background while messages go to the one known (a
  failed refresh keeps it). Errors reach the status bar and log through `Outbox::errors`
  → `Engine::problem`. Lookups go through a `Resolver` (`system_resolver`, or a stand-in
  in tests that a test holds shut). Order between a name and an address for the same
  machine is not kept: they are two queues.

  `Feedback` (`feedback.rs`) uses an address as it is and looks a name up on a thread; the
  slot carries a generation number so a slow, older lookup can't overwrite a newer
  setting (R14 of the Fable review).

## Part of the `src/remote.rs` entry (a run of other files' notes): time-of-day triggers

  `src/clock.rs`: time-of-day triggers (local time via `jiff`).

## Part of the `src/remote.rs` entry (a run of other files' notes): the web remote

  `src/web.rs` and `web_page.html`: the web remote (HTTP on plain `std`
  sockets, handlers tested as functions and over loopback). `/api/timeline/<ref>/<verb>[/<place>]`
  (`timeline_command_for`) and the state's `timelines` (`timelines_json`: ref, state, clock, length,
  next pause, markers), drawn as a card each by the page's `renderTimelines`.
