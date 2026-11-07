#!/bin/sh
# Which ci.yml jobs a change needs (T-4985). Prints rust=, ui=, apps=, sdk= as true/false and ui_scope=
# for $GITHUB_OUTPUT, from the paths changed between <base> and HEAD (or read on stdin, one per line).
# No base, a line `*`, or ci.yml itself runs everything; gitleaks runs on every push and main's
# base (its last green commit, T-4696) is decided in ci.yml.
#
#   sh scripts/ci/changes.sh "$base"
#   git diff --name-only "$base" HEAD | sh scripts/ci/changes.sh -
#   sh scripts/ci/changes.sh --selftest
#
# rust runs for every path except ui/, docs/ and the top-level *.md, so a new folder is covered
# by default. A ui/ file still runs it when the Rust the fast lane compiles or tests names it
# (ui/openapi.json): the lane's unit tests read sdk/, apps/ and ui/ files. The other way round,
# the UI suite reads a few files outside ui/ and sdk/: the help clips' script, public-docs/, the
# Rust fixtures, and the activity kinds the server writes (src/**/*.rs, see kinds_added).
# A language catalogue (ui/src/locales/<xx>.json) runs rust only when its `problem` texts moved:
# tests/problem_types_tests.rs is the one Rust that reads the folder, and only those files by
# name (T-6153; without a base, always).
# ui_scope (T-6153) is `related` when every UI path the change touched is a file under ui/src/,
# ui/tests/ (but its setup file) or ui/e2e/: the ui job then runs only the tests that import a
# changed file, the changed tests and every test that reads the tree (scripts/ci/ui-related.sh).
# Anything else the suite reads (the SDK, the package or config files, public-docs/, the Rust
# fixtures, a new folder) is `full`: the whole suite in four shards. sdk (T-6311) runs the SDK's
# own vitest, which reads only sdk/: no other lane ran it, so three reds sat on main unseen.
# ci-full runs the whole suite,
# the SDK's screens and the component-gallery baselines every day.
set -eu
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
# Exit 0 when the fast lane's Rust names the ui/ file $1 or one of its folders, as a whole
# string literal ("ui/openapi.json", "ui/src/locales").
rust_reads() {
  p=$1 pats=""
  while case $p in ui/*/*) true ;; *) false ;; esac; do
    pats="$pats$p\"
"
    p=${p%/*}
  done
  pats="$pats$p\""
  # What the rust job compiles and tests: the crate (--lib --bins) and the integration tests it
  # names. Keep this list in step with the job's `cargo test` lines.
  (cd "$ROOT" && printf '%s\n' "$pats" |
    grep -rqF -f - src build.rs tests/openapi_tests.rs tests/route_coverage_tests.rs \
      tests/authz_matrix_tests.rs tests/problem_types_tests.rs tests/gitea_client_tests.rs \
      tests/connector_scenarios.rs)
}

classify() {
  rust=false ui=false apps=false sdk=false ui_scope=related catalogues=""
  while IFS= read -r f; do
    [ -n "$f" ] || continue
    case $f in
      '*' | .github/workflows/ci.yml) everything ;;
    esac
    case $f in
      ui/* | sdk/* | scripts/bundle-budget* | public-docs/* | tests/fixtures/*) ui=true ;;
    esac
    case $f in
      apps/* | sdk/* | builder/* | scripts/*) apps=true ;;
    esac
    case $f in
      sdk/*) sdk=true ;;
    esac
    case $f in
      ui/tests/setup.ts) ui_scope=full ;;
      ui/src/* | ui/tests/* | ui/e2e/* | scripts/bundle-budget*) ;;
      ui/* | sdk/* | public-docs/* | tests/fixtures/*) ui_scope=full ;;
    esac
    case $f in
      docs/*) ;;
      ui/src/locales/[a-z][a-z].json) catalogues="$catalogues $f" ;;
      ui/src/locales/*) ;;
      ui/*) if rust_reads "$f"; then rust=true; fi ;;
      */*) rust=true ;;
      *.md) ;;
      *) rust=true ;;
    esac
  done
}

# Exit 0 when the `problem` member differs between the JSON files $1 and $2, or either is no JSON.
problem_differs() {
  old=$(jq -cS '.problem' "$1" 2>/dev/null) && new=$(jq -cS '.problem' "$2" 2>/dev/null) || return 0
  [ "$old" != "$new" ]
}

# rust=true when a catalogue the change touched moved its problem texts since $1 (no base: always).
settle_catalogues() {
  [ "$rust" = false ] && [ -n "$catalogues" ] || return 0
  [ -n "$1" ] || { rust=true; return 0; }
  was=$(mktemp)
  for f in $catalogues; do
    git -C "$ROOT" show "$1:$f" >"$was" 2>/dev/null || : >"$was"
    if problem_differs "$was" "$ROOT/$f"; then rust=true; break; fi
  done
  rm -f "$was"
}

# Exit 0 when the Rust between $1 and HEAD adds a line ui/tests/activity_kinds.test.ts collects
# as an activity kind, which the UI must then label.
KIND_LINE='^\+.*(kind: "[a-z]+[.-]|record\(|let kind = ")'
kinds_added() {
  git -C "$ROOT" diff -U0 "$1" HEAD -- 'src/*.rs' | grep -qE "$KIND_LINE"
}

report() {
  printf 'rust=%s\nui=%s\napps=%s\nui_scope=%s\nsdk=%s\n' "$rust" "$ui" "$apps" "$ui_scope" "$sdk"
}

