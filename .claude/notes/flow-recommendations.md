# Making the build, check and release flow faster (without making it less safe)

Written 2026-11-22, after the 0.27.0 release. Read-only study: nothing in the repository was
changed but this file. Every number is marked **measured** (read from GitHub's records of the CI and
Release runs, or from the files), **claimed** (what the owner's brief said; I did not run the full
local check, since that was ruled out) or **guessed** (my estimate).

## The short version

The flow is not slow because the checks are slow. It is slow because of four things, in this order:

1. **Windows and macOS first see the work on release day.** Every pull request since #75 has been a
   release, opened with the whole batch in it (#82: 45 commits). So the first Windows and macOS test
   run *is* the release, and when one test fails there, a full 25-38 minute round is lost. On 0.27.0
   that was 73 of the 98 CI minutes (measured).
2. **One test in a thousand fails by timing, often.** At least 13 of the last 33 finished
   pull-request runs failed; 10 of those were a single test on Windows or macOS, nearly always a
   different one (measured). The true count is higher: runs that were simply run again show as
   passed, and I found two such (both one Windows test that passed untouched the second time).
3. **The same check is run too many times in the wrong place.** Three agents each run the whole
   real-time suite at once on 4 processors, then the session runs it again, then Ubuntu CI runs it
   again (claimed flow; the conclusion is mine).
4. **Avoidable waiting in the machinery**: each new pull request starts with no cache (10-14
   minutes, measured), the Mac release builds its two halves one after the other (7-9 minutes,
   measured), and the Mac CI job is the one everybody waits for (25 minutes warm, measured).

Checking the brief against the records:

**Right.**
- About 1,880 tests and 390-420 s of running: Ubuntu CI ran 1,882 in 379-384 s on the same kind of
  machine, so I take the local test time as confirmed.
- CI's ranges (15-22 / 30-38 / 18-35 minutes): right. But the spread is cold cache against warm
  cache, not luck. Warm, the three are about 15 / 17 / 25 minutes.
- Three rounds, the two causes, the quick run on `main`, the 20-second dry run: all as described.
- "2 h 40 m, 1 h 50 m of it waiting on CI": the pull request opened 2 h 12 m before publishing, with
  98 minutes of CI and 21 of Release, which fits if about half an hour of local checking and the bump
  came first (guessed).

**Wrong.**
- **"release.yml is about 20 minutes, mostly macOS notarisation": no.** Signing, notarising and
  stapling together took 83-94 seconds. The Mac job's 15-20 minutes is its *Build* step: two release
  builds (Apple and Intel processors) one after the other, 11.5-18 minutes.
- **"Windows CI cost specifically (linker, Defender, cache)"**: of those three only the cache
  matters. Windows reached 30-38 minutes because it had no cache twice running (a new pull request,
  then a failed job, which saves no cache). Warm, it is about 17 minutes and the Mac, at 25, is what
  a green run waits for.

**Out of date in the repository's own notes** (not the brief's fault, but agents read them):
CLAUDE.md says "over 700 tests, about 3 minutes" (1,896 tests; 6.4-12.2 minutes of running plus
2.5-5.5 of compiling) and "the notarisation wait is about 30 minutes" (about one); the release skill
says the Release workflow takes "about 8 to 10 minutes" (16-21).

## 1. Where the time goes

### Per release (0.27.0, measured unless marked)

| Minutes | What | Avoidable? |
|---|---|---|
| 73 | Two failed CI rounds (35 + 38), each for one test on one system | Almost all: found earlier, or not flaky |
| 25 | The green CI round | Down to about 15 (guessed) with the changes below |
| about 12 of the above | Cold caches in rounds 1 and 2 | Yes |
| 21 | Release workflow, 18 of it the Mac's two builds in a row | Down to about 11-12 |
| 13 | Fixes, merge, dispatching, between steps | Little |
| about 30 (guessed) | Local full check and bump before the pull request | Most, if CI already passed on that work |

### Per change, locally (claimed numbers, my ranking)

| Minutes | What | Avoidable? |
|---|---|---|
| 15-20 each, x3 at once | Every agent's own full check | Yes: this is the largest waste per change |
| 15-20 | The session's full check on the combined result | Keep the Linux part (about 10), not always the rest |
| 5-8 of that | Cross clippy, twice, each from an empty folder because the folders are deleted | Mostly |
| 6.5-7 of that | Running the tests | About 2 of it (the tail) |
| 2-4 per rebuild | Recompiling the one 180,000-line crate from scratch (`CARGO_INCREMENTAL=0`) | Perhaps half (to be tried) |

Three agents sharing one target folder do not build in parallel: Cargo locks the folder for the
length of a build, so they queue (that is how Cargo behaves; I did not watch it happen here). And when their test runs overlap, three real-time suites share 4
processors, which is the very starvation the `heavy()` lock in `src/testmode/mod.rs` was written to
stop inside one run. Those are the least trustworthy test runs in the whole flow, and the most
expensive.

