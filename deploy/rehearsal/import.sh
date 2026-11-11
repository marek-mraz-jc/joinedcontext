#!/usr/bin/env bash
# import.sh <env> <port> <pg_port> <export dir>: the import of one environment's bundle, scripted.
#   1. up.sh with the broker on allow-all, jc on a loopback bind only (up.sh refuses any other);
#   2. rehearsal.py import (subject-less writes with the broker's admin bearer token); a short or
#      refused import stops jc and fails the script; every manifest passes `jcctl validate` first
#      (JCCTL_BIN, default jcctl beside jc: cargo build --manifest-path
#      vendor/joinedcontext-platform/Cargo.toml -p jcctl);
#   3. the prod run log beside the bundle into the Portal's tables (when <export>-runlog exists);
#      up.sh again on jc's default policy;
#   4. rehearsal.py policy-check: an anonymous write must answer 401 and the engine must be
#      joinedcontext, or jc is stopped and the script fails.
# REHEARSAL_HOLD_PIPELINES=1 holds the pipelines from jc's first start on (up.sh jc_role): the rows
# then are the export's alone until a plain up.sh starts them.
set -euo pipefail
env_name=${1:?env}; port=${2:?port}; pg_port=${3:?postgres port}; export_dir=${4:?export dir}
here=$(cd "$(dirname "$0")" && pwd)
home=${REHEARSAL_HOME:-$HOME/rehearsal}/$env_name
jc_bin=${JC_BIN:-${CARGO_TARGET_DIR:-$here/../../target}/debug/jc}
export REHEARSAL_EXPORT=$export_dir

REHEARSAL_IMPORT_POLICY=allow-all "$here/up.sh" "$env_name" "$port" "$pg_port"
# rehearsal.py import fails on any refused broker call, an entity short of the export or a temporal
# refusal other than the 24-hour cut-off; jc on allow-all never stays up after a failed import.
if ! python3 -I "$here/rehearsal.py" import --export "$export_dir" --base "http://127.0.0.1:$port" \
  --state "$home/state" --report "$home/import-report.json" --daiya-bin "$(dirname "$jc_bin")/daiya" \
  --jcctl-bin "${JCCTL_BIN:-$(dirname "$jc_bin")/jcctl}"; then
  kill "$(cat "$home/jc.pid")" 2>/dev/null || true
  echo "$env_name: import FAILED ($home/import-report.json); jc on allow-all stopped" >&2
  exit 1
fi
# The prod Portal's run log beside the bundle (<export>-runlog, or REHEARSAL_RUNLOG), loaded before
# jc restarts so the run queue keeps each pipeline's clock instead of running the backlog at once.
runlog=${REHEARSAL_RUNLOG:-${export_dir%/}-runlog}
if [ -d "$runlog" ]; then
  python3 -I "$here/rehearsal.py" import-runlog --runlog "$runlog" --port "$pg_port" \
    --password-file "$home/secrets/portal-pw" > "$home/runlog-report.json"
fi
"$here/up.sh" "$env_name" "$port" "$pg_port"
# Wait until the broker answers at all (no credential needed for that).
for _ in $(seq 60); do
  [ "$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port/broker/q/health")" != 000 ] && break; sleep 2
done
if ! python3 -I "$here/rehearsal.py" policy-check --base "http://127.0.0.1:$port" --state "$home/state"; then
  kill "$(cat "$home/jc.pid")" 2>/dev/null || true
  echo "$env_name: the broker does not refuse an anonymous write after the import; jc stopped" >&2
  exit 1
fi
echo "$env_name: imported; broker back on its default policy (anonymous write 401)"
