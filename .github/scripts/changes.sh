#!/bin/sh
# Which ci.yml jobs a change needs: reads the changed paths (one per line) and prints
# `job=true|false` lines, to $GITHUB_OUTPUT when set. Without a usable base (a dispatch, a new
# branch, a base git cannot reach) every job runs.
# `ship` says whether the push changes what the jc image holds (ci.yml builds and promotes one on
# main and ship-proof/** only). With REUSE=1 (a push to those refs) a check job whose exact
# name already concluded success in another ci run of this same commit (a fast-forward merge of a
# branch head ci passed) is not run again: the same commit is the same input, workflow file
# included. supply-chain, gitleaks and the image always run.
#   EVENT=push BEFORE=<sha> sh changes.sh      # in ci.yml (reads GITHUB_REF)
#   sh changes.sh --selftest
set -eu
JOBS="root antares ekiben portal ui supply workflows image dockerfiles platform"

# map: changed paths on stdin -> the job lines
map() {
  awk -v jobs="$JOBS" '
    BEGIN { n = split(jobs, j, " "); for (i = 1; i <= n; i++) on[j[i]] = "false" }
    # docs never run a build: Markdown anywhere, the docs/ tree and the nested trees docs
    /\.md$/ || /^docs\// || /^crates\/[^\/]+\/docs\// { next }
    /^\.github\// { on["workflows"] = "true" }
    /^\.github\/workflows\/ci\.yml$/ || /^\.github\/scripts\// { for (k in on) on[k] = "true"; next }
    /^\.github\// { next }
    /^bins\/jc\/Dockerfile$/ || /^\.dockerignore$/ { on["image"] = "true" }
    # the App bundles both images ship in /srv/apps: their lists must stay equal
    /^bins\/jc\/Dockerfile$/ || /^crates\/portal\/Dockerfile$/ || /^crates\/portal\/apps\// || /^crates\/portal\/scripts\/(app-integrity\.mjs|ci\/check-dockerfile-members\.py)$/ { on["dockerfiles"] = "true" }
    /(^|\/)Cargo\.lock$/ || /(^|\/)deny\.toml$/ || /(^|\/)\.cargo\/audit\.toml$/ { on["supply"] = "true" }
    # the vendored platform crates (platform-test), the assistant aside (a root member), and the
    # root lockfile and manifest its lockfile repeats
    /^vendor\/joinedcontext-platform\// && !/^vendor\/joinedcontext-platform\/crates\/assistant\// { on["platform"] = "true" }
    /^Cargo\.(lock|toml)$/ { on["platform"] = "true" }
    /^crates\/antares\// { on["antares"] = "true" }
    /^crates\/ekiben\// { on["ekiben"] = "true" }
    /^crates\/portal\/(ui|sdk)\// { on["ui"] = "true" }
    /^crates\/portal\/ui\// { next }
    /^crates\/portal\// || /^vendor\// { on["portal"] = "true" }
    # jc links every module, the nested workspaces included, so any other build input reaches it
    { on["root"] = "true" }
    END { for (i = 1; i <= n; i++) print j[i] "=" on[j[i]] }'
}

all() { for j in $JOBS; do echo "$j=true"; done; }

