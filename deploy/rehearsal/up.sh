#!/usr/bin/env bash
# Starts one rehearsal environment: PostgreSQL 17 in docker and `jc --role all` on it
# (REHEARSAL_ROLE: another role list, e.g. one module left out for a red smoke run).
#
#   deploy/rehearsal/up.sh <env> <public port> <postgres port>
#
# Every secret is a 0600 file under $REHEARSAL_HOME/<env>/secrets (generated once, never printed).
# kaisatsu, Antares, the Portal, shako, daiya, dataroom and ekiben keep their state in PostgreSQL.
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
# A jc embedding a Portal UI older than the clone's UI sources serves the old UI silently: refuse it.
"$here/ui-fresh.sh" "$here/../.." "$jc_bin" || exit 1
export_dir=${REHEARSAL_EXPORT:-$home/export}
bind=${REHEARSAL_BIND:-127.0.0.1}

mkdir -p "$home"/secrets "$home"/state/realms "$home"/import "$home"/repos "$home"/files
chmod 700 "$home/state" "$home/state/realms"
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

# shako, daiya, dataroom and ekiben: each migration runs as <m>_rh_migrate (owner of its database, member of
# <m>_migrate) and grants its tables to <m>_app, which the login role <m>_rh holds (NOBYPASSRLS).
module_db() { # module_db <module>
  local m=$1
  sql >/dev/null <<SQL
SELECT format('CREATE ROLE %I NOLOGIN NOBYPASSRLS', r) FROM unnest(ARRAY['${m}_app', '${m}_migrate']) r
 WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = r) \gexec
SQL
  role "${m}_rh" "$(secret "${m}-pw")" >/dev/null
  role "${m}_rh_migrate" "$(secret "${m}-migrate-pw")" >/dev/null
  sql -c "GRANT ${m}_app TO ${m}_rh" -c "GRANT ${m}_migrate TO ${m}_rh_migrate" >/dev/null
  database "$m" "${m}_rh_migrate" >/dev/null
  # A [dataroom] section must name its organization and catalogue: jc's defaults.
  [ "$m" = dataroom ] && printf '\n[dataroom]\norg_id = "00000000-0000-7000-8000-000000000001"\ncatalogue_id = "00000000-0000-7000-8000-000000000010"\n'
  # jc reads [<m>.database] (bins/jc/src/pg.rs): the URLs name role, host and database and carry
  # no password (jc refuses one); each password stays in its 0600 file.
  printf '\n[%s.database]\nurl = "postgres://%s_rh@127.0.0.1:%s/%s"\npassword_file = "%s"\n' \
    "$m" "$m" "$pg_port" "$m" "$(secret "${m}-pw")"
  printf 'migrate_url = "postgres://%s_rh_migrate@127.0.0.1:%s/%s"\nmigrate_password_file = "%s"\n' \
    "$m" "$pg_port" "$m" "$(secret "${m}-migrate-pw")"
}
db_config=$(for m in shako daiya dataroom ekiben; do module_db "$m"; done)
# Antares migrates as its own request role and takes no migrate_url; the section switches it to
# its Postgres store (ANTARES_STORE).
db_config+=$(printf '\n[antares.database]\nurl = "postgres://antares@127.0.0.1:%s/antares"\npassword_file = "%s"\n' \
  "$pg_port" "$(secret antares-pw)")

kaisatsu_bin=$(dirname "$jc_bin")/kaisatsu
"$kaisatsu_bin" migrate --database-url "postgres://kaisatsu_migrate@127.0.0.1:$pg_port/kaisatsu" \
  --db-password-file "$(secret kaisatsu-migrate-pw)"
printf 'postgres://kaisatsu_app@127.0.0.1:%s/kaisatsu\n' "$pg_port" > "$home/secrets/kaisatsu-db-url"

