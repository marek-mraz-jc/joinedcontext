#!/bin/sh
# Which ci.yml jobs a change needs: reads the changed paths (one per line) and prints
# `job=true|false` lines, to $GITHUB_OUTPUT when set. Without a usable base (a dispatch, a new
# branch, a base git cannot reach) every job runs.
# `ship` says whether the push changes what the jc image holds (ci.yml builds and promotes one on
# main and ship-proof/** only). With REUSE=1 (every push) a check job whose exact name already
# concluded success in another ci run of this same commit (a fast-forward of a branch head ci
# passed) is not run again: the same commit is the same input, workflow file included. The
# repository is public, so only a push run of ci.yml in this repository counts: a pull_request run
# tests refs/pull/N/merge (another tree, with that merge's own ci.yml) and a fork's run is
# anyone's. Each key turned off prints `reused <job> from run <id>` per job. Reuse is per job,
# never per run: a run's own diff base decides which jobs it ran, so a green run can have tested
# fewer jobs than this one needs. supply-chain, gitleaks, the image, its scan and its signature
# always run. On the refs that write the caches (trunk) a key is not reused while one of its
# jobs' target caches (.github/actions/rust `key:`, the job name without its part) has no entry
# this ref restores: a fast-forward skipped every job and an evicted cache was never saved again
# (portal-test cold for 8 h, 14 min per part, run 42969260266); that run re-seeds it.
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
    # the App bundles both images ship in /srv/apps: their lists must stay equal; the App images
    # (ci.yml ship-apps) build from crates/portal/builder
    /^bins\/jc\/Dockerfile$/ || /^crates\/portal\/Dockerfile$/ || /^crates\/portal\/apps\// || /^crates\/portal\/builder\// || /^crates\/portal\/scripts\/(app-integrity\.mjs|ci\/check-dockerfile-members\.py)$/ { on["dockerfiles"] = "true" }
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

# covered <"job run-id" lines that succeeded> [<target caches with no entry>]: turns off each key
# all of whose jobs are in it and none of whose caches is missing, and prints, to stderr, the run
# each of its jobs is reused from (or the cache it runs again to save)
covered() {
  printf '%b\n' "$1" > "${TMPDIR:-/tmp}/ci-done.$$"
  awk -v done="${TMPDIR:-/tmp}/ci-done.$$" -v missing=" ${2:-} " '
    BEGIN {
      while ((getline l < done) > 0) { split(l, d, " "); if (d[1] != "") ok[d[1]] = d[2] }
      # every job of a key, each test partition of ci.yml (matrix part) by its name
      jobs["root"] = "root-clippy root-test-1 root-test-2 root-test-3 root-test-4"
      jobs["antares"] = "antares-clippy antares-test-1 antares-test-2 antares-test-3 antares-test-4"
      jobs["platform"] = "platform-test"
      jobs["ekiben"] = "ekiben"; jobs["portal"] = "portal-clippy portal-test-1 portal-test-2 portal-test-3 portal-test-4"; jobs["ui"] = "ui ui-test-1 ui-test-2 ui-test-3 ui-test-4"
      jobs["image"] = "dockerfile"
    }
    {
      split($0, kv, "=")
      if (kv[2] == "true" && (kv[1] in jobs)) {
        n = split(jobs[kv[1]], need, " "); all = 1
        for (i = 1; i <= n; i++) if (!(need[i] in ok)) all = 0
        for (i = 1; all && i <= n; i++) {
          c = need[i]; sub(/-[0-9]+$/, "", c)
          if (index(missing, " " c " ")) { print "no cache " c ": " kv[1] " runs again to save it" > "/dev/stderr"; all = 0 }
        }
        if (all) {
          for (i = 1; i <= n; i++) print "reused " need[i] " from run " ok[need[i]] > "/dev/stderr"
          print kv[1] "=false"; next
        }
      }
      print
    }'
  rm -f "${TMPDIR:-/tmp}/ci-done.$$"
}

