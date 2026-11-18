# Fixes from the second two-computer test (0.24.2)

A list only. Owner's instruction (2026-11-18): don't start fixing until the logs from both
computers are in; then read the logs, add what they show, and fix in order of harm.

## Seen during set-up

1. **"Adding…" forever.** On the primary's Show network page, *Add to the show* on a waiting
   display computer turned grey ("Adding…") and never finished, with no message. Likely cause:
   Windows Firewall blocking newq.exe's incoming connections on the laptop (the new version was
   in a new folder, so Windows treats it as a new program; its "allow access" prompt can hide
   behind the full-screen waiting card). Beacons go out, so the primary sees the laptop, but the
   primary's dial in (`hub.rs` `NetAction::Adopt` -> `pending` + redial) fails silently.
   Fix: after ~10 s without a session, say so on the row ("Can't reach Assistant at
   192.168.x.x") with the dial's own error, and log each failed dial. Neither log covers that
   moment, so the firewall is a guess (the duplicate identity of item 7 is a rival cause): name
   it only as one possibility on Windows.
   Owner (2026-11-18): a second NewQ *was* open on the Mac (item 7 confirmed). For "Adding…" they
   allowed NewQ in the firewall (no change at once), then turned off **Tailscale** and **Wi-Fi**
   on the laptop; one of those let it connect. So: when a dial fails, try every address the
   computer's beacons came from (`Peer::heard_from`, best first) before giving up, and name the
   address tried and the adapters seen (a VPN such as Tailscale, Wi-Fi) in the row's reason.
2. **Display computer: the firewall prompt is hidden behind the full-screen card.** Consider the
   card saying "Not reached by a primary yet: Windows may be blocking NewQ" when beacons go out
   but no primary has connected for a while, or checking for an inbound firewall rule at start
   (needs care: no admin rights).
3. **The release page said the Mac app was "signed and notarised"** when 0.24.2's Mac download
   was ad-hoc signed (`mac_unsigned`). The notes text should follow what was built.
4. **The `mac_unsigned` release option is only on the session branch** (commit "Release
   workflow: a choice to build the Mac download ad-hoc signed"). It needs a PR to `main`; the
   branch first needs restarting from `main` (one force-push of the session branch, owner's
   go-ahead needed).
5. **Number each screen's card on a display computer** (owner, 2026-11-18). Every output's
   waiting card now looks the same; each should say which screen it is (e.g. "Assistant ·
   Screen 1", "Screen 2"), even when the computer has only one, matching the numbering the
   primary's Test mode *Show on* list and Displays menu use for that computer's monitors.
   (`app/play.rs` draws `card`/`card_text` per monitor; Identify already flashes the name.)
6a. **Plain `newq.exe` opens as a display computer once the computer has been one** (owner,
   2026-11-18). `app/play.rs::display_computer`: `Role::Display => started_as_display ||
   (me.on && me.adopted())`, so the role kept in `node.json` decides, not how NewQ was started.
   Owner's rule: only the launcher (NewQ Display, `--display`) starts display mode; NewQ itself
   opens the normal window (and should then offer to leave the show network or keep following
   with the control window shown). Mind start at login, which runs `newq --display` (fine) and
   the D8 tests that assume an adopted Display hides the control window.
6b. **Start at login turns itself on, unasked.** `play_role_changed`: being adopted as a Display
   calls `set_start_at_login(true)`, which writes the value `NewQ` under
   `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` (`autostart.rs`; a LaunchAgent on a Mac,
   `~/.config/autostart` on Linux), not a Startup-folder shortcut, so the owner didn't find it.
   Owner's rule (2026-11-18): never without the user's input; it can be switched on **and off**
   from the primary (the computer's row) and on the display computer itself. Say in NewQ where
   it is and how to turn it off, and name the entry "NewQ Display". Existing entries: check
   whether they were written with `--display` (0.24.0's may not have been).
6. Optional: rebuild 0.24.2's Mac download signed and notarised once Apple's servers are back.

## Known before this test (still open)

- A display computer claimed by a primary that is gone can't be freed without deleting
  `node.json`; proposed a "leave the network" key.
- A sound output naming the old primary stays with it after a takeover.

## Testing workflow (not NewQ)

- Driving two laptops through other Claude sessions didn't work well: cross-session messages
  were refused (trusted-device check) or never arrived, and every step needed approval. Running
  the test by hand and sending the one zip is simpler: the network test already gathers each
  computer's log into the primary's zip.

## From the logs

Mac zip (2026-11-18, Medium, Cue list + Timeline + Network, 1 h; primary MacBook Pro M4 Pro on
10.1.81.160 and 2.0.0.100; display computer **WNO-2**, a different Windows laptop from 0.24.1's
"Assistant": RTX 5060, monitors 165 Hz and 60 Hz, wired). Windows logs still to come.

7. **Two NewQs on the Mac with one identity: the likely cause of nearly every sync problem.**
   The Mac's NewQ listened on port **47821** in both runs that day (`listening on port 47821`),
   so something already held 47820: another NewQ, most likely the copy opened from
   `~/Documents/newq` the evening before and left running. WNO-2 kept its clock with
   **2.0.0.100:47820**, the other copy. Both read the same `node.json` (same NodeId), so their
   beacons carry one id with two ports; `hub.rs::keep_clock` aims at `peers[primary].tcp_port`,
   which follows whichever beacon came last. Seen: the clock lock flapped 182 / 251 / 54 times
   in the three parts (locked ~65 % of rows), the cue-list part drifted 5-24 ms apart and was
   "put right" 27 times (moves of -26 to -41 ms, once +184 ms), the reported clock error was
   2,605,312 ms whenever it was unlocked. Fixes: (a) **one NewQ per node folder**: a lock on the
   node folder at start; a second copy says so and offers to bring the first forward or open
   alone (tests already use separate `NEWQ_NODE_DIR`s); (b) the hub notices one id beaconing from
   two ports/addresses and warns on both computers instead of switching between them; (c) the
   clock keeper aims at the session's own peer address and port, not the latest beacon's.
   **WNO-2's own log supports it, not proves it**: it started keeping the clock with
   `2.0.0.100:47820` (09:29:17) while the primary listened on 47821; after a restart at 10:35 it
   chose `:47821`. `keep_clock` re-aims (`k.aim`) without logging, so how often it switched is not
   in the log; the ~490 lock flips suggest two sources. Of the 14 corrections, some came in
   locked spells, so not all drift is explained yet. To confirm: one NewQ on the Mac (port
   47820), rerun Short with Cue list + Network. Log every re-aim.
   Also: unlocked with a large backward `e` the clock slews at `max_slew_unlocked` (1 %); never
   chase a big step that way while cues play.
8. **The clock error figure is nonsense when unlocked** (2,605,312 ms = 43 min). Find what the
   bound adds up while unlocked (`clock.rs::now`: `left` = slew × time remaining after a huge
   backward `e`, consistent with answers from two clocks); report "not locked" rather than a
   number, and cap the report's figure.
9. **"Frames apart" counts at the follower's fastest display** (165 Hz): 24 ms reads as 4
   frames. Count at the slowest display a cue is on, or at the primary's show frame rate, and
   say which.
10. **Mac: main-loop stalls of 100-180 ms "making the pictures"**, several right after a whole
   minute (09:35:00, 09:55:00, 10:06:00, 10:09:00, 10:10:00), with the slowest picture a 1080p
   "Layer: sliding, speed ramp" at 130 ms. Each came with **sound dropouts** on the MacBook
   speakers (19 in all; NewQ's mixing took 11.1 ms of a 10.7 ms buffer: 512 frames is tight).
   Check what runs on the minute (stats sampler, clock cue redraw, the second NewQ?) and
   whether the speed-ramp cue's work is on the main thread; consider a larger sound buffer on
   built-in speakers.
11. **Timeline: one clip 33.5 s late** ("Earthrise (second display)", lap 12 at 25:49), and once
   "not every clip was ready in time" (lap 3, "Title: The Pacific" 1.5 s late; WNO-2 then
   started Earthrise 69.77 s in and stopped it 2 s later). Lap 12 is the unfinished last lap:
   check whether 33.5 s is the measurement (the clock not at 0 when the lap began) before
   treating it as real.
12. **Timeline part: WNO-2 "worst 1650 frames apart" (10 s) for about 4 s**, around the same
   lap-3 moment. Probably the late Earthrise above; check.
13. **Network part looked good** (1.8 ms apart, 0 late commands) but `apart_ms` compares when cues began, and command-started cues carry the primary's stamp, so it reads near 0 even with a wrong clock; it was unlocked for 260 of 689 rows. Measure what the screens show instead (the cue's time now on each side). Sound up to
   174 ms from the primary's (sound ahead is never held back, by design).
14. **WNO-2 drew at its displays' rates** (162-165 fps at 165 Hz, 59-60 at 60 Hz): no stutter on
   this laptop. The 0.24.1 4-fps laptop ("Assistant", RTX 4050) was not in this test, so that
   question stays open.
15. The Mac's external monitor (VA24EHF at 100 Hz) drew about 85 fps in the network part.
17. **The display computers' logs are already in the primary's zip**
   (`<part>-computer-WNO-2-newq.log`, the end of its log per part), but the owner didn't find
   them and sent the files separately. Make it obvious: one `computers/<name>/newq.log` with the
   whole log since the test began, the report naming it ("WNO-2's own log is in this zip"), and
   the Test window's finish line saying every computer's log is inside. Display computers make
   no zip of their own, by design.
16. Report wording: "WNO-2 1 / WNO-2 2" in the displays line; the per-computer lines repeat the
   problems twice; the clock-error "use a wire" advice is wrong when the link is wired.

## Found while fixing (2026-11-18)

18. **A timeline clip loses its run tag while a loop's cueing runs out of time** (lap 3, 367.7 s:
   cueing took the full 2.0 s `CUE_LIMIT`). The primary's `net_state` then listed "Earthrise"
   as a cue not on a timeline (`a.run == None`), and WNO-2's `net_correct` started a copy
   "69.77 s in" and stopped it 2 s later. Fix in the engine (keep the run tag through a cueing
   that runs out of time), and a follower never starts a timeline clip as a free cue.
19. **Lap 12: Earthrise 33.5 s late on WNO-2's display** (= the 34 s pause less its 0.5 s place):
   cause unknown. `timeline.csv` now says what a late clip waited for ("(waited: …)"); read it
   after the next real run.
20. **0.24.3 kept the start-at-login entry 0.24.2 had made by itself** (owner, 2026-11-18: the tick
   was on when the laptop was added). `autostart::fix_old_entry` renames a 0.24 entry instead of
   removing it, so a choice nobody made survives. Proposed: remove entries older versions made,
   once, with a one-line note on that computer ("Start at login was switched off; tick it here to
   have it back"). Waiting on the owner's yes.
21. **The Preview pane's aspect ratio is sometimes wrong** (owner, 2026-11-18). Not yet known: which
   display it was showing, its size and shape against the pane's, whether in the classic or new
   interface, and whether it was during a test. Owner: don't start fixing until the logs from the
   0.24.3 run are in. Owner's detail: it was previewing the **HDMI monitor on the MacBook** (the
   VA24EHF, 1920x1080, which the 0.24.2 run's report showed at 100 Hz) and the picture was
   **squeezed horizontally: the Earth (a square NASA still) looked like an egg**. So the preview drew
   that display's picture narrower than its 16:9. Owner's read (2026-11-18): it follows the size of
   the Preview pane: changing the zoom/size squeezes the picture instead of letterboxing,
   pillarboxing or scaling it to fit. So the preview is stretched to the pane's shape; it should
   keep the display's aspect and fit inside the pane with bars. (Earlier guess, Retina points vs
   HDMI pixels, now second.)

## Status (2026-11-18): owner: "do everything in the fix list before the next version"

Done in 0.24.3: 1, 2, 3, 4, 5, 6a, 6b, 7, 8, 9, 11 (measurement), 12, 13, 16, 17, 18, the
"leave the network" key (6a's banner). Moot: 6 (0.24.3 was signed), 14, 25.
In progress (agents): 21 + 24 (preview aspect, sound wording), 22 (GPU recovery), 23 (26 ms offset).
Next: 20 (remove old start-at-login entries once, with a note; the owner's "everything" is the yes),
26 (generated network load in Test Mode), 10 + 15 (Mac main-loop stalls on the minute, the 100 Hz
monitor at 81-85 fps), the takeover sound output, 19 (test the suspected Watch path; nothing to fix
unless it reproduces).

## Seen in the 0.24.3 run (2026-11-18)

22. **Display computer (WNO-2) drew about one frame every 10-15 s and its two screens were out of
   step** (owner). The same laptop drew 162-165 fps on 0.24.2, so suspect something new in 0.24.3
   first: the clock's big-step jump (`SharedNow::jumps` -> `Engine::net_shift_shared`, logged as
   "the shared clock jumped to the primary's") firing again and again, the single-instance or
   handoff checks, the not-reached card, or the per-screen card numbering. Read WNO-2's log first.
   **Cause found in the 0.24.3 zip** (`computers/WNO-2/newq.log`): at 15:37:44 Windows reconfigured
   the displays (one pseudo-display "WinDisc" 3840x1080, then `\\.\DISPLAY6/7`) and DXGI reported the
   device suspended (0x887A0005). NewQ's device-loss recovery (`watch_gpu`) then picked **Microsoft
   Basic Render Driver (Cpu)**, the only adapter offered at that instant, and stayed on it to the end:
   uploads took 10.9 and 13.1 s, every frame waited ~1 s, and the laptop dropped out of the network
   during the long stalls. An output render thread also panicked once ("Surface is not configured
   for presentation", survived and restarted). Before 15:37 the laptop was fine: cue-list part
   clock locked throughout (error up to 0.4 ms), 161 and 60 fps. Fixes: (a) never run outputs on a
   software adapter if a hardware one exists: prefer hardware at every rebuild, and while on a
   software adapter retry for the hardware one every few seconds and switch back; (b) if only a
   software adapter is left, say so loudly (card, primary's row, report: "this computer lost its
   graphics card"); (c) a surface that isn't configured is skipped and reconfigured, not a panic.
   Why the driver reset is unknown (Windows Event Viewer, System log, around 15:37, e.g. nvlddmkm
   4101, would say); the owner may know what happened on the laptop then.
23. **Steady -26 to -27 ms corrections on WNO-2** (42 in the cue-list part, clock locked, error
   0.4 ms): its cues seem to start about 26 ms off the primary's, just over `LEAVE_UNDER` (1.5
   frames at 60 Hz = 25 ms), so each is corrected once. Look for a fixed offset (video lag, first
   frame, present latency) on one side.
24. **The Mac's sound dropouts are the system calling late, not NewQ's mixing**: 124 stream errors
   in newq.log, starting 14:51 before the test with nothing playing; "NewQ's mixing took at most
   0.0 ms … the longest time between two calls for sound was 32 ms". The report's "Mixing the sound
   took up to 144% of the time it has" reads the gap as load: say "the system called for sound late"
   instead. Possible causes: MacBook speakers' power saving, another app; a larger buffer may help.
25. No timeline clip came up more than 0.5 s late in this run (the 33.5 s one didn't recur).
26. **A large LAN transfer ran during the 0.24.3 test** (owner, 2026-11-18; on purpose a useful
   destructive test). Fits the timeline part's round trips up to 157 ms (28 ms in the cue-list
   part). The late commands (up to 7.7 s) and the network drop-outs line up with the laptop's
   10.9 and 13.1 s main-loop stalls on the software renderer, not the transfer alone. The GPU loss
   itself is a display reconfiguration plus a DXGI device suspension; whether the transfer played
   a part is unknown. Idea (not asked for): a Test Mode option that loads the network on purpose
   (the media server pushing a large file between computers during the run) so this is repeatable.
   **Owner (2026-11-18): yes**, but no big file download: stream arbitrary generated data between the
   computers over the LAN during the run (a load generator, not real media). Queued after the three
   running agents.
   Owner on the Mac's sound dropouts (item 24): don't try to fix them unless it's easy; nobody was
   listening, so it's unknown whether they were audible. The running agent only corrects the report's
   wording (system called late vs NewQ's mixing).
