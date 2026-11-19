#!/bin/sh
# Fast-forwards main to a commit whose train smoke went green (smoke.yml ff-main, T-7420): only
# when ci.yml's push run on jc-portal-1 at that commit is green too and main is an ancestor of it.
# Never a force, never a merge commit: GitHub's compare says `ahead` (main is an ancestor, the git
# merge-base --is-ancestor test without a full-history clone) and the ref update is sent with
# force=false, so the server refuses anything but a fast-forward as well. A push by GITHUB_TOKEN
# starts no workflow, so the script then dispatches ci.yml on main, which ships it like a push
# (ci.yml `release`), and deploy-pin.yml with the sha: a run a GITHUB_TOKEN dispatch created fires
# no workflow_run either, so deploy-pin's dispatch path waits for that main ci itself
# (`--await-main-ci`, its job `wait`) and pins only on green. Writes the old and new main to $GITHUB_STEP_SUMMARY; a
# commit it leaves main behind is a summary line and exit 0 (the smoke verdict stays the signal).
#   GITHUB_REPOSITORY=o/r SHA=<40 hex> sh ff-main.sh    # in smoke.yml, GH_TOKEN set
#   GITHUB_REPOSITORY=o/r SHA=<40 hex> sh ff-main.sh --await-main-ci   # deploy-pin.yml wait
#   sh ff-main.sh --selftest
set -eu

# ci_state <sha> [event branch]: green when a run of ci.yml for that event and branch (default a
# push to jc-portal-1) in this repository concluded success on that commit, pending while one is
# queued or running, none without a run, red when all failed. The query's filters are repeated in
# jq: the API's are not the trust boundary.
ci_state() {
  gh api "repos/$GITHUB_REPOSITORY/actions/workflows/ci.yml/runs?head_sha=$1&event=${2:-push}&branch=${3:-jc-portal-1}&per_page=50" |
    jq -r --arg repo "$GITHUB_REPOSITORY" --arg sha "$1" --arg ev "${2:-push}" --arg br "${3:-jc-portal-1}" '
      [.workflow_runs[]
        | select(.event == $ev and .head_sha == $sha and .head_branch == $br
            and .path == ".github/workflows/ci.yml"
            and .repository.full_name == $repo and .head_repository.full_name == $repo)]
      | if any(.[]; .status == "completed" and .conclusion == "success") then "green"
        elif any(.[]; .status != "completed") then "pending"
        elif length == 0 then "none"
        else "red" end'
}

check_sha() {
  case "$1" in *[!0-9a-f]* | "") say "ff-main: not a commit sha: [$1]"; return 1 ;; esac
  [ "${#1}" = 40 ] || { say "ff-main: not a commit sha: [$1]"; return 1; }
}

# await_main_ci <sha>: waits up to WAIT seconds for the ci.yml run ff() dispatched on main to
# finish (a fresh dispatch can take a moment to be listed, so none waits too); prints ok=true only
# when it is green and main still points at the commit, for deploy-pin.yml's pin job.
await_main_ci() {
  sha=$1 waited=0
  check_sha "$sha" || return 1
  while :; do
    state=$(ci_state "$sha" workflow_dispatch main)
    case "$state" in pending | none) ;; *) break ;; esac
    [ "$waited" -lt "${WAIT:-7200}" ] || break
    sleep "${POLL:-60}"; waited=$((waited + ${POLL:-60}))
  done
  if [ "$state" != green ]; then
    say "deploy-pin: no pin: main ci on $sha is $state (after ${waited}s)"; echo ok=false; return 0
  fi
  say "deploy-pin: main ci on $sha is green (after ${waited}s)"; echo ok=true
}

