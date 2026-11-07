#!/usr/bin/env bash
# SPDX-License-Identifier: EUPL-1.2
# One cell of a split workspace suite, keeping each antares-sql pg binary whole.
#
#   dev/test-cell.sh <config-name> <part> <parts> [nextest args...]
#   dev/test-cell.sh --self-test
#
# Every other test is dealt by nextest --partition count:<part>/<parts>. The
# antares-sql pg_* binaries are not: their plan and partition tests lean on
# what sibling tests of the same binary seed and ANALYZE in the cell's one
# database, so a binary split over four cells turned pg_explain and
# pg_temporal red (T-6151 bounce, fast 41015023930 and 41018489700). Each pg
# binary runs whole in the cell named below; the last cell takes every pg
# binary the others do not name, so a new pg_*.rs always runs somewhere.
# Weights: summed test seconds on dev 3651d55 (fast 41007122716).
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)

# cell_binaries <parts> <part>: the pg binaries a cell runs whole, as a
# space-separated list; the last part prints "rest".
cell_binaries() {
  case "$1/$2" in
    2/1) echo "pg_explain" ;;                                  # 434 s
    4/1) echo "pg_explain" ;;                                  # 434 s
    4/2) echo "pg_query_parity pg_temporal pg_temporal_last_n" ;; # 154 s
    4/3) echo "pg_retention pg_rls_pentest pg" ;;              # 114 s
    "$1/$1") echo "rest" ;;
    *) echo "dev/test-cell.sh: no pg cell layout for part $2 of $1" >&2; return 2 ;;
  esac
}

# any_of <binary>...: "binary(=a) or binary(=b)"
any_of() {
  local out="" b
  for b; do out+="${out:+ or }binary(=$b)"; done
  printf %s "$out"
}

# pg_filter <parts> <part>: the nextest filterset of the cell's pg binaries
pg_filter() {
  local own others=() p
  own=$(cell_binaries "$1" "$2") || return
  if [ "$own" != rest ]; then
    # shellcheck disable=SC2086 # a word list by design
    printf 'package(antares-sql) and (%s)' "$(any_of $own)"
    return
  fi
  for ((p = 1; p < $1; p++)); do
    # shellcheck disable=SC2207 # a word list by design
    others+=($(cell_binaries "$1" "$p"))
  done
  printf 'package(antares-sql) and binary(/^pg/) and not (%s)' "$(any_of "${others[@]}")"
}

PG='package(antares-sql) and binary(/^pg/)'

if [ "${1:-}" = --self-test ]; then
  # each pg test file lands in exactly one cell of every layout, and every
  # binary a cell names exists
  bad=0
  files=()
  for f in "$here"/../crates/antares-sql/tests/pg*.rs; do f=${f##*/}; files+=("${f%.rs}"); done
  for parts in 2 4; do
    for f in "${files[@]}"; do
      hits=0
      for ((p = 1; p <= parts; p++)); do
        own=$(cell_binaries "$parts" "$p")
        if [ "$own" = rest ]; then
          named=" "; for ((q = 1; q < parts; q++)); do named+="$(cell_binaries "$parts" "$q") "; done
          [[ "$named" == *" $f "* ]] || hits=$((hits + 1))
        else
          [[ " $own " == *" $f "* ]] && hits=$((hits + 1))
        fi
      done
      [ "$hits" = 1 ] || { echo "self-test: $f runs in $hits cells of $parts"; bad=1; }
    done
    for ((p = 1; p < parts; p++)); do
      for b in $(cell_binaries "$parts" "$p"); do
        [ -f "$here/../crates/antares-sql/tests/$b.rs" ] || { echo "self-test: cell $p/$parts names $b, which has no test file"; bad=1; }
      done
    done
  done
  [ "$(pg_filter 4 1)" = 'package(antares-sql) and (binary(=pg_explain))' ] || { echo "self-test: filter 4/1 is $(pg_filter 4 1)"; bad=1; }
  ((bad)) || echo "self-test: ok (${#files[@]} pg binaries, layouts 2 and 4)"
  exit $bad
fi

[ $# -ge 3 ] || { echo "usage: dev/test-cell.sh <config-name> <part> <parts> [nextest args...] | --self-test" >&2; exit 2; }
name=$1 part=$2 parts=$3
shift 3
own=$(pg_filter "$parts" "$part") || exit 2
rc=0
"$here/unit-junit.sh" "$name" "$@" --partition "count:$part/$parts" -E "not ($PG)" || rc=$?
"$here/unit-junit.sh" "$name-pg" "$@" -E "$own" || rc=$?
exit $rc
