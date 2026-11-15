#!/usr/bin/env bash
#
# One-off cleanup of repeated history rows in the broker's attr_instances (T-4509, OPS-27).
#
# Before Antares T-4507 every re-upsert of an unchanged value recorded a new instance. T-4507
# stopped that; this deletes the old copies. A row goes when it equals the previous instance of
# the same (tenant, entity, attribute, datasetId) by (observed_at, created_at, instance_id),
# where "equal" is the broker's own T-4507 test: `data` without instanceId, createdAt and
# modifiedAt (so a new observedAt is a new observation and stays). The first row of each run is
# kept, so what is deleted is by definition a copy of a kept row.
#
# Native range partitions only, oldest first, closed ones only (upper bound <= now()), so rows
# the broker still writes are never touched. Keyset batches of BATCH entities ride the
# (tenant_id, entity_id, attr_id, observed_at DESC) index; each batch is one short transaction
# under statement_timeout. VACUUM per partition afterwards; sizes before and after are printed.
# VACUUM frees the space for reuse inside the partition, it does not shrink the files.
#
# Connection: the usual libpq environment (PGHOST, PGDATABASE, PGUSER, ...), as a role that may
# delete from attr_instances. Off-peak, by the deployer:
#
#   scripts/history-dedup.sh                          # every closed partition
#   scripts/history-dedup.sh attr_instances_2026w40   # only these
#
# Re-running is safe: a cleaned partition deletes nothing.
set -euo pipefail

BATCH="${BATCH:-200}"
TIMEOUT="${TIMEOUT:-60s}"
[[ "$BATCH" =~ ^[1-9][0-9]*$ ]] || { echo "BATCH must be a positive integer" >&2; exit 2; }

# Unit separator, not a tab: read collapses empty tab fields, and a tenant id may be empty.
q() { psql -X -q -v ON_ERROR_STOP=1 -At -F $'\x1f' "$@"; }

if [[ "$(q -c "SELECT relkind FROM pg_class WHERE oid = 'attr_instances'::regclass")" != p ]]; then
  echo "attr_instances is not a natively partitioned table (TimescaleDB?): this script does not apply" >&2
  exit 2
fi
# A default-partition row inside the span of the range partitions (a gap between them) may sit
# between two range rows; the window below would not see it and could delete a real change back
# to an earlier value. Rows before or after every range only precede or follow them, and the
# older-partition lookup below reads them, so those are fine.
if [[ "$(q -c "
  WITH b AS (SELECT substring(pg_get_expr(c.relpartbound, c.oid) FROM 'FROM \(''([^'']+)''\)')::timestamptz AS lo,
                    substring(pg_get_expr(c.relpartbound, c.oid) FROM 'TO \(''([^'']+)''\)')::timestamptz AS hi
               FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid
              WHERE i.inhparent = 'attr_instances'::regclass AND pg_get_expr(c.relpartbound, c.oid) <> 'DEFAULT')
  SELECT EXISTS (SELECT 1 FROM ONLY attr_instances_default d, (SELECT min(lo) lo, max(hi) hi FROM b) s
                  WHERE d.observed_at >= s.lo AND d.observed_at < s.hi)")" == t ]]; then
  echo "attr_instances_default holds rows inside the range partitions' span: refusing, an equal-looking pair could straddle one of them" >&2
  exit 2
fi

closed=$(q -c "
  SELECT c.relname FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid
   CROSS JOIN LATERAL (SELECT pg_get_expr(c.relpartbound, c.oid) AS b) e
   WHERE i.inhparent = 'attr_instances'::regclass AND e.b <> 'DEFAULT'
     AND substring(e.b FROM 'TO \(''([^'']+)''\)')::timestamptz <= now()
   ORDER BY substring(e.b FROM 'FROM \(''([^'']+)''\)')::timestamptz")
if (($#)); then
  for p in "$@"; do
    grep -qxF -- "$p" <<<"$closed" || { echo "$p: not a closed partition of attr_instances" >&2; exit 2; }
  done
  parts=$(printf '%s\n' "$@")
else
  parts=$closed
fi
[[ -n "$parts" ]] || { echo "no closed partition to clean"; exit 0; }

# One batch: the next :n entities after (:t, :e) by a loose index scan, their repeated rows
# deleted. Prints the deleted count and the last key; no key means the partition is done.
read -r -d '' BATCH_SQL <<'SQL' || true
SET statement_timeout = :'timeout';
WITH RECURSIVE k AS (
  (SELECT tenant_id, entity_id FROM ONLY :"part"
    WHERE (tenant_id, entity_id) > (:'t', :'e') ORDER BY tenant_id, entity_id LIMIT 1)
  UNION ALL
  SELECT nx.tenant_id, nx.entity_id FROM k CROSS JOIN LATERAL (
    SELECT p.tenant_id, p.entity_id FROM ONLY :"part" p
     WHERE (p.tenant_id, p.entity_id) > (k.tenant_id, k.entity_id)
     ORDER BY p.tenant_id, p.entity_id LIMIT 1) nx
), ks AS MATERIALIZED (SELECT * FROM k LIMIT :n),
r AS (
  SELECT p.ctid AS row_id, p.tenant_id, p.entity_id, p.attr_id, p.dataset_id, p.observed_at,
         p.data - ARRAY['instanceId','createdAt','modifiedAt'] AS v,
         lag(p.data - ARRAY['instanceId','createdAt','modifiedAt']) OVER w AS pv,
         row_number() OVER w AS nth
    FROM ONLY :"part" p JOIN ks USING (tenant_id, entity_id)
  WINDOW w AS (PARTITION BY p.tenant_id, p.entity_id, p.attr_id, p.dataset_id
               ORDER BY p.observed_at, p.created_at, p.instance_id)
), d AS (
  DELETE FROM ONLY :"part" p USING r
   WHERE p.ctid = r.row_id
     AND (r.v = r.pv
          -- the first row here repeats the newest row of an older partition
          OR (r.nth = 1 AND r.v = (
                SELECT x.data - ARRAY['instanceId','createdAt','modifiedAt'] FROM attr_instances x
                 WHERE x.tenant_id = r.tenant_id AND x.entity_id = r.entity_id
                   AND x.attr_id = r.attr_id AND x.dataset_id IS NOT DISTINCT FROM r.dataset_id
                   AND x.observed_at < r.observed_at
                 ORDER BY x.observed_at DESC, x.created_at DESC, x.instance_id DESC LIMIT 1)))
  RETURNING 1
)
SELECT (SELECT count(*) FROM d), l.tenant_id, l.entity_id
  FROM (SELECT 1) one
  LEFT JOIN (SELECT tenant_id, entity_id FROM ks
              ORDER BY tenant_id DESC, entity_id DESC LIMIT 1) l ON true;
SQL

size() { q -c "SELECT pg_size_pretty(pg_total_relation_size('$1'::regclass))"; }

total=0
while IFS= read -r part; do
  before=$(size "$part")
  t='' e='' deleted=0
  while :; do
    IFS=$'\x1f' read -r n t e < <(q -v part="$part" -v t="$t" -v e="$e" -v n="$BATCH" \
      -v timeout="$TIMEOUT" <<<"$BATCH_SQL")
    deleted=$((deleted + n))
    [[ -n "$t$e" ]] || break
  done
  q -c "VACUUM (ANALYZE) \"$part\""
  echo "$part: deleted $deleted rows, size $before -> $(size "$part")"
  total=$((total + deleted))
done <<<"$parts"
echo "total deleted: $total"