### Inside one CI job (warm, measured)

| | Ubuntu | Windows | macOS |
|---|---|---|---|
| Set-up, FFmpeg, cache restore | about 1.5 m | about 2 m | about 2 m |
| clippy | 0.5-0.7 m | 0.9-1 m | 1-1.4 m |
| Compile the tests | about 2.5 m | about 3.7 m | about 5.6 m |
| Run the tests | 6.4 m | 7.0 m | 12.2 m |
| Compile NewQ again for the self-test | 1.7 m | 2.3 m | 2.7 m |
| Self-test | 2.0 m | 2.0 m | 2.1 m |
| Whole job | about 15 m | about 17 m | about 25 m |

Two things stand out. The self-test's compile is a second build of the whole program that waits in
line behind the tests. And the end of every test run is a queue of 18 "heavy" Test Mode tests that
run one at a time: 4.5 minutes on Ubuntu and Windows, 8.4 on macOS, the last two minutes with nothing
else left to run.

## 2. Recommendations

Each has: the change, the time, the risk, and what covers the risk.

### A. Open the pull request at the first push, and stop making the release the first test

**Change (habit, plus two lines in CLAUDE.md and the release skill).** At the start of a batch of
work, push the branch and open a draft pull request. Push each integrated batch to it as it is
done. CI then tests Windows and macOS while the next batch is being written. A release becomes:
the pull request is already green; add the bump commit; one more round; merge; cut.

Even better, and my actual advice: **merge when a batch is green, release when you decide to.**
Merging and releasing are different acts. Today they are welded together, so every merge costs a
release and every release carries a day of untested-on-Windows work.

**Time.** On 0.27.0 the two failures would have shown up hours earlier, while nobody was waiting:
73 minutes off that release. Over the last 33 runs about 4 in 10 failed, so expect roughly 25-35
minutes saved per release on average (guessed from measured rates), and the release itself becomes
predictable.

**Risk.** None to reliability: the same three systems run the same tests, more often. One practical
trap: a newer push cancels the run in progress (`cancel-in-progress`), so pushing more often than
every 25 minutes means no run ever finishes. **Cover:** push per integrated batch, not per commit;
before pushing again, look whether the last run finished.

### B. Give `main` a cache, and keep the cache when a job fails

**Change (`.github/workflows/`).** A pull request can only use caches made by itself or by `main`.
`main` never makes one, because the `gate` job (rightly) skips the builds after a rebase merge. So
the first round of every pull request compiles all the libraries from nothing. Cold against warm,
whole job (measured on #81 and #82): Ubuntu about +7 to 9 minutes, macOS about +4 to 9, **Windows
about +14 to 18**. And a job that fails saves nothing, which is why Windows was cold twice on 0.27.0.

- Add a small separate workflow (say `warm-cache.yml`) on pushes to `main` that touch `Cargo.lock`,
  `Cargo.toml` or the workflows, plus once a week (GitHub drops a cache nobody used for 7 days): on
  the three systems, restore, `cargo clippy --all-targets`, `cargo test --no-run`, `cargo build`,
  save. It must be its own workflow, not a job in `ci.yml`: `scripts/check_main_ci.sh` waits for the
  CI *run* to succeed, and a release must not wait for a cache.
- **The two workflows must name the cache the same way, or this does nothing.** rust-cache puts the
  job's name in the key (today's keys read `v0-rust-build-Linux-x64-…`, `build` being the job). So
  give `Swatinem/rust-cache@v2` an explicit `shared-key` (for example `newq-${{ runner.os }}`) and
  `add-job-id-key: false` in both `ci.yml` and `warm-cache.yml`. Recommendation E splits the CI jobs
  and needs the same `shared-key`, so do B and E in one change.
- In `ci.yml`, also give it `cache-on-failure: true`.
- **How to know it worked:** the first run of the next new pull request says "Cache restored" on
  all three systems, not "No cache found".

**Time.** About 10 minutes off the first round of every pull request (a run is as long as its
slowest job: cold that was Windows at 34-37 minutes, warm it is macOS at 25). 14-18 minutes off
Windows in the round after a failure. Measured basis. It matters more once A is adopted: merging per
batch means more pull requests, so more first rounds.

**Risk.** A cache holding a bad build. **Cover:** Cargo checks every cached library against the
lock file and the compiler version before using it, and NewQ's own code is never cached
(`cache-workspace-crates: false`, as now). Release builds stay uncached (see "What not to change").

### C. Agents run a small check; the integrator runs the full one, once

**Change (how the session briefs its agents; a `scripts/check.sh` to make it one command).**