# The realm config of the export (no users), applied at start next to jc's own Portal realm. It
# sits in the 0700 state dir: on dev it also holds people (REHEARSAL_PEOPLE, default
# <rehearsal home>/people/<env>, the deployer's users-<realm>.jsonl; never on prod), without any
# credential. Only the personas of REHEARSAL_DEMO_USERS (demo-users.yaml) keep e-mail and names.
people=${REHEARSAL_PEOPLE:-$(dirname "$home")/people/$env_name}
demo_users=${REHEARSAL_DEMO_USERS:-$(dirname "$home")/demo-users.yaml}
people_args=()
[ -d "$people" ] && people_args+=(--people "$people")
[ -f "$demo_users" ] && people_args+=(--demo-users "$demo_users")
if [ -d "$export_dir/keycloak" ]; then
  python3 -I "$here/rehearsal.py" realms --export "$export_dir" --out "$home/state/realms" \
    --kaisatsu-bin "$(dirname "$jc_bin")/kaisatsu" "${people_args[@]}"
  # Each secret the realm config names gets a fresh value of the rehearsal (never the old one).
  while read -r name; do
    [ -n "$name" ] && secret "realm-$name" > /dev/null
  done < "$home/state/realm-vars.txt"
  # jc's kaisatsu reads no environment for a realm file: each secret it names is a
  # $(file:...) under the rehearsal's secrets dir (--kaisatsu-import-secrets-dir below).
  python3 -I - "$home/state/realms" "$home/secrets" <<'PY'
import pathlib, re, sys
realms, secrets = pathlib.Path(sys.argv[1]), sys.argv[2]
for f in realms.glob("*.json"):
    text = re.sub(r"\$\((?:env:)?([A-Z][A-Z0-9_]*)\)", lambda m: f"$(file:{secrets}/realm-{m[1]})", f.read_text())
    f.write_text(text)
PY
fi

url() { printf 'postgres://%s:%s@127.0.0.1:%s/%s' "$1" "$(cat "$(secret "$1-pw")")" "$pg_port" "$2"; }
JC_PORTAL_DATABASE_URL=$(url portal portal); export JC_PORTAL_DATABASE_URL
# The broker serves on jc's default policy (joinedcontext: an anonymous write answers 401). Only
# import.sh asks for allow-all, for the import's subject-less writes, and only on a loopback bind;
# it restarts on the default policy afterwards and checks it.
if [ "${REHEARSAL_IMPORT_POLICY:-}" = allow-all ]; then
  case $bind in
    127.*|::1) export ANTARES_POLICY=allow-all ;;
    *) echo "refusing allow-all on $bind: the import runs on a loopback-only jc" >&2; exit 1 ;;
  esac
else
  unset ANTARES_POLICY
