#!/bin/sh
# Fast-forwards main to a commit whose train smoke went green (smoke.yml ff-main, T-7420): only
# when ci.yml's push run on jc-portal-1 at that commit is green too and main is an ancestor of it.
# Never a force, never a merge commit: GitHub's compare says `ahead` (main is an ancestor, the git
# merge-base --is-ancestor test without a full-history clone) and the ref update is sent with
# force=false, so the server refuses anything but a fast-forward as well. A push by GITHUB_TOKEN
# starts no workflow, so the script then dispatches ci.yml on main, which ships it like a push
# (ci.yml `release`, deploy-pin.yml). Writes the old and new main to $GITHUB_STEP_SUMMARY; a
# commit it leaves main behind is a summary line and exit 0 (the smoke verdict stays the signal).
#   GITHUB_REPOSITORY=o/r SHA=<40 hex> sh ff-main.sh    # in smoke.yml, GH_TOKEN set
#   sh ff-main.sh --selftest
set -eu

# ci_state <sha>: green when a push run of ci.yml on jc-portal-1 in this repository concluded
# success on that commit, pending while one is queued or running, red otherwise (none, or all
# failed). The query's filters are repeated in jq: the API's are not the trust boundary.
ci_state() {
  gh api "repos/$GITHUB_REPOSITORY/actions/workflows/ci.yml/runs?head_sha=$1&event=push&branch=jc-portal-1&per_page=50" |
    jq -r --arg repo "$GITHUB_REPOSITORY" --arg sha "$1" '
      [.workflow_runs[]
        | select(.event == "push" and .head_sha == $sha and .head_branch == "jc-portal-1"
            and .path == ".github/workflows/ci.yml"
            and .repository.full_name == $repo and .head_repository.full_name == $repo)]
      | if any(.[]; .status == "completed" and .conclusion == "success") then "green"
        elif any(.[]; .status != "completed") then "pending"
        else "red" end'
}

say() { echo "$1"; echo "$1" >> "${GITHUB_STEP_SUMMARY:-/dev/null}"; }

# ff <sha>: the whole decision; WAIT seconds for a pending ci, polled every POLL seconds
ff() {
  sha=$1 waited=0
  case "$sha" in *[!0-9a-f]* | "") say "ff-main: not a commit sha: [$sha]"; return 1 ;; esac
  [ "${#sha}" = 40 ] || { say "ff-main: not a commit sha: [$sha]"; return 1; }
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
  echo "$CALLS" | grep -q 'dispatches -f ref=main$' || { echo "FAIL: no ci dispatch after the move"; exit 1; }
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
  unset -f gh
  echo "ff-main.sh selftest ok"
  exit 0
fi

ff "${SHA:?SHA: the commit the smoke run tested}"
