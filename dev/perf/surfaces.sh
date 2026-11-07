#!/usr/bin/env bash
# Load on every API surface (T-4889): the vanilla and the platform binary each against its own
# PostgreSQL database, seeded with dev/perf/k6-surfaces.js (100 000 entities over ten types,
# a 30-day history, 5 000 Events), then one k6 run per shape of dev/perf/surfaces-budgets.json
# at the shape's rate, with the broker's and PostgreSQL's CPU and RSS sampled beside it.
# dev/perf/surfaces_gate.py judges the run: exit 1 on any breach of a budget or the baseline.
#
#   DATABASE_URL=postgresql://antares:antares@localhost:15432/antares \
#   BIN_VANILLA=target/release/antares BIN_PLATFORM=target/release/antares-platform \
#   dev/perf/surfaces.sh
#
# Env: DATABASE_URL (an admin URL; the run creates antares_load_vanilla and _platform and drops
#      them after), BIN_VANILLA, BIN_PLATFORM (either may be unset: its build is skipped and
#      the gate counts its shapes as not run), OUT (results/surfaces), PORT (9471),
#      SHAPES (unset = every shape), ENTITIES, EVENTS, HISTORY_ENTITIES, HISTORY_DAYS,
#      SUBSCRIBERS (passed to k6), PG_CONTAINER (PostgreSQL's container: its CPU and memory
#      from docker stats; unset leaves them out), BUDGETS (dev/perf/surfaces-budgets.json), BASELINE (ci/perf-baseline/surfaces.json), PERF_MACHINE.
set -euo pipefail
cd "$(dirname "$0")/../.."

: "${DATABASE_URL:?set DATABASE_URL to an admin PostgreSQL URL}"
OUT="${OUT:-results/surfaces}"
PORT="${PORT:-9471}"
BUDGETS="${BUDGETS:-dev/perf/surfaces-budgets.json}"
command -v k6 >/dev/null || { echo "k6 missing" >&2; exit 1; }
command -v psql >/dev/null || { echo "psql missing" >&2; exit 1; }
mkdir -p "$OUT"
DURATION=$(python3 -c "import json; print(json.load(open('$BUDGETS'))['duration'])")
PID=
SAMPLER=
cleanup() {
  [ -n "$SAMPLER" ] && kill "$SAMPLER" 2>/dev/null
  [ -n "$PID" ] && kill "$PID" 2>/dev/null
  wait 2>/dev/null || true
}
trap cleanup EXIT

k6env=(-e "BROKER_URL=http://127.0.0.1:$PORT")
for v in ENTITIES EVENTS HISTORY_ENTITIES HISTORY_DAYS SUBSCRIBERS; do
  [ -z "${!v:-}" ] || k6env+=(-e "$v=${!v}")
done

run_build() {  # run_build <build> <binary> <surfaces>
  local build=$1 bin=$2 surfaces=$3 db="antares_load_$1"
  [ -x "$bin" ] || { echo "$build: missing $bin" >&2; exit 1; }
  psql "$DATABASE_URL" -qX -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS $db" -c "CREATE DATABASE $db"
  local url="${DATABASE_URL%/*}/$db"
  env ANTARES_STORE=postgres ANTARES_DATABASE_URL="$url" ANTARES_HTTP_PORT="$PORT" \
    ANTARES_ALLOW_SHARED_LOCAL=1 ${surfaces:+ANTARES_API_SURFACES=$surfaces} ${surfaces:+ANTARES_SCHEMA_MAX_BYTES=65536} \
    "$bin" > "$OUT/$build-broker.log" 2>&1 & PID=$!
  for _ in $(seq 600); do curl -sf -o /dev/null "http://127.0.0.1:$PORT/q/health" && break; sleep 0.1; done
  curl -sf -o /dev/null "http://127.0.0.1:$PORT/q/health" || { echo "$build: broker did not start" >&2; tail "$OUT/$build-broker.log" >&2; exit 1; }
  echo "$build: seeding"
  k6 run --quiet "${k6env[@]}" -e SHAPE=seed -e SURFACES="${surfaces:+1}" dev/perf/k6-surfaces.js > "$OUT/$build-seed.log" 2>&1 \
    || { echo "$build: seed failed" >&2; tail -20 "$OUT/$build-seed.log" >&2; exit 1; }
  # every shape measures a settled dataset, never the seed still being written out (T-3735)
  psql "$url" -qX -v ON_ERROR_STOP=1 -c 'VACUUM (ANALYZE)' -c 'CHECKPOINT'
  for shape in $(python3 -c "
import json, os
b = json.load(open('$BUDGETS'))['shapes']
want = os.environ.get('SHAPES', '').split(',') if os.environ.get('SHAPES') else list(b)
print(' '.join(s for s in want if s in b and '$build' in b[s]['builds']))"); do
    rate=$(python3 -c "import json; print(json.load(open('$BUDGETS'))['shapes']['$shape']['rate'])")
    python3 dev/perf/surfaces_gate.py sample --pid "$PID" ${PG_CONTAINER:+--container "$PG_CONTAINER"} --out "$OUT/$build-$shape.res" & SAMPLER=$!
    # a breach is the gate's to judge, so k6's own exit (it has no thresholds here) is kept apart
    k6 run --quiet "${k6env[@]}" -e SHAPE="$shape" -e RATE="$rate" -e DURATION="$DURATION" \
      --summary-export "$OUT/$build-$shape.json" dev/perf/k6-surfaces.js > "$OUT/$build-$shape.log" 2>&1 \
      || echo "$build $shape: k6 exited $?" >&2
    kill "$SAMPLER"; wait "$SAMPLER" 2>/dev/null || true; SAMPLER=
    echo "$build $shape done"
  done
  kill "$PID"; wait "$PID" 2>/dev/null || true; PID=
  psql "$DATABASE_URL" -qX -c "DROP DATABASE IF EXISTS $db"
}

[ -z "${BIN_VANILLA:-}" ] || run_build vanilla "$BIN_VANILLA" ""
[ -z "${BIN_PLATFORM:-}" ] || run_build platform "$BIN_PLATFORM" admin,views,odata,ogc,sta,ical,mcp,ws,ws-views

python3 dev/perf/surfaces_gate.py judge --results "$OUT" --budgets "$BUDGETS" --baseline "${BASELINE:-ci/perf-baseline/surfaces.json}" \
  | tee "$OUT/surfaces.md"
