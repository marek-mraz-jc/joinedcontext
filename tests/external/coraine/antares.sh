# shellcheck shell=bash
# SPDX-License-Identifier: Apache-2.0
#
# antares.sh - Coraine's functional suite against Antares (T-3549).
#
# corTest sources this file as COR_TEST_FUNCTIONS. It sources Coraine's own
# test/funcTests/corTestFunctions.sh (Apache-2.0, SEAMWARE/coraine, see NOTICE)
# and replaces the four functions that start, stop and empty a broker, so the
# cases run unchanged against the Antares binary on one of its stores.
#
# Parameters (environment):
#   CORAINE_DIR          the Coraine checkout the cases come from (required)
#   ANTARES_BIN          the antares binary under test (required)
#   ANTARES_TEST_STORE   memory | file | postgres | timescale (default memory)
#   ANTARES_TEST_DIR     per-role data and state (default /tmp/antares-cortest)
#   ANTARES_TEST_PG      postgres base URL without a database, for postgres and
#                        timescale (e.g. postgresql://postgres:pw@localhost:5432);
#                        each role gets its own database antares_<role>
#
# A start flag with no Antares counterpart is never silently dropped: the
# start prints "CORAINE-ONLY <flag>" on stderr and fails, and compare.py files
# the case as not applicable. Flags whose behaviour Antares has by default are
# accepted and ignored (ANTARES_IGNORED_FLAGS below).

: "${CORAINE_DIR:?CORAINE_DIR names the Coraine checkout}"
: "${ANTARES_BIN:?ANTARES_BIN names the antares binary}"
ANTARES_TEST_STORE="${ANTARES_TEST_STORE:-memory}"
ANTARES_TEST_DIR="${ANTARES_TEST_DIR:-/tmp/antares-cortest}"

# shellcheck source=/dev/null
source "$CORAINE_DIR/test/funcTests/corTestFunctions.sh"

# corCurl adds its own Content-Type and Accept, then the case's -H for either,
# so a request carries two. Content-Type is a singleton (RFC 9110 5.3) and
# Antares refuses two media types; two Accept lines combine into one list in
# which corCurl's application/json, q=1, outranks what the case asked for.
# Coraine honoured the case's, the last one, so curl, for every caller, sends
# only the last Content-Type and the last Accept it is given.
curl() {
  local -a args=()
  local ct="" accept=""
  while [ "$#" != 0 ]; do
    if [ "$1" = -H ] || [ "$1" = --header ]; then
      case "${2,,}" in
        content-type:*) ct="$2"; shift 2; continue ;;
        accept:*) accept="$2"; shift 2; continue ;;
      esac
    fi
    args+=("$1")
    shift
  done
  [ -z "$accept" ] || args+=(-H "$accept")
  [ -z "$ct" ] || args+=(-H "$ct")
  command curl "${args[@]}"
}

# Coraine flag -> what Antares does without it:
#   -dist, -distOps, --distributed   forwarding to registered sources is always on
#   --troe <x>, --troeSync, --troe{Name,User,Host,Port} <x>
#                                    history follows the store (ANTARES_TEMPORAL)
#                                    and is written in the same transaction
#   --noSplitEntities                Antares never splits an Entity across sources
#                                    on its own
#   -v, --httpLoops <n>, --connectionPoolSize <n>
#                                    logging and sizing, no answer depends on them
ANTARES_IGNORED_FLAGS=" -dist -distOps --distributed --troeSync --noSplitEntities -noSplitEntities -v "
ANTARES_IGNORED_VALUE_FLAGS=" --troe --troeName --troeUser --troeHost --troePort --httpLoops --connectionPoolSize "

antaresRoleDir() { echo "$ANTARES_TEST_DIR/$1"; }
antaresRoleDb()  { echo "antares_${1,,}"; }

