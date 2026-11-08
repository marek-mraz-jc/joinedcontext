#!/usr/bin/env bash
# Starts one rehearsal environment: PostgreSQL 17 in docker and `jc --role all` on it.
#
#   deploy/rehearsal/up.sh <env> <public port> <postgres port>
#
# Every secret is a 0600 file under $REHEARSAL_HOME/<env>/secrets (generated once, never printed).
# kaisatsu, Antares and the Portal keep their state in PostgreSQL; shako, daiya, dataroom and
# ekiben have no Postgres path in jc yet and start empty, so import.sh runs after every start.
# The 24 h rule (owner 2026-11-08): `rehearsal.py retain` deletes Antares temporal instances older
# than 24 h at start and hourly; Antares' own retention (whole days, whole weekly partitions) runs
# beside it with ANTARES_TEMPORAL_RETENTION_DAYS=1. Current entity state is never deleted.
set -euo pipefail

env_name=${1:?usage: up.sh <env> <public port> <postgres port>}
port=${2:?public port}
pg_port=${3:?postgres port}
here=$(cd "$(dirname "$0")" && pwd)
home=${REHEARSAL_HOME:-$HOME/rehearsal}/$env_name
jc_bin=${JC_BIN:-${CARGO_TARGET_DIR:-$here/../../target}/debug/jc}
export_dir=${REHEARSAL_EXPORT:-$home/export}
bind=${REHEARSAL_BIND:-127.0.0.1}

mkdir -p "$home"/secrets "$home"/state "$home"/import/realms "$home"/repos "$home"/files
chmod 700 "$home" "$home"/secrets "$home"/state

secret() { # secret <name>: a random secret file, made once
  local f="$home/secrets/$1"
  if [ ! -s "$f" ]; then
    (umask 077; head -c 24 /dev/urandom | base64 | tr -d '/+=\n' > "$f")
  fi
  printf '%s' "$f"
}

# PostgreSQL 17 with PostGIS (Antares' migrations create the extension's types).
pg_image=${REHEARSAL_PG_IMAGE:-postgis/postgis:17-3.5@sha256:01a6a70e41e6c4467c8f55f6063555ed72db2d6662cd0d571040d42eadaeb6f6}
container=rh-pg-$env_name
if ! docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null | grep -q true; then
  docker rm -f "$container" >/dev/null 2>&1 || true
  # --network host: the per-task dind bridge gives a container no address the host reaches.
  docker run -d --name "$container" --network host --shm-size=512m \
    -e PGPORT="$pg_port" -e POSTGRES_PASSWORD_FILE=/run/rh/superuser \
    -v "$(secret superuser)":/run/rh/superuser:ro \
    -v "$home/pgdata":/var/lib/postgresql/data \
    "$pg_image" >/dev/null
fi
for _ in $(seq 60); do pg_isready -q -h 127.0.0.1 -p "$pg_port" && break; sleep 1; done
pg_isready -q -h 127.0.0.1 -p "$pg_port"

sql() { PGPASSWORD=$(cat "$(secret superuser)") psql -q -v ON_ERROR_STOP=1 -h 127.0.0.1 -p "$pg_port" -U postgres "$@"; }
role() { # role <name> <password file>: a login role, its password read by psql, never in argv
  RH_PW=$(cat "$2") sql <<SQL
\getenv pw RH_PW
SELECT format('CREATE ROLE %I LOGIN', '$1') WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '$1') \gexec
SELECT format('ALTER ROLE %I LOGIN NOSUPERUSER NOBYPASSRLS PASSWORD %L', '$1', :'pw') \gexec
SQL
}
database() { # database <name> <owner>
  sql -tAc "SELECT 1 FROM pg_database WHERE datname = '$1'" | grep -q 1 || sql -c "CREATE DATABASE $1 OWNER $2"
}
role kaisatsu_migrate "$(secret kaisatsu-migrate-pw)"
role kaisatsu_app "$(secret kaisatsu-app-pw)"
role portal "$(secret portal-pw)"
role antares "$(secret antares-pw)"
database kaisatsu kaisatsu_migrate
database portal portal
database antares antares
sql -d antares -c "CREATE EXTENSION IF NOT EXISTS postgis"