- An agent runs: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and the tests of
  what it touched (`cargo test <module path>`, for example `cargo test ui::timeline` or
  `cargo test stage3d`), plus any test it wrote. A few minutes instead of 15-20.
- The session, after cherry-picking, runs the full Linux check once, alone on the machine.
- Put the exact commands in one script, `scripts/check.sh quick <filter>` and `scripts/check.sh
  full`, so CLAUDE.md, the release skill and every agent brief say the same thing. Today the three
  places differ (the skill leaves out the GPU settings; CLAUDE.md's test count and time are long out of date).

**Time.** Roughly 30-45 agent-minutes per batch of three, and I'd expect the batch to finish 15-25
minutes sooner by the clock (guessed). Fewer false alarms too: suites that overlap on 4 processors
fail for reasons that aren't in the code.

**Risk.** An agent's change breaks a test outside its own area and nobody sees until integration.
**Cover:** the integrator's full run sees it before anything is pushed, and CI sees it again. What
is lost is only *which* agent to ask, and `git bisect` over three small commits answers that in
minutes. This is a clear win; do it.

### D. Cross clippy only when it can find something, and keep its folders

**Judgement.** Local cross clippy checks one thing CI also checks: that the Windows-only and
Mac-only code compiles cleanly. In the last 60 runs no CI failure was of that kind (measured), which
may mean it works or may mean it is rarely needed; 22 of 208 files hold such code. Its cost today,
5-8 minutes a round, is mostly self-inflicted: the folders are deleted each time, so every run
compiles every library again.

**Change (habit; one line in CLAUDE.md).**
- Run it when the batch touches one of those 22 files, `Cargo.toml`, `Cargo.lock` or `build.rs`:
  `git diff --name-only origin/main | xargs -r git grep -lE 'cfg\((windows|unix|target_os|not\((windows|unix|target_os)|any\(target_os|all\(target_os)' --`
  (no answer: skip it. The `-r` matters: without it an empty list searches everything.)
- The rule is not watertight: changing a shared function in an ordinary file can break a
  Windows-only caller elsewhere. That case goes to CI.
- Keep the two folders (about 1 GB each; this container showed 60 GB free). A second run is then
  under a minute (guessed).
- Never run it in an agent, only in the integrator.

**Time.** 5-8 minutes off most rounds.

**Risk.** A Windows-only or Mac-only compile error reaches CI. **Cover:** with the pull request
open early (A), clippy is the first real step in CI and fails within about 5 minutes of the push
(warm), on a run that nobody is waiting for. Don't adopt D without A.

### E. Stop the Mac CI job being the one everyone waits for

**Change (`ci.yml`).** Split each system's job in two that run side by side:

- "tests": fmt, clippy, `cargo test`.
- "self-test": `cargo run -- --self-test` and the artifact upload.

Give both the same rust-cache `shared-key` per system and let only "tests" save, so the cache
storage doesn't double (GitHub keeps 10 GB per repository; today's three caches are about 0.8 GB
each, measured).

If macOS is then still the slowest (it will be: 5.6 m compile + 12.2 m of tests), split its tests
in two jobs by name: `cargo test -- testmode::` (the heavy chain, 8.4 m there) and
`cargo test -- --skip testmode::`. Add a line that fails the job if the two together ran fewer
tests than `cargo test -- --list` counts, so a test can never fall between them.

**Time.** A warm green round goes from about 25 minutes to about 18 with the first split and about
15 with the second (guessed from the measured step times). That is per round, so it also shortens
every failure.

**Risk.** A test silently not run (the count check covers it); more jobs to go wrong at set-up
(each is the same set-up as now). The same tests run on the same systems; nothing is dropped.

**Unknown worth an hour:** the network Test Mode tests take 37-88 s each on the Mac runner and
10-20 s elsewhere. If that has a cause (a wait that depends on the processor count, a sleep that
macOS stretches), fixing it is worth more than the split, and it may be the same slowness a real
Mac shows.

### F. A written policy for timing tests, and a nightly run that finds them first

This is the reliability recommendation, and it also saves the most time after A.

**Policy (CLAUDE.md, under Conventions, about eight lines):**

1. A test waits for a *condition*, never for a time or for one wording. When several outcomes are
   possible, it accepts each legitimate one by name and fails at once on any other; it does not sit
   out a 180-second limit. (The 0.27.0 store test waited three minutes for one sentence while the
   other legitimate sentence was already on screen.)
2. No assertion on how *little* time something took, or on an upper count of things that happen per
   tick. (The skill already says this; it belongs with the conventions.)
3. A test that reads a file from the repository reads it whatever its line ends. (`.gitattributes`
   has `* text=auto`, so Windows checks out CRLF.) Better still, add `*.md text eol=lf` to
   `.gitattributes` so the question can't come up again.
