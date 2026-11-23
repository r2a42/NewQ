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
| 22-23 Nov (#83, #84, two hunts) | Ubuntu 26.04 | The whole test program: SIGSEGV, five times (tests seen: `render3d::tests::images_and_meshes_are_kept_by_key_and_let_go`, `render::capture_loop`, `stage_render::tests::every_view_drawn_is_counted`) | Inside the Vulkan loader (libvulkan 1.4.341), in a call that looks up a device (`vkSetDebugUtilsObjectNameEXT`, `vkGetDeviceProcAddr` while making one) at the moment another test's new instance had the loader unload (`dlclose`) one of the seven Mesa drivers for hardware that isn't there. Found by `scripts/crash_backtrace.sh` (every thread's stack). The tests make many instances at once; the app makes one, and another only to look for a card while drawing in software | Fixed for CI and `check.sh full`: `VK_DRIVER_FILES` names lavapipe alone, so nothing is unloaded. Also `gpu::with_env` leaves wgpu's `DEBUG` off (one fewer loader call). The first fix, `DEBUG` alone, still crashed once in three rounds. Watch the nightly hunt |
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