# trunk <ref>: the refs that keep their own base (`before`): each merged head is checked against
# the head before it
trunk() { case "$1" in refs/heads/main | refs/heads/jc-portal-1 | refs/heads/ship-proof/*) return 0 ;; esac; return 1; }

# ship: the image holds the Rust build (root reaches every crate) and the embedded UI
ship() { awk -F= '($1 == "root" || $1 == "ui" || $1 == "image") && $2 == "true" { s = 1 } END { print "ship=" (s ? "true" : "false") }'; }

# covered <job names that succeeded, one per line>: turns off each key all of whose jobs are in it
covered() {
  printf '%b\n' "$1" > "${TMPDIR:-/tmp}/ci-done.$$"
  awk -v done="${TMPDIR:-/tmp}/ci-done.$$" '
    BEGIN {
      while ((getline l < done) > 0) ok[l] = 1
      # every job of a key, each test partition of ci.yml (matrix part) by its name
      jobs["root"] = "root-clippy root-test-1 root-test-2 root-test-3 root-test-4"
      jobs["antares"] = "antares-clippy antares-test-1 antares-test-2 antares-test-3 antares-test-4"
      jobs["platform"] = "platform-test"
      jobs["ekiben"] = "ekiben"; jobs["portal"] = "portal-clippy portal-test-1 portal-test-2 portal-test-3 portal-test-4"; jobs["ui"] = "ui ui-test-1 ui-test-2 ui-test-3 ui-test-4"
    }
    {
      split($0, kv, "=")
      if (kv[2] == "true" && (kv[1] in jobs)) {
        n = split(jobs[kv[1]], need, " "); all = 1
        for (i = 1; i <= n; i++) if (!(need[i] in ok)) all = 0
        if (all) { print kv[1] "=false"; next }
      }
      print
    }'
  rm -f "${TMPDIR:-/tmp}/ci-done.$$"
}

# The names of the jobs that concluded success in any other ci run of this commit.
done_jobs() {
  gh api "repos/$GITHUB_REPOSITORY/actions/workflows/ci.yml/runs?head_sha=$GITHUB_SHA&per_page=20" \
    --jq ".workflow_runs[] | select(.id != $GITHUB_RUN_ID) | .id" |
    while read -r id; do
      gh api --paginate "repos/$GITHUB_REPOSITORY/actions/runs/$id/jobs?per_page=100" \
        --jq '.jobs[] | select(.conclusion == "success") | .name'
    done | sort -u
}

if [ "${1:-}" = --selftest ]; then
  check() { # paths expected
    got=$(printf '%b' "$1" | map | grep '=true' | cut -d= -f1 | tr '\n' ' ' | sed 's/ $//')
    [ "$got" = "$2" ] || { echo "FAIL: [$1] -> [$got], want [$2]"; exit 1; }
  }
  check 'README.md\ndocs/a.txt\ncrates/antares/docs/x.html' ''
  check 'crates/kaisatsu/core/src/lib.rs' 'root'
  check 'crates/antares/crates/antares-broker/src/lib.rs' 'root antares'
  check 'crates/ekiben/Cargo.lock' 'root ekiben supply'
  check 'crates/portal/ui/src/App.tsx' 'ui'
  check 'crates/portal/sdk/src/sdk/index.ts' 'root portal ui'
  check 'crates/portal/apps/bbsk-mapa/ui/src/App.tsx' 'root portal dockerfiles'
  check 'crates/portal/Dockerfile' 'root portal dockerfiles'
  check 'vendor/ngsild/x.rs' 'root portal'
  check 'vendor/joinedcontext-platform/crates/context-gateway/src/lib.rs' 'root portal platform'
  check 'vendor/joinedcontext-platform/crates/assistant/src/lib.rs' 'root portal'
  check 'bins/jc/Dockerfile' 'root image dockerfiles'
  check 'Cargo.lock' 'root supply platform'
  check '.github/workflows/image.yml' 'workflows'
  check '.github/scripts/changes.sh' "$JOBS"
  for r in refs/heads/jc-portal-1 refs/heads/main refs/heads/ship-proof/x; do trunk "$r" || { echo "FAIL: trunk $r"; exit 1; }; done
  for r in refs/heads/jc-ui-node24 refs/pull/1/merge; do ! trunk "$r" || { echo "FAIL: branch $r"; exit 1; }; done
  [ "$(printf 'root=false\nui=true\nimage=false\n' | ship)" = ship=true ] || { echo "FAIL: ship ui"; exit 1; }
  [ "$(printf 'root=false\nui=false\nimage=false\n' | ship)" = ship=false ] || { echo "FAIL: ship docs"; exit 1; }
  [ "$(printf 'root=true\nportal=true\nsupply=true\n' | covered 'root-clippy\nroot-test-1\nroot-test-2\nroot-test-3\nroot-test-4\nportal-clippy\nportal-test-1')" = "$(printf 'root=false\nportal=true\nsupply=true')" ] \
    || { echo "FAIL: covered"; exit 1; }
  [ "$(printf 'portal=true\n' | covered 'portal-clippy\nportal-test-1\nportal-test-2\nportal-test-3')" = portal=true ] \
    && [ "$(printf 'portal=true\n' | covered 'portal-clippy\nportal-test-1\nportal-test-2\nportal-test-3\nportal-test-4')" = portal=false ] \
    || { echo "FAIL: covered portal parts"; exit 1; }
  echo "changes.sh selftest ok"
  exit 0
fi

out=${GITHUB_OUTPUT:-/dev/null}
zero=0000000000000000000000000000000000000000
case "${EVENT:-}" in
  pull_request) base=${PR_BASE:-} ;;
  push) base=${BEFORE:-} ;;
  *) base= ;;
esac
# A branch push checks what the branch brings to jc-portal-1: its diff from the merge base. A new
# branch has no `before`, and a push that merged jc-portal-1 would otherwise count trunk's changes
# as its own (one ci.yml change on trunk ran all 21 jobs of a 6-file branch).
if [ "${EVENT:-}" = push ] && [ -n "${GITHUB_REF:-}" ] && ! trunk "$GITHUB_REF"; then
  base=$(git merge-base origin/jc-portal-1 HEAD 2>/dev/null) || base=
fi
if [ -z "$base" ] || [ "$base" = "$zero" ] || ! git cat-file -e "${base}^{commit}" 2>/dev/null; then
  echo "no base: every job runs"
  lines=$(all)
else
  lines=$(git diff --name-only "$base" HEAD | map)
fi
echo "$lines" | ship | tee -a "$out"
if [ "${REUSE:-0}" = 1 ]; then
  ok=$(done_jobs)
  if [ -n "$ok" ]; then
    echo "jobs already green on $GITHUB_SHA in another ci run:"; echo "$ok" | sed 's/^/  /'
    lines=$(echo "$lines" | covered "$ok")
  fi
fi
echo "$lines" | tee -a "$out"