4. Where the code can be driven by a clock the test owns (`Engine::tick_shared`, the simulated
   network in `net/sim.rs`, `World` in `play_tests.rs`), the test uses that and not the wall clock.
   Real-time tests are for what only real time can show (FFmpeg, sockets, the sound device stand-in).
5. When a test fails in CI and passes when run again, it is a bug with a name: it goes in
   `.claude/notes/flaky-tests.md` (test, date, system, cause once known) and is fixed at the cause.
   Raising its timeout is not a fix. A second entry for the same test means it is rewritten onto a
   test-owned clock.

**Nightly run (new workflow, `flaky-hunt.yml`).** On a schedule, on the three systems, build the
tests once and run the suite three times. Any failure is a flaky test found while everyone sleeps
rather than on release day. The repository is public, so the minutes cost nothing.

**Time.** Each flaky failure avoided is 25-38 minutes. At the measured rate (at least 10 in 33
runs) this is the biggest standing cost in the flow. One test,
`testmode::full::tests::a_full_check_stopped_part_way…`, failed on Windows on both 14 and 15
November: the list in point 5 would have caught it as a repeat.

**Risk.** None; this adds testing. The one thing to refuse is the easy alternative, below.

### G. cargo-nextest: try it, don't assume it

What it would give that matters here:
- a time limit per test, so a test stuck in a long wait is named and stopped;
- a report of the slowest tests each run (today nobody can see which tests the minutes go to
  without reading timestamps out of logs, as I did);
- a proper way to say "these tests one at a time" that doesn't waste a test slot the way a test
  waiting on the `heavy()` lock does;
- built-in splitting across jobs (`--partition`) for E.

What can go wrong:
- It runs each test in its own process. The `heavy()` lock lives inside one process, so it would
  silently stop working; those 18 tests must be put in a one-at-a-time group in its config first.
  The same goes for anything else tests share in memory (`TEST_RUNTIME` in `ndi.rs`, the once-only
  look-ups of FFmpeg and the GPU). I found no test that sets environment variables, which is the
  usual trap.
- 1,896 process starts of a large program, many of which open the software GPU or find FFmpeg
  afresh, may be *slower*, on Windows especially. Not measured.

**Retries:** you asked for retries only for known-flaky tests, and that is the right instinct.
Blanket retries would hide exactly the kind of fault a show program must not have (a race that
shows one night in twenty). If nextest is adopted: no retries by default; one retry only for tests
named in the flaky list from F, each reported as "flaky" in the job summary, and a test leaves the
list by being fixed.

**Verdict.** A one-day trial on a branch, on all three systems. Adopt it if the Mac and Windows
times drop and three runs in a row are green; otherwise keep `cargo test` and take E and F, which
give most of the gain. Not in the first five.

### H. Shorten the tail of the test run

Three tests that play in real time, one after another, are the last two minutes of every run on
every machine, local and CI (measured): the short Test Mode run (71-74 s), the timeline test stopped
part way (27-36 s) and the short timeline test (30 s). Before them the decode comparison takes
42-57 s.

**Change (`src/testmode/session.rs`, `timeline.rs`).** Ask of each: does it need to play at the
speed of a show? One test already plays "Timeline short at 2x through an engine". If the short run
can be driven the same way, or its test-only plan can be a third as long while passing through the
same steps, about 1.5-2 minutes come off every full run everywhere.

Without touching the tests: start the heavy chain at the beginning rather than two minutes in, by
running `cargo test -- testmode::` and `cargo test -- --skip testmode:: --test-threads=3` as two
commands at once (in `scripts/check.sh full`). The thread limit leaves the heavy chain the
processor it has today, so this is not the overload I argue against in C; even so, try it ten
times before trusting it. Local run about 6.5 → about 5 minutes (guessed). In CI, E's split does
the same job more safely.

**Risk.** A shortened test covers less. **Cover:** the self-test in CI still plays every cue kind
in real time on all three systems, and Test Mode itself is run by hand on real machines.

### I. Build settings

- **`debug = "line-tables-only"` for NewQ and no debug information for libraries, in the dev
  profile** (`Cargo.toml`: `[profile.dev] debug = "line-tables-only"` and under
  `[profile.dev.package."*"] debug = false`). Crash reports and panics keep their file and line.
  What is lost is looking at variables in a debugger, which this flow never does. Expect the
  target folder a third to a half smaller and builds 10-20 % quicker, most on Windows, where writing
  debug files is slow (guessed; try it and compare `cargo test --no-run` before and after). This is
  the best answer to the disk filling up. *Risk:* a crash report from a debug build shows less; the
  downloads are release builds and are not affected.
- **Keep `opt-level = 1` and `2` for libraries.** The tests play media against the clock; slower
  code would make more timing failures. The cost (slow cold builds) is what the cache in B removes.
