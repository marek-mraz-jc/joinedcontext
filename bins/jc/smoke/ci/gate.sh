#!/usr/bin/env bash
# gate.sh <out dir> [public port] [postgres port]: jc on synthetic data, then `run all` against it.
#
# Unpacks the committed synthetic bundle (synth-export.tgz: `rehearsal.py synth` of the deployment's
# dev config, sliced to configuration, zilina, helsinki and one helsinki App), moves its timestamps
# to now, imports it through deploy/rehearsal/import.sh (PostgreSQL in docker, `jc --role all` on a
# loopback bind) and runs every smoke suite with the rehearsal's state, log and admin listener.
# JC_BIN names the jc binary (kaisatsu and daiya beside it); SMOKE_E2E=1 adds `cargo test -p jc
# --tests`; SMOKE_ARGS adds flags to `run all`. Beside it, two more jcs on the ten-Organization
# bundle compose.py makes of synth-export.tgz (hel, zilina) and multiorg-export.tgz (the other eight):
# one with each Organization on its own *.localhost hosts for `multiorg` (host mode), one without
# for `multiorg@path`; both import while the gate's own jc does. The CI job
# (.github/workflows/smoke.yml) and a local check run the same script. Answers `run all`'s verdict: 0 green, 1 red.
set -euo pipefail
out=$(realpath -m "${1:?usage: gate.sh <out dir> [port] [pg port]}")
port=${2:-42101}; pg_port=${3:-55441}
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)
export REHEARSAL_HOME=${REHEARSAL_HOME:-$HOME/rehearsal-gate}
home=$REHEARSAL_HOME/gate
export_dir=$REHEARSAL_HOME/gate-export

mo_export=$REHEARSAL_HOME/multiorg-export
mo_hosts=$REHEARSAL_HOME/multiorg-hosts.toml
mo_port=$((port + 10)); mo_pg=$((pg_port + 10))   # multiorg, host mode
mp_port=$((port + 20)); mp_pg=$((pg_port + 20))   # multiorg@path

# fresh <env>: a fresh stack every run: the import only creates (rehearsal.py import_shako refuses a
# main that holds files the export would change or delete), and the last run's suites edited main.
# The gate envs hold synthetic data only. pgdata belongs to the container's postgres user: emptied
# from inside a container, as the restore suite does.
fresh() {
  local h=$REHEARSAL_HOME/$1
  if [ -s "$h/jc.pid" ]; then "$repo/deploy/rehearsal/stop-jc.sh" "$(cat "$h/jc.pid")"; fi
  docker rm -f "rh-pg-$1" >/dev/null 2>&1 || true
  if [ -d "$h/pgdata" ]; then
    # shellcheck disable=SC2016 # the sed pattern reads up.sh's line, it expands nothing
    pg_image=${REHEARSAL_PG_IMAGE:-$(sed -n 's/^pg_image=\${REHEARSAL_PG_IMAGE:-\(.*\)}$/\1/p' "$repo/deploy/rehearsal/up.sh")}
    docker run --rm -v "$h":/h --entrypoint rm "$pg_image" -rf /h/pgdata
  fi
  rm -rf "${h:?}"
}

# converge <env> <log line>: the suites read what the reconciler made of the import (views,
# schemas, policies): wait for the second leader pass logged after that line, since the first may
# have started before the import ended. Without a Portal (SMOKE_STOP=portal) no pass comes, so the
# wait is bounded and only logs.
converge() {
  local log=$REHEARSAL_HOME/$1/jc.log n=0
  for _ in $(seq "${SMOKE_CONVERGE_SECS:-300}"); do
    n=$(tail -n +"$(($2 + 1))" "$log" | grep -c 'sync run finished.*leader=true') || true
    [ "$n" -ge 2 ] && break
    sleep 1
  done
  echo "gate: $1: $n reconciler passes since the import"
}

