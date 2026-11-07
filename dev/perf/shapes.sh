#!/usr/bin/env bash
# Throughput table for the published request shapes: query at c50 and
# c200, single-entity retrieve at c50, median of three 5 s runs, p99 from
# the same runs. One broker, started here per store.
#
#   dev/perf/shapes.sh                          # memory
#   DATABASE_URL=postgres://… dev/perf/shapes.sh   # + postgres
#
#   BIN_SURFACES=target/release/antares-full dev/perf/shapes.sh
#                                               # + ADR-0027 section 6 rows
#
# Env: BIN (target/release/antares), SETUP (a command run against PORT once
#      each broker answers, before its seed), BIN_SURFACES (unset: a `full` build,
#      whose `query`, `view`, `odata`, `mcp` and `ogc` rows follow the vanilla
#      ones), SURFACE_VUS (50: their closed-loop clients), OUT
#      (results/perf), PORT (9471),
#      DATABASE_URL, RUNS (3), DURATION (5s), ENTITIES (100, seeded per
#      store), SPECS ("query 50,query 200,
#      retrieve 50": shape and closed-loop clients per row).
#
# Each broker is seeded once, before its rows, and PostgreSQL is vacuumed,
# analysed and checkpointed after the seed (psql on DATABASE_URL): a row
# measures its shape against a settled dataset, never the seed's own
# upserts nor the database still writing them out (T-3735).
set -euo pipefail
cd "$(dirname "$0")/../.."

BIN="${BIN:-target/release/antares}"
OUT="${OUT:-results/perf}"
PORT="${PORT:-9471}"
RUNS="${RUNS:-3}"
DURATION="${DURATION:-5s}"
mkdir -p "$OUT"
command -v k6 >/dev/null || { echo "k6 missing"; exit 1; }
[ -z "${DATABASE_URL:-}" ] || command -v psql >/dev/null || { echo "psql missing"; exit 1; }
[ -x "$BIN" ] || { echo "missing $BIN"; exit 1; }
DATA=$(mktemp -d); trap 'rm -rf "$DATA"; [ -n "${PID:-}" ] && kill "$PID" 2>/dev/null' EXIT

