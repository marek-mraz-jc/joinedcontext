#!/usr/bin/env bash
# One cluster-watch tick for alerts (T-5154): reads Alertmanager's active alerts, turns the ones
# that started or resolved since the last tick into lines with scripts/alert-notes.py, and posts
# each one as `tasks/note "@deployer <line>"`. The host job runs it every 5 minutes with
# KUBECONFIG set:
#   scripts/alert-watch.sh <alertmanager namespace> <state file>
# Alertmanager has no route at the edge, and the API server's service proxy is an unmeshed caller
# its Linkerd proxy refuses (default-inbound-policy cluster-authenticated), so the read goes through
# a port-forward, which reaches the pod on its own loopback. A failed fetch posts nothing and keeps
# the state file, so an unreachable cluster never reads as every alert resolving.
set -euo pipefail

namespace="${1:?usage: alert-watch.sh <alertmanager namespace> <state file>}"
state="${2:?usage: alert-watch.sh <alertmanager namespace> <state file>}"
note="${TASKS_NOTE:-/workspace/tasks/note}"
port="${ALERT_WATCH_PORT:-19093}"
here="$(cd "$(dirname "$0")" && pwd)"

alerts="$(mktemp)"
forward=""
trap 'if [ -n "$forward" ]; then kill "$forward" 2>/dev/null || true; fi; rm -f "$alerts"' EXIT

kubectl -n "$namespace" port-forward --address 127.0.0.1 svc/alertmanager-operated "${port}:9093" >/dev/null &
forward=$!
for _ in $(seq 1 20); do
	if curl -fsS --max-time 10 "http://127.0.0.1:${port}/api/v2/alerts" >"$alerts" 2>/dev/null; then
		python3 "$here/alert-notes.py" "$state" <"$alerts" | while IFS= read -r line; do
			if [ -n "$line" ]; then
				"$note" "@deployer $line"
			fi
		done
		exit 0
	fi
	sleep 0.5
done
echo "alert-watch: Alertmanager in ${namespace} did not answer on the port-forward" >&2
exit 1
