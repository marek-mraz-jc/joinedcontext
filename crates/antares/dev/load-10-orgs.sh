#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Ten organizations at once (T-6628): a short load test of the joinedcontext broker image on a
# local PostGIS, 10 Tenants each with its own caller and rule document, every request decided by
# the policy engine under forced RLS, and a k6 threshold that fails the run on any cross-Tenant
# leak. Never against dev or prod: it starts its own containers and removes them at exit.
#
#   dev/load-10-orgs.sh                       # 10,000 req/s, 10 Tenants, 15 s warm-up + 60 s
#   RATE=4000 OUT=results/load-10-orgs/r4k dev/load-10-orgs.sh
#
# Env: IMAGE (the dev broker image by digest), PG_IMAGE (workspace.yml's postgis), RATE (10000),
#      TENANTS (10), DURATION (60s), WARMUP (15s), POOL (13, dev's ANTARES_PG_POOL),
#      PG_MAX_CONNECTIONS (55, dev's), BROKER_CPUS (0-3), PG_CPUS (4-7), K6_CPUS (8-11),
#      PG_PORT (25432), PORT (29080), METRICS_PORT (29464), OUT (results/load-10-orgs/<stamp>).
# Out: $OUT/summary.json (k6), report.md (the table), pg_stat_statements.txt, metrics-{before,after}.txt,
#      docker-stats.txt, pg-connections.txt, broker.log, meta.txt. Exit 99 = a k6 threshold failed
#      (a leak or a refused cross-Tenant read answered otherwise).
set -euo pipefail
cd "$(dirname "$0")/.."

IMAGE="${IMAGE:-ghcr.io/marek-mraz-jc/antares-broker-jc@sha256:433000c28215f279d1b63e16060223b029958d65ed3918e0491b867de2f3174e}"
PG_IMAGE="${PG_IMAGE:-$(sed -n 's/^ *image: \(postgis\/postgis:.*\)$/\1/p' .github/workflows/workspace.yml | head -1)}"
RATE="${RATE:-10000}" TENANTS="${TENANTS:-10}" DURATION="${DURATION:-60s}" WARMUP="${WARMUP:-15s}"
POOL="${POOL:-13}" PG_MAX_CONNECTIONS="${PG_MAX_CONNECTIONS:-55}"
BROKER_CPUS="${BROKER_CPUS:-0-3}" PG_CPUS="${PG_CPUS:-4-7}" K6_CPUS="${K6_CPUS:-8-11}"
PG_PORT="${PG_PORT:-25432}" PORT="${PORT:-29080}" METRICS_PORT="${METRICS_PORT:-29464}"
OUT="${OUT:-results/load-10-orgs/$(date -u +%Y%m%dT%H%M%SZ)}"
for tool in docker k6 curl psql python3 taskset openssl; do
  command -v "$tool" >/dev/null || { echo "$tool missing" >&2; exit 1; }
done
[ -n "$PG_IMAGE" ] || { echo "no postgis image line in .github/workflows/workspace.yml" >&2; exit 1; }
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
RUN=$$
PG="load10-pg-$RUN" BROKER="load10-broker-$RUN"
SECRETS=$(mktemp -d)
SAMPLER=
cleanup() {
  if [ -n "$SAMPLER" ]; then kill "$SAMPLER" 2>/dev/null || true; fi
  docker logs "$BROKER" > "$OUT/broker.log" 2>&1 || true
  docker rm -f "$BROKER" "$PG" >/dev/null 2>&1 || true
  rm -rf "${SECRETS:?}"
}
trap cleanup EXIT

# Secrets are generated per run, readable by the image's user (65532), and never written to $OUT.
openssl rand -hex 32 > "$SECRETS/admin-token"
openssl rand -hex 16 > "$SECRETS/pg-password"
chmod 755 "$SECRETS" && chmod 644 "$SECRETS/admin-token"
ADMIN="$(cat "$SECRETS/admin-token")"
PGPASS="$(cat "$SECRETS/pg-password")"

{
  echo "antares image: $IMAGE"
  echo "antares origin/dev: $(git rev-parse origin/dev 2>/dev/null || echo unknown)"
  echo "postgis image: $PG_IMAGE"
  echo "script commit: $(git rev-parse HEAD)"
  echo "rate=$RATE tenants=$TENANTS warmup=$WARMUP duration=$DURATION pool=$POOL pg_max_connections=$PG_MAX_CONNECTIONS"
  echo "cpus: broker=$BROKER_CPUS postgis=$PG_CPUS k6=$K6_CPUS (host $(nproc) cores)"
  echo "host load at start: $(cut -d' ' -f1-3 /proc/loadavg)"
  echo "started: $(date -u +%FT%TZ)"
} > "$OUT/meta.txt"

