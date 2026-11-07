#!/usr/bin/env bash
# Heavy mixed workload against the memory budget (T-5898): the platform binary on PostgreSQL,
# seeded by dev/perf/k6-surfaces.js (SHAPE=seed: ENTITIES synthetic entities over ten types,
# a HISTORY_DAYS history of pm25 on HISTORY_ENTITIES stations), then dev/perf/k6-heavy.js
# runs READERS concurrent large reads on every surface with upserts beside them for DURATION,
# while dev/perf/surfaces_gate.py samples the broker's resident memory. The verdict
# (dev/perf/heavy.py) fails the run on any failed read (a 503 MEMORY_OVERLOADED or any other
# 5xx) or, when BASE_BIN ran the same load first, a peak RSS, p95 or p99 more than LATENCY_SLACK
# over the baseline's; a peak RSS over RSS_TARGET_MB is reported (the gate without BASE_BIN).
#
#   DATABASE_URL=postgresql://antares:antares@localhost:15432/antares \
#   BIN=target/release/antares BASE_BIN=bin/antares-base dev/perf/heavy.sh
#
#
# Env: DATABASE_URL (admin URL; the run creates and drops antares_heavy_<build>), BIN,
#      BASE_BIN (optional baseline binary), OUT (results/heavy), PORT (9481), READERS (30),
#      DURATION (10m), WRITE_RATE (5), ENTITIES (100000), EVENTS (5000), HISTORY_ENTITIES (577),
#      HISTORY_DAYS (30), RSS_TARGET_MB (200), LATENCY_SLACK (1.10), ANTARES_MEMORY_BUDGET
#      (passed to the broker under test only).
set -euo pipefail
cd "$(dirname "$0")/../.."

: "${DATABASE_URL:?set DATABASE_URL to an admin PostgreSQL URL}"
: "${BIN:?set BIN to the platform binary under test}"
OUT="${OUT:-results/heavy}"
PORT="${PORT:-9481}"
export ENTITIES="${ENTITIES:-100000}" EVENTS="${EVENTS:-5000}"
export HISTORY_ENTITIES="${HISTORY_ENTITIES:-577}" HISTORY_DAYS="${HISTORY_DAYS:-30}"
command -v k6 >/dev/null || { echo "k6 missing" >&2; exit 1; }
command -v psql >/dev/null || { echo "psql missing" >&2; exit 1; }
mkdir -p "$OUT"
JEMALLOC=$(sed -n 's/^ENV MALLOC_CONF=\([^ ]*\).*/\1/p' Dockerfile)
[ -n "$JEMALLOC" ] || { echo "no MALLOC_CONF in Dockerfile" >&2; exit 1; }
PID=
SAMPLER=
cleanup() {
  if [ -n "$SAMPLER" ]; then kill "$SAMPLER" 2>/dev/null || true; fi
  if [ -n "$PID" ]; then kill "$PID" 2>/dev/null || true; fi
  wait 2>/dev/null || true
}
trap cleanup EXIT

k6env=(-e "BROKER_URL=http://127.0.0.1:$PORT")
for v in ENTITIES EVENTS HISTORY_ENTITIES HISTORY_DAYS READERS DURATION WRITE_RATE; do
  [ -z "${!v:-}" ] || k6env+=(-e "$v=${!v}")
done

run_build() {  # run_build <build> <binary> <memory budget or empty>
  local build=$1 bin=$2 budget=$3 db="antares_heavy_$1"
  [ -x "$bin" ] || { echo "$build: missing $bin" >&2; exit 1; }
  psql "$DATABASE_URL" -qX -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS $db" -c "CREATE DATABASE $db"
  local url="${DATABASE_URL%/*}/$db"
  # the image's allocator settings (Dockerfile): without them jemalloc keeps ~3x the live heap
  # resident and the RSS limit measures the allocator, not the broker
  env _RJEM_MALLOC_CONF="$JEMALLOC" MALLOC_CONF="$JEMALLOC" ANTARES_STORE=postgres ANTARES_DATABASE_URL="$url" ANTARES_HTTP_PORT="$PORT" \
    ANTARES_ALLOW_SHARED_LOCAL=1 ANTARES_TELEMETRY=1 ANTARES_API_SURFACES=admin,views,odata,ogc,sta,mcp \
    ANTARES_SCHEMA_MAX_BYTES=65536 ${budget:+ANTARES_MEMORY_BUDGET=$budget} \
    "$bin" > "$OUT/$build-broker.log" 2>&1 & PID=$!
  for _ in $(seq 600); do curl -sf -o /dev/null "http://127.0.0.1:$PORT/q/health" && break; sleep 0.1; done
  curl -sf -o /dev/null "http://127.0.0.1:$PORT/q/health" || { echo "$build: broker did not start" >&2; tail "$OUT/$build-broker.log" >&2; exit 1; }
  echo "$build: seeding"
  k6 run --quiet "${k6env[@]}" -e SHAPE=seed -e SURFACES=1 dev/perf/k6-surfaces.js > "$OUT/$build-seed.log" 2>&1 \
    || { echo "$build: seed failed" >&2; tail -20 "$OUT/$build-seed.log" >&2; exit 1; }
  psql "$url" -qX -v ON_ERROR_STOP=1 -c 'VACUUM (ANALYZE)' -c 'CHECKPOINT'
  echo "$build: heavy load"
  python3 dev/perf/surfaces_gate.py sample --pid "$PID" --out "$OUT/$build.res" & SAMPLER=$!
  # the verdict is heavy.py's: k6 has no thresholds here, so its exit is only reported
  k6 run --quiet "${k6env[@]}" --summary-trend-stats "avg,med,p(95),p(99),max" \
    --summary-export "$OUT/$build.json" dev/perf/k6-heavy.js > "$OUT/$build.log" 2>&1 \
    || echo "$build: k6 exited $?" >&2
  kill "$SAMPLER"; wait "$SAMPLER" 2>/dev/null || true; SAMPLER=
  curl -sf "http://127.0.0.1:$PORT/q/metrics" > "$OUT/$build.metrics" || true
  kill "$PID"; wait "$PID" 2>/dev/null || true; PID=
  psql "$DATABASE_URL" -qX -c "DROP DATABASE IF EXISTS $db"
}

[ -z "${BASE_BIN:-}" ] || run_build base "$BASE_BIN" ""
run_build head "$BIN" "${ANTARES_MEMORY_BUDGET:-}"

python3 dev/perf/heavy.py --results "$OUT" --rss-target-mb "${RSS_TARGET_MB:-200}" \
  --slack "${LATENCY_SLACK:-1.10}" | tee "$OUT/heavy.md"
