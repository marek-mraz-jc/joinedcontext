#!/bin/sh
# The monthly restore drill (OPS-11, OPS-12, T-0046, T-5101). Restores $SOURCE twice into the
# throwaway Cluster $DRILL, one after the other: first the latest base backup and WAL, then a
# point in time, the instant the drill counted every table of the source. It compares row
# counts (per database for the latest, per table for the point in time), checks the realm and
# the Portal's schema in the copy, measures RTO and RPO, records the result and deletes the
# copy. With the nightly local dump on (LOCAL_BACKUP, T-5556) the pod's init container first runs
# this script as `dr-drill.sh dump`: it restores the newest dump into a Postgres of its own and
# compares its tables with the counts the dump job took. Every number in the result is read from
# the clusters; none is typed here.
set -u
# SOURCE, DRILL, RESULT, BACKUPS_ENABLED, LOCAL_BACKUP, TOLERANCE_PERCENT, MIN_MEM_MIB and
# TIMEOUT_MINUTES come from the CronJob; the dump mode reads only the last three.

started=$(date +%s)
deadline=$((started + TIMEOUT_MINUTES * 60))
# Where the dump mode hands its verdict to the drill: one file per field.
HANDOFF=${HANDOFF:-/tmp/local-dump}

# The node is shared: a restore beside a full node evicts the platform, so the drill waits for
# room and gives up visibly rather than squeezing in. Leaves the MiB it saw in $free.
room() {
  while :; do
    free=$(awk '/^MemAvailable:/ { print int($2 / 1024) }' "${MEMINFO:-/proc/meminfo}")
    [ "${free:-0}" -ge "$MIN_MEM_MIB" ] && return 0
    [ "$(date +%s)" -lt "$deadline" ] || return 1
    sleep "${POLL_SECONDS:-60}"
  done
}

# Every table of a database as `database.schema.table|rows`, the line the dump job writes too.
TABLES="SELECT schemaname || '.' || relname, (xpath('/row/c/text()', query_to_xml(format('SELECT count(*) AS c FROM %I.%I', schemaname, relname), false, true, '')))[1]::text FROM pg_stat_user_tables ORDER BY 1"
count_tables() { # pod database file
  rows=$(sql "$1" "$2" "$TABLES" 2>/dev/null) || return 1
  [ -z "$rows" ] || printf '%s\n' "$rows" | sed "s|^|$2.|" >>"$3"
}
# The tables of the second file that differ from the first: missing, or apart by more than the
# tolerance in percent of the first's count, and at least one row. A first-file line may carry a
# second count (`table|before|after`, the dump job's counts around pg_dumpall): a restored count
# anywhere between the two matches.
compare() { # want got
  awk -F'|' -v tol="$TOLERANCE_PERCENT" '
    NR == FNR {
      lo[$1] = $2 + 0; hi[$1] = ($3 == "" ? $2 : $3) + 0
      if (lo[$1] > hi[$1]) { x = lo[$1]; lo[$1] = hi[$1]; hi[$1] = x }
      want[$1] = (lo[$1] == hi[$1] ? lo[$1] : lo[$1] ".." hi[$1]); next
    }
    { got[$1] = $2 }
    END {
      for (t in want) {
        if (!(t in got)) { print t "(missing)"; continue }
        d = 0; if (got[t] < lo[t]) d = lo[t] - got[t]; else if (got[t] > hi[t]) d = got[t] - hi[t]
        lim = lo[t] * tol / 100; if (lim < 1) lim = 1
        if (d > lim) print t "(" got[t] " of " want[t] ")"
      }
    }' "$1" "$2" | sort | tr '\n' ' '
}