# view <env>: the suites read their credentials from one state dir: jc's own, plus the service
# tokens up.sh gave jc from the secrets dir (jc keeps no file of them). Links only, in a 0700 dir,
# and directories too: the kaisatsu suite reads the realm kaisatsu serves from <state>/realms/, and
# without it judges the export's realm (gate 2026-11-10T203500Z-gc: main's realm lines unseen).
view() {
  local h=$REHEARSAL_HOME/$1 f
  rm -rf "$h/smoke-state" && mkdir -m 700 "$h/smoke-state"
  for f in "$h"/state/* "$h"/secrets/{shako-token,ekiben-token,dashboards-password}; do
    if [ -e "$f" ]; then ln -s "$f" "$h/smoke-state/"; fi
  done
}

# multiorg <env> <port> <pg port>: import the composed bundle into a fresh <env> and wait for its
# reconciler, logging to <out>/<env>.log. A failure stops nothing: the multiorg rows then fail.
# Pipelines held (no ekiben): multiorg reads their manifests only, and two more run queues would
# take the runner's CPU from the gate's own suites.
multiorg() {
  fresh "$1"
  if REHEARSAL_HOLD_PIPELINES=1 "$repo/deploy/rehearsal/import.sh" "$1" "$2" "$3" "$mo_export"; then
    converge "$1" 0
  else
    echo "gate: $1: import FAILED; its multiorg rows fail"
  fi
}

rm -rf "$export_dir" "$mo_export"; mkdir -p "$export_dir" "$out"
fresh gate
# Every 30 s into the step log (a runner that is lost uploads no artifact): free memory, jc's RSS,
# the three largest processes and the disk under REHEARSAL_HOME, so a cut-off run tells an
# out-of-memory or full runner from a signal (smoke runs 42873989236, 42907019912: no verdict).
( while sleep 30; do
    echo "gate probe $(date -u +%T): $(free -m | awk 'NR==2{print "mem used "$3" avail "$7" MB"}')," \
      "jc rss $(ps -o rss= -p "$(cat "$home/jc.pid" 2>/dev/null)" 2>/dev/null || echo -) kB," \
      "top $(ps -eo rss=,comm= --sort=-rss | head -3 | awk '{printf "%s:%dMB ", $2, $1/1024}')," \
      "disk $(df -m "$REHEARSAL_HOME" | awk 'NR==2{print "free "$4" MB"}')"
  done ) &
probe=$!
trap 'kill "$probe" 2>/dev/null' EXIT
tar -xzf "$here/synth-export.tgz" -C "$export_dir"
python3 -I "$here/redate.py" "$export_dir"
# redate rewrites the history lines, so the bundle is stamped again: the import refuses a file that
# differs from its manifest's digest (P-26).
python3 -I "$repo/deploy/rehearsal/rehearsal.py" digest --export "$export_dir" >/dev/null
# The ten-Organization bundle (GOAL-multiorg), composed, moved to now and stamped: hel's helsinki
# and zilina's zilina from the gate's own synth (its manifests pass jcctl validate); barcelona, the
# realm and the configuration repository from multiorg-export.tgz (`rehearsal.py synth` of the
# deployment's dev config, sliced), and from it bbsk, banskabystrica, praha, slovensko and
# bratislava, each project an Organization of its own; ctl-a and ctl-b with no project, the controls
# that make an empty Organization's leak visible. No Organization holds the installation role: the
# operators are the platform-admins group. At most 40 Endpoints a space (bratislava's 453 held the
# first reconciler pass 413 s). Each Organization on its own Portal, open data, API and MCP host.
mo_src=$(mktemp -d)
mkdir "$mo_src/gate" "$mo_src/bcn"
tar -xzf "$here/synth-export.tgz" -C "$mo_src/gate"
tar -xzf "$here/multiorg-export.tgz" -C "$mo_src/bcn"
mo_orgs=(hel="$mo_src/gate" zilina="$mo_src/gate" barcelona="$mo_src/bcn")
for o in bbsk banskabystrica praha slovensko bratislava; do mo_orgs+=("$o=$mo_src/bcn:$o"); done
mo_orgs+=(ctl-a= ctl-b=)
python3 -I "$repo/bins/jc/smoke/multiorg/compose.py" --base "$mo_src/bcn" "${mo_orgs[@]/#/--org=}" \
  --out "$mo_export" --entities-per-type 0 --endpoints-per-space 40 --no-installation
rm -rf "${mo_src:?}"
python3 -I "$here/redate.py" "$mo_export"
python3 -I "$repo/deploy/rehearsal/rehearsal.py" digest --export "$mo_export" >/dev/null
# Each Organization's catalogue takes the Portal's datasets only with a token of its own
# (portal_token_file, bins/jc/CUTOVER.md 4a): random per run, in a 0700 dir, never printed.
mo_tokens=$REHEARSAL_HOME/multiorg-tokens
rm -rf "$mo_tokens" && mkdir -m 700 "$mo_tokens"
: > "$mo_hosts"
i=0
for org in "${mo_orgs[@]}"; do
  o=${org%%=*}; i=$((i + 1))
  (umask 077 && python3 -c 'import secrets; print(secrets.token_urlsafe(32))' > "$mo_tokens/$o")
  printf '[[organizations]]\nname = "%s"\ncatalogue_id = "00000000-0000-7000-8000-0000000000%s"\n' "$o" "$(printf %02x "$i")"
  printf 'portal_token_file = "%s"\n' "$mo_tokens/$o"
  printf 'hosts = { portal = "portal.%s.localhost:%s", opendata = "opendata.%s.localhost:%s", api = "api.%s.localhost:%s", mcp = "mcp.%s.localhost:%s" }\n\n' \
    "$o" "$mo_port" "$o" "$mo_port" "$o" "$mo_port" "$o" "$mo_port"
done >> "$mo_hosts"
REHEARSAL_JC_TOML_EXTRA=$mo_hosts multiorg mo-hosts "$mo_port" "$mo_pg" > "$out/mo-hosts.log" 2>&1 &
mo_hosts_pid=$!
multiorg mo-path "$mp_port" "$mp_pg" > "$out/mo-path.log" 2>&1 &
mo_path_pid=$!

"$repo/deploy/rehearsal/import.sh" gate "$port" "$pg_port" "$export_dir"
# The red proof: SMOKE_STOP=<module> restarts jc without that module, and the verdict must be RED.
if [ -n "${SMOKE_STOP:-}" ]; then
  roles=$(printf '%s\n' kaisatsu dataroom daiya ekiben antares gateway portal shako | grep -vx "$SMOKE_STOP" | paste -sd,)
  REHEARSAL_ROLE=$roles "$repo/deploy/rehearsal/up.sh" gate "$port" "$pg_port"
fi
converge gate "$(wc -l < "$home/jc.log")"
wait "$mo_hosts_pid" "$mo_path_pid" || true
cat "$out/mo-hosts.log" "$out/mo-path.log" | grep '^gate: ' || true

# jc names itself 127.0.0.1:<port> (up.sh --base-host): tokens carry that issuer.
for e in gate mo-hosts mo-path; do view "$e"; done
view="$home/smoke-state"
args=(--base-url "http://127.0.0.1:$port" --out "$out" --export "$export_dir" --state "$view"
      --log-file "$home/jc.log" --admin-url "http://127.0.0.1:$((pg_port + 200))"
      --admin-token-file "$home/secrets/metrics-token")
[ "${SMOKE_E2E:-0}" = 1 ] && args+=(--e2e)
# The logs suite also looks for every secret value of the rehearsal in the log: the service
# tokens, the realm's client secrets (up.sh writes them to secrets/realm) and jc's state files,
# which the link view above hides from it (collect_secrets reads plain files only).
args+=(--with "logs=--secrets-dir $home/secrets --secrets-dir $home/secrets/realm --secrets-dir $home/state")
# The subscriptions probe is written by the project's ServiceAccount: its client secret is here.
args+=(--with "subscriptions=--realm-secrets-dir $home/secrets/realm")
# The assistant is a kept component on dev beside jc (tasks/GOAL-jc-dev.md): rows the rehearsal's
# assistant module being off prevents are named skips, never known failures.
args+=(--with "assistant=--kept-component")
# restore backs this environment up, restores it beside it (its own Postgres and ports) and compares.
args+=(--with "restore=--home $home")
# multiorg judges the composed bundle on its own two jcs (each --with wins over the common flags);
# --composed makes a bundle that is missing or not composed a failure, never the setup skip.
args+=(--with "multiorg=--base-url http://127.0.0.1:$mo_port --export $mo_export --composed
  --state $REHEARSAL_HOME/mo-hosts/smoke-state --hosts $mo_hosts")
args+=(--with "multiorg@path=--base-url http://127.0.0.1:$mp_port --export $mo_export --composed
  --state $REHEARSAL_HOME/mo-path/smoke-state")
# Bounded: each run is polled until it ends, so ten runs of five minutes outlast the job.
args+=(--with "pipelines=--runs 3 --wait 120")
# Failures the gate knows, each with where it is filed; only a new one turns the verdict red.
args+=(--known "$repo/bins/jc/smoke/ci/known.txt")
# Screenshots need Playwright for Python and its Chromium; without them charts-apps skips the shots.
python3 -c 'import playwright' 2>/dev/null || args+=(--with "charts-apps=--no-shots")
# shellcheck disable=SC2206 # SMOKE_ARGS is a flag list by design
args+=(${SMOKE_ARGS:-})
# The two multiorg jcs take most of a core each while they idle (their catalogue waves): both
# stopped once multiorg@path has written its results, and in any case once `run all` ends.
stop_mo() {
  local e
  for e in mo-hosts mo-path; do
    if [ -s "$REHEARSAL_HOME/$e/jc.pid" ]; then "$repo/deploy/rehearsal/stop-jc.sh" "$(cat "$REHEARSAL_HOME/$e/jc.pid")" || true; fi
  done
}
rm -f "$out/multiorg@path/results.json"
( while [ ! -e "$out/multiorg@path/results.json" ]; do sleep 5; done; stop_mo ) &
mo_stop=$!
set +e
python3 "$repo/bins/jc/smoke/run" all "${args[@]}"
verdict=$?
set -e
kill "$mo_stop" 2>/dev/null || true
stop_mo
cp "$home/jc.log" "$out/jc.log" 2>/dev/null || true
cp "$REHEARSAL_HOME/mo-hosts/jc.log" "$out/mo-hosts-jc.log" 2>/dev/null || true
cp "$REHEARSAL_HOME/mo-path/jc.log" "$out/mo-path-jc.log" 2>/dev/null || true
exit $verdict
