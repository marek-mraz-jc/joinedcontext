#!/usr/bin/env bash
# The per-operation baseline every surface is budgeted against (T-3467,
# docs/src/perf-baseline.md): Retrieve, Query (type, type + q) over 1 000 and
# 100 000 entities, Batch Upsert and Create Subscription at c50, then the
# notification fan-out over 1, 100 and 10 000 subscriptions; per store.
#
#   BIN=… DATABASE_URL=postgres://… OUT=results/baseline dev/perf/baseline.sh
#
# Runs on a GitHub-hosted runner (perf-baseline.yml), never on a shared
# host: the numbers are only read as ratios within one job.
set -euo pipefail
cd "$(dirname "$0")/../.."
BIN="${BIN:-target/release/antares}"
OUT="${OUT:-results/baseline}"
export BIN
mkdir -p "$OUT"
ENTITIES=1000 SPECS="query 50,query-q 50,retrieve 50,upsert 50,subscribe 50" \
  OUT="$OUT/e1k" dev/perf/shapes.sh
ENTITIES=100000 SPECS="query 50,query-q 50,retrieve 50" \
  OUT="$OUT/e100k" dev/perf/shapes.sh

python3 dev/perf/sink.py 9800 > "$OUT/sink.log" 2>&1 & SINK=$!
trap 'kill "$SINK" ${B:+"$B"} 2>/dev/null || true' EXIT
for store in memory ${DATABASE_URL:+postgres}; do
  D=$(mktemp -d)
  ANTARES_STORE=$store ANTARES_DATA_DIR=$D ANTARES_HTTP_PORT=9472 \
    ANTARES_DATABASE_URL="${DATABASE_URL:-}" ANTARES_ALLOW_SHARED_LOCAL=1 \
    ANTARES_EGRESS_ALLOW_PRIVATE=true "$BIN" > "$OUT/fanout-$store.log" 2>&1 & B=$!
  for _ in $(seq 300); do curl -sf -o /dev/null http://127.0.0.1:9472/q/health && break; sleep 0.1; done
  echo "### $store" >> "$OUT/fanout.md"
  python3 dev/perf/fanout.py http://127.0.0.1:9472 http://127.0.0.1:9800 1 100 10000 | tee -a "$OUT/fanout.md"
  kill "$B"; wait "$B" 2>/dev/null || true; B=; rm -rf "$D"
done