coraineStart() {
  local role="CB" alias="" saved=("$@")
  while [ $# -gt 0 ]; do
    case "$1" in
      -role) role="$2"; shift ;;
      --csourceAlias) alias="$2"; shift ;;
      *)
        if [[ "$ANTARES_IGNORED_FLAGS" == *" $1 "* ]]; then :
        elif [[ "$ANTARES_IGNORED_VALUE_FLAGS" == *" $1 "* ]]; then shift
        else
          echo "CORAINE-ONLY $1" >&2
          return 1
        fi
        ;;
    esac
    shift
  done

  corRoleLookup "$role" || return 1
  coraineStop -role "$role"
  corAwaitPortClosed "$COR_ROLE_PORT" 10 || return 1

  local dir; dir=$(antaresRoleDir "$role")
  mkdir -p "$dir"
  : > "$dir.args"
  [ ${#saved[@]} -gt 0 ] && printf '%s\0' "${saved[@]}" > "$dir.args"

  local -a env=(
    ANTARES_HTTP_PORT="$COR_ROLE_PORT"
    ANTARES_PUBLIC_URL="http://localhost:$COR_ROLE_PORT"
    ANTARES_STORE="$ANTARES_TEST_STORE"
    ANTARES_DATA_DIR="$dir"
    ANTARES_EGRESS_ALLOW_PRIVATE=true
    # the spec sets no batch ceiling; Coraine upserts 1101 entities in one batch
    ANTARES_MAX_BATCH_ITEMS=2000
    # each role is a source of its own (5.2.9 contextSourceAlias): one shared
    # default alias made every forward between two roles a loop (508)
    ANTARES_HOST_ALIAS="${alias:-antares-${role,,}}"
  )
  case "$ANTARES_TEST_STORE" in
    postgres|timescale)
      : "${ANTARES_TEST_PG:?ANTARES_TEST_PG is needed for $ANTARES_TEST_STORE}"
      antaresDbEnsure "$role" || return 1
      env+=(ANTARES_DATABASE_URL="$ANTARES_TEST_PG/$(antaresRoleDb "$role")" ANTARES_ALLOW_SHARED_LOCAL=1)
      ;;
  esac

  # the broker refuses an unknown ANTARES_* key; ANTARES_BIN is the harness's, not its (run 38806308784)
  env -u ANTARES_BIN "${env[@]}" "$ANTARES_BIN" > "/tmp/coraine.${role}.log" 2>&1 &
  echo $! > "$COR_ROLE_PID_FILE"
  corAwaitPort "$COR_ROLE_PORT" 30
}

coraineStop() {
  local role="CB" all=0
  while [ $# -gt 0 ]; do
    case "$1" in
      -role) role="$2"; shift ;;
      -all) all=1 ;;
    esac
    shift
  done

  if [ $all == 1 ]; then
    local r
    for r in $(echo "$COR_ROLES" | awk '{print $1}'); do coraineStop -role "$r"; done
    return 0
  fi

  corRoleLookup "$role" || return 1
  local pid=""
  [ -f "$COR_ROLE_PID_FILE" ] && pid=$(< "$COR_ROLE_PID_FILE")
  if [ -n "$pid" ] && corPidAlive "$pid"; then
    kill -TERM "$pid" 2>/dev/null
    local n=0
    while corPidAlive "$pid" && [ $n -lt 100 ]; do sleep 0.1; n=$((n + 1)); done
    corPidAlive "$pid" && kill -KILL "$pid" 2>/dev/null
  fi
  rm -f "$COR_ROLE_PID_FILE"
  return 0
}

# The role's database exists, with the extensions the store needs
antaresDbEnsure() {
  local db; db=$(antaresRoleDb "$1")
  local exists
  exists=$(psql "$ANTARES_TEST_PG/postgres" -tAq -c "SELECT 1 FROM pg_database WHERE datname = '$db'" 2>&1) || { echo "$exists" >&2; return 1; }
  if [ "$exists" != 1 ]; then
    psql "$ANTARES_TEST_PG/postgres" -q -c "CREATE DATABASE $db" > /dev/null || return 1
  fi
  if [ "$ANTARES_TEST_STORE" == timescale ]; then
    psql "$ANTARES_TEST_PG/$db" -q -c "CREATE EXTENSION IF NOT EXISTS timescaledb" > /dev/null 2>&1 || return 1
  fi
}

#
# corDbDrop [-role <role>] [-tenant <t>] [-db <name>]: the role's store emptied.
# Antares keeps every tenant in one store, so -tenant empties the whole role, and
# -db (Coraine's own database, its @contexts) the CB role. A running broker is
# stopped first and started again on the empty store with the flags it had.
#
corDbDrop() {
  local role="CB"
  while [ $# -gt 0 ]; do
    case "$1" in
      -role) role="$2"; shift ;;
      -tenant|-db) shift ;;
    esac
    shift
  done

  corRoleLookup "$role" || return 1
  local dir; dir=$(antaresRoleDir "$role")
  local running=0 pid=""
  [ -f "$COR_ROLE_PID_FILE" ] && pid=$(< "$COR_ROLE_PID_FILE")
  if [ -n "$pid" ] && corPidAlive "$pid"; then
    running=1
    coraineStop -role "$role"
  fi

  case "$ANTARES_TEST_STORE" in
    memory) ;;
    file) rm -rf "${ANTARES_TEST_DIR:?}/${role:?}" ;;
    postgres|timescale)
      # IF EXISTS on a missing database prints a NOTICE, and corTest fails an INIT on any stderr
      PGOPTIONS="-c client_min_messages=warning" psql "$ANTARES_TEST_PG/postgres" -q -c "DROP DATABASE IF EXISTS $(antaresRoleDb "$role") WITH (FORCE)" > /dev/null || return 1
      ;;
    *) echo "corDbDrop: unknown ANTARES_TEST_STORE $ANTARES_TEST_STORE" >&2; return 1 ;;
  esac

  if [ $running == 1 ]; then
    local -a args
    mapfile -d '' -t args < "$dir.args"
    coraineStart "${args[@]}"
  fi
}

# History lives in the role's store: corDbDrop empties it with the rest
corTroeInit() { :; }
corTroeDrop() { :; }
