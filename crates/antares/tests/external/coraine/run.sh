#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# run.sh - one cell of Coraine's functional suite against Antares (T-3549).
#
# Usage: run.sh <coraine-dir> <corlibs-bin> <store> <out-dir> [corTest args...]
#   <corlibs-bin>  corLibs/bin: corTest, corDiff, corJson, corTestClient
#   <store>        memory | file | postgres | timescale (postgres and timescale
#                  need ANTARES_TEST_PG, see antares.sh)
#   ANTARES_BIN    the binary under test
# Extra arguments go to corTest (e.g. --toIx 40 for a smoke).
#
# Writes <out-dir>/cortest.log, coraine-report.{json,md}; the per-case
# .expect/.out/.diff stay beside the cases (corTest --keep). Exits non-zero only
# when the harness itself broke; the verdicts are in the report.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
CORAINE_DIR=$(cd "$1" && pwd); BIN=$(cd "$2" && pwd); STORE=$3; OUT=$4; shift 4
: "${ANTARES_BIN:?ANTARES_BIN names the binary under test}"
mkdir -p "$OUT"; OUT=$(cd "$OUT" && pwd)

# The not-applicable entries of this store, as corTest's skip lists
list() { # $1 = tag | name
  awk -v want="$1" -v store="$STORE" '
    /^[[:space:]]*(#|$)/ { next }
    { split($1, kv, ":"); split(kv[1], ks, "@")
      if (ks[1] == want && (ks[2] == "" || ks[2] == store)) printf "%s%s", (n++ ? "," : ""), kv[2] }
  ' "$HERE/not-applicable.txt"
}

export CORAINE_DIR ANTARES_TEST_STORE="$STORE" COR_DB_TYPE=NONE
export COR_TEST_FUNCTIONS="$HERE/antares.sh" CORJSON="$BIN/corJson"
export ANTARES_TEST_DIR="${ANTARES_TEST_DIR:-/tmp/antares-cortest-$STORE}"
cd "$CORAINE_DIR"
rc=0
"$BIN/corTest" --keep --noColor --skip-tags "$(list tag)" --skipNames "$(list name)" "$@" \
  test/funcTests/cases > "$OUT/cortest.log" 2>&1 || rc=$?
echo "corTest exit $rc (non-zero on any failed case)" >> "$OUT/cortest.log"
python3 "$HERE/compare.py" test/funcTests/cases "$HERE/not-applicable.txt" "$STORE" "$OUT"
