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
