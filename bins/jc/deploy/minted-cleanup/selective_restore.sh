#!/usr/bin/env bash
# selective_restore.sh <dump> <ids.tsv> <stage conninfo> <target conninfo> [--apply]
#
# Puts back what the cleanup deleted for the listed ids only, from a pg_dump -Fc of jc_antares
# taken before it: the entities rows and their history (temporal_entities, attr_instances,
# attr_versions). Restoring the whole dump onto the live database would roll back every write
# after it; this inserts only rows that are absent (ON CONFLICT DO NOTHING) and touches nothing
# else. ids.tsv: tenant, id per line (W/st/done.txt rows joined with W/list.tsv, or a hand list).
#
# <stage> is an EMPTY scratch database on a scratch server with the dump's extensions (PostGIS,
# plus timescaledb when `pg_restore --list` names it): the dump is restored there whole and the
# rows are read out of it. <target> is the database to restore into; the role must be a superuser
# (the history tables FORCE row level security). Conninfo strings carry no password: psql reads
# it from PGPASSFILE (0600). Without --apply the target transaction is rolled back after the
# counts, so a dry run shows what would be inserted. Rows go through a 0700 work dir that is
# removed at the end; it holds entity values (personal data on prod): keep it on the scratch host.
#
# ponytail: attr_rows (history_cutover.at set) needs its iri_ids rows renumbered for the target;
# the script refuses such a dump instead of guessing. Add that when a cut-over database needs it.
set -euo pipefail
dump=${1:?dump}; ids=${2:?ids.tsv}; stage=${3:?stage conninfo}; target=${4:?target conninfo}
apply=${5:-}
[ -z "$apply" ] || [ "$apply" = --apply ] || { echo "unknown flag $apply" >&2; exit 2; }
case "$stage$target" in *password=*) echo "no password in a conninfo: use PGPASSFILE" >&2; exit 2;; esac
tables=(entities temporal_entities attr_instances attr_versions)
idcol=(id id entity_id entity_id)
psq() { psql -X -q -v ON_ERROR_STOP=1 -At "$@"; }

work=$(mktemp -d); chmod 700 "$work"; trap 'rm -rf "$work"' EXIT
# Tab-separated tenant and id, nothing else (an id never holds a tab or a quote).
awk -F'\t' 'NF != 2 || $2 !~ /^urn:ngsi-ld:[^\t"\\]+$/ { bad = 1; print "bad line " NR > "/dev/stderr" } END { exit bad }' "$ids"
sort -u "$ids" > "$work/ids.tsv"
echo "ids: $(wc -l < "$work/ids.tsv")"

if pg_restore --list "$dump" | grep -qi timescaledb; then
  echo "the dump names timescaledb: the stage server needs it" >&2
fi
[ "$(psq "$stage" -c "SELECT count(*) FROM pg_class WHERE relname = 'entities'")" = 0 ] \
  || { echo "stage database is not empty" >&2; exit 2; }
pg_restore --no-owner --no-privileges --exit-on-error -d "$stage" "$dump"
if [ "$(psq "$stage" -c "SELECT at IS NOT NULL FROM history_cutover")" = t ]; then
  echo "history_cutover.at is set in the dump: attr_rows needs iri_ids renumbering; not done here" >&2
  exit 2
fi

# The columns a row is written with: every stored column (generated ones are rebuilt).
cols() { psq "$stage" -c "SELECT string_agg(quote_ident(column_name), ',' ORDER BY ordinal_position)
  FROM information_schema.columns WHERE table_schema = 'public' AND table_name = '$1' AND is_generated = 'NEVER'"; }

{
  echo "BEGIN;"
  echo "CREATE TEMP TABLE ids (tenant_id text, id text) ON COMMIT DROP;"
  echo "\\copy ids FROM '$work/ids.tsv'"
  for i in "${!tables[@]}"; do
    t=${tables[$i]}; c=$(cols "$t")
    echo "\\copy (SELECT $c FROM $t x WHERE EXISTS (SELECT 1 FROM ids WHERE ids.tenant_id = x.tenant_id AND ids.id = x.${idcol[$i]})) TO '$work/$t.copy'"
  done
  echo "ROLLBACK;"
} | psq "$stage"

{
  echo "BEGIN;"
  echo "DO \$\$ BEGIN IF NOT (SELECT rolsuper FROM pg_roles WHERE rolname = current_user) THEN RAISE EXCEPTION 'target role is no superuser: row level security would hide the history tables'; END IF; END \$\$;"
  for t in "${tables[@]}"; do
    c=$(cols "$t")
    echo "CREATE TEMP TABLE r_$t ON COMMIT DROP AS SELECT $c FROM $t WITH NO DATA;"
    echo "\\copy r_$t FROM '$work/$t.copy'"
    echo "WITH n AS (INSERT INTO $t ($c) SELECT $c FROM r_$t ON CONFLICT DO NOTHING RETURNING 1)"
    echo "SELECT '$t', (SELECT count(*) FROM r_$t) AS in_dump, (SELECT count(*) FROM n) AS inserted;"
  done
  [ "$apply" = --apply ] && echo "COMMIT;" || echo "ROLLBACK;"
} | psql -X -q -v ON_ERROR_STOP=1 -At -F "$(printf '\t')" "$target" \
  | awk -F'\t' 'BEGIN { print "table\tin dump\tinserted\talready there" } { print $1 "\t" $2 "\t" $3 "\t" $2 - $3 }'
echo "${apply:-dry run: rolled back}"
