#!/usr/bin/env bash
# Where a crashed test program was when it died (Linux CI only).
#
#   scripts/crash_backtrace.sh setup   before the tests: core files to /tmp/cores, gdb installed
#   scripts/crash_backtrace.sh report  after a failure: every core's stacks, through gdb
#
# The tests themselves must run with `ulimit -c unlimited` in the same shell (a limit
# set in another step doesn't carry over). A test program that dies by a signal
# (SIGSEGV) prints no backtrace of its own; this is how to see whether the fault is
# in NewQ's code or a library's. See .claude/notes/flaky-tests.md.
set -u

case "${1:-}" in
setup)
    [ "$(uname)" = Linux ] || exit 0
    mkdir -p /tmp/cores
    chmod 1777 /tmp/cores
    # Ubuntu sends cores to apport by default, which keeps none for us.
    # %E is the program's whole path with each / as !, so the report can find it
    # (the path inside the core is cut at 80 characters).
    sudo sysctl -w kernel.core_pattern='/tmp/cores/core.%p.%E'
    command -v gdb > /dev/null || sudo apt-get -o Acquire::Retries=3 install -y gdb > /dev/null
    echo "core files go to /tmp/cores; gdb: $(gdb --version | head -1)"
    ;;
report)
    [ "$(uname)" = Linux ] || exit 0
    shopt -s nullglob
    cores=(/tmp/cores/core.*)
    if [ ${#cores[@]} -eq 0 ]; then
        echo "no core file: nothing crashed by a signal (or cores weren't allowed)"
        exit 0
    fi
    for core in "${cores[@]}"; do
        # The program's path, from the core's name (core.<pid>.<path with ! for />).
        name=${core##*/}
        exe=${name#core.*.}
        exe=${exe//!//}
        echo "::group::$core ($exe)"
        if [ -n "$exe" ] && [ -x "$exe" ]; then
            gdb -batch -q \
                -ex 'set pagination off' \
                -ex 'echo \n=== the thread that crashed\n' -ex 'bt 40' \
                -ex 'echo \n=== every thread\n' -ex 'thread apply all bt 25' \
                "$exe" "$core" 2>&1 | grep -v '^\[New LWP' | head -2000
        else
            echo "the program ($exe) isn't there to read the core with"
        fi
        echo "::endgroup::"
    done
    ;;
*)
    echo "usage: $0 setup|report" >&2
    exit 2
    ;;
esac