# PostGIS: dev's major, its max_connections, pg_stat_statements for the stage split. The broker's
# login is no superuser, so ANTARES_REQUIRE_RLS holds and forced RLS decides every row.
docker run -d --name "$PG" --network host --cpuset-cpus "$PG_CPUS" --shm-size=1g \
  -e POSTGRES_PASSWORD="$PGPASS" -e PGPORT="$PG_PORT" "$PG_IMAGE" \
  -c port="$PG_PORT" -c listen_addresses=127.0.0.1 -c max_connections="$PG_MAX_CONNECTIONS" \
  -c shared_buffers=512MB -c shared_preload_libraries=pg_stat_statements \
  -c pg_stat_statements.track=all >/dev/null
export PGPASSWORD="$PGPASS"
SU="postgresql://postgres@127.0.0.1:$PG_PORT/postgres"
for _ in $(seq 120); do psql "$SU" -qAtc 'SELECT 1' >/dev/null 2>&1 && break; sleep 0.5; done
psql "$SU" -qX -v ON_ERROR_STOP=1 -v pass="$PGPASS" <<'SQL'
CREATE ROLE antares LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD :'pass';
CREATE DATABASE antares OWNER antares;
\c antares
CREATE EXTENSION IF NOT EXISTS postgis;
CREATE EXTENSION IF NOT EXISTS btree_gin;
CREATE EXTENSION IF NOT EXISTS pg_stat_statements;
GRANT pg_read_all_stats TO antares;
SQL
DB="postgresql://postgres@127.0.0.1:$PG_PORT/antares"

# The broker with dev's runtime keys (deployment base + development values); the image carries
# dev's FEATURES and _RJEM_MALLOC_CONF.
docker run -d --name "$BROKER" --network host --cpuset-cpus "$BROKER_CPUS" \
  -v "$SECRETS:/run/load-secrets:ro" \
  -e ANTARES_HTTP_PORT="$PORT" -e ANTARES_METRICS_PORT="$METRICS_PORT" -e ANTARES_TELEMETRY=1 \
  -e ANTARES_STORE=postgres -e ANTARES_BUS=local -e ANTARES_ALLOW_SHARED_LOCAL=1 -e ANTARES_REQUIRE_RLS=1 \
  -e ANTARES_DATABASE_URL="postgresql://antares:$PGPASS@127.0.0.1:$PG_PORT/antares" \
  -e ANTARES_MAX_CONNECTIONS=512 -e ANTARES_PG_POOL="$POOL" -e ANTARES_PG_BATCH_ASYNC_COMMIT=true \
  -e ANTARES_API_SURFACES=admin,views,mcp,access,ws,ws-views,ogc,linkml,odata,sta,ical,feed \
  -e ANTARES_WS_MAX_CONNECTIONS=200 -e ANTARES_WS_MAX_CONNECTIONS_PER_TENANT=50 \
  -e ANTARES_SCHEMA_MAX_BYTES=1048576 -e ANTARES_RETENTION_HISTORY_DEFAULT=P28D \
  -e ANTARES_TEMPORAL_RECORD=ruled -e ANTARES_HISTORY_BACKFILL=on \
  -e ANTARES_POLICY=joinedcontext \
  -e ANTARES_POLICY_SUBJECT_HEADERS=x-jc-subject,x-jc-groups,x-jc-roles,x-jc-client,x-jc-restricted,x-jc-did,x-jc-read-only,x-jc-member \
  -e ANTARES_ADMIN_TOKEN_FILE=/run/load-secrets/admin-token -e ANTARES_ADMIN_METRICS=open \
  -e ANTARES_MCP_ALLOWED_HOSTS="127.0.0.1:$PORT" -e ANTARES_EGRESS_ALLOW_PRIVATE=false \
  "$IMAGE" >/dev/null
B="http://127.0.0.1:$PORT"
for _ in $(seq 300); do curl -sf -o /dev/null "$B/q/health" && break; sleep 0.2; done
curl -sf -o /dev/null "$B/q/health" || { echo "broker did not start" >&2; docker logs "$BROKER" 2>&1 | tail -20 >&2; exit 1; }

