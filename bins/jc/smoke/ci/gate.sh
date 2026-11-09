#!/usr/bin/env bash
# gate.sh <out dir> [public port] [postgres port]: jc on synthetic data, then `run all` against it.
#
# Unpacks the committed synthetic bundle (synth-export.tgz: `rehearsal.py synth` of the deployment's
# dev config, sliced to configuration, zilina, helsinki and one helsinki App), moves its timestamps
# to now, imports it through deploy/rehearsal/import.sh (PostgreSQL in docker, `jc --role all` on a
# loopback bind) and runs every smoke suite with the rehearsal's state, log and admin listener.
# JC_BIN names the jc binary (kaisatsu and daiya beside it); SMOKE_E2E=1 adds `cargo test -p jc
# --tests`; SMOKE_ARGS adds flags to `run all`. The CI job (.github/workflows/smoke.yml) and a
# local check run the same script. Answers `run all`'s verdict: 0 green, 1 red.
set -euo pipefail
out=$(realpath -m "${1:?usage: gate.sh <out dir> [port] [pg port]}")
port=${2:-42101}; pg_port=${3:-55441}
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)
export REHEARSAL_HOME=${REHEARSAL_HOME:-$HOME/rehearsal-gate}
home=$REHEARSAL_HOME/gate
export_dir=$REHEARSAL_HOME/gate-export

rm -rf "$export_dir"; mkdir -p "$export_dir"
tar -xzf "$here/synth-export.tgz" -C "$export_dir"
python3 -I "$here/redate.py" "$export_dir"
"$repo/deploy/rehearsal/import.sh" gate "$port" "$pg_port" "$export_dir"
# The red proof: SMOKE_STOP=<module> restarts jc without that module, and the verdict must be RED.
if [ -n "${SMOKE_STOP:-}" ]; then
  roles=$(printf '%s\n' kaisatsu dataroom daiya ekiben antares gateway portal shako | grep -vx "$SMOKE_STOP" | paste -sd,)
  REHEARSAL_ROLE=$roles "$repo/deploy/rehearsal/up.sh" gate "$port" "$pg_port"
fi
# The suites read what the reconciler made of the import (views, schemas, policies): wait for the
# second leader pass logged from here, since the first may have started before the import ended.
# Without a Portal (SMOKE_STOP=portal) no pass comes, so the wait is bounded and only logs.
mark=$(wc -l < "$home/jc.log")
for _ in $(seq "${SMOKE_CONVERGE_SECS:-300}"); do
  [ "$(tail -n +"$((mark + 1))" "$home/jc.log" | grep -c 'sync run finished.*leader=true')" -ge 2 ] && break
  sleep 1
done
echo "gate: $(tail -n +"$((mark + 1))" "$home/jc.log" | grep -c 'sync run finished.*leader=true') reconciler passes since the import"

# jc names itself localhost:<port> (up.sh --base-host): tokens carry that issuer.
# The suites read their credentials from one state dir: jc's own, plus the service tokens
# up.sh gave jc from the secrets dir (jc keeps no file of them). Links only, in a 0700 dir.
view="$home/smoke-state"
rm -rf "$view" && mkdir -m 700 "$view"
for f in "$home"/state/* "$home"/secrets/{shako-token,ekiben-token,dashboards-password}; do
  [ -f "$f" ] && ln -s "$f" "$view/"
done
args=(--base-url "http://localhost:$port" --out "$out" --export "$export_dir" --state "$view"
      --log-file "$home/jc.log" --admin-url "http://127.0.0.1:$((pg_port + 200))"
      --admin-token-file "$home/secrets/metrics-token")
[ "${SMOKE_E2E:-0}" = 1 ] && args+=(--e2e)
# The logs suite also looks for every secret value of the rehearsal in the log.
args+=(--with "logs=--secrets-dir $home/secrets")
# The subscriptions probe is written by the project's ServiceAccount: its client secret is here.
args+=(--with "subscriptions=--realm-secrets-dir $home/secrets/realm")
# The assistant is a kept component on dev beside jc (tasks/GOAL-jc-dev.md): rows the rehearsal's
# assistant module being off prevents are named skips, never known failures.
args+=(--with "assistant=--kept-component")
# restore backs this environment up, restores it beside it (its own Postgres and ports) and compares.
args+=(--with "restore=--home $home")
# Bounded: each run is polled until it ends, so ten runs of five minutes outlast the job.
args+=(--with "pipelines=--runs 3 --wait 120")
# Failures the gate knows, each with where it is filed; only a new one turns the verdict red.
args+=(--known "$repo/bins/jc/smoke/ci/known.txt")
# Screenshots need Playwright for Python and its Chromium; without them charts-apps skips the shots.
python3 -c 'import playwright' 2>/dev/null || args+=(--with "charts-apps=--no-shots")
# shellcheck disable=SC2206 # SMOKE_ARGS is a flag list by design
args+=(${SMOKE_ARGS:-})
set +e
python3 "$repo/bins/jc/smoke/run" all "${args[@]}"
verdict=$?
set -e
cp "$home/jc.log" "$out/jc.log" 2>/dev/null || true
exit $verdict