selftest() {
  check() {
    got=$(printf '%b' "$2" | { classify; settle_catalogues ""; report; } | tr '\n' ' ')
    [ "$got" = "$3" ] || { echo "changes selftest: $1: got '$got', want '$3'" >&2; exit 1; }
  }
  check "a page" 'ui/src/pages/apps/AppTemplates.tsx\n' "rust=false ui=true apps=false ui_scope=related sdk=false "
  check "the colour tokens" 'ui/src/tokens.css\n' "rust=false ui=true apps=false ui_scope=related sdk=false "
  check "a test and a journey" 'ui/tests/gate_routes.test.ts\nui/e2e/models.spec.ts\n' "rust=false ui=true apps=false ui_scope=related sdk=false "
  check "the suite's setup" 'ui/tests/setup.ts\n' "rust=false ui=true apps=false ui_scope=full sdk=false "
  check "the UI's package" 'ui/package.json\n' "rust=false ui=true apps=false ui_scope=full sdk=false "
  check "the vite config" 'ui/vite.config.ts\n' "rust=false ui=true apps=false ui_scope=full sdk=false "
  check "a new ui folder" 'ui/newdir/x.ts\n' "rust=false ui=true apps=false ui_scope=full sdk=false "
  check "a page and the lockfile" 'ui/src/App.tsx\nui/pnpm-lock.yaml\n' "rust=false ui=true apps=false ui_scope=full sdk=false "
  check "committed OpenAPI spec" 'ui/openapi.json\n' "rust=true ui=true apps=false ui_scope=full sdk=false "
  check "a locale the problem-types test reads" 'ui/src/locales/sk.json\n' "rust=true ui=true apps=false ui_scope=related sdk=false "
  check "the problem-type test is not a catalogue" 'ui/src/locales/public-groups.json\n' "rust=false ui=true apps=false ui_scope=related sdk=false "
  check "the bundle budget" 'scripts/bundle-budget.mjs\n' "rust=true ui=true apps=true ui_scope=related sdk=false "
  check "src only" 'src/main.rs\nmigrations/0001.sql\n' "rust=true ui=false apps=false ui_scope=related sdk=false "
  check "a fixture the UI suite reads" 'tests/fixtures/bus-expires.blobl\n' "rust=true ui=true apps=false ui_scope=full sdk=false "
  check "public docs" 'public-docs/en/x.md\n' "rust=true ui=true apps=false ui_scope=full sdk=false "
  check "lock" 'Cargo.lock\n' "rust=true ui=false apps=false ui_scope=related sdk=false "
  check "apps only" 'apps/air-quality/ui/src/App.tsx\n' "rust=true ui=false apps=true ui_scope=related sdk=false "
  check "sdk" 'sdk/src/sdk/index.ts\n' "rust=true ui=true apps=true ui_scope=full sdk=true "
  check "an SDK test" 'sdk/tests/grid_narrow.test.ts\n' "rust=true ui=true apps=true ui_scope=full sdk=true "
  check "docs only" 'docs/analysis/x.md\nREADME.md\n' "rust=false ui=false apps=false ui_scope=related sdk=false "
  check "nested md is not docs" 'src/agents/server_section.md\n' "rust=true ui=false apps=false ui_scope=related sdk=false "
  check "unknown folder runs rust" 'newdir/x.txt\n' "rust=true ui=false apps=false ui_scope=related sdk=false "
  check "the workflow" '.github/workflows/ci.yml\n' "rust=true ui=true apps=true ui_scope=full sdk=true "
  check "another workflow" '.github/workflows/image.yml\n' "rust=true ui=false apps=false ui_scope=related sdk=false "
  check "no base" '*\n' "rust=true ui=true apps=true ui_scope=full sdk=true "
  check "nothing" '' "rust=false ui=false apps=false ui_scope=related sdk=false "
  a=$(mktemp) b=$(mktemp)
  printf '{"problem":{"x":"A"},"spaces":{"size":"Size"}}' >"$a"
  printf '{"spaces":{"size":"Bytes"},"problem":{"x":"A"}}' >"$b"
  ! problem_differs "$a" "$b" || { echo "changes selftest: a text outside problem runs rust" >&2; exit 1; }
  printf '{"problem":{"x":"B"}}' >"$b"
  problem_differs "$a" "$b" || { echo "changes selftest: a moved problem text skips rust" >&2; exit 1; }
  : >"$a"
  problem_differs "$a" "$b" || { echo "changes selftest: a new catalogue skips rust" >&2; exit 1; }
  rm -f "$a" "$b"
  printf '+        kind: "app.shared".to_owned(),\n' | grep -qE "$KIND_LINE" ||
    { echo "changes selftest: an added activity kind is not seen" >&2; exit 1; }
  ! printf '+    let kinds = 3;\n-        kind: "a.b".to_owned(),\n' | grep -qE "$KIND_LINE" ||
    { echo "changes selftest: a removed kind or plain code runs the UI suite" >&2; exit 1; }
  echo "changes selftest: ok"
}

everything() {
  rust=true ui=true apps=true sdk=true ui_scope=full
}

case ${1:-} in
  --selftest) selftest ;;
  -) classify; settle_catalogues ""; report ;;
  "") everything; report ;;
  *)
    if paths=$(git -C "$ROOT" diff --name-only "$1" HEAD 2>/dev/null); then
      classify <<EOF
$paths
EOF
      settle_catalogues "$1"
      if [ "$ui" = false ] && kinds_added "$1"; then ui=true; fi
    else
      # A base the shallow clone does not hold is no base either.
      everything
    fi
    report
    ;;
esac
