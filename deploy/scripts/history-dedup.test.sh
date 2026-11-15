#!/usr/bin/env bash
#
# Test for scripts/history-dedup.sh (T-4509) against a throwaway Postgres in Docker:
# runs of equal values, a run across two partitions, observedAt, datasetId, an empty tenant id,
# the open partition, a second run, a default row before every range and the refusal of one in a gap.
#
#   scripts/history-dedup.test.sh
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
name="history-dedup-test-$$"
docker run -d --rm --name "$name" -e POSTGRES_PASSWORD=test -p 127.0.0.1::5432 postgres:18 >/dev/null
trap 'docker rm -f "$name" >/dev/null' EXIT
export PGHOST=127.0.0.1 PGUSER=postgres PGPASSWORD=test PGDATABASE=postgres
PGPORT="$(docker port "$name" 5432/tcp | head -1 | sed 's/.*://')"
export PGPORT
for _ in $(seq 60); do pg_isready -q && psql -Xqc 'SELECT 1' >/dev/null 2>&1 && break; sleep 1; done

q() { psql -X -q -v ON_ERROR_STOP=1 -At "$@"; }
q >/dev/null <<'SQL'
CREATE TABLE attr_instances (
  tenant_id text NOT NULL, entity_id text NOT NULL, attr_id text NOT NULL,
  instance_id text NOT NULL, dataset_id text, observed_at timestamptz NOT NULL,
  created_at timestamptz NOT NULL, modified_at timestamptz NOT NULL, deleted_at timestamptz,
  data jsonb NOT NULL) PARTITION BY RANGE (observed_at);
CREATE TABLE attr_instances_default PARTITION OF attr_instances DEFAULT;
CREATE TABLE attr_instances_w1 PARTITION OF attr_instances FOR VALUES FROM ('2026-01-05') TO ('2026-01-12');
CREATE TABLE attr_instances_w2 PARTITION OF attr_instances FOR VALUES FROM ('2026-01-12') TO ('2026-01-19');
CREATE TABLE attr_instances_open PARTITION OF attr_instances FOR VALUES FROM (now() - interval '1 day') TO (now() + interval '6 days');
CREATE INDEX i_attr_instances_lookup ON attr_instances (tenant_id, entity_id, attr_id, observed_at DESC);

-- one row: tenant, entity, attr, dataset, observed_at, value, observedAt in data or not
CREATE FUNCTION r(t text, e text, a text, ds text, at timestamptz, v int, obs boolean DEFAULT false)
RETURNS void LANGUAGE sql AS $$
  INSERT INTO attr_instances VALUES (t, e, a, gen_random_uuid()::text, ds, at, at, at, NULL,
    jsonb_build_object('type', 'Property', 'value', v,
                       'instanceId', 'urn:ngsi-ld:' || gen_random_uuid(),
                       'createdAt', at, 'modifiedAt', at)
    || CASE WHEN obs THEN jsonb_build_object('observedAt', at) ELSE '{}' END
    || CASE WHEN ds IS NULL THEN '{}' ELSE jsonb_build_object('datasetId', ds) END) $$;

-- A: 1 1 1 2 1 in w1, then 1 in w2: the 2nd, 3rd and the w2 row go (keep 1 2 1)
SELECT r('city', 'urn:A', 'p', NULL, '2026-01-06 00:00'::timestamptz + n * interval '1 h', v)
  FROM unnest(ARRAY[1,1,1,2,1]) WITH ORDINALITY AS u(v, n);
SELECT r('city', 'urn:A', 'p', NULL, '2026-01-13', 1);
-- B: the same value with its own observedAt each time: new observations, all stay
SELECT r('city', 'urn:B', 'p', NULL, '2026-01-06 00:00'::timestamptz + n * interval '1 h', 7, true)
  FROM generate_series(1, 3) n;
-- C: datasets ds1 ds2 ds1 with one value: only the 2nd ds1 goes
SELECT r('city', 'urn:C', 'p', 'ds1', '2026-01-06 01:00', 5);
SELECT r('city', 'urn:C', 'p', 'ds2', '2026-01-06 02:00', 5);
SELECT r('city', 'urn:C', 'p', 'ds1', '2026-01-06 03:00', 5);
-- D: two attributes interleaved, each repeated once: one row of each goes
SELECT r('city', 'urn:D', a, NULL, '2026-01-06 00:00'::timestamptz + n * interval '1 h', 3)
  FROM generate_series(1, 2) n, unnest(ARRAY['p', 'q']) a;
-- E: empty tenant id, repeated: one row goes
SELECT r('', 'urn:E', 'p', NULL, '2026-01-06 01:00', 9);
SELECT r('', 'urn:E', 'p', NULL, '2026-01-06 02:00', 9);
-- F: the open partition is not touched
SELECT r('city', 'urn:F', 'p', NULL, now() - interval '2 h', 4);
SELECT r('city', 'urn:F', 'p', NULL, now() - interval '1 h', 4);
SQL

fail() { echo "FAIL: $*" >&2; exit 1; }
count() { q -c "SELECT count(*) FROM attr_instances WHERE $1"; }

out=$(BATCH=1 "$here/history-dedup.sh")
echo "$out"
grep -q '^total deleted: 7$' <<<"$out" || fail "first run should delete 7 rows"
[[ "$(q -c "SELECT string_agg(data->>'value', ' ' ORDER BY observed_at) FROM attr_instances WHERE entity_id = 'urn:A'")" == "1 2 1" ]] \
  || fail "urn:A should keep 1 2 1"
[[ "$(count "entity_id = 'urn:B'")" == 3 ]] || fail "urn:B observations must stay"
[[ "$(q -c "SELECT string_agg(dataset_id, ' ' ORDER BY observed_at) FROM attr_instances WHERE entity_id = 'urn:C'")" == "ds1 ds2" ]] \
  || fail "urn:C should keep ds1 ds2"
[[ "$(count "entity_id = 'urn:D'")" == 2 ]] || fail "urn:D should keep one row per attribute"
[[ "$(count "tenant_id = '' AND entity_id = 'urn:E'")" == 1 ]] || fail "urn:E (empty tenant) should keep one row"
[[ "$(count "entity_id = 'urn:F'")" == 2 ]] || fail "the open partition must not be touched"

grep -q '^total deleted: 0$' <<<"$("$here/history-dedup.sh")" || fail "a second run should delete nothing"
"$here/history-dedup.sh" attr_instances_open >/dev/null 2>&1 && fail "an open partition named on the command line must be refused"

q -c "INSERT INTO attr_instances_default SELECT tenant_id, entity_id, attr_id, instance_id, dataset_id, '2020-01-01', created_at, modified_at, deleted_at, data FROM attr_instances_w1 LIMIT 1"
# it repeats the first w1 row, so that row now goes as its copy
grep -q '^total deleted: 1$' <<<"$("$here/history-dedup.sh")" || fail "a default row before every range must not block the run"
q -c "INSERT INTO attr_instances_default SELECT tenant_id, entity_id, attr_id, instance_id, dataset_id, '2026-03-01', created_at, modified_at, deleted_at, data FROM attr_instances_w1 LIMIT 1"
"$here/history-dedup.sh" >/dev/null 2>&1 && fail "a default row in a gap between the ranges must be refused"
echo "history-dedup: ok"
