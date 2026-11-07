#!/usr/bin/env bash
# The history write bench (T-5243): PostgreSQL CPU per PATCH of two Attributes
# (`level` without observedAt, half of them repeating the stored value: the
# NEWEST_REPEATS probe; `speed` with observedAt: the EXISTS probe), on a fresh
# PostGIS container capped at 2 cores, the broker connected as a role under
# RLS with ANTARES_TEMPORAL_RECORD=all. `keyed` sets history_cutover a day
# back (every write goes to attr_rows), `text` sets it NULL (attr_instances),
# `short` sets it NULL and a PT5M retention rule for Tank (T-5249: every write
# goes to the unlogged attr_rows_short; BIN built with --features views).
# k6 creates N Entities and warms 60 s, then 60 s are measured; the line it
# prints divides the container's cgroup CPU and pg_stat_wal's bytes by the requests. Interleave runs:
# the host load moves the numbers more than most changes do.
#
#   BIN=$CARGO_TARGET_DIR/release/antares N=5000 tests/perf/history-write/run.sh keyed results/hw-keyed-1
#
# PROFILE=1 adds pg_stat_statements and pg_stat_user_functions to OUT/profile.txt.
# Throwaway credentials only: the container binds to loopback and lives for the run.
set -euo pipefail
MODE=$1; OUT=$2; BIN=${BIN:?}; S=$(dirname "$0"); PORT=9481; PGP=55440; PG=antares-history-write-pg
mkdir -p "$OUT"
ADMIN="postgresql://antares:bench@127.0.0.1:$PGP/antares"; APP="postgresql://antares_app:bench-app@127.0.0.1:$PGP/antares"
docker rm -f $PG >/dev/null 2>&1 || true
docker run -d --name $PG --cpus 2 --memory 2g -p 127.0.0.1:$PGP:5432 -e POSTGRES_USER=antares -e POSTGRES_PASSWORD=bench \
  -e POSTGRES_DB=antares ghcr.io/baosystems/postgis:17-3.5 -c shared_buffers=512MB ${PROFILE:+-c shared_preload_libraries=pg_stat_statements -c pg_stat_statements.track=all -c track_functions=all} >/dev/null
for _ in $(seq 120); do psql "$ADMIN" -qXc 'SELECT 1' >/dev/null 2>&1 && break; sleep 1; done
PID=
broker() { local log=$1; shift
  env ANTARES_STORE=postgres ANTARES_HTTP_PORT=$PORT ANTARES_ALLOW_SHARED_LOCAL=1 ANTARES_TEMPORAL_RECORD=all "$@" \
    taskset -c 0-3 "$BIN" > "$OUT/$log" 2>&1 & PID=$!
  for _ in $(seq 600); do curl -sf -o /dev/null "http://127.0.0.1:$PORT/q/health" && return; sleep 0.1; done
  echo "broker did not start" >&2; tail "$OUT/$log" >&2; exit 1; }
stop() { kill "$PID"; wait "$PID" 2>/dev/null || true; }
trap 'kill $PID 2>/dev/null; docker rm -f $PG >/dev/null 2>&1' EXIT
broker migrate.log ANTARES_DATABASE_URL="$ADMIN"; stop
if [ "$MODE" = keyed ]; then at="now() - interval '1 day'"; else at=NULL; fi
psql "$ADMIN" -qX -v ON_ERROR_STOP=1 -c "CREATE ROLE antares_app LOGIN PASSWORD 'bench-app'" \
  -c 'GRANT USAGE ON SCHEMA public TO antares_app' \
  -c 'GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO antares_app' \
  -c 'GRANT USAGE ON ALL SEQUENCES IN SCHEMA public TO antares_app' \
  -c "UPDATE history_cutover SET at = $at"
# the owner's maintenance creates the weeks the cut-over needs, as the deployed migrate job does
broker maint.log ANTARES_DATABASE_URL="$ADMIN"; sleep 2; stop
broker broker.log ANTARES_DATABASE_URL="$APP" ANTARES_MIGRATE=0 ANTARES_REQUIRE_RLS=1
if [ "$MODE" = short ]; then
  curl -sf -X PUT -H 'Content-Type: application/json' "http://127.0.0.1:$PORT/q/retention/default" \
    -d '{"rules":[{"type":"https://uri.etsi.org/ngsi-ld/default-context/Tank","history":"PT5M"}]}'
fi
k6 run --quiet -e BROKER_URL=http://127.0.0.1:$PORT -e CREATE=1 -e N="${N:-200}" -e DUR=60s "$S/write.js" > "$OUT/warm.log" 2>&1
c0=$(docker exec $PG sh -c "grep usage_usec /sys/fs/cgroup/cpu.stat" | awk '{print $2}')
w0=$(psql "$ADMIN" -qXAtc 'SELECT wal_bytes FROM pg_stat_wal')
k6 run --quiet -e BROKER_URL=http://127.0.0.1:$PORT -e N="${N:-200}" -e DUR=60s --summary-export "$OUT/sum.json" "$S/write.js" > "$OUT/run.log" 2>&1
c1=$(docker exec $PG sh -c "grep usage_usec /sys/fs/cgroup/cpu.stat" | awk '{print $2}')
w1=$(psql "$ADMIN" -qXAtc 'SELECT wal_bytes FROM pg_stat_wal')
n=$(jq '.metrics.http_reqs.count' "$OUT/sum.json"); f=$(jq '.metrics.http_req_failed.passes // .metrics.http_req_failed.value' "$OUT/sum.json")
rows=$(psql "$ADMIN" -qXAtc "SELECT (SELECT count(*) FROM attr_rows)||' attr_rows ('||(SELECT count(*) FROM attr_rows WHERE short)||' short), '||(SELECT count(*) FROM attr_instances)||' attr_instances, '||(SELECT count(*) FROM iri_ids)||' iri_ids'")
if [ -n "${PROFILE:-}" ]; then psql "$ADMIN" -qX -c 'CREATE EXTENSION IF NOT EXISTS pg_stat_statements' \
  -c "SELECT calls, round(total_exec_time::numeric,0) ms, round((total_exec_time/calls)::numeric,3) ms_per, toplevel, left(regexp_replace(query,'\s+',' ','g'),110) q FROM pg_stat_statements ORDER BY total_exec_time DESC LIMIT 14" \
  -c "SELECT funcname, calls, round(total_time::numeric,0) ms, round(self_time::numeric,0) self FROM pg_stat_user_functions ORDER BY total_time DESC LIMIT 8" \
  -c "SELECT count(*) partitions FROM pg_inherits WHERE inhparent = 'attr_rows'::regclass" > "$OUT/profile.txt"; fi
echo "$MODE pg_us_per_req=$(( (c1 - c0) / n )) wal_b_per_req=$(( (w1 - w0) / n )) reqs=$n failed=$f $rows load=$(cut -d' ' -f1 /proc/loadavg)"