- **mold or lld: no.** Since Rust 1.90 the Linux build already links with lld. Nothing to gain.
- **sccache in CI: no.** rust-cache already keeps the libraries; NewQ's own crate changes with
  every push and can't be reused.
- **The shared target folder with a different `codegen-units` per agent: keep it, but know what it
  does.** It is a sound trick for disk (the libraries are built once and shared). It does not give
  parallel builds: Cargo locks the folder, so agents wait for each other. With C the agents build
  less, so the queue matters less. Separate folders would build in parallel but cost several GB
  each and three library builds; not worth it on 4 processors.
- **`CARGO_INCREMENTAL=0`: try turning it on for the integrator only.** With it off, every edit
  recompiles 180,000 lines. With it on in a single worktree, a small edit should rebuild in well
  under a minute (guessed). It costs disk (several GB), so not for the three agents. *Risk:*
  incremental builds have, rarely, produced a wrong program; *cover:* CI builds without it.
- **Pin the Rust version** (`rust-toolchain.toml`). CI used Rust 1.100 on 22 November while this
  container has 1.97 (measured). A newer clippy knows lints the local one doesn't, so "clippy clean
  here" does not mean clean there; the 13 November run failed clippy on all three systems, which
  fits (guessed cause). Pinned, a new Rust arrives by a deliberate one-line change, not by surprise
  on release day. *Risk:* the pin file must list what the workflows install today (`clippy`,
  `rustfmt`, and the targets `aarch64-apple-darwin` and `x86_64-apple-darwin` for the Mac release),
  or the Mac release build breaks; and a pin that is never moved goes stale. *Cover:* a dry run of
  `release.yml` (no tag) after adding it, and move the pin on purpose every couple of months, in a
  pull request of its own. Saves no minutes on a normal day; it removes one kind of surprise round
  (25 minutes each).
- **Splitting the crate into several: not now.** It is the real cure for rebuild times, and a large,
  risky change to a program that works. Revisit when a rebuild passes five minutes.
- **Session-start hook** (`.claude/hooks/session-start.sh`): add `mesa-vulkan-drivers libvulkan1` to
  the packages and `WGPU_BACKEND=vulkan`, `NEWQ_REQUIRE_GPU=1` to the environment file, since the
  local check needs them and every session sets them up by hand. A minute or two per session; no risk.

### J. The release workflow

- **Build the Mac's two halves side by side** (`release.yml`): one job for
  `aarch64-apple-darwin`, one for `x86_64-apple-darwin`, each uploading its two programs as a
  one-day artifact; then the existing Mac job downloads both, joins them with `lipo`, signs,
  notarises and packages exactly as now. The Mac goes from 15-20 minutes to about 8-10, and the whole
  release from 16-21 to about 11-12 (Windows, 10-11.5 minutes, becomes the longest). **Risk:** a
  change in the path that signs. **Cover:** the signing and notarising steps are untouched; do a
  dry run of `release.yml` (no tag) and compare the app's two architectures with `lipo -info`, and
  `package_release.sh` already refuses an un-notarised tagged release.
- **Notarisation needs no attention.** It takes about a minute. Correct the two notes that say 30.
- **Don't cache release builds.** A release built from nothing is worth five minutes.
- **The dry run, then the real run: keep.** 20 seconds for a check that has refused bad tags before.
- **Starting earlier:** nothing useful. The release can only start from a tag on a `main` that CI
  passed, and after A and E that wait is one 15-minute round.

### K. Waiting

- `scripts/gh_wait.sh` polling once a minute is fine; the waiting is CI's length, not the polling.
  Give the script a `--for 25m` option that exits with "still running" before the Monitor's
  30-minute limit, so each watch ends by design rather than by expiry. Saves no minutes; removes a
  way for a release to stall unwatched. No risk.