say() { echo "$1"; echo "$1" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"; }

# ff <sha>: the whole decision; WAIT seconds for a pending ci, polled every POLL seconds
ff() {
  sha=$1 waited=0
  check_sha "$sha" || return 1
  while :; do
    state=$(ci_state "$sha")
    if [ "$state" != pending ] || [ "$waited" -ge "${WAIT:-3600}" ]; then break; fi
    sleep "${POLL:-60}"; waited=$((waited + ${POLL:-60}))
  done
  if [ "$state" != green ]; then
    say "ff-main: main left as is: ci on $sha is $state (after ${waited}s)"; return 0
  fi
  old=$(gh api "repos/$GITHUB_REPOSITORY/git/ref/heads/main" --jq .object.sha)
  if [ "$old" = "$sha" ]; then say "ff-main: main already at $sha"; return 0; fi
  rel=$(gh api "repos/$GITHUB_REPOSITORY/compare/$old...$sha" --jq .status)
  if [ "$rel" != ahead ]; then
    say "ff-main: main left at $old: compare main...$sha is $rel, not a fast-forward"; return 0
  fi
  gh api -X PATCH "repos/$GITHUB_REPOSITORY/git/refs/heads/main" -f sha="$sha" -F force=false > /dev/null
  say "ff-main: main $old -> $sha"
  gh api -X POST "repos/$GITHUB_REPOSITORY/actions/workflows/ci.yml/dispatches" -f ref=main > /dev/null
  say "ff-main: ci.yml dispatched on main (ships $sha)"
  gh api -X POST "repos/$GITHUB_REPOSITORY/actions/workflows/deploy-pin.yml/dispatches" -f ref=main -f "inputs[sha]=$sha" > /dev/null
  say "ff-main: deploy-pin.yml dispatched on main (pins $sha once that ci is green)"
}

if [ "${1:-}" = --selftest ]; then
  GITHUB_REPOSITORY=o/r GITHUB_STEP_SUMMARY=/dev/null POLL=1 WAIT=2
  A=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa M=1111111111111111111111111111111111111111
  run() { # status conclusion event branch head-repo
    printf '{"status":"%s","conclusion":%s,"event":"%s","head_sha":"%s","head_branch":"%s","path":".github/workflows/ci.yml","repository":{"full_name":"o/r"},"head_repository":{"full_name":"%s"}}' \
      "$1" "$2" "$3" "$A" "$4" "$5"
  }
  # case: <runs json> <main sha> <compare status> -> the gh calls made, in order
  case_() {
    RUNS=$1 MAIN=$2 REL=$3 CALLS=
    gh() {
      CALLS="$CALLS|$*"
      case "$*" in
        *"/workflows/ci.yml/runs?"*) printf '{"workflow_runs":[%s]}' "$RUNS" ;;
        *"git/ref/heads/main --jq"*) echo "$MAIN" ;;
        *"/compare/"*) echo "$REL" ;;
        "api -X PATCH repos/o/r/git/refs/heads/main -f sha=$A -F force=false") ;;
        "api -X POST repos/o/r/actions/workflows/ci.yml/dispatches -f ref=main") ;;
        "api -X POST repos/o/r/actions/workflows/deploy-pin.yml/dispatches -f ref=main -f inputs[sha]=$A") ;;
        *) echo "unexpected gh $*" >&2; return 1 ;;
      esac
    }
    ff "$A" > /dev/null
    echo "$CALLS" | grep -c 'PATCH' || true
  }
  green=$(run completed '"success"' push jc-portal-1 o/r)
  red=$(run completed '"failure"' push jc-portal-1 o/r)
  want() { [ "$2" = "$3" ] || { echo "FAIL: $1 -> moved=$2, want $3"; exit 1; }; }
  want "green smoke + green ci moves main" "$(case_ "$green" $M ahead)" 1
  case_ "$green" $M ahead > /dev/null
  echo "$CALLS" | grep -q 'ci.yml/dispatches -f ref=main|' || { echo "FAIL: no ci dispatch after the move"; exit 1; }
  echo "$CALLS" | grep -q "deploy-pin.yml/dispatches -f ref=main -f inputs\[sha\]=$A\$" || { echo "FAIL: no deploy-pin dispatch after the move"; exit 1; }
  want "red ci leaves main" "$(case_ "$red" $M ahead)" 0
  want "a re-run that went green counts" "$(case_ "$red,$green" $M ahead)" 1
  want "no ci run leaves main" "$(case_ "" $M ahead)" 0
  want "ci still running past WAIT leaves main" "$(case_ "$(run in_progress null push jc-portal-1 o/r)" $M ahead)" 0
  want "main not an ancestor (diverged) leaves main" "$(case_ "$green" $M diverged)" 0
  want "main ahead of the commit leaves main" "$(case_ "$green" $M behind)" 0
  want "main already there is no write" "$(case_ "$green" $A identical)" 0
  want "a fork's green run does not count" "$(case_ "$(run completed '"success"' push jc-portal-1 fork/r)" $M ahead)" 0
  want "a pull_request run does not count" "$(case_ "$(run completed '"success"' pull_request jc-portal-1 o/r)" $M ahead)" 0
  want "another branch's run does not count" "$(case_ "$(run completed '"success"' push agent/x o/r)" $M ahead)" 0
  if ff "not-a-sha" > /dev/null 2>&1; then echo "FAIL: a bad sha passed"; exit 1; fi
  # await: <runs json> -> the ok= line deploy-pin's pin job reads
  await() {
    RUNS=$1
    gh() {
      case "$*" in
        *"/workflows/ci.yml/runs?head_sha=$A&event=workflow_dispatch&branch=main&"*) printf '{"workflow_runs":[%s]}' "$RUNS" ;;
        *) echo "unexpected gh $*" >&2; return 1 ;;
      esac
    }
    await_main_ci "$A" | grep '^ok='
  }
  disp() { run "$1" "$2" workflow_dispatch "$3" "$4"; }
  want "await: green main ci pins" "$(await "$(disp completed '"success"' main o/r)")" ok=true
  want "await: red main ci does not pin" "$(await "$(disp completed '"failure"' main o/r)")" ok=false
  want "await: no main ci within WAIT does not pin" "$(await "")" ok=false
  want "await: main ci still running past WAIT does not pin" "$(await "$(disp in_progress null main o/r)")" ok=false
  want "await: a push run's green does not count" "$(await "$(run completed '"success"' push main o/r)")" ok=false
  want "await: a dispatch on another branch does not count" "$(await "$(disp completed '"success"' jc-portal-1 o/r)")" ok=false
  want "await: a fork's run does not count" "$(await "$(disp completed '"success"' main fork/r)")" ok=false
  if await_main_ci "not-a-sha" > /dev/null 2>&1; then echo "FAIL: await took a bad sha"; exit 1; fi
  unset -f gh
  echo "ff-main.sh selftest ok"
  exit 0
fi

if [ "${1:-}" = --await-main-ci ]; then
  await_main_ci "${SHA:?SHA: the commit ff-main moved main to}"
  exit 0
fi
ff "${SHA:?SHA: the commit the smoke run tested}"
