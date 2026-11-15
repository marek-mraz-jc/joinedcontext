#!/usr/bin/env bash
# The edge smoke of a throwaway k3d installation: what the variants harness proves after a fresh
# install (smoke_ingress_http in test-deployment-variants.sh), over a verified chain and the names
# the caller resolves. ci-upgrade-e2e.sh hands it to `just upgrade --smoke` (T-4795).
#
#   scripts/edge-smoke.sh https://<apex> https://idm.<apex>
#
# scripts/smoke.sh is the smoke of a live installation and expects what k3d never has: live feeds,
# an Actions runner for the sample apps, APISIX as the outermost hop (k3d puts ingress-nginx in
# front, which rewrites the Server header and the caller's address). Red there says nothing about
# the upgrade (ci-full 39353463338, 39362707152), so the upgrade job gates on this one:
#   * Keycloak's OIDC discovery through the edge answers 200;
#   * the Portal host answers through the edge with a status of its own (2xx, 3xx, 401, 403),
#     not a gateway error (000, 404, 5xx).
# Both are retried until JC_EDGE_SMOKE_SECONDS (default 180) have passed. Exit 0 when both hold.

set -euo pipefail

apex="${1:?usage: edge-smoke.sh https://<apex> https://idm.<apex>}"
idm="${2:?usage: edge-smoke.sh https://<apex> https://idm.<apex>}"
case "$apex" in https://*) ;; *) echo "edge-smoke: $apex is not an https URL" >&2; exit 2 ;; esac
case "$idm" in https://*) ;; *) echo "edge-smoke: $idm is not an https URL" >&2; exit 2 ;; esac
portal="https://portal.${apex#https://}/"
deadline=$((SECONDS + ${JC_EDGE_SMOKE_SECONDS:-180}))

code() { curl -s -o /dev/null -w '%{http_code}' --max-time 10 "$1" 2>/dev/null || true; }

# check <name> <url> <status regex>: retried until the deadline
check() {
	local got
	while :; do
		got="$(code "$2")"
		got="${got:-000}"
		if [[ "$got" =~ ^($3)$ ]]; then
			echo "  ok    $1 ($got)"
			return 0
		fi
		if ((SECONDS >= deadline)); then
			echo "  FAIL  $1: last status $got for $2" >&2
			return 1
		fi
		sleep 10
	done
}

echo "edge smoke: $apex"
check "keycloak OIDC discovery through the edge" "$idm/realms/master/.well-known/openid-configuration" '200'
check "the portal host answers through the edge" "$portal" '2[0-9][0-9]|3[0-9][0-9]|401|403'