kaisatsu_bin=$(dirname "$jc_bin")/kaisatsu
"$kaisatsu_bin" migrate --database-url "postgres://kaisatsu_migrate@127.0.0.1:$pg_port/kaisatsu" \
  --db-password-file "$(secret kaisatsu-migrate-pw)"
printf 'postgres://kaisatsu_app@127.0.0.1:%s/kaisatsu\n' "$pg_port" > "$home/secrets/kaisatsu-db-url"

# The realm config of the export (no users), applied at start next to jc's own Portal realm.
if [ -d "$export_dir/keycloak" ]; then
  python3 -I "$here/rehearsal.py" realms --export "$export_dir" --out "$home/import/realms"
  # Each secret the realm config names gets a fresh value of the rehearsal (never the old one).
  while read -r name; do
    [ -n "$name" ] && export "$name=$(cat "$(secret "realm-$name")")"
  done < "$home/import/realm-vars.txt"
fi

url() { printf 'postgres://%s:%s@127.0.0.1:%s/%s' "$1" "$(cat "$(secret "$1-pw")")" "$pg_port" "$2"; }
export JC_PORTAL_DATABASE_URL=$(url portal portal)
export ANTARES_STORE=postgres ANTARES_DATABASE_URL=$(url antares antares)
export ANTARES_TEMPORAL_RETENTION_DAYS=1 ANTARES_SWEEP_SECS=${REHEARSAL_SWEEP_SECS:-3600}
export ANTARES_HISTORY_BACKFILL=on
# jc is one process: the local notification bus cannot double-fire.
export ANTARES_ALLOW_SHARED_LOCAL=1
export JC_PORTAL_DECISIONS_NEED_SECOND_FACTOR=false JC_PORTAL_SYNC_INTERVAL=10
# The Portal's internal listener (/metrics, run callbacks) on loopback, one port per environment.
export JC_INTERNAL_BIND=127.0.0.1:$((pg_port + 100))
export RUST_LOG=${RUST_LOG:-info,sqlx=warn}
# The deployment's configuration repository (defaults/environment/global.yaml configRepo).
export JC_GITEA_OWNER=${REHEARSAL_CONFIG_OWNER:-joinedcontext} JC_GITEA_REPO=${REHEARSAL_CONFIG_REPO:-configuration}

# A jc of an earlier up.sh of this environment stops first (its memory stores go with it).
if [ -s "$home/jc.pid" ] && kill "$(cat "$home/jc.pid")" 2>/dev/null; then
  for _ in $(seq 20); do kill -0 "$(cat "$home/jc.pid")" 2>/dev/null || break; sleep 0.5; done
fi
printf '[mounts.portal]\npath_prefix = ""\n' > "$home/jc.toml"
nohup "$jc_bin" --role all --scheme http --log-format text --config "$home/jc.toml" \
  --listen "$bind:$port" --base-host "localhost:$port" --state-dir "$home/state" \
  --kaisatsu-import-dir "$home/import/realms" \
  --kaisatsu-database-url-file "$home/secrets/kaisatsu-db-url" \
  --kaisatsu-db-password-file "$(secret kaisatsu-app-pw)" \
  --shako-repo-root "$home/repos" --ekiben-file-root "$home/files" \
  > "$home/jc.log" 2>&1 &
echo $! > "$home/jc.pid"
for _ in $(seq 180); do
  grep -q "jc listening" "$home/jc.log" && break
  kill -0 "$(cat "$home/jc.pid")" 2>/dev/null || { tail -40 "$home/jc.log" >&2; exit 1; }
  sleep 1
done
grep -q "jc listening" "$home/jc.log" || { tail -40 "$home/jc.log" >&2; exit 1; }
echo "$env_name: jc on http://localhost:$port (log $home/jc.log)"


# The 24 h rule, now and hourly, beside jc (rehearsal database only).
[ -s "$home/retain.pid" ] && kill "$(cat "$home/retain.pid")" 2>/dev/null || true
nohup python3 -I "$here/rehearsal.py" retain --port "$pg_port" --password-file "$(secret antares-pw)" \
  --every "${REHEARSAL_RETAIN_SECS:-3600}" > "$home/retain.log" 2>&1 &
echo $! > "$home/retain.pid"
