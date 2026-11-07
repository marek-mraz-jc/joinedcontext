#!/usr/bin/env bash
# SPDX-License-Identifier: EUPL-1.2
# Latency of each NGSI-LD query shape as the broker's role runs it (T-4852):
# antares_app under the tables' forced RLS, beside a second Tenant of the
# same size. Seeds Tenants bench-a and bench-b with N Entities each (same
# data as the pg_explain RLS seed), then runs every shape in its statement
# form before T-4852 (T-5303 for q and attrs) and after, R times through pgbench, and prints
# shape, form, p50 ms, p99 ms and the plan's scan nodes.
#
#   dev/rls-plan-bench.sh <superuser-url> <N> [R] [R_BEFORE]
#   e.g. dev/rls-plan-bench.sh postgresql://antares:antares@localhost:15432/antares 100000
#
# Needs psql and pgbench on PATH and the broker's migrations applied. The
# app role's password is 'app', as in the tests. Leaves the seed in place;
# reruns at the same N reuse it.
set -euo pipefail
url=${1:?superuser url}
n=${2:?entities per tenant}
r=${3:-200}
# the forms before T-4852 read the Tenant: seconds per run at 1M
r_before=${4:-$r}
app=${url/\/\/*:*@///antares_app:app@}
[ "$app" != "$url" ] || { echo "the url must carry user:password" >&2; exit 2; }
work=$(mktemp -d)
trap 'rm -rf "${work:?}"' EXIT

have=$(psql "$url" -Atc "SELECT count(*) FROM entities WHERE tenant_id = 'bench-b'")
if [ "$have" != "$n" ]; then
  psql "$url" -q -v ON_ERROR_STOP=1 -v n="$n" <<'SQL'
DO $$ BEGIN CREATE ROLE antares_app LOGIN PASSWORD 'app';
EXCEPTION WHEN duplicate_object THEN NULL; END $$;
GRANT USAGE ON SCHEMA public TO antares_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO antares_app;
DELETE FROM entities WHERE tenant_id IN ('bench-a', 'bench-b');
DELETE FROM temporal_entities WHERE tenant_id IN ('bench-a', 'bench-b');
DELETE FROM attr_instances WHERE tenant_id IN ('bench-a', 'bench-b');
INSERT INTO tenants (tenant_id) VALUES ('bench-a'), ('bench-b') ON CONFLICT DO NOTHING;
INSERT INTO entities (tenant_id, id, entity, types, location, created_at, modified_at)
SELECT t, 'urn:ngsi-ld:T' || (g % 10) || ':' || lpad(g::text, 8, '0'),
       jsonb_build_object(
         'https://x/tenant', t,
         'https://x/speed', jsonb_build_array(jsonb_build_object('type', 'Property', 'value', g % 100)),
         'https://x/brand', jsonb_build_array(jsonb_build_object('type', 'Property', 'value', 'b' || (g % 50)))),
       CASE WHEN g % 100 = 0 THEN ARRAY['https://x/T' || (g % 10), 'https://x/M']
            ELSE ARRAY['https://x/T' || (g % 10)] END,
       ST_SetSRID(ST_MakePoint(10 + (g % 1000) / 100.0, 45 + ((g / 1000) % 100) / 100.0), 4326),
       now(), now()
  FROM (VALUES ('bench-a'), ('bench-b')) v(t), generate_series(1, :n) g;
INSERT INTO temporal_entities (tenant_id, id, types, meta, created_at, modified_at)
SELECT tenant_id, id, types, jsonb_build_object('id', id), now(), now()
  FROM entities WHERE tenant_id IN ('bench-a', 'bench-b');
INSERT INTO attr_instances (tenant_id, entity_id, attr_id, instance_id, observed_at,
                            created_at, modified_at, data)
SELECT t, 'urn:ngsi-ld:T' || (g % 10) || ':' || lpad(g::text, 8, '0'), 'https://x/speed',
       'urn:i:' || k, now() - k * interval '1 minute', now(), now(),
       jsonb_build_object('type', 'Property', 'value', k)
  FROM (VALUES ('bench-a'), ('bench-b')) v(t), generate_series(1, least(:n, 1000)) g,
       generate_series(1, 50) k;
VACUUM ANALYZE entities;
VACUUM ANALYZE entity_values;
VACUUM ANALYZE temporal_entities;
ANALYZE attr_instances;
SQL
fi

w="tenant_id = 'bench-a' AND (expires_at IS NULL OR expires_at > now())"
speed() { echo "entity @? '\$.\"https://x/speed\"[*].\"value\" ? (@ $1)'::jsonpath"; }
# the type arms of store::pg::entity::arms; $1 the extra WHERE, then types
arms() {
  local extra=$1; shift
  local named sel parts="" t
  named=$(printf "'%s'," "$@"); named="ARRAY[${named%,}]"
  sel=$(printf "types @> ARRAY['%s'] OR " "$@"); sel="(${sel% OR })"
  for t in "$@"; do
    if [ $# -eq 1 ]; then f="types[1] = '$t'"; else f="types[1] = '$t' AND $sel"; fi
    parts+="(SELECT id, entity FROM entities WHERE $w$extra AND $f ORDER BY id LIMIT 21) UNION ALL "
  done
  parts+="(SELECT id, entity FROM entities WHERE $w$extra AND cardinality(types) > 1 AND types[1] <> ALL($named) AND $sel ORDER BY id LIMIT 21)"
  echo "SELECT entity FROM ($parts) AS entities ORDER BY id LIMIT 21"
}
one() { # T-4781's split, unchanged by T-4852 for one type
  echo "SELECT entity FROM ((SELECT id, entity FROM entities WHERE $w$1 AND types[1] = 'https://x/T1' ORDER BY id LIMIT 21) UNION ALL (SELECT id, entity FROM entities WHERE $w$1 AND cardinality(types) > 1 AND types[1] IS DISTINCT FROM 'https://x/T1' AND types @> ARRAY['https://x/T1'] ORDER BY id LIMIT 21)) AS entities ORDER BY id LIMIT 21"
}
within="ST_Within(location, ST_SetSRID(ST_GeomFromGeoJSON('{\"type\":\"Polygon\",\"coordinates\":[[[15.0,45.5],[15.0,45.52],[15.05,45.52],[15.05,45.5],[15.0,45.5]]]}'), 4326))"
near="ST_DWithin(location::geography, ST_SetSRID(ST_GeomFromGeoJSON('{\"type\":\"Point\",\"coordinates\":[15.0,45.5]}'), 4326)::geography, 503.5)"
# compile_geo's output for the same regions (pg_explain.rs pins its plan)
near_cell="((((location_cell >= 'u24dv4' AND location_cell < 'u24dv4~') OR (location_cell >= 'u24dv5' AND location_cell < 'u24dv5~') OR (location_cell >= 'u24dv6' AND location_cell < 'u24dv6~') OR (location_cell >= 'u24dv7' AND location_cell < 'u24dv7~') OR location_cell = ANY(ARRAY['', 'u', 'u2', 'u24', 'u24d', 'u24dv'])) AND (ST_DWithin(location::geography, ST_SetSRID(ST_GeomFromGeoJSON('{\"coordinates\":[15.0,45.5],\"type\":\"Point\"}'), 4326)::geography, 503.49999999999994))) OR location_ambiguous)"
within_cell="((((location_cell >= 'u24dv' AND location_cell < 'u24dv~') OR (location_cell >= 'u24dy' AND location_cell < 'u24dy~') OR location_cell = ANY(ARRAY['', 'u', 'u2', 'u24', 'u24d'])) AND (ST_Within(location, ST_SetSRID(ST_GeomFromGeoJSON('{\"coordinates\":[[[15.0,45.5],[15.0,45.52],[15.05,45.52],[15.05,45.5],[15.0,45.5]]],\"type\":\"Polygon\"}'), 4326)))) OR location_ambiguous)"
temporal() { # the paged temporal query's entity selection, $1 its type predicate
  echo "SELECT m.meta, count(*) OVER () FROM temporal_entities m WHERE m.tenant_id = 'bench-a' AND $1 AND EXISTS (SELECT 1 FROM attr_instances ai WHERE ai.tenant_id = m.tenant_id AND ai.entity_id = m.id) ORDER BY m.id LIMIT 20"
}
# store::pg::entity::value_probe and attrs_probe (T-5303): the probe, then
# the page with the ids it found as `id = ANY(...)`, or the page as before
# when 1 001 rows or more answer it. $1 the probe's condition, $2 the page
# form taking the extra WHERE as its argument, $3 the term's own WHERE.
probed() {
  local probe="SELECT id FROM entity_values WHERE tenant_id = 'bench-a' AND $1 LIMIT 1001"
  local ids n
  ids=$(PGOPTIONS="-c antares.tenant=bench-a" psql "$app" -Atc "$probe" | sort -u)
  n=$(printf '%s' "$ids" | grep -c . || true)
  if [ "$n" -ge 1001 ]; then
    printf '%s;\n%s\n' "$probe" "$($2 " AND $3")"
  else
    ids=$(printf '%s' "$ids" | sed "s/.*/'&'/" | paste -sd, -)
    printf '%s;\n%s\n' "$probe" "$($2 " AND $3 AND id = ANY(ARRAY[${ids}]::text[])")"
  fi
}
spd="attr = 'https://x/speed'"
plain() { echo "SELECT entity FROM entities WHERE $w$1 ORDER BY id LIMIT 21"; }
inst="FROM attr_instances WHERE tenant_id = 'bench-a' AND entity_id = 'urn:ngsi-ld:T1:00000001' AND attr_id = 'https://x/speed'"

