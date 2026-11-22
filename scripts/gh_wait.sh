#!/usr/bin/env bash
# Wait for something on GitHub to finish, with plain polling (the repository is public, so
# no token is needed; $GITHUB_TOKEN is used if set). Meant to run under Claude Code's
# Monitor tool, which wakes the session when it exits, instead of scheduled tasks
# (those have been refused with ACCESS_DENIED).
#
#   gh_wait.sh run RUN_ID            wait for a workflow run; exit 0 if it succeeded
#   gh_wait.sh pr NUMBER             wait for every check on a PR's head; exit 0 if all passed
#   gh_wait.sh release TAG           wait for a release to exist; prints its assets
#   gh_wait.sh latest WORKFLOW.yml   print the id and status of the newest run of a workflow
#
#   --for 25m (before the command; s, m or h, or plain seconds): give up waiting
#   after that long, print "still running: ..." and exit 4, so a watch ends by
#   design before the Monitor's 30-minute limit. Start it again to keep waiting.
#
# One request a minute: the unauthenticated limit is 60 an hour.
set -u
REPO="${GH_REPO:-r2a42/NewQ}"
EVERY="${GH_WAIT_SECS:-60}"
STILL_RUNNING=4
deadline=""
if [ "${1:-}" = --for ]; then
  limit="${2:-}"
  [[ "$limit" =~ ^[0-9]+[smh]?$ ]] || { echo "--for needs a time such as 25m"; exit 2; }
  case "$limit" in
    *h) secs=$(( ${limit%h} * 3600 )) ;;
    *m) secs=$(( ${limit%m} * 60 )) ;;
    *s) secs="${limit%s}" ;;
    *) secs="$limit" ;;
  esac
  [[ "$secs" =~ ^[0-9]+$ ]] || { echo "--for needs a time such as 25m"; exit 2; }
  deadline=$((SECONDS + secs))
  shift 2
fi
# wait_a_bit SECS WHAT: sleep, but never past --for; past it, say so and exit.
wait_a_bit() {
  local s="$1"
  if [ -n "$deadline" ]; then
    local left=$((deadline - SECONDS))
    if [ "$left" -le 0 ]; then echo "still running: $2"; exit "$STILL_RUNNING"; fi
    [ "$s" -gt "$left" ] && s="$left"
  fi
  sleep "$s"
}
api() {
  local auth=()
  [ -n "${GITHUB_TOKEN:-}" ] && auth=(-H "Authorization: Bearer $GITHUB_TOKEN")
  curl -sS -m 20 "${auth[@]}" "https://api.github.com/repos/$REPO/$1"
}
field() { python3 -c "import sys,json; d=json.load(sys.stdin); print($1)" 2>/dev/null; }
limited() { grep -qi "rate limit" <<<"$1"; }

case "${1:-}" in
run)
  id="${2:?run id}"
  while :; do
    r=$(api "actions/runs/$id")
    if limited "$r"; then echo "rate limited, waiting 5 minutes"; wait_a_bit 300 "run $id"; continue; fi
    st=$(field "d.get('status')" <<<"$r"); co=$(field "d.get('conclusion')" <<<"$r")
    if [ "$st" = completed ]; then echo "run $id: $co"; [ "$co" = success ]; exit $?; fi
    wait_a_bit "$EVERY" "run $id ${st:-unknown}"
  done ;;
pr)
  n="${2:?pr number}"
  sha=$(api "pulls/$n" | field "d['head']['sha']")
  [ -n "$sha" ] || { echo "no such PR"; exit 2; }
  while :; do
    r=$(api "commits/$sha/check-runs?per_page=100")
    if limited "$r"; then echo "rate limited, waiting 5 minutes"; wait_a_bit 300 "PR $n"; continue; fi
    out=$(field "'%d %d %s' % (len(d['check_runs']), sum(c['status']!='completed' for c in d['check_runs']), ','.join(c['name']+'='+str(c['conclusion']) for c in d['check_runs']))" <<<"$r")
    total=${out%% *}; rest=${out#* }; pending=${rest%% *}; detail=${rest#* }
    if [ "${total:-0}" -gt 0 ] && [ "${pending:-1}" -eq 0 ]; then
      echo "PR $n head ${sha:0:8}: $detail"
      grep -qE '=(failure|cancelled|timed_out|action_required)' <<<"$detail" && exit 1
      exit 0
    fi
    wait_a_bit "$EVERY" "PR $n head ${sha:0:8}: ${pending:-?} of ${total:-?} checks not done"
  done ;;
release)
  tag="${2:?tag}"
  while :; do
    r=$(api "releases/tags/$tag")
    if limited "$r"; then echo "rate limited, waiting 5 minutes"; wait_a_bit 300 "release $tag"; continue; fi
    url=$(field "d.get('html_url') or ''" <<<"$r")
    if [ -n "$url" ]; then
      echo "$url"; field "'\n'.join(a['name'] for a in d['assets'])" <<<"$r"; exit 0
    fi
    wait_a_bit "$EVERY" "release $tag not there yet"
  done ;;
latest)
  w="${2:?workflow file}"
  api "actions/workflows/$w/runs?per_page=1" | field "'%s %s %s %s' % (d['workflow_runs'][0]['id'], d['workflow_runs'][0]['status'], d['workflow_runs'][0]['conclusion'], d['workflow_runs'][0]['head_sha'][:8])" ;;
*) sed -n '2,16p' "$0"; exit 2 ;;
esac