# 0. The nightly dump, in the init container (the cluster's image, no API token): the newest
# .sql.gz.age, opened with the drill key (T-5839), restored with psql into a Postgres on an emptyDir that listens on no port, the dump
# volume read-only. Its tables are held to the counts the dump job took just before pg_dumpall,
# written beside the dump.
if [ "${1:-}" = dump ]; then
  backup=${BACKUP_DIR:-/backup}
  data=${DRILL_DATA:-/var/lib/drill}
  export PGHOST="$data/run" PGUSER=drill
  sql() { # pod database query; the pod is this one
    psql -X -At -d "$2" -c "$3"
  }
  work=$(mktemp -d)
  pg=""
  # The restored copy goes however the stage ends; only the verdict leaves this container.
  trap '[ -z "$pg" ] || { kill "$pg"; wait "$pg"; } 2>/dev/null; rm -rf "$data/data" "$data/run" "$work"' EXIT
  local_dump=failed
  local_rto=""
  local_bytes=""
  local_detail=""
  local_stage() {
    if ! room; then
      local_dump=skipped
      local_detail="the node had ${free:-0} MiB available, under ${MIN_MEM_MIB} MiB, for ${TIMEOUT_MINUTES} minutes"
      return 1
    fi
    begin=$(date +%s)
    # The names are dates, so the glob's order ends on the newest.
    file=""
    for f in "$backup"/*.sql.gz.age; do
      [ ! -f "$f" ] || file=$f
    done
    if [ -z "$file" ]; then
      local_detail="no dump on the volume"
      return 1
    fi
    local_bytes=$(wc -c <"$file" | tr -d ' ')
    if [ ! -s "$file.counts" ]; then
      local_detail="no table counts beside ${file##*/}"
      return 1
    fi
    # A superuser of another name than the dump's: pg_dumpall --clean drops and creates every
    # role it holds, and the connected one cannot be dropped. Throwaway, so no fsync.
    if ! { mkdir -p "$PGHOST" && initdb -D "$data/data" -U drill --auth=trust >"$work/initdb.log" 2>&1; }; then
      local_detail="the restore database could not be created"
      return 1
    fi
    postgres -D "$data/data" -c listen_addresses= -c unix_socket_directories="$PGHOST" \
      -c fsync=off -c full_page_writes=off -c synchronous_commit=off >"$work/postgres.log" 2>&1 &
    pg=$!
    tries=0
    until pg_isready -q; do
      tries=$((tries + 1))
      if [ "$tries" -ge "${START_TRIES:-120}" ]; then
        local_detail="the restore database did not start"
        return 1
      fi
      sleep 1
    done
    # The dump decrypted in the pipe with the drill key, the plain text never on a disk; a
    # damaged or truncated chunk is refused and leaves $work/refused.
    unseal() {
      python3 "${AGE_PY:-/opt/age/age.py}" decrypt --identity-file "${AGE_KEY:-/key/key}" <"$file" || : >"$work/refused"
    }
    # The role memberships name their grantor (`GRANT pg_monitor TO x GRANTED BY postgres`), and
    # PostgreSQL 16 refuses a grantor without ADMIN on the role unless it is the bootstrap
    # superuser, which here is `drill`: jc-prod's dump stopped at its first GRANT (T-6725,
    # 2026-11-11). Only the roles section, before the first \connect, loses the clause; the
    # memberships restore as granted by the restoring superuser, every database line is untouched.
    ungrant() {
      sed -E '1,/^\\connect /{/^GRANT .* GRANTED BY [^;]+;$/s/ GRANTED BY [^;]+;$/;/}'
    }
    # gzip -t reads the whole file first: a truncated dump fails here, not halfway through psql.
    if ! { unseal | gzip -t && [ ! -e "$work/refused" ] \
      && unseal | gunzip | ungrant | psql -X -q -v ON_ERROR_STOP=1 -d postgres >/dev/null && [ ! -e "$work/refused" ]; } 2>"$work/restore.err"; then
      # The first error, cut at its first quote: a quoted value may be a row of the dump.
      local_detail="${file##*/} did not restore: $(grep -m 1 -E 'age.py:|ERROR|gzip:' "$work/restore.err" | sed 's/".*//' | tr -d '\134')"
      return 1
    fi
    local_rto=$(($(date +%s) - begin))
    cut -d. -f1 "$file.counts" | sort -u | while read -r db; do
      count_tables here "$db" "$work/restored" || :
    done
    touch "$work/restored"
    off=$(compare "$file.counts" "$work/restored")
    if [ -n "$off" ]; then
      local_detail="${file##*/} short: ${off% }"
      return 1
    fi
    local_dump=local-dump-passed
    local_detail="${file##*/}: $(wc -l <"$file.counts" | tr -d ' ') tables match"
  }
  local_stage || :
  mkdir -p "$HANDOFF"
  printf '%s' "$local_dump" >"$HANDOFF/result"
  printf '%s' "$local_rto" >"$HANDOFF/rto"
  printf '%s' "$local_bytes" >"$HANDOFF/bytes"
  printf '%s' "$local_detail" >"$HANDOFF/detail"
  # The drill records the verdict; a failed restore is its finding, not a failed pod.
  exit 0
fi

verdict=failed
detail=""
rto=""
rpo=""
pitr_target=""
pitr_rto=""
pitr_detail=""
local_dump=off
local_rto=""
local_bytes=""
local_detail=""

