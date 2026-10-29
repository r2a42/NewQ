---
name: release
description: Cut a NewQ release end to end (bump, PR, CI, merge, main CI, dry run, real run, verify). Use whenever the user asks to release, ship or publish a version.
---

# Releasing NewQ

The user asks once. **Do not hand control back until the release is published and
verified, or you are blocked on something only they can do.** Never end a turn with
"waiting for CI" unless a check-in is already scheduled (see Staying with it).

## Steps

1. **Bump.** `version` in `Cargo.toml` and the `newq` entry in `Cargo.lock` (`cargo check`
   updates it). Commit "Bump version to X.Y.Z". Run fmt, clippy (also
   `--target x86_64-pc-windows-msvc` and `--target aarch64-apple-darwin`) and
   `NEWQ_REQUIRE_MEDIA=1 cargo test` first: one clean push beats three.
2. **PR.** Push the session branch, `create_pull_request` (base `main`), then
   `subscribe_pr_activity`. Schedule the first check-in (below) immediately.
3. **CI.** Three jobs (ubuntu, windows, macos) plus `gate`. On a failure, read the job log
   (`get_job_logs` with the `job_id` from the event) and decide:
   - Ours (a test failed): fix, run the checks, push. The push restarts CI.
   - Infrastructure (Chocolatey/apt/brew 5xx or timeout before any test ran, a runner
     that never started): comment once on the PR, then `actions_run_trigger`
     `rerun_failed_jobs` on the run. It answers 403 "already running" while other jobs of
     the run are still going: that is not a failure, schedule a check-in and retry
     when the run has finished. Re-run once; a second failure is real.
   - Known trap: Homebrew's FFmpeg has no `hap` encoder. Tests that make Hap files use
     `test_media_with_hap` and skip. Timing tests must not assert upper bounds on
     counts: CI runners oversleep.
4. **Merge.** When all checks are green on the head, `merge_pull_request` with
   `merge_method: rebase` (never merge or squash: `main` stays linear).
5. **Main CI.** The rebase merge has the same tree, so the `gate` job skips the builds and
   the run is quick. `get_workflow_run` on that run's id (find it from the one run
   with `head_sha` = the merge `sha` that the merge call returned). It must be
   `completed`/`success`.
6. **Dry run.** `actions_run_trigger` `run_workflow` with `workflow_id: cut-release.yml`,
   `ref: main`, `inputs: {"tag": "vX.Y.Z", "dry_run": "true"}`. Find the new run and wait
   for `success` (about 15 seconds).
7. **Real run.** The same with `"dry_run": "false"`. It tags `main` and starts the
   `Release` workflow (about 8 to 10 minutes: builds for all three platforms).
8. **Verify.** The `Release` run must be `success`, then `get_release_by_tag vX.Y.Z`
   must exist with the Windows, macOS and Linux downloads. Report the link and what is
   in it. A `Release` failure: `get_job_logs`, fix or report, do not stop silently.
9. After the merge, `git fetch origin main && git checkout -B <session-branch> origin/main`.

Do not post `/release` as a comment: the comment tool mangles slash commands, and the
workflow is meant to be started by `workflow_dispatch` (owner's token).

## Tool pitfalls (all hit before)

- `actions_list list_workflow_runs` ignores `per_page` and returns ~100 runs, a huge
  output. Prefer `actions_get get_workflow_run` with a run id. To find the id of a run
  you just started, call `list_workflow_runs` once with `workflow_runs_filter`
  `{"branch":"main"}` and read only the first entry; do not call it repeatedly.
- `get_job_logs` needs `job_id` (not `run_id`) unless `failed_only` with `run_id`; use
  `tail_lines` (60 to 80).
- Your own PR comments come back as `issue_comment.created` events from the same account:
  that is an echo, not a request.
- A `check_suite.completed` event with no failures means CI is done, but confirm with
  `pull_request_read get_check_runs` before merging.
- Never `pkill -f target/debug/newq` from a shell whose command line contains that
  string: it kills the shell. Use `pkill -x newq`.
- Cloud sessions can't push tags; releases go only through the workflow.

## Staying with it (use Monitor, not scheduled tasks)

**Do not use `send_later` / `create_trigger` for waiting.** Since 2026-10-29 every one-shot
trigger bound to this session fires and is refused within milliseconds
(`ROUTINE_RUN_FAILURE_REASON_ACCESS_DENIED`; the docs don't cover this case). The trigger then
stays `enabled`, retries the next day and sends the user a "routine isn't permitted to run"
notification each time. If you did create one, `delete_trigger` it. (`list_triggers` shows
`last_run.failure_reason`.)

Instead use the Monitor tool with `scripts/gh_wait.sh`, which polls the public GitHub API
once a minute with plain `curl` (no token, no routine). Monitor wakes the session when the
script exits, and that works. Each wait is one Monitor call (`persistent: false`,
`timeout_ms` up to its limit; if the monitor expires first, start another):

```
scripts/gh_wait.sh pr 34              # all checks on the PR head finished (exit 0 = all passed)
scripts/gh_wait.sh latest ci.yml      # id/status/sha of the newest run (find the main CI run)
scripts/gh_wait.sh run 40796485911    # a workflow run finished (exit 0 = success)
scripts/gh_wait.sh latest cut-release.yml   # find the dry/real run you just dispatched
scripts/gh_wait.sh latest release.yml       # the Release run it started
scripts/gh_wait.sh release v0.12.0    # the release exists; prints its assets
```

So the loop is: do a step, start the Monitor for the next wait **in the same turn**, and when
it reports, do the next step. Chain through step 8 without waiting to be prompted. Stop only
when the release is verified, you are blocked (say what you need in one line), or the user says
stop. The same script serves any "wait for CI" in other work.
