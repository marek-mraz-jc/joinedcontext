#!/bin/sh
# The UI tests a change reaches (T-6153), for ci.yml's ui job when scripts/ci/changes.sh says
# ui_scope=related: the tests that import a file the change touched (vitest related), the changed
# test files, and every test that reads the tree itself (the gate inventories, the locale and
# control registries), which no import graph names. A deleted or renamed file runs the whole
# suite: what imported it is no longer in any graph. A language catalogue (src/locales/<xx>.json)
# is imported by i18n.ts, which every page reaches, so it stands for the tests that name a key
# it added, dropped or changed (or its parent before a `${`) or quote a text it changed. A text
# breaks only a test that asserts it (the suite keeps no snapshots), and the tree readers above
# (i18n_used_keys among them) refuse a key a source still names. ci-full runs the whole suite
# every day.
#
#   UI_SHARD=1/4 sh scripts/ci/ui-related.sh <base>   run them, that shard of them (ui/ installed)
#   sh scripts/ci/ui-related.sh --list <base> print the vitest arguments only
#   sh scripts/ci/ui-related.sh --selftest
set -eu
UI=$(cd "$(dirname "$0")/../../ui" && pwd)
# A test reads the tree when it opens files itself rather than importing them.
TREE='node:fs|from "fs"|readFileSync|readdirSync|existsSync|globSync|import\.meta\.glob|child_process|\?raw'

CATALOGUE='^src/locales/[a-z][a-z]\.json$'

# One `key<TAB>text` line per string of the JSON on stdin, the key dotted.
leaves() {
  jq -r 'paths(scalars) as $p | [($p | map(tostring) | join(".")), (getpath($p) | tostring)] | @tsv'
}

# The grep -F patterns for the catalogue texts $1 (before) and $2 (after): each key that was added,
# dropped or changed, its parent followed by `${` (a key built in a template), and each text it had
# or has, quoted as a test quotes it. Nothing when either side is no JSON: the caller runs all.
moved() {
  jq -e . "$1" >/dev/null 2>&1 && jq -e . "$2" >/dev/null 2>&1 || return 0
  { leaves <"$1" && leaves <"$2"; } | sort | uniq -u |
    awk -F '\t' '{
      print $1
      if ($1 ~ /\./) { p = $1; sub(/\.[^.]*$/, "", p); print p ".${" }
      if (length($2) > 1) { print "\"" $2 "\""; print "\047" $2 "\047"; print "`" $2 "`" }
    }' | sort -u
}

# The test files that hold one of the patterns in the file $1.
users() {
  [ -s "$1" ] || return 0
  grep -rlF -f "$1" --include='*.test.ts' --include='*.test.tsx' src tests || true
}

# The vitest arguments for the paths (relative to ui/) on stdin: `all` for the whole suite, or
# one file per line.
args() {
  files=$(sed '/^$/d')
  printf '%s\n' "$files" | grep -qvE '^(src|tests|e2e)/' && [ -n "$files" ] && { echo all; return; }
  {
    printf '%s\n' "$files" | grep -E '^(src|tests)/' || true
    grep -rlE "$TREE" --include='*.test.ts' --include='*.test.tsx' src tests
  } | sort -u
}

selftest() {
  cd "$UI"
  a=$(mktemp) b=$(mktemp) p=$(mktemp)
  printf '{"spaces":{"sizeMissing":"Gone","kept":"Same"}}' >"$a"
  printf '{"spaces":{"sizeMissing":"Not reported","kept":"Same"}}' >"$b"
  moved "$a" "$b" >"$p"
  grep -qxF 'spaces.sizeMissing' "$p" || { echo "ui-related selftest: a changed key is not named" >&2; exit 1; }
  grep -qxF "spaces.\${" "$p" || { echo "ui-related selftest: a templated key is missed" >&2; exit 1; }
  grep -qxF '"Gone"' "$p" || { echo "ui-related selftest: a test quoting the old text is missed" >&2; exit 1; }
  ! grep -q 'kept\|Same' "$p" || { echo "ui-related selftest: an unchanged key runs its users" >&2; exit 1; }
  printf 'spaces.sizeMissing\n' >"$p"
  users "$p" | grep -qx 'tests/page_spaces_page.test.tsx' || { echo "ui-related selftest: the test naming a key is missed" >&2; exit 1; }
  ! users "$p" | grep -qvE '\.test\.tsx?$' || { echo "ui-related selftest: a source went to vitest related" >&2; exit 1; }
  : >"$a"
  [ -z "$(moved "$a" "$b")" ] || { echo "ui-related selftest: a catalogue that is no JSON gave keys" >&2; exit 1; }
  rm -f "$a" "$b" "$p"
  got=$(printf 'src/i18n.ts\ntests/foo.test.ts\ne2e/models.spec.ts\n' | args)
  printf '%s\n' "$got" | grep -qx 'src/i18n.ts' || { echo "ui-related selftest: a changed source is not passed" >&2; exit 1; }
  printf '%s\n' "$got" | grep -qx 'tests/foo.test.ts' || { echo "ui-related selftest: a changed test is not passed" >&2; exit 1; }
  printf '%s\n' "$got" | grep -qx 'tests/gate_routes.test.ts' || { echo "ui-related selftest: a tree-reading gate is missed" >&2; exit 1; }
  ! printf '%s\n' "$got" | grep -q '^e2e/' || { echo "ui-related selftest: a journey went to vitest" >&2; exit 1; }
  [ "$(printf 'src/a.ts\npackage.json\n' | args)" = all ] || { echo "ui-related selftest: a file outside src/tests/e2e did not run all" >&2; exit 1; }
  [ "$(printf '' | args | grep -c '^tests/gate_')" -ge 6 ] || { echo "ui-related selftest: no change still runs the gates" >&2; exit 1; }
  echo "ui-related selftest: ok"
}

list() {
  cd "$UI"
  # Deleted and renamed paths (git diff --name-status letters D and R) run the whole suite.
  if git diff --name-status "$1" HEAD -- . | grep -qE '^[DR]'; then echo all; return; fi
  changed=$(git diff --name-only --relative "$1" HEAD -- .)
  was=$(mktemp) pats=$(mktemp)
  for f in $(printf '%s\n' "$changed" | grep -E "$CATALOGUE" || true); do
    git show "$1:ui/$f" >"$was" 2>/dev/null || : >"$was"
    moved "$was" "$f" >>"$pats"
    # A catalogue jq cannot read on either side gave no pattern: it goes to vitest itself.
    if ! { jq -e . "$was" && jq -e . "$f"; } >/dev/null 2>&1; then echo "$f"; fi
  done >"$was.keep"
  {
    printf '%s\n' "$changed" | grep -vE "$CATALOGUE" || true
    cat "$was.keep"
    users "$pats"
  } | args
  rm -f "$was" "$was.keep" "$pats"
}

case ${1:-} in
  --selftest) selftest ;;
  --list) list "$2" ;;
  "") echo "usage: ui-related.sh [--list] <base> | --selftest" >&2; exit 2 ;;
  *)
    picked=$(list "$1")
    cd "$UI"
    shard=--shard=${UI_SHARD:-1/1}
    if [ "$picked" = all ]; then exec pnpm vitest run "$shard"; fi
    echo "$picked" | sed 's/^/vitest related: /'
    printf '%s\n' "$picked" | xargs pnpm vitest related --run --passWithNoTests "$shard"
    ;;
esac
