---
name: release
description: Cut a NewQ release end to end (bump, PR, CI, merge, main CI, dry run, real run, verify). Use whenever the user asks to release, ship or publish a version.
---

# Releasing NewQ

The user asks once. **Do not hand control back until the release is published and
verified, or you are blocked on something only they can do.** Never end a turn with
"waiting for CI" unless a Monitor is already watching (see Staying with it).

## Steps

0. **The pull request is already open.** Work goes onto a draft pull request from its first
   push, each integrated batch pushed to it (CLAUDE.md, Pull requests), so Windows and macOS
   have already tested it. If there is none yet, push the session branch, `create_pull_request`
   (base `main`, draft), `subscribe_pr_activity`, and let CI go green before the bump.
1. **Bump.** On the pull request that is already green: `version` in `Cargo.toml` and the `newq`
   entry in `Cargo.lock` (`cargo check` updates it). Add the version's section to `CHANGELOG.md`
   (`## X.Y.Z`, moved from `## Unreleased`): plain words for people who run shows, what they will
   notice, not commit subjects. `scripts/changelog_section.sh vX.Y.Z` prints it; the dry run fails
   without one, and the Release workflow puts it at the top of the release page. Commit "Bump
   version to X.Y.Z". The bump needs `cargo fmt --check` and one CI round, not a full local check:
   the code under it was already checked (`scripts/check.sh full`) and is green on all three
   systems. If code changed since the last green round, run `scripts/check.sh full` first: one
   clean push beats three.
2. **Push.** Push the bump to the pull request. Start the Monitor for CI (below) in the same turn.
3. **CI.** On each system a tests job and a self-test job, plus `gate`. On a failure, read the job
   log (`get_job_logs` with the `job_id` from the event) and decide:
   - Ours (a test failed): fix, run `scripts/check.sh quick <the area>`, push. The push restarts CI.
     A test that passes when run again goes in `.claude/notes/flaky-tests.md`.
   - Infrastructure (Chocolatey/apt/brew 5xx or timeout before any test ran, a runner
     that never started): comment once on the PR, then `actions_run_trigger`
     `rerun_failed_jobs` on the run. It answers 403 "already running" while other jobs of
     the run are still going: that is not a failure, wait for the run to finish and retry
     then. Re-run once; a second failure is real.
   - Known trap: Homebrew's FFmpeg has no `hap` encoder. Tests that make Hap files use
     `test_media_with_hap` and skip. Timing tests must not assert upper bounds on
     counts: CI runners oversleep.
4. **Merge.** When all checks are green on the head, mark the pull request ready
   (`update_pull_request` with `draft: false`), then `merge_pull_request` with
   `merge_method: rebase` (never merge or squash: `main` stays linear).
5. **Main CI.** The rebase merge has the same tree, so the `gate` job skips the builds and
   the run is quick. `get_workflow_run` on that run's id (find it from the one run
   with `head_sha` = the merge `sha` that the merge call returned). It must be
   `completed`/`success`.
6. **Dry run.** `actions_run_trigger` `run_workflow` with `workflow_id: cut-release.yml`,
   `ref: main`, `inputs: {"tag": "vX.Y.Z", "dry_run": "true"}`. Find the new run and wait
   for `success` (about 20 seconds).
7. **Real run.** The same with `"dry_run": "false"`. It tags `main` and starts the
   `Release` workflow (16 to 21 minutes on 0.26.0 and 0.27.0, before the Mac release was
   built as two parallel halves; signing, notarising and stapling take about 90 seconds of it).
8. **Verify.** The `Release` run must be `success`, then `get_release_by_tag vX.Y.Z`
   must exist with the Windows, macOS and Linux downloads. Report the link and what is
   in it. A `Release` failure: `get_job_logs`, fix or report, do not stop silently.
9. After the merge, delete the session branch on GitHub (`git push origin --delete <session-branch>`;
   the repository's "Automatically delete head branches" setting does it by itself), then
   `git fetch origin main && git checkout -B <session-branch> origin/main`. A rebase merge
   gives the commits new ids on `main`, so a branch left on GitHub still holds the old copies
   and the next push would need a force-push; with it deleted, the next push makes it afresh.

Do not post `/release` as a comment: the comment tool mangles slash commands, and the
workflow is meant to be started by `workflow_dispatch` (owner's token).

## Tool pitfalls (all hit before)

- `actions_list list_workflow_runs` ignores `per_page` and returns ~100 runs, a huge
  output. Prefer `actions_get get_workflow_run` with a run id, and find the id of a run you
  just started with `scripts/gh_wait.sh latest <workflow>.yml`.
- `get_job_logs` needs `job_id` (not `run_id`) unless `failed_only` with `run_id`; use
  `tail_lines` (60 to 80).
- Your own PR comments come back as `issue_comment.created` events from the same account:
  that is an echo, not a request.
- A `check_suite.completed` event with no failures means CI is done, but confirm with
  `pull_request_read get_check_runs` before merging.
- Never `pkill -f target/debug/newq` from a shell whose command line contains that
  string: it kills the shell. Use `pkill -x newq`.
- Cloud sessions can't push tags; releases go only through the workflow.

## Staying with it

Wait with the Monitor tool and `scripts/gh_wait.sh --for 25m` (one Monitor call per wait,
`persistent: false`; on "still running" start another), never with scheduled tasks (`send_later`/`create_trigger` are refused here;
`delete_trigger` any you made):

```
scripts/gh_wait.sh --for 25m pr 34              # all checks on the PR head finished (exit 0 = all passed)
scripts/gh_wait.sh latest ci.yml                # id/status/sha of the newest run (find the main CI run)
scripts/gh_wait.sh --for 25m run 40796485911    # a workflow run finished (exit 0 = success)
scripts/gh_wait.sh latest cut-release.yml       # find the dry/real run you just dispatched
scripts/gh_wait.sh latest release.yml           # the Release run it started
scripts/gh_wait.sh --for 25m release v0.12.0    # the release exists; prints its assets
```

So the loop is: do a step, start the Monitor for the next wait **in the same turn**, and when
it reports, do the next step. Chain through step 8 without waiting to be prompted. Stop only
when the release is verified, you are blocked (say what you need in one line), or the user says
stop. The same script serves any "wait for CI" in other work.