run() {  # run <shape> <vus>: prints "<req/s> <p99 ms>" (median over RUNS by req/s)
  local rows=()
  for _ in $(seq "$RUNS"); do
    k6 run --quiet --summary-export "$OUT/shape.json" -e BROKER_URL="http://127.0.0.1:$PORT" \
      -e SHAPE="$1" -e VUS="$2" -e DURATION="$DURATION" -e ENTITIES="${ENTITIES:-100}" \
      -e SEED=0 dev/perf/k6-shapes.js >/dev/null
    rows+=("$(python3 -c "
import json, sys; s=json.load(open('$OUT/shape.json'))['metrics']
# a refused request answers fast: a row with a failed check is no number
if s.get('checks', {}).get('fails'): sys.exit('$1: ' + str(s['checks']['fails']) + ' checks failed')
print(f\"{s['http_reqs']['rate']:.0f} {s['http_req_duration']['p(99)']:.2f}\")")")
  done
  printf '%s\n' "${rows[@]}" | sort -n | sed -n "$(( (RUNS + 1) / 2 ))p"
}

seed() {  # seed <store>: ENTITIES Vehicles into the broker on PORT, settled
  k6 run --quiet -e BROKER_URL="http://127.0.0.1:$PORT" -e SHAPE=seed \
    -e ENTITIES="${ENTITIES:-100}" dev/perf/k6-shapes.js >/dev/null
  # a seed that did not land would measure an empty broker
  curl -sf -o /dev/null ${JC_SUBJECT:+-H "x-jc-subject: $JC_SUBJECT" -H "x-jc-roles: ${JC_ROLES:-}"} \
    "http://127.0.0.1:$PORT/ngsi-ld/v1/entities/urn:ngsi-ld:Vehicle:shape:$(( ${ENTITIES:-100} - 1 ))" \
    || { echo "$1: seed missing" >&2; exit 1; }
  if [ "$1" = postgres ]; then
    psql "$DATABASE_URL" -qX -v ON_ERROR_STOP=1 -c 'VACUUM (ANALYZE)' -c 'CHECKPOINT'
  fi
}

unseed() {  # unseed <store>: the seeded Vehicles out again, so a PostgreSQL keeps nothing
  k6 run --quiet -e BROKER_URL="http://127.0.0.1:$PORT" -e SHAPE=unseed \
    -e ENTITIES="${ENTITIES:-100}" dev/perf/k6-shapes.js >/dev/null
  # and gives the space back: a plain VACUUM leaves a 100 000-entity seed's
  # pages in the files, and the next run's 1 000-entity query-q scanned
  # them (perf-baseline.md section 5, T-4853)
  if [ "$1" = postgres ]; then
    psql "$DATABASE_URL" -qX -v ON_ERROR_STOP=1 -c 'VACUUM (FULL, ANALYZE)'
  fi
}

{
  # this block is a subshell (the pipe into tee): it stops its own broker
  # when a row fails, or the next run's health check reads that broker
  trap '[ -z "${PID:-}" ] || { kill "$PID" 2>/dev/null; wait "$PID" 2>/dev/null; }' EXIT
  if curl -sf -o /dev/null "http://127.0.0.1:$PORT/q/health"; then
    echo "port $PORT already answers: another broker would be measured" >&2; exit 1
  fi
  echo "| store | shape | concurrency | req/s | p99 |"
  echo "|---|---|---|---|---|"
  for store in memory ${DATABASE_URL:+postgres}; do
    ANTARES_STORE="$store" ANTARES_DATA_DIR="$DATA" ANTARES_HTTP_PORT="$PORT" \
      ANTARES_DATABASE_URL="${DATABASE_URL:-}" ANTARES_ALLOW_SHARED_LOCAL=1 \
      "$BIN" > "$OUT/shapes-$store.log" 2>&1 & PID=$!
    until curl -sf -o /dev/null "http://127.0.0.1:$PORT/q/health"; do
      kill -0 "$PID" 2>/dev/null || { echo "broker exited: $OUT/shapes-$store.log" >&2; exit 1; }
      sleep 0.05
    done
    # SETUP: a command that configures the started broker before its seed
    # (dev/perf/platform-overhead.sh loads views and a policy document)
    [ -z "${SETUP:-}" ] || PORT="$PORT" $SETUP || { echo "$store: SETUP failed" >&2; exit 1; }
    echo "shapes $store seed" > "$OUT/phase"
    seed "$store"
    IFS=, read -ra specs <<<"${SPECS:-query 50,query 200,retrieve 50}"
    # The façade pair prices the in-process seam: the reference façade route
    # against the NGSI-LD request it makes. It exists only in a build with
    # the plugin-example feature, and a shipped build carries no addon, so
    # the rows are added when the route answers and skipped when it does not.
    if [ -z "${SPECS:-}" ] && curl -sf -o /dev/null \
         "http://127.0.0.1:$PORT/x/example/things?kind=Vehicle"; then
      specs+=("facade 50" "facade-twin 50")
    fi
    for spec in "${specs[@]}"; do
      echo "shapes $store ${spec// /-c}" > "$OUT/phase"
      read -r shape vus <<<"$spec"
      read -r rps p99 < <(run "$shape" "$vus")
      [ -n "${rps:-}" ] || { echo "$store $shape: no number" >&2; exit 1; }
      echo "| $store | $shape | c$vus | $rps | $p99 ms |"
    done
    unseed "$store"
    kill "$PID"; wait "$PID" 2>/dev/null || true; PID=
  done
  # ADR-0027 section 6 end to end (T-3523): the `full` binary with view
  # `perf` (Vehicle, every Attribute shown) and the default Tenant's LinkML
  # schema, its own `query` row first, so each surface is priced against
  # the same query on the same binary; the vanilla rows above stay the
  # vanilla binary's.
  if [ -n "${BIN_SURFACES:-}" ]; then
    [ -x "$BIN_SURFACES" ] || { echo "missing $BIN_SURFACES" >&2; exit 1; }
    mkdir -p "$DATA/surfaces"
    ANTARES_STORE=memory ANTARES_DATA_DIR="$DATA/surfaces" ANTARES_HTTP_PORT="$PORT" \
      ANTARES_ALLOW_SHARED_LOCAL=1 ANTARES_API_SURFACES=admin,views,odata,mcp,ogc \
      ANTARES_SCHEMA_MAX_BYTES=65536 "$BIN_SURFACES" > "$OUT/shapes-surfaces.log" 2>&1 & PID=$!
    until curl -sf -o /dev/null "http://127.0.0.1:$PORT/q/health"; do
      kill -0 "$PID" 2>/dev/null || { echo "broker exited: $OUT/shapes-surfaces.log" >&2; exit 1; }
      sleep 0.05
    done
    put() {  # put <path> <json>: fails the run on a refusal
      curl -sf -o /dev/null -X PUT -H 'Content-Type: application/json' --data "$2" \
        "http://127.0.0.1:$PORT$1" || { echo "PUT $1 refused" >&2; exit 1; }
    }
    put /q/views/perf '{"id": "perf", "tenant": "default", "selector": ["Vehicle"],
      "rules": [{"types": ["Vehicle"], "attributes": ["brand", "speed", "mileage", "colour", "location"]}],
      "operations": ["query_entities", "retrieve_entity"]}'
    put /q/schemas/default '{"id": "https://example.org/perf", "name": "perf",
      "prefixes": {"d": {"prefix_prefix": "d", "prefix_reference": "https://uri.etsi.org/ngsi-ld/default-context/"}},
      "default_prefix": "d",
      "classes": {"Vehicle": {"slots": ["brand", "speed", "mileage", "colour", "location"]}},
      "slots": {"brand": {"range": "string"}, "speed": {"range": "integer"}, "mileage": {"range": "integer"},
                "colour": {"range": "string"},
                "location": {"slot_uri": "https://uri.etsi.org/ngsi-ld/location",
                             "annotations": {"ngsi_ld_kind": {"tag": "ngsi_ld_kind", "value": "GeoProperty"}}}}}'
    echo "shapes surfaces seed" > "$OUT/phase"
    seed memory
    for shape in query view odata mcp ogc; do
      spec="$shape ${SURFACE_VUS:-50}"
      echo "shapes surfaces ${spec// /-c}" > "$OUT/phase"
      read -r shape vus <<<"$spec"
      read -r rps p99 < <(run "$shape" "$vus")
      [ -n "${rps:-}" ] || { echo "full $shape: no number" >&2; exit 1; }
      echo "| full | $shape | c$vus | $rps | $p99 ms |"
    done
    kill "$PID"; wait "$PID" 2>/dev/null || true; PID=
  fi
} | tee "$OUT/shapes.md"
