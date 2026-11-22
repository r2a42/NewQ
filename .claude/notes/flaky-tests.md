# Flaky tests

A test that failed in CI and passed when run again, or failed only on one system for a reason that
isn't in the code under test. Each is a bug with a name: it is fixed at the cause, never by raising
a timeout or retrying. A second entry for the same test means it is rewritten onto a test-owned
clock (`Engine::tick_shared`, `net/sim.rs`, `World` in `net/play_tests.rs`). The policy is in
CLAUDE.md, Conventions.

Add a row when it happens: the date, the system, the test's full path, and the cause once known.
The nightly `flaky-hunt.yml` (the suite three times on each system) is meant to find them first.

Seeded from the CI records of 11 to 22 November 2026 (`.claude/notes/flow-recommendations.md`,
appendix). "Open" means only that the records name no fix: some may have been fixed since, so
look (`git log -S <test name>`) before starting on one.

| Date | System | Test | Cause | State |
|---|---|---|---|---|
| 22 Nov (#83) | Ubuntu | The whole test program: SIGSEGV during or just after the `render3d::tests` GPU tests (lavapipe), last named `render3d::tests::a_little_stage_draws` | Cause unknown: the test process crashed, no test failed by assertion; the same code passed on Ubuntu one round earlier. First crash of this kind on record | Open |
| 22 Nov (0.27.0) | Windows | `ui::keys::tests::the_readme_keyboard_table_is_this_table` | Windows checks README out with CRLF line ends and the table didn't match | Fixed in 9c143e9: reads the table whatever its line ends |
| 22 Nov (0.27.0) | macOS | `net::store::tests::when_room_is_short_replace_frees_the_old_file_before_fetching` | Two wordings are legitimate (the plan's or the fetch's); the test waited 180 s for one while the other was already on screen | Fixed in cea6cd9: accepts either by name |
| 21 Nov (#81) | Windows | `media::video::tests::hardware_decoding_asks_ffmpeg_and_still_decodes_what_it_cannot` | Cause unknown; passed when re-run with no change (a race) | Open |
| 19 Nov | macOS | `net::node::lock_tests::an_unattended_start_waits_for_the_node_folder_as_long_as_it_takes` | Cause unknown | Open |
| 17 Nov | macOS | `engine::net::sound_tests::a_sound_that_starts_late_on_one_computer_catches_up_and_the_pictures_never_differ` | Cause unknown | Open |
| 16 Nov | Windows | `testmode::timeline::tests::the_short_timeline_test_plays_to_its_end_with_go_pressed_at_its_pause` | Cause unknown | Open |
| 15 Nov | Windows | `stats::tests::the_log_writes_a_header_once_and_rows_as_they_fall_due` | Cause unknown | Open |
| 15 Nov | Windows | `ui::timeline::tests::space::an_output_window_with_the_keys_follows_the_same_rules` | Cause unknown | Open |
| 15 Nov | Windows | `testmode::session::tests::the_memory_watch_counts_the_processes_it_started` | Cause unknown | Open |
| 15 Nov | macOS | `engine::timeline::tests::a_clip_at_opacity_0_starts_when_a_fade_cue_raises_it_and_never_without_one` | Cause unknown | Open |
| 15 Nov | Windows | `testmode::full::tests::a_full_check_stopped_part_way_gives_everything_back_and_still_makes_the_zip` | Cause unknown; **second entry** (see 14 Nov): rewrite it onto a test-owned clock | Open |
| 14 Nov | Windows | `testmode::full::tests::a_full_check_stopped_part_way_gives_everything_back_and_still_makes_the_zip` | Cause unknown; passed when re-run with no change (a race) | Open |
| 14 Nov | Windows | `media::tests::many_files_are_probed_a_few_at_a_time_and_the_urgent_one_first` | Cause unknown | Open |

Every one was a single test out of 850-1,880, on Windows or macOS, never on Ubuntu. The table
undercounts: five of those 60 runs were re-run after failing and only two of those were looked at.
