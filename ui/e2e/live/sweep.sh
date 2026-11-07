#!/usr/bin/env bash
# T-2734: the hourly live sweep, started by the cluster batch after dev-smoke (TS-01, TS-26).
#
#   PORTAL_URL=… PORTAL_PASSWORD=… APPROVER_PASSWORD=… PORTAL_TOTP=… APPROVER_TOTP=… VIEWER_PASSWORD=… EDITOR_PASSWORD=… \
#     PROPOSER_CLIENT_SECRET=… \
#     TASKS_DIR=/workspace/tasks ui/e2e/live/sweep.sh
#
# Runs every live journey and the page walker, except what spends the assistant's model: the
# specs that build or integrate by conversation, and every test whose title names the assistant
# (those run in the nightly eval batch, T-2733, within the daily budget). The Playwright report
# becomes a summary (scripts/sweep-summary.ts: one result per test, passwords and tokens taken
# out), and with TASKS_DIR set, `tasks/file-failures` turns each new failure into a task and
# leaves a known one on its task. Exits with Playwright's status, so the batch sees red as red.
set -uo pipefail
ui=$(cd "$(dirname "$0")/../.." && pwd)
out=${SWEEP_OUT:-$ui/test-results/sweep}
mkdir -p "$out"
cd "$ui" || exit 2

# The model-spending specs, by file: each of their tests is a conversation with the model. The
# App probe has a runner and a check of its own (app-probe.sh, T-2795). readiness-project opens
# and deletes a project, whose name stays reserved for 30 days, so it is nightly too (T-2746).
# clips_* are recording tools, run by hand with the line in T-6454: each run records videos nobody
# reads and leaves revoked connectors on demo.editor, and they can run a branch's UI against dev.
# open-data-speed times every dataset page of two publishers at two widths (about an hour of
# anonymous reads, T-6539): a measurement run by hand with the line in its header.
spending='/(excel_assistant|assistant-paths|assistant-evals|app-generated-full|build-samples|readiness-app|readiness-project|app-probe|open-data-speed|clips_[^/]*)\.spec\.ts$'
# SWEEP_SKIP, an extra regex over the same paths, leaves more out: the release gate (T-5899) skips the
# critic-* walks, which record shots for the UI critic tasks and took 1,619 s of its first hour.
skip=${SWEEP_SKIP:-'^$'}
# Timing runs by hand, one environment at a time (T-6541): their budgets are a measurement, not a gate.
byhand='/(open-data-times)\.spec\.ts$'
mapfile -t specs < <(ls e2e/live/*.spec.ts | grep -vE "$spending" | grep -vE "$byhand" | grep -vE -- "$skip")

# SWEEP_LIST=1 only lists the tests the sweep would run ("Total: N tests in M files"): the release
# gate sizes its sweep stage from that count (T-6354). Nothing signs in, nothing is written.
if [ -n "${SWEEP_LIST:-}" ]; then
  exec npx playwright test --config playwright.live.config.ts --list --grep-invert '\bassistant\b' "${specs[@]}"
fi

# SWEEP_WORKERS specs at once (T-6404, default 4): the tests of one spec still run in order, and the
# specs e2e/live/stateGroups.ts names keep to their group, the exclusive ones alone at the end.
rm -f "$out"/requests-*.json
started=$(date +%s)
LIVE_WORKERS=${SWEEP_WORKERS:-4} PLAYWRIGHT_JSON_OUTPUT_NAME="$out/report.json" npx playwright test \
  --config playwright.live.config.ts --reporter=list,json \
  --grep-invert '\bassistant\b' "${specs[@]}"
status=$?
echo "sweep: $(( $(date +%s) - started )) s on ${SWEEP_WORKERS:-4} workers"
# What the signed-in contexts sent the edge, per minute, summed over the workers (e2e/live/edgeShare.ts).
node -e '
  const fs = require("fs"), dir = process.argv[1], sum = {};
  for (const f of fs.readdirSync(dir).filter((f) => /^requests-\d+\.json$/.test(f)))
    for (const [m, n] of Object.entries(JSON.parse(fs.readFileSync(`${dir}/${f}`, "utf8")))) sum[m] = (sum[m] ?? 0) + n;
  fs.writeFileSync(`${dir}/requests-per-minute.json`, JSON.stringify(sum, null, 1));
  console.log(`sweep: at most ${Math.max(0, ...Object.values(sum))} edge requests in one minute (${dir}/requests-per-minute.json)`);
' "$out" || true

if [ ! -s "$out/report.json" ]; then
  echo "sweep: Playwright wrote no report (status $status); nothing to summarise" >&2
  exit "${status:-2}"
fi
node scripts/sweep-summary.ts "$out/report.json" > "$out/summary.json" || exit 2
echo "sweep: $(grep -c '"verdict": "pass"' "$out/summary.json") passed, $(grep -cE '"verdict": "(fail|error)"' "$out/summary.json") failed; summary $out/summary.json"
if [ -n "${TASKS_DIR:-}" ]; then
  "$TASKS_DIR/file-failures" "$out/summary.json" || exit 2
fi
exit "$status"