# `job run-id` for each job that concluded success in another push run of ci.yml in this repository
# on this commit (the first such run per job name). The query's filters are repeated in jq: the
# API's are not the trust boundary.
done_jobs() {
  gh api "repos/$GITHUB_REPOSITORY/actions/workflows/ci.yml/runs?head_sha=$GITHUB_SHA&event=push&per_page=50" |
    jq -r --arg repo "$GITHUB_REPOSITORY" --arg sha "$GITHUB_SHA" --arg self "$GITHUB_RUN_ID" '
      .workflow_runs[]
      | select((.id | tostring) != $self and .event == "push" and .head_sha == $sha
          and .path == ".github/workflows/ci.yml"
          and .repository.full_name == $repo and .head_repository.full_name == $repo)
      | .id' |
    while read -r id; do
      gh api --paginate "repos/$GITHUB_REPOSITORY/actions/runs/$id/jobs?per_page=100" |
        jq -r --arg id "$id" '.jobs[] | select(.status == "completed" and .conclusion == "success") | "\(.name) \($id)"'
    done | sort -s -k1,1 -u
}

# missing_caches: the target caches ci.yml's jobs save (`key:` of .github/actions/rust) with no
# rust-cache entry on this ref or on jc-portal-1, the default branch every ref restores from.
# ponytail: an entry under a superseded toolchain (env hash) still counts as present until
# cache-prune.sh drops it; compare the hash once the changes job knows the toolchain's.
missing_caches() {
  have=$(gh api --paginate "repos/$GITHUB_REPOSITORY/actions/caches?per_page=100&key=v0-rust-" |
    jq -r --arg ref "$GITHUB_REF" '.actions_caches[] | select(.ref == $ref or .ref == "refs/heads/jc-portal-1") | .key' |
    sed -n 's/^v0-rust-\(.*\)-[^-]*-[^-]*-[^-]*-[^-]*$/\1/p' | sort -u)
  grep -o '[{ ]key: [a-z0-9-]*' "${CI_YML:-.github/workflows/ci.yml}" | sed 's/.*key: //' | sort -u |
    while read -r c; do echo "$have" | grep -qx "$c" || printf '%s ' "$c"; done
}

# reuse_selftest: done_jobs against stubbed API answers (one run per case, each with one job)
reuse_selftest() {
  GITHUB_REPOSITORY=o/r GITHUB_SHA=aaa GITHUB_RUN_ID=1
  run() { # id event head-repo sha path
    printf '{"id":%s,"event":"%s","head_sha":"%s","path":"%s","repository":{"full_name":"o/r"},"head_repository":{"full_name":"%s"}}' "$1" "$2" "$4" "$5" "$3"
  }
  ci=.github/workflows/ci.yml
  runs="$(run 1 push o/r aaa $ci),$(run 10 push o/r aaa $ci),$(run 11 pull_request o/r aaa $ci),$(run 12 push fork/r aaa $ci)"
  runs="$runs,$(run 13 push o/r aaa $ci),$(run 14 push o/r aaa $ci),$(run 15 push o/r aaa $ci),$(run 16 push o/r aaa $ci)"
  runs="$runs,$(run 17 push o/r bbb $ci),$(run 18 push o/r aaa .github/workflows/other.yml)"
  job() { printf '{"jobs":[{"name":"%s","status":"%s","conclusion":%s}]}' "$1" "$2" "$3"; }
  gh() {
    case "$*" in
      *"/workflows/ci.yml/runs?"*) printf '{"workflow_runs":[%s]}' "$runs" ;;
      *runs/1/jobs*) job self completed '"success"' ;;
      *runs/10/jobs*) job push-same-repo completed '"success"' ;;
      *runs/11/jobs*) job pull-request completed '"success"' ;;
      *runs/12/jobs*) job fork completed '"success"' ;;
      *runs/13/jobs*) job failed completed '"failure"' ;;
      *runs/14/jobs*) job cancelled completed '"cancelled"' ;;
      *runs/15/jobs*) job skipped completed '"skipped"' ;;
      *runs/16/jobs*) job in-progress in_progress null ;;
      *runs/17/jobs*) job other-sha completed '"success"' ;;
      *runs/18/jobs*) job other-workflow completed '"success"' ;;
      *) echo "unexpected gh $*" >&2; return 1 ;;
    esac
  }
  got=$(done_jobs)
  [ "$got" = "push-same-repo 10" ] || { echo "FAIL: done_jobs -> [$got], want [push-same-repo 10]"; exit 1; }
  unset -f gh
}

