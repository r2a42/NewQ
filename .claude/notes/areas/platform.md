# Platform: windows, power, crashes and logs

Covers `src/awake.rs`, `src/splash.rs`, `src/macwin.rs`, `src/freeze.rs`, `src/power.rs`, `src/crash.rs`, `src/reaper.rs`, `src/logging.rs`, `src/crumbs.rs`, display loss in `src/app.rs`.

Moved word for word from the Layout section of `CLAUDE.md` (commit 2589061). Read this before
working in these files, and keep it true when the code changes. A heading that says "Part of"
is a piece of a longer entry that covered several areas.

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

## Part of the "Starting video and audio" entry: `crumbs.rs`

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
  `UiState::keeping_awake/on_battery` for the status bar and the This computer page's badge.
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
  `NEWQ_TEST_PANIC=<engine|ui|outputs|render|audio|uploads>@<seconds>` makes one happen. Never take time from an
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
- `src/logging.rs`: the log. `LogLevel` (`Prefs::log_level`, Settings ▸ This computer ▸ Logs and recovery;
  `set_level` applies it at once) or `NEWQ_LOG` (`Spec`: a level and `part=level` pairs; it wins, and the
  setting is greyed). Always to `newq.log` in the log folder (and stderr except on a Windows release),
  through a bounded queue to a `log-writer` thread (`write_loop`; a full queue drops and counts), put
  aside as `newq.earlier.log` at 20 MB (`Rotating`; the last run's stays `newq.previous.log`). Libraries (targets not `newq::`) are a step quieter
  (`for_libraries`). Levels: `Engine::problem` (warn) for what went wrong and `Engine::log` (info) both
  show in the status bar; debug is each cue's start/playing/out/end (`Engine::log_cue_changes`) and each
  FFmpeg's command line; trace is preloads and FFmpeg exits. The panic hook flushes the log.

## Part of the `src/remote.rs` entry (a run of other files' notes): crash reports

  `src/crash.rs`: the log file and crash reports (Windows has no console).
  Reports go to the log folder (`%LOCALAPPDATA%\NewQ` etc.); ask for
  `newq-crash.log` and `newq.log` when a user reports a crash.
