#!/usr/bin/env bash
# The smoke checks of one component, after `just dev-apply-pins` moved it (T-6110).
#   scripts/smoke-component.sh <domain> <namespace> <component>
# portal: the served binary and UI bundle are the pinned commit, the login hands over to
# Keycloak, one public Endpoint read through the gateway. context-gateway: one Endpoint read
# and an MCP tools/list. Any other component: its rollouts were waited on by wait-rollouts.sh,
# which is its whole check here; the hourly `just dev-smoke` runs the rest. Each route checked
# also answers with the edge's security response headers (T-6155), as smoke.sh checks them on idm.
set -uo pipefail

domain="${1:?usage: smoke-component.sh <domain> <namespace> <component>}"
ns="${2:?usage: smoke-component.sh <domain> <namespace> <component>}"
component="${3:?usage: smoke-component.sh <domain> <namespace> <component>}"
root="$(cd "$(dirname "$0")/.." && pwd)"
portal="https://portal.${domain}"
api="https://api.${domain}"
idm="https://idm.${domain}"
fail=0
ok() { printf '  ok    %s\n' "$1"; }
ko() { printf '  FAIL  %s\n' "$1"; fail=1; }

# status <expected> <description> <curl args...>; a 429 (the edge's per-address budget every
# agent of the host shares) is waited out up to three times, as smoke.sh does.
status() {
	local expected="$1" desc="$2" got tries=0; shift 2
	while :; do
		got=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 20 "$@" 2>/dev/null || echo curl-error)
		if [ "$got" != 429 ] || [ "$tries" -ge 3 ]; then break; fi
		tries=$((tries + 1)); sleep 31
	done
	if [ "$got" = "$expected" ]; then ok "$desc ($got)"; else ko "$desc (expected $expected, got $got)"; fi
}

# headers <url>: the edge's security response headers on the route the component serves.
headers() {
	local got missing="" h
	got=$(curl -sSI --max-time 20 "$1" 2>/dev/null | tr -d '\r')
	for h in strict-transport-security x-content-type-options x-frame-options referrer-policy content-security-policy; do
		grep -qi "^$h:" <<<"$got" || missing="$missing $h"
	done
	if [ -z "$missing" ]; then ok "$1 answers the edge security headers"; else ko "$1 lacks the security headers:$missing"; fi
}

# The public Helsinki Endpoint the seed declares (the file the gateway's checkouts serve), read
# from the tree, so the smoke needs no cluster credential.
endpoint() {
	yq '.spec.slug // ""' "$root/components/context-gateway/seed/helsinki/helsinki-endpoint-all.yaml" 2>/dev/null
}
granted() {
	curl -sS --max-time 20 -H 'Accept: application/json' "$api/api/endpoint/$1/access" 2>/dev/null |
		python3 -c 'import json, sys
try:
    doc = json.load(sys.stdin)
except ValueError:
    sys.exit(0)
print(next((t for p in doc.get("permissions", []) if (t := (p.get("resource") or {}).get("type"))), ""))'
}
read_endpoint() {
	local slug type
	slug=$(endpoint)
	if [ -z "$slug" ]; then ko "the seed declares no helsinki-all Endpoint slug to read"; return; fi
	type=$(granted "$slug")
	if [ -z "$type" ]; then ko "Endpoint $slug names no type in its access document"; return; fi
	status 200 "Endpoint $slug answers an NGSI-LD read of $type" "$api/api/endpoint/$slug/ngsi-ld/v1/entities?type=$type&limit=1"
	headers "$api/api/endpoint/$slug/ngsi-ld/v1/entities?type=$type&limit=1"
	if [ "$component" = context-gateway ]; then
		status 200 "Endpoint $slug answers an MCP tools/list" -X POST -H 'Content-Type: application/json' \
			-H 'Accept: application/json, text/event-stream' -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}' \
			"$api/api/endpoint/$slug/mcp"
	fi
}

echo "smoke $component"
case "$component" in
	portal)
		pinned=$(yq '.portal.portal.commit' "$root/components/portal/images.yaml")
		ui=$(yq '.portal.ui.commit' "$root/components/portal/images.yaml")
		served=$(curl -sS --max-time 20 "$portal/api/v1/version" 2>/dev/null |
			python3 -c 'import json, sys
try:
    v = json.load(sys.stdin)
except ValueError:
    v = {}
print((v.get("portal") or {}).get("commit", ""), (v.get("ui") or {}).get("commit", ""), v.get("contract", ""))')
		read -r commit bundle contract <<<"$served"
		if [ "${commit:-}" = "$pinned" ]; then ok "the Portal serves the pinned commit ${pinned:0:12}"
		else ko "the Portal serves '${commit:-unreadable}', the pin is ${pinned}"; fi
		if [ "${bundle:-}" = "$ui" ] && [ "${contract:-}" = match ]; then ok "the UI bundle is the pinned ${ui:0:12}, built against the binary's API"
		else ko "the UI bundle is '${bundle:-unreadable}' (pin ${ui}), contract '${contract:-unreadable}'"; fi
		location=$(curl -sS -o /dev/null -w '%{redirect_url}' --max-time 20 "$portal/api/v1/auth/login" 2>/dev/null || true)
		case "$location" in
			"$idm/realms/$ns/protocol/openid-connect/auth?"*) ok "the Portal login hands over to Keycloak" ;;
			*) ko "the Portal login redirects to '${location}', not to $idm/realms/$ns" ;;
		esac
		headers "$portal/"
		read_endpoint
		;;
	context-gateway) read_endpoint ;;
	keycloak) headers "$idm/realms/$ns/" ;;
	*) ok "$component: rollouts Ready (wait-rollouts.sh); no component check beyond them" ;;
esac
exit "$fail"