shape() { printf '%s\n' "$3" > "$work/$1.$2.sql"; echo "$1 $2"; }
{
  shape id same "SELECT entity FROM entities WHERE $w AND id = ANY(ARRAY['urn:ngsi-ld:T1:00000001', 'urn:ngsi-ld:T2:00000002']) ORDER BY id LIMIT 21"
  shape type same "$(one '')"
  shape type=A,B before "SELECT entity FROM entities WHERE $w AND (types @> ARRAY['https://x/T1'] OR types @> ARRAY['https://x/T2']) ORDER BY id LIMIT 21"
  shape type=A,B after "$(arms '' https://x/T1 https://x/T2)"
  shape q-equality before "$(one " AND $(speed '== 42')")"
  shape q-equality after "$(probed "$spd AND f = ANY(ARRAY[42, 'NaN'::float8])" one "$(speed '== 42')")"
  shape q-range before "$(one " AND $(speed '> 98')")"
  shape q-range after "$(probed "$spd AND f >= 98" one "$(speed '> 98')")"
  shape q-range-all before "$(one " AND $(speed '>= 0')")"
  shape q-range-all after "$(probed "$spd AND f >= 0" one "$(speed '>= 0')")"
  shape attrs before "$(one " AND entity ?| ARRAY['https://x/brand']")"
  shape attrs after "$(probed "attr = ANY(ARRAY['https://x/brand'])" one "entity ?| ARRAY['https://x/brand']")"
  shape q-eq-untyped before "$(plain " AND $(speed '== 42')")"
  shape q-eq-untyped after "$(probed "$spd AND f = ANY(ARRAY[42, 'NaN'::float8])" plain "$(speed '== 42')")"
  shape q-all-untyped before "$(plain " AND $(speed '>= 0')")"
  shape q-all-untyped after "$(probed "$spd AND f >= 0" plain "$(speed '>= 0')")"
  shape attrs-untyped before "$(plain " AND entity ?| ARRAY['https://x/rare']")"
  shape attrs-untyped after "$(probed "attr = ANY(ARRAY['https://x/rare'])" plain "entity ?| ARRAY['https://x/rare']")"
  shape geo-near before "SELECT entity FROM entities WHERE $w AND (($near) OR location_ambiguous) ORDER BY id LIMIT 10000"
  shape geo-near after "SELECT entity FROM entities WHERE $w AND $near_cell ORDER BY id LIMIT 10000"
  shape geo-within before "SELECT entity FROM entities WHERE $w AND (($within) OR location_ambiguous) ORDER BY id LIMIT 10000"
  shape geo-within after "SELECT entity FROM entities WHERE $w AND $within_cell ORDER BY id LIMIT 10000"
  shape temporal-type before "$(temporal "m.types && ARRAY['https://x/T1']")"
  shape temporal-type after "$(temporal "(m.types[1] = ANY(ARRAY['https://x/T1']) OR (cardinality(m.types) > 1 AND m.types && ARRAY['https://x/T1']))")"
  shape temporal-lastN same "SELECT data $inst ORDER BY observed_at DESC LIMIT 5"
  shape temporal-between same "SELECT data $inst AND observed_at >= now() - interval '20 minutes' AND observed_at < now() ORDER BY observed_at"
} > "$work/list"