# One organization per Tenant: its own caller, one Policy granting that caller the four clauses
# the load uses on its three types, and a retention rule giving Sensor (observed values only:
# temperature) a history.
VOCAB=https://uri.etsi.org/ngsi-ld/default-context
for i in $(seq "$TENANTS"); do
  t="t$i"
  doc=$(printf '{"policies":[{"id":"%s/load","effect":"permit","assignee":{"kind":"serviceAccount","id":"load-%s"},"operations":["5.6.8","5.7.1","5.7.2","5.7.3"],"information":[{"entities":[{"type":"%s/Sensor"},{"type":"%s/Building"},{"type":"%s/Vehicle"}]}]}]}' "$t" "$t" "$VOCAB" "$VOCAB" "$VOCAB")
  curl -sf -o /dev/null -X PUT -H "Authorization: Bearer $ADMIN" -H 'Content-Type: application/json' \
    --data "$doc" "$B/q/policy/$t" || { echo "policy $t refused" >&2; exit 1; }
  curl -sf -o /dev/null -X PUT -H "Authorization: Bearer $ADMIN" -H 'Content-Type: application/json' \
    --data "{\"rules\":[{\"type\":\"$VOCAB/Sensor\",\"history\":\"P28D\"}]}" "$B/q/retention/$t" \
    || { echo "retention $t refused" >&2; exit 1; }
done

k6env=(-e "BROKER_URL=$B" -e "TENANTS=$TENANTS" -e "RATE=$RATE" -e "DURATION=$DURATION" -e "WARMUP=$WARMUP")
for v in SENSORS BUILDINGS VEHICLES HISTORY XTENANT_EVERY MAX_VUS_PER_TENANT; do
  [ -z "${!v:-}" ] || k6env+=(-e "$v=${!v}")
done
echo "seeding $TENANTS Tenants"
MODE=seed taskset -c "$K6_CPUS" k6 run -q --summary-mode=disabled "${k6env[@]}" -e MODE=seed dev/load-10-orgs.js
psql "$DB" -qAtc "VACUUM ANALYZE" >/dev/null
psql "$DB" -qAtc "SELECT 'rows', count(*) FROM entities" >> "$OUT/meta.txt" 2>/dev/null || true

metrics() { curl -sf "http://127.0.0.1:$METRICS_PORT/q/metrics" > "$1" || curl -sf -H "Authorization: Bearer $ADMIN" "$B/q/metrics" > "$1" || true; }
psql "$DB" -qAtc "SELECT pg_stat_statements_reset()" >/dev/null
metrics "$OUT/metrics-before.txt"
(
  while :; do
    printf '%s ' "$(date -u +%T)" >> "$OUT/docker-stats.txt"
    docker stats --no-stream --format '{{.Name}} cpu={{.CPUPerc}} mem={{.MemUsage}}' "$BROKER" "$PG" | tr '\n' ' ' >> "$OUT/docker-stats.txt"
    echo >> "$OUT/docker-stats.txt"
    psql "$DB" -qAtc "SELECT now()::time(0), count(*), count(*) FILTER (WHERE state = 'active') FROM pg_stat_activity WHERE datname = 'antares'" >> "$OUT/pg-connections.txt" 2>/dev/null || true
    sleep 2
  done
) & SAMPLER=$!

echo "load: $RATE req/s over $TENANTS Tenants, $WARMUP warm-up + $DURATION"
set +e
taskset -c "$K6_CPUS" k6 run -q --log-output=none "${k6env[@]}" -e MODE=load -e "SUMMARY=$OUT/summary.json" dev/load-10-orgs.js
code=$?
set -e
kill "$SAMPLER" 2>/dev/null || true
SAMPLER=
metrics "$OUT/metrics-after.txt"
psql "$DB" -qX -P pager=off -c "SELECT calls, round(total_exec_time) AS total_ms, round(mean_exec_time::numeric, 3) AS mean_ms, rows, left(regexp_replace(query, '\s+', ' ', 'g'), 160) AS query FROM pg_stat_statements WHERE dbid = (SELECT oid FROM pg_database WHERE datname = 'antares') ORDER BY total_exec_time DESC LIMIT 20" > "$OUT/pg_stat_statements.txt"
psql "$DB" -qAtc "SELECT round(sum(total_exec_time)), sum(calls) FROM pg_stat_statements WHERE dbid = (SELECT oid FROM pg_database WHERE datname = 'antares')" > "$OUT/pg-total.txt"
echo "host load at end: $(cut -d' ' -f1-3 /proc/loadavg)" >> "$OUT/meta.txt"
python3 dev/load-10-orgs-report.py "$OUT" > "$OUT/report.md" || true
cat "$OUT/report.md"
exit "$code"