# The result goes two ways: one JSON line in this pod's log, and the ConfigMap $RESULT that an
# operator or a check reads without the log.
# One JSON string: backslashes and quotes escaped, control characters dropped. The details carry
# database and table names, which may hold either; a bare one broke the patch or added a "result"
# of its own (F-R2DEP3-015).
j() { printf '"%s"' "$(printf '%s' "$1" | tr -d '\000-\037' | sed 's/\\/\\\\/g; s/"/\\"/g')"; }

record() {
  at=$(date -u +%Y-%m-%dT%H:%M:%SZ)
  fields="\"result\":$(j "$verdict"),\"rtoSeconds\":$(j "$rto"),\"rpoSeconds\":$(j "$rpo"),\"pitrTarget\":$(j "$pitr_target"),\"pitrRtoSeconds\":$(j "$pitr_rto"),\"pitrDetail\":$(j "$pitr_detail"),\"localDump\":$(j "$local_dump"),\"localDumpRtoSeconds\":$(j "$local_rto"),\"localDumpBytes\":$(j "$local_bytes"),\"localDumpDetail\":$(j "$local_detail"),\"at\":$(j "$at"),\"detail\":$(j "$detail")"
  echo "{\"event\":\"dr-drill\",\"cluster\":\"$SOURCE\",$fields}"
  kubectl patch configmap "$RESULT" --type merge -p "{\"data\":{$fields}}" >/dev/null ||
    echo "the result could not be written to configmap/$RESULT" >&2
}

finish() {
  # A dump that does not restore fails the drill whatever the archive's restores said.
  [ "$local_dump" != failed ] || verdict=failed
  record
  [ "$verdict" = passed ] || [ "$verdict" = no-backup ]
  exit $?
}

if [ "$BACKUPS_ENABLED" != true ] && [ "$LOCAL_BACKUP" != true ]; then
  verdict=no-backup
  detail="no off-site backup to drill: postgres.cluster.backups is off"
  finish
fi

# The dump mode's verdict, read as data: only its four words, and no quote or backslash into
# the JSON of the result.
if [ "$LOCAL_BACKUP" = true ]; then
  local_dump=$(cat "$HANDOFF/result" 2>/dev/null) || local_dump=""
  case "$local_dump" in
    local-dump-passed | failed | skipped)
      local_rto=$(tr -cd '0-9' <"$HANDOFF/rto")
      local_bytes=$(tr -cd '0-9' <"$HANDOFF/bytes")
      local_detail=$(tr -d '"\134' <"$HANDOFF/detail" | tr '\n' ' ')
      ;;
    *)
      local_dump=failed
      local_detail="the dump's restore container left no verdict"
      ;;
  esac
  if [ "$local_dump" = skipped ]; then
    verdict=skipped
    detail="the local dump: $local_detail"
    finish
  fi
fi
if [ "$BACKUPS_ENABLED" != true ]; then
  verdict=no-backup
  detail="no off-site backup to drill: postgres.cluster.backups is off"
  finish
fi

if ! room; then
  verdict=skipped
  detail="the node had ${free:-0} MiB available, under ${MIN_MEM_MIB} MiB, for ${TIMEOUT_MINUTES} minutes"
  finish
fi

sql() { # pod database query
  kubectl exec "$1" -c postgres -- psql -X -At -d "$2" -c "$3"
}
# Foreground deletion returns once the copy's volume is gone too, so the next restore under the
# same name starts on an empty disk.
drop() {
  kubectl delete cluster "$DRILL" --ignore-not-found --cascade=foreground --wait=true >/dev/null
}
work=$(mktemp -d)
# A copy a crashed drill left behind goes first, and this one goes however the drill ends.
drop
trap 'kubectl delete cluster "$DRILL" --ignore-not-found --wait=false >/dev/null; rm -rf "$work"' EXIT

primary=$(kubectl get cluster "$SOURCE" -o jsonpath='{.status.currentPrimary}')
copy="$DRILL-1"
databases=$(sql "$primary" postgres "SELECT datname FROM pg_database WHERE NOT datistemplate AND datname <> 'postgres' ORDER BY 1") || {
  detail="the source's databases could not be listed"
  finish
}

# Every table's count on the source, then the instant after the last count: the point the
# second restore goes back to. The WAL switch sends the segment holding that instant to the
# archive now rather than at the next archive timeout.
for db in $databases; do
  count_tables "$primary" "$db" "$work/source" || {
    detail="the source's tables in $db could not be counted"
    finish
  }