# missing_selftest: missing_caches against a stubbed cache list and a two-job ci.yml
missing_selftest() {
  GITHUB_REPOSITORY=o/r GITHUB_REF=refs/heads/main CI_YML=${TMPDIR:-/tmp}/ci-yml.$$
  printf '%s\n' '      - uses: ./.github/actions/rust' '        with: { key: root-test }' \
    '        with: { workspace: crates/portal, key: portal-test }' '        with: { workspace: crates/ekiben, key: ekiben, components: "clippy" }' \
    '        with: { workspace: crates/antares, key: antares-test }' > "$CI_YML"
  gh() {
    e() { printf '{"key":"%s","ref":"%s"}' "$1" "$2"; }
    printf '{"actions_caches":[%s,%s,%s,%s]}' "$(e v0-rust-root-test-Linux-x64-43996dc4-7649df7e refs/heads/main)" \
      "$(e v0-rust-portal-test-Linux-x64-01bad09c-ac79bf3c refs/heads/jc-portal-1)" \
      "$(e v0-rust-antares-test-Linux-x64-01bad09c-ac79bf3c refs/heads/agent/x)" "$(e sccache/0/0/0/00 refs/heads/main)"
  }
  got=$(missing_caches)
  rm -f "$CI_YML"; unset -f gh; unset CI_YML
  [ "$got" = "antares-test ekiben " ] || { echo "FAIL: missing_caches -> [$got], want [antares-test ekiben ]"; exit 1; }
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
  check 'crates/portal/builder/Dockerfile' 'root portal dockerfiles'
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
  [ "$(printf 'root=true\nportal=true\nsupply=true\n' | covered 'root-clippy 1\nroot-test-1 1\nroot-test-2 1\nroot-test-3 1\nroot-test-4 1\nportal-clippy 1\nportal-test-1 1' 2>/dev/null)" = "$(printf 'root=false\nportal=true\nsupply=true')" ] \
    || { echo "FAIL: covered"; exit 1; }
  [ "$(printf 'portal=true\n' | covered 'portal-clippy 1\nportal-test-1 1\nportal-test-2 1\nportal-test-3 1' 2>/dev/null)" = portal=true ] \
    && [ "$(printf 'portal=true\n' | covered 'portal-clippy 1\nportal-test-1 1\nportal-test-2 1\nportal-test-3 1\nportal-test-4 1' 2>/dev/null)" = portal=false ] \
    || { echo "FAIL: covered portal parts"; exit 1; }
  [ "$(printf 'image=true\nsupply=true\n' | covered 'dockerfile 7' 2>&1 >/dev/null)" = "reused dockerfile from run 7" ] \
    || { echo "FAIL: covered names its source run"; exit 1; }
  [ "$(printf 'root=true\nportal=true\n' | covered 'root-clippy 1\nroot-test-1 1\nroot-test-2 1\nroot-test-3 1\nroot-test-4 1\nportal-clippy 1\nportal-test-1 1\nportal-test-2 1\nportal-test-3 1\nportal-test-4 1' 'antares-test portal-test' 2>/dev/null)" = "$(printf 'root=false\nportal=true')" ] \
    || { echo "FAIL: covered re-seeds a missing cache"; exit 1; }
  reuse_selftest
  missing_selftest
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
    echo "jobs already green on $GITHUB_SHA in another push run of ci.yml here:"; echo "$ok" | sed 's/^/  /'
    missing=
    if trunk "${GITHUB_REF:-}"; then missing=$(missing_caches); fi
    lines=$(echo "$lines" | covered "$ok" "$missing" 2>"${TMPDIR:-/tmp}/ci-reused.$$")
    cat "${TMPDIR:-/tmp}/ci-reused.$$"; rm -f "${TMPDIR:-/tmp}/ci-reused.$$"
  fi
fi
echo "$lines" | tee -a "$out"
