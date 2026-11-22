#!/usr/bin/env bash
# The checks to run before handing work on, in one command.
#
#   scripts/check.sh quick [filter...]   fmt, clippy, and `cargo test` of the given
#                                        filters (e.g. ui::timeline stage3d), or all
#                                        tests when none are given. For an agent's
#                                        own change.
#   scripts/check.sh full                fmt, clippy, the whole suite as CI's Linux
#                                        job runs it (lavapipe, real media, GPU
#                                        required), then cross clippy for Windows
#                                        and the Mac when the change can need it.
#                                        For the integrator, once, alone on the machine.
#
# Environment:
#   CARGO_TARGET_DIR   honoured as cargo honours it.
#   CHECK_CARGO_ARGS   extra arguments for cargo clippy and cargo test (split on
#                      spaces), e.g. "--config profile.dev.package.newq.codegen-units=236".
#   CHECK_BASE         what a change is measured against for cross clippy
#                      (default origin/main).
#
# Cross clippy (scripts/cross_clippy.sh, its folders kept under $TMPDIR) runs
# when the commits since CHECK_BASE, or uncommitted or untracked files, touch
# Cargo.toml, Cargo.lock, build.rs or a file with code for one system only
# (cfg(windows), cfg(unix), cfg(target_os ...)). Otherwise it is skipped and the
# script says so. The rule is not watertight: a shared function changed in an
# ordinary file can still break a Windows-only caller; CI catches that.
#
# Prints a line per step with its time, stops at the first failure and exits
# with its status.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

read -r -a extra <<< "${CHECK_CARGO_ARGS:-}"
summary=()

# step NAME COMMAND...: run it, time it, stop the script if it fails.
step() {
  local name="$1" start=$SECONDS rc
  shift
  echo "== $name: $*"
  "$@"
  rc=$?
  local line
  if [ "$rc" -eq 0 ]; then
    line="$(printf '%-28s ok      %4ds' "$name" $((SECONDS - start)))"
  else
    line="$(printf '%-28s FAILED  %4ds  (exit %d)' "$name" $((SECONDS - start)) "$rc")"
  fi
  echo "== $line"
  summary+=("$line")
  if [ "$rc" -ne 0 ]; then
    finish "$rc"
  fi
}

note() {
  echo "== $1"
  summary+=("$1")
}

finish() {
  echo
  echo "== summary"
  printf '   %s\n' "${summary[@]}"
  exit "$1"
}

common() {
  step "fmt" cargo fmt --check
  step "clippy" cargo clippy ${extra[@]+"${extra[@]}"} --all-targets -- -D warnings
}

# Does the change reach code that compiles differently on Windows or the Mac?
# Prints the reason when it does; prints nothing when it doesn't.
cross_reason() {
  local base="${CHECK_BASE:-origin/main}"
  if ! git rev-parse --verify -q "$base" > /dev/null; then
    echo "$base is not known here, so the change can't be measured"
    return
  fi
  local files
  files="$( {
    git diff --name-only "$base...HEAD"
    git diff --name-only HEAD
    git ls-files --others --exclude-standard
  } | sort -u)"
  local build
  build="$(grep -E '^(Cargo\.toml|Cargo\.lock|build\.rs)$' <<< "$files" | tr '\n' ' ')"
  if [ -n "$build" ]; then
    echo "it touches $build"
    return
  fi
  local cfgs
  # Rust files only (these notes and this script name the cfgs too); xargs -r:
  # with no files, git grep would search everything.
  cfgs="$(printf '%s\n' "$files" | grep -E '\.rs$' | tr '\n' '\0' |
    xargs -0 -r git grep --untracked -lE 'cfg\((windows|unix|target_os|not\((windows|unix|target_os)|any\(target_os|all\(target_os)' -- 2> /dev/null |
    tr '\n' ' ' || true)"
  if [ -n "$cfgs" ]; then
    echo "it touches code for one system only: $cfgs"
  fi
}

case "${1:-}" in
quick)
  shift
  common
  if [ "$#" -gt 0 ]; then
    step "test ($*)" cargo test ${extra[@]+"${extra[@]}"} -- "$@"
  else
    step "test" cargo test ${extra[@]+"${extra[@]}"}
  fi
  finish 0
  ;;
full)
  # NEWQ_REQUIRE_MEDIA fails every media test without it: say so in seconds.
  # (test_media/ is git-ignored, so a fresh worktree has none.)
  if [ ! -f test_media/testsrc_10s.mp4 ]; then
    note "no test media in test_media/: run scripts/make_test_media.sh first"
    finish 1
  fi
  common
  step "test (whole suite)" env WGPU_BACKEND=vulkan NEWQ_REQUIRE_MEDIA=1 NEWQ_REQUIRE_GPU=1 \
    cargo test ${extra[@]+"${extra[@]}"}
  reason="$(cross_reason)"
  if [ -n "$reason" ]; then
    note "cross clippy runs: $reason"
    # One at a time, each in the folder it keeps between runs.
    step "cross clippy (Windows)" scripts/cross_clippy.sh windows
    step "cross clippy (Mac)" scripts/cross_clippy.sh mac
  else
    note "cross clippy SKIPPED: nothing since ${CHECK_BASE:-origin/main} touches Cargo.toml, Cargo.lock, build.rs or a file with cfg(windows|unix|target_os)"
  fi
  finish 0
  ;;
*)
  sed -n '2,12p' "$0"
  exit 2
  ;;
esac