printf '%-17s %-7s %9s %9s  %s\n' shape form p50_ms p99_ms plan
while read -r name form; do
  f="$work/$name.$form.sql"
  runs=$r; [ "$form" != before ] || runs=$r_before
  # one statement a line; a probed shape has two
  plan=$(while IFS= read -r stmt; do
           PGOPTIONS="-c antares.tenant=bench-a" psql "$app" -Atc "EXPLAIN ${stmt%;}"
         done < "$f" \
    | { grep -oE '(Seq Scan|Index Only Scan|Index Scan|Bitmap Index Scan)( Backward)? (using|on) [a-z_]+' || true; } \
    | sed -E 's/ (using|on) / /' | sort | uniq | paste -sd, -)
  ( cd "$work" && PGOPTIONS="-c antares.tenant=bench-a" pgbench "$app" -n -c 1 -t "$runs" \
      -f "$f" -l --log-prefix="$name.$form" >/dev/null 2>&1 )
  # pgbench log: client txn latency_us ...; one log file per run
  read -r p50 p99 < <(cat "$work/$name.$form".* | awk '{print $3}' | sort -n \
    | awk '{a[NR]=$1} END {printf "%.2f %.2f\n", a[int(NR*0.5)]/1000, a[int(NR*0.99)]/1000}')
  printf '%-17s %-7s %9s %9s  %s\n' "$name" "$form" "$p50" "$p99" "$plan"
done < "$work/list"