fi
export ANTARES_TEMPORAL_RETENTION_DAYS=1 ANTARES_SWEEP_SECS=${REHEARSAL_SWEEP_SECS:-3600}
export ANTARES_HISTORY_BACKFILL=on
# jc is one process: the local notification bus cannot double-fire.
export ANTARES_ALLOW_SHARED_LOCAL=1
export JC_PORTAL_DECISIONS_NEED_SECOND_FACTOR=false JC_PORTAL_SYNC_INTERVAL=10
# The catalogue over plain http, for this one-machine rehearsal only (jc and the Portal warn).
export JC_PORTAL_CKAN_ALLOW_HTTP=true
# Every imported CkanInstance names this catalogue (rehearsal.py fence_writes); the Portal calls it
# at the rehearsal's own dataroom.
export JC_PORTAL_CKAN_URL=https://catalogue.rehearsal.invalid JC_PORTAL_CKAN_API_URL=http://127.0.0.1:$port/opendata
# The CkanInstances name apiTokenRef.envVar CKAN_API_TOKEN: here it is jc's own dataroom publisher
# token (Read and Propose on the rehearsal's default catalogue), made before jc's first start,
# which keeps an existing file.
if [ ! -s "$home/state/catalogue-token" ]; then
  (umask 077; head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$home/state/catalogue-token")
fi
CKAN_API_TOKEN=$(cat "$home/state/catalogue-token"); export CKAN_API_TOKEN
# The Portal's internal listener (/metrics, run callbacks) on loopback, one port per environment.
export JC_INTERNAL_BIND=127.0.0.1:$((pg_port + 100))
export RUST_LOG=${RUST_LOG:-info,sqlx=warn}
# The deployment's configuration repository (defaults/environment/global.yaml configRepo).
# jc since 3a64bee5 reads JC_SHAKO_*, earlier builds JC_GITEA_*: both, so either binary starts on it.
export JC_SHAKO_OWNER=${REHEARSAL_CONFIG_OWNER:-joinedcontext} JC_SHAKO_REPO=${REHEARSAL_CONFIG_REPO:-configuration}
export JC_GITEA_OWNER=$JC_SHAKO_OWNER JC_GITEA_REPO=$JC_SHAKO_REPO

# A jc of an earlier up.sh of this environment stops first (its memory stores go with it).
if [ -s "$home/jc.pid" ]; then
  "$here/stop-jc.sh" "$(cat "$home/jc.pid")" || exit 1
fi
# The Grafana files' datasources as daiya provisioning on jc's own address (REHEARSAL_DASHBOARDS,
# default $home/dashboards: a rehearsal.py dashboards dir); daiya lets only file-provisioned
# datasources reach it.
REHEARSAL_DASHBOARDS=${REHEARSAL_DASHBOARDS:-$home/dashboards}
if compgen -G "$REHEARSAL_DASHBOARDS/*.datasource.*.json" >/dev/null; then
  python3 -I "$here/rehearsal.py" provisioning --dashboards "$REHEARSAL_DASHBOARDS" --port "$port" \
    --out "$home/daiya-provisioning"
  db_config+=$(printf '\n[daiya]\nprovisioning_dir = "%s"\n' "$home/daiya-provisioning")
fi
printf '[mounts.portal]\npath_prefix = ""\n%s\n' "$db_config" > "$home/jc.toml"
# The service tokens jc would hold in memory only (shako, ekiben, daiya's Portal password) come
# from files here instead, so the import and the smoke suites can call those modules too.
nohup "$jc_bin" --role "${REHEARSAL_ROLE:-all}" --scheme http --log-format text --config "$home/jc.toml" \
  --listen "$bind:$port" --base-host "localhost:$port" --state-dir "$home/state" \
  --kaisatsu-import-dir "$home/state/realms" --kaisatsu-import-secrets-dir "$home/secrets" \
  --shako-token-file "$(secret shako-token)" --ekiben-token-file "$(secret ekiben-token)" \
  --daiya-portal-password-file "$(secret dashboards-password)" \
  --kaisatsu-database-url-file "$home/secrets/kaisatsu-db-url" \
  --kaisatsu-db-password-file "$(secret kaisatsu-app-pw)" \
  --shako-repo-root "$home/repos" --ekiben-file-root "$home/files" \
  --admin-listen "127.0.0.1:$((pg_port+200))" --admin-token-file "$(secret metrics-token)" \
  > "$home/jc.log" 2>&1 &
echo $! > "$home/jc.pid"
"$here/wait-jc.sh" "$(cat "$home/jc.pid")" "$home/jc.log" || exit 1
echo "$env_name: jc on http://localhost:$port (log $home/jc.log)"


# The 24 h rule, now and hourly, beside jc (rehearsal database only).
if [ -s "$home/retain.pid" ]; then kill "$(cat "$home/retain.pid")" 2>/dev/null || true; fi
nohup python3 -I "$here/rehearsal.py" retain --port "$pg_port" --password-file "$(secret antares-pw)" \
  --every "${REHEARSAL_RETAIN_SECS:-3600}" > "$home/retain.log" 2>&1 &
echo $! > "$home/retain.pid"
