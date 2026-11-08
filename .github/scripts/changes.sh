#!/bin/sh
# Which ci.yml jobs a change needs: reads the changed paths (one per line) and prints
# `job=true|false` lines, to $GITHUB_OUTPUT when set. Without a usable base (a dispatch, a new
# branch, a base git cannot reach) every job runs.
#   EVENT=push BEFORE=<sha> sh changes.sh      # in ci.yml
#   sh changes.sh --selftest
set -eu
JOBS="root antares ekiben portal ui supply workflows image"

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
    /(^|\/)Cargo\.lock$/ || /(^|\/)deny\.toml$/ || /(^|\/)\.cargo\/audit\.toml$/ { on["supply"] = "true" }
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
  check 'vendor/ngsild/x.rs' 'root portal'
  check 'bins/jc/Dockerfile' 'root image'
  check 'Cargo.lock' 'root supply'
  check '.github/workflows/image.yml' 'workflows'
  check '.github/scripts/changes.sh' "$JOBS"
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
if [ -z "$base" ] || [ "$base" = "$zero" ] || ! git cat-file -e "${base}^{commit}" 2>/dev/null; then
  echo "no base: every job runs"
  all | tee -a "$out"
else
  git diff --name-only "$base" HEAD | map | tee -a "$out"
fi