- A container restart killing agents: keep each agent's work in commits on its own branch, pushed
  as it goes (branches can be pushed even though tags can't). Work that exists only in a worktree
  is the thing that gets lost. Same rule as this report: save as you go. No risk; it costs a push.

### L. Fewer, larger releases, or small ones?

The owner decides the release schedule; this section is advice only and is not adopted.

Measured: eight releases in five days (#75 to #82). Each is a download for people who run shows,
and each cost between 30 minutes and two hours of waiting.

My view: **small merges, fewer releases.** Merge every green batch (that is what keeps Windows and
macOS honest). Release when there is something an operator would want *and* it has been through a
real run on real machines, which is the one check CI can never do; the notes in CLAUDE.md show the
serious faults (the 4 fps laptop panel, the 118 ms sound, the lost NVIDIA driver) were all found
that way, not by tests. A release every few days of such use, not three a day, loses nothing in
safety and removes most of the release cost outright. A fix that a show needs tonight can still go
out alone; with A, B, E and J that takes about half an hour.

What I would not do is the opposite: larger *merges*. The 45-commit pull request is what made
0.27.0's release slow.

### M. CLAUDE.md and the release skill

CLAUDE.md is 141,636 bytes, about 19,000 words, roughly 35,000-40,000 tokens (measured size, guessed
tokens). It is loaded into every session and into **every sub-agent**: a batch of three agents
plus the session reads it four times. Three costs:

- tokens and a little time on every turn;
- room: it takes a sixth or more of what a session can hold, so long sessions reach the point of
  summarising themselves sooner, and that is where instructions get dropped;
- trust: it is too long to keep true. It says "over 700 tests, about 3 minutes" (1,896; 6.5-12
  minutes), "notarisation about 30 minutes" (about one), and the skill says the Release workflow takes
  "8 to 10 minutes" (16-21). 18 of the last 51 commits edited it.

**Change.** Keep CLAUDE.md to what every session needs, about 200 lines:

- what NewQ is, and the hard rules in bold (a partial file is never handed out; R21; never take time
  from an `Instant`; show files stay backwards compatible; every preview goes through the governor;
  one NewQ per node folder; the words: display, monitor, screen);
- Build and test (pointing at `scripts/check.sh`), Conventions (with F's policy), Running headless,
  CI and releases in ten lines pointing at the skill;
- a map: one line per area and the file to read before working there.

Move the Layout section, **word for word, not rewritten**, into area files under
`.claude/notes/areas/`: `network.md`, `timeline.md`, `stage3d.md` (with projectors and render3d),
`testmode.md`, `previews-and-ui.md`, `media-and-streams.md`, `platform.md` (Mac windows, display
loss, panics, reaper, logging), `packaging.md` (signing, FFmpeg and MediaMTX pins). An agent's brief
names the one or two it must read. Moving it unchanged means nothing is lost in the move; trimming
can come later, area by area.

**Time.** About 30,000 tokens less per session and per agent; the time is small per turn but it is
every turn (guessed). The larger gain is fewer wrong facts.

**Risk.** An agent doesn't read the area file and breaks a rule. **Cover:** the bold rules stay in
CLAUDE.md; most are held by tests anyway; the integrator's brief to each agent names the file.

**The release skill** is mostly good. Change: the stale times; "run the full check before the bump"
becomes "the pull request is already green; the bump needs fmt and one CI round"; move the long
story about scheduled triggers to one line ("wait with Monitor and `gh_wait.sh`, never scheduled
tasks"); add "open the pull request at the first push" as step 0.

## 3. Do these first

In order. The first three need no code change to NewQ at all.

1. **Open a draft pull request at the first push of a batch and push each integrated batch to it;
   merge when green; release separately** (A, L). Removes the failed rounds from release day:
   25-35 minutes per release on average, 73 on 0.27.0. No risk.
2. **Agents run fmt, clippy and their own area's tests; only the integrator runs the full check;
   cross clippy only when system-specific files changed, with its folders kept** (C, D). 20-30
   minutes per batch by the clock (guessed). Covered by the integrator's run and CI.
3. **Cache for `main`, `cache-on-failure`, and the self-test in its own CI job** (B and E, one
   change to the workflows because they share the cache's name). About 10 minutes off every pull
   request's first round, 14-18 off Windows after a failure, about 7 off every round. Same checks.
   Proof: "Cache restored" on a new pull request's first run.
4. **Build the Mac release's two halves side by side** (J). 7-9 minutes off every release. Signing
   untouched; prove it with a dry run of `release.yml`.
5. **The timing-test policy and the nightly flaky hunt** (F), with `*.md text eol=lf`. Slower to
   pay back, but it is the one that removes the cause. (The Rust pin in I is worth doing too, but
   it needs a release dry run, so it is not in the first five.)

With all five, a release from an already-green pull request should take about 35-45 minutes from
"release it" to published (one CI round of about 15, merge, about 12 of Release, the steps between),
against 2 h 12 m to 2 h 40 m for 0.27.0. That figure is a guess built on measured parts.

## 4. What not to change

- **Three systems before every release, real media, lavapipe GPU tests, the self-test.** Every test
  failure in the record was on Windows or macOS and none on Ubuntu. Those two systems are where the
  faults are found; they are the last thing to cut.
- **`fail-fast: false`.** One round then shows every system's failures at once.
- **The `gate` job** (a rebase merge isn't tested twice). It works: 7 seconds on `main`.
- **The dry run before the real release run, the draft release that is only published when all
  three downloads are on it, the owner-only start.** Cheap and each has earned its place.
- **Signing and notarising.** They cost about 90 seconds, not 30 minutes.
- **Release builds from nothing, with `--locked`.** No cache there.
- **Pinned and mirrored FFmpeg and MediaMTX.**
- **The dev profile's optimisation levels**, and **`heavy()`** (keep it even if nextest arrives).
- **No blanket retries, and no raising of timeouts to make a test pass.** That trades the owner's
  time today for a fault in front of an audience later.
- **Rebase merges and one feature per commit.** They make "which commit broke it" a short question.
- **Don't split the crate or change the linker now.** The first has real risk for a gain the items
  above mostly deliver; the second has no gain.

## What I could not check

- The local timings (I was asked not to build or run the suite; there was no `target/` folder).
  All local numbers are the brief's.
- Why the Mac runner is four times slower on the network Test Mode tests.
- How many of the ten single-test failures were races and how many were real differences between
  systems (the README one was real). I read the names and the panics' places, not each cause.
- Whether nextest is faster here; whether incremental builds help in this container; how much
  smaller the target folder gets with less debug information. Each is a short trial.
- The state of GitHub's caches (the session's proxy refused that API); the cache conclusion comes
  from the logs ("No cache found" on the first run of #81 and #82, and no "Saving cache" after the
  failed Windows job).

## Appendix: what was measured

### CI, per step (seconds), last five full pull-request runs

"Warm" = the cache of compiled libraries was found; "cold" = "No cache found" in the log.

| Run (date, PR) | System | Cache | clippy | Test step | of which tests running | Self-test step | Result |
|---|---|---|---|---|---|---|---|
| 45018410141 (22 Nov, #82 round 1) | Ubuntu | cold | 117 | 849 | 379 | 284 | ok |
| | Windows | cold | 306 | 1462 | 510 | - | **failed** (README line ends) |
| | macOS | cold | 161 | 1425 | 725 | 385 | ok |
| 45021924123 (#82 round 2) | Ubuntu | warm | 29 | 532 | | 221 | ok |
| | Windows | cold again | 261 | 1473 | | 431 | ok |
| | macOS | warm | 47 | 965 | 761 | - | **failed** (store test race) |
| 45025483764 (#82 round 3) | Ubuntu | warm | 41 | 531 | 384 | 222 | ok |
| | Windows | warm | 53 | 646 | 421 | 259 | ok |
| | macOS | warm | 61 | 1069 | 734 | 287 | ok |
| 44841696441 (21 Nov, #81 round 1) | Ubuntu | cold | 138 | 919 | | 279 | ok |
| | Windows | cold | 267 | 1415 | | 418 | ok |
| | macOS | cold | 184 | 1342 | | 276 | ok |
| 44850070484 (#81 round 2) | Ubuntu | warm | 31 | 533 | | 231 | ok |
| | Windows | warm | 58 | 676 | | 267 | ok |
| | macOS | warm | 84 | 1215 | | 280 | ok |

Whole runs: #82 took 34 m 44 s (failed), 37 m 44 s (failed), 25 m 23 s (green). The run on `main`
after the merge took 7 s (the `gate` job skipped the builds, as designed).

The self-test step is two things: compiling NewQ a second time as the plain program (1 m 39 s Ubuntu,
2 m 16 s Windows, 2 m 43 s macOS, warm) and then 121-123 s of the self-test itself.

The Test step, warm, is: compiling NewQ's test program (about 150 s Ubuntu, 225 s Windows, 335 s
macOS) and then the tests (384 / 421 / 734 s).

### Where the test minutes go inside a run (run 45025483764)

Seconds after the first test at which things finished:

| | Ubuntu | Windows | macOS |
|---|---|---|---|
| Everything before the "heavy" Test Mode tests | 116 | 156 | 230 |
| Every other test done | 264 | 313 | 688 |
| The heavy chain (the 18 tests that take the `testmode::heavy()` lock and run one at a time) done | 384 | 421 | 734 |
| Length of the heavy chain | 268 | 265 | 504 |

On Ubuntu and Windows the last 110-120 s of every run is three tests playing in real time, one after
another, with every other test already finished: `a_short_test_runs_logs_and_gives_the_show_back`
(71-74 s), `a_timeline_test_stopped_part_way…` (27-36 s), `the_short_timeline_test_plays_to_its_end…`
(30 s). Before them `a_decoding_comparison_runs…` takes 42-57 s. On macOS the network Test Mode
tests take 37-88 s each where the other two systems take 10-20 s; for about 4.5 minutes almost
nothing else finishes there (the runner has 3 processors, and a test waiting for the lock still
occupies one of the three test slots). I have not found out why macOS is 4x slower on those.

### CI failures, last 60 runs of ci.yml (11 to 22 Nov)

Pull-request runs: 20 passed, 13 failed, 7 cancelled by a newer push.

| Date | System | What failed |
|---|---|---|
| 22 Nov | macOS | 1 test: `net::store::tests::when_room_is_short_replace_frees_the_old_file_before_fetching` |
| 22 Nov | Windows | 1 test: `ui::keys::tests::the_readme_keyboard_table_is_this_table` (CRLF) |
| 19 Nov | macOS | 1 test: `net::node::lock_tests::an_unattended_start_waits_for_the_node_folder_as_long_as_it_takes` |
| 17 Nov | macOS | 1 test: `engine::net::sound_tests::a_sound_that_starts_late_on_one_computer_catches_up…` |
| 16 Nov | Windows | 1 test: `testmode::timeline::tests::the_short_timeline_test_plays_to_its_end…` |
| 15 Nov | Windows | 1 test: `stats::tests::the_log_writes_a_header_once_and_rows_as_they_fall_due` |
| 15 Nov | Windows | 1 test: `ui::timeline::tests::space::an_output_window_with_the_keys_follows_the_same_rules` |
| 15 Nov | Windows | 1 test: `testmode::session::tests::the_memory_watch_counts_the_processes_it_started` |
| 15 Nov | macOS + Windows | 1 test each: `engine::timeline::tests::a_clip_at_opacity_0_starts_when_a_fade_cue_raises_it…`, `testmode::full::tests::a_full_check_stopped_part_way…` |
| 14 Nov | Windows | 1 test: `media::tests::many_files_are_probed_a_few_at_a_time_and_the_urgent_one_first` |
| 13 Nov | all three | clippy |
| 11 Nov (twice) | Ubuntu | installing packages (the DNS trouble ci.yml now works around) |

Every test failure was one test out of 850-1,880, on Windows or macOS, never on Ubuntu. No run
failed to compile on Windows or macOS only.

This table is an undercount: it shows each run's last attempt. Five of the 60 runs were run again
after failing. I looked at two: 21 Nov (#81), Windows,
`media::video::tests::hardware_decoding_asks_ffmpeg_and_still_decodes_what_it_cannot`, and 14 Nov,
Windows, `testmode::full::tests::a_full_check_stopped_part_way…` (the same test as 15 Nov). Both
passed the second time with no change, so both are races, not Windows differences.

The suite grew from 858 tests run (14 Nov) to 1,108 (15 Nov), 1,381 (16 Nov), 1,560 (19 Nov) and
1,882 (22 Nov) by the counts in those logs: more than double in eight days. macOS's test running time
went from 201 s to 734 s over the same days.

### Release workflow (release.yml)

| Run | Ubuntu job | Windows job | macOS job | of which Build | of which Package (signing + notarising + stapling) | Whole run |
|---|---|---|---|---|---|---|
| 0.27.0 (45027867825) | 7 m 25 s | 11 m 29 s | 20 m 02 s | 17 m 56 s | 94 s | 20 m 34 s |
| 0.26.0 (44854851517) | 5 m 27 s | 10 m 09 s | 15 m 09 s | 11 m 30 s | 83 s | 15 m 42 s |

### 0.27.0, by the clock (UTC, measured)

11:39 PR #82 opened with 45 commits (+12,509 / -1,432 lines), CI round 1 starts. 12:14 round 1 over
(Windows failed). 12:22 fix pushed. 13:00 round 2 over (macOS failed). 13:03 fix pushed. 13:29 round
3 green, merged, `main` CI 7 s. 13:30 Release started. 13:51 published. PR opened to published:
2 h 12 m, of which CI 98 m (73 m of it the two failed rounds), Release 21 m, people-and-agent time
between steps 13 m.

Pull requests #75 to #82 were each opened at release time and merged 30-110 minutes later: every
pull request is a release, and Windows and macOS first see the work then.

### Other facts read from the files

- 1,896 `#[test]`s, 10 ignored, in 208 source files (about 180,000 lines), one crate, one test program.
- 22 of the 208 files have code for one system only (`cfg(windows)`, `cfg(target_os…)`, `cfg(unix)`).
- CLAUDE.md is 141,636 bytes, about 19,000 words (roughly 35,000-40,000 tokens). 18 of the last 51 commits touched it.
- No `.cargo/config.toml`, no `rust-toolchain.toml` (CI uses whatever "stable" is that day: 1.100 on 22 Nov).
- rust-cache keys in the logs: `v0-rust-build-Linux-x64-2fd77104-115ed3cd`, restore key `v0-rust-build-Linux-x64-2fd77104` (job name, system, Rust version, then the lock file).
- `Cargo.toml`: dev profile `opt-level = 1`, libraries `opt-level = 2`, full debug information (the default).
- This container: 4 processors, 60 GB free, Rust 1.97, no nextest, no sccache; no `target/` folder (so nothing could be timed locally without a long build).
- The session-start hook does not install `mesa-vulkan-drivers` or set `WGPU_BACKEND`/`NEWQ_REQUIRE_GPU`, which the local check uses.
