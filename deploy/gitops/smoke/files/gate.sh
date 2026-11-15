#!/usr/bin/env bash
# The PostSync gate (ADR-N-056 §2.6, OPS-58): every Application of the project Synced at this
# revision and Healthy, then each public surface in the checks file answers with its exact
# status over valid TLS. A failure fails the Job, the Job fails the smoke Application's sync,
# and the sync's failure is the alert and the issue. It reads; it creates, execs and mints
# nothing (the full `just dev-smoke` stays the deployer's).
set -uo pipefail

revision="${REVISION:?}" self="${SELF:?}"
deadline=$((SECONDS + ${WAIT_SECONDS:-900}))
while :; do
	pending=$(kubectl get applications.argoproj.io -n "$NAMESPACE" -o json | jq -r --arg r "$revision" --arg self "$self" '
		.items[] | select(.spec.project == "platform" and .metadata.name != $self)
		| select(.status.sync.status != "Synced" or .status.sync.revision != $r or .status.health.status != "Healthy")
		| "\(.metadata.name) \(.status.sync.status)/\(.status.health.status) at \(.status.sync.revision)"') || pending="the Applications could not be read"
	[ -z "$pending" ] && break
	if [ "$SECONDS" -ge "$deadline" ]; then
		printf 'FAIL  not every Application is Synced at %s and Healthy:\n%s\n' "$revision" "$pending"
		exit 1
	fi
	sleep 15
done
echo "ok    every Application Synced at $revision and Healthy"

fail=0
while read -r expected url; do
	got=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 20 "$url" 2>/dev/null) || got=curl-error
	if [ "$got" = "$expected" ]; then echo "ok    $url ($got)"; else echo "FAIL  $url (expected $expected, got $got)"; fail=1; fi
done <"${CHECKS:-/gate/checks}"
exit "$fail"