done
pitr_target=$(sql "$primary" postgres "SELECT to_char(clock_timestamp() AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"'), pg_switch_wal()" | cut -d'|' -f1)
case "$pitr_target" in
  [0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]T[0-9][0-9]:[0-9][0-9]:[0-9][0-9]Z) ;;
  *)
    detail="the source's clock could not be read for the point in time"
    finish
    ;;
esac
# A commit at the target's own second lands on either side of it.
sleep "${TARGET_SETTLE_SECONDS:-2}"

restore() { # manifest → $took, or the reason in $why
  why=""
  begin=$(date +%s)
  if ! kubectl create -f "$1" >/dev/null; then
    why="the throwaway cluster was refused"
    return 1
  fi
  left=$((deadline - begin))
  [ "$left" -gt 60 ] || left=60
  if ! kubectl wait "cluster/$DRILL" --for=condition=Ready --timeout="${left}s" >/dev/null; then
    why="the restore did not become ready within ${TIMEOUT_MINUTES} minutes"
    return 1
  fi
  took=$(($(date +%s) - begin))
}

# What a working copy holds besides rows: the realm's realms and people (a login reads them),
# and the Portal's schema at the source's migration (the Portal starts only on its own schema).
key_checks() {
  bad=""
  for db in $databases; do
    case "$db" in
      keycloak)
        people=$(sql "$copy" keycloak "SELECT count(*) FROM realm r JOIN user_entity u ON u.realm_id = r.id WHERE r.enabled AND u.enabled" 2>/dev/null) || people=""
        [ "${people:-0}" -gt 0 ] || bad="$bad keycloak(no enabled person in an enabled realm)"
        ;;
      portal)
        q="SELECT coalesce(max(version)::text, '') FROM _sqlx_migrations WHERE success"
        want=$(sql "$primary" portal "$q") || want=""
        got=$(sql "$copy" portal "$q" 2>/dev/null) || got=""
        [ -n "$got" ] && [ "$got" = "$want" ] || bad="$bad portal(schema ${got:-none} of ${want:-unreadable})"
        ;;
    esac
  done
  echo "${bad# }"
}

# 1. The latest: everything the archive holds, as an incident on the source would restore it.
if ! restore /opt/drill/cluster.yaml; then
  detail="$why"
  finish
fi
rto=$took
# The last transaction the archive held: how much the restore lost against now.
rpo=$(sql "$copy" postgres "SELECT coalesce(round(extract(epoch FROM now() - pg_last_xact_replay_timestamp()))::text, '')")

ROWS="SELECT coalesce(sum((xpath('/row/c/text()', query_to_xml(format('SELECT count(*) AS c FROM %I.%I', schemaname, relname), false, true, '')))[1]::text::bigint), 0) FROM pg_stat_user_tables"
short=""
for db in $databases; do
  want=$(sql "$primary" "$db" "$ROWS") || want=""
  got=$(sql "$copy" "$db" "$ROWS" 2>/dev/null) || got=""
  if [ -z "$want" ]; then
    short="$short $db(source unreadable)"
  elif [ -z "$got" ]; then
    short="$short $db(missing)"
  elif [ $((got * 100)) -lt $((want * (100 - TOLERANCE_PERCENT))) ]; then
    short="$short $db($got of $want rows)"
  fi
  detail="$detail $db=$got/$want"
done
detail="${detail# }"
keys=$(key_checks)
[ -z "$keys" ] || short="$short $keys"
if [ -n "$short" ]; then
  detail="short:$short; $detail"
  finish
fi

# 2. The point in time: the same copy again, replayed only to the instant of the counts above.
drop
sed "s|__TARGET_TIME__|$pitr_target|" /opt/drill/cluster-pitr.yaml >"$work/pitr.yaml"
if ! restore "$work/pitr.yaml"; then
  pitr_detail="$why"
  finish
fi
pitr_rto=$took
for db in $databases; do
  count_tables "$copy" "$db" "$work/copy" || :
done
# A table may differ by what was committed between its count and the target.
off=$(compare "$work/source" "$work/copy")
tables=$(wc -l <"$work/source" | tr -d ' ')
keys=$(key_checks)
if [ -n "$off$keys" ]; then
  pitr_detail="short: ${off}${keys}"
  pitr_detail="${pitr_detail% }"
else
  pitr_detail="$tables tables match"
  verdict=passed
fi
finish
