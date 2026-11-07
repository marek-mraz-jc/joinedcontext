#!/usr/bin/env bash
# T-2795: the App probe, started by the cluster batch (AP-136).
#
#   PORTAL_URL=… PROBE_PASSWORD=… TASKS_DIR=/workspace/tasks ui/e2e/live/app-probe.sh
#
# PROBE_PASSWORD is Secret keycloak-user-demo-probe, key `password`, read at run time and never
# written anywhere. Runs e2e/live/app-probe.spec.ts, which writes the summary of the check
# `apps`; with TASKS_DIR set, `tasks/file-failures` turns a broken App into a task. Publish the
# chips after it: `just dev-publish-health <summary> 1 true` in joinedcontext-deployment.
set -uo pipefail
ui=$(cd "$(dirname "$0")/../.." && pwd)
export APP_PROBE_OUT=${APP_PROBE_OUT:-$ui/test-results/app-probe/summary.json}
cd "$ui" || exit 2
rm -f "$APP_PROBE_OUT"
npx playwright test --config playwright.live.config.ts --reporter=list e2e/live/app-probe.spec.ts
status=$?
if [ ! -s "$APP_PROBE_OUT" ]; then
  echo "app-probe: no summary written (status $status)" >&2
  exit "${status:-2}"
fi
echo "app-probe: summary $APP_PROBE_OUT"
if [ -n "${TASKS_DIR:-}" ]; then
  "$TASKS_DIR/file-failures" "$APP_PROBE_OUT" || exit 2
fi
exit "$status"
