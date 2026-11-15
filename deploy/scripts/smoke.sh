#!/usr/bin/env bash
# Smoke-test a deployed instance from outside the cluster.
#   scripts/smoke.sh <base-url> <idm-url>
# e.g. scripts/smoke.sh https://dev.joinedcontext.com https://idm.dev.joinedcontext.com
#
# Every check asserts an exact HTTP status over a *valid* TLS chain (no -k), so an
# expired or missing certificate fails the run. Cluster-side checks (workload
# readiness) use the current kube context.
set -uo pipefail

base="${1:?usage: smoke.sh <base-url> <idm-url>}"
idm="${2:?usage: smoke.sh <base-url> <idm-url>}"
realm="${JC_REALM:-dev}"
slug="${JC_INSTANCE:-$realm}"
# The Portal has a host of its own behind the edge login (ADR-N-019), derived the way the
# route derives it; the apex keeps the shared surfaces and redirects the rest there.
portal="${JC_PORTAL_URL:-https://portal.${base#https://}}"
# The public data host (ADR-N-052 §2.1, EP-128, T-4630): /api/endpoint, /api/mcp, /cs and
# /catalog answer there; the apex and the Portal host send an anonymous caller to it with 308.
api="${JC_API_URL:-https://api.${base#https://}}"
# The ceiling on the egress settle window (OPS-27, SEC-GAP-05). The network plugin programs a
# new pod's egress chains after that pod is already on the network, and the egress probe below
# measures how long that takes. Over this many seconds the run fails instead of printing a
# larger number nobody reads: the gap is only an accepted one while it is bounded. Raising it on
# a cluster is a decision with a line in AI_shared_folder.md, never a quiet default change.
#
# The probe keeps its own upper bound (18 rounds of 4 seconds), so a ceiling at or above that
# could never be exceeded and would silently switch the check off; such a value is refused.
settle_probe_seconds=72
settle_ceiling="${JC_NETPOL_SETTLE:-30}"
case "$settle_ceiling" in
'' | *[!0-9]*)
	printf 'smoke: JC_NETPOL_SETTLE must be a whole number of seconds, not "%s"\n' "$settle_ceiling" >&2
	exit 2
	;;
esac
if [ "$settle_ceiling" -ge "$settle_probe_seconds" ]; then
	printf 'smoke: JC_NETPOL_SETTLE=%ss is not below the probe own bound of %ss, so nothing would fail on it\n' \
		"$settle_ceiling" "$settle_probe_seconds" >&2
	exit 2
fi

pass=0
fail=0
skipped=0
warned=0

ok() { printf '  ok    %s\n' "$1"; pass=$((pass + 1)); }
ko() { printf '  FAIL  %s\n' "$1"; fail=$((fail + 1)); }
# A check whose subject is not deployed in this instance. Not a pass: it never ran.
skip() { printf '  skip  %s\n' "$1"; skipped=$((skipped + 1)); }
# A finding that does not fail the run: the platform serves, something is not as built.
warn() { printf '  warn  %s\n' "$1"; warned=$((warned + 1)); }

# The pipeline runner carries every project's streams in one process, so an OOMKill during the
# run is every pipeline stopped at once (T-3537). Its restart count is read now and again at
# the end, and it must not have moved.
runner_restarts() {
	kubectl get pods -n "$slug" -l app.kubernetes.io/name=pipeline-runner-runner \
		-o 'jsonpath={.items[*].status.containerStatuses[?(@.name=="pipeline-runner-runner")].restartCount}' 2>/dev/null |
		tr ' ' '\n' | awk '{ total += $1 } END { if (NR) print total }'
}
runner_restarts_before=$(runner_restarts)
smoke_started=$(date +%s)

# The entity type an endpoint grants this caller, from its own access document (EP-55).
#
# A read names a type (GW33), and these grants are `retrieveOps`, which is `retrieveEntity` and
# `queryEntity` and nothing else (CIM 009 Table 4.20-2) — the type list is `federationOps`, so
# asking an endpoint what types it has is a 403 by design. The access document is how a client
# learns the type, and what a probe reads for the same reason.
type_of() {
	curl -sS --max-time 20 -H 'Accept: application/json' "$api/api/endpoint/$1/access" 2>/dev/null |
		python3 -c 'import json,sys
try:
    document = json.load(sys.stdin)
except Exception:
    sys.exit(0)
for permission in document.get("permissions", []):
    entity_type = (permission.get("resource") or {}).get("type")
    if entity_type:
        print(entity_type)
        break'
}

# The gateway routes are only configured for the components this instance deploys, so the
# suite asks the edge's base file what exists instead of assuming a fixed platform. The served
# file is a Secret with every App's client secret in it (AP-112), which the smoke never reads.
routes=$(kubectl get configmap apisix-standalone-base -n "$slug" -o jsonpath='{.data.apisix\.yaml}' 2>/dev/null || true)
has_route() { grep -q "^  - id: $1\$" <<<"$routes"; }
# The ConfigMap holding a project's seed (its first part): gitea-seed-s<release>-<project> since the
# seed is packed by size over its releases (T-4431), gitea-seed-<project> before.
# One seed file of a project, wherever its release's parts hold it: a grown seed moves files between
# gitea-seed-<project> and gitea-seed-<project>--N (dev 2026-11-02: three helsinki Endpoints moved, six red checks).
seed_file() {
	local cm
	for cm in $(kubectl get configmaps -n "$slug" -o name 2>/dev/null | sed -n "s|^configmap/\(gitea-seed-\(s[0-9]*-\)\{0,1\}$1\(--[0-9]*\)\{0,1\}\)\$|\1|p"); do
		kubectl get configmap "$cm" -n "$slug" -o jsonpath="{.data${2:+.$2}}" 2>/dev/null
	done
}

# status <expected> <description> <curl args...>
# A 429 is the edge's per-IP budget, which every agent on this host's address spends as well
# (apply 78, 2026-10-15: the UX photo audit walking the Portal). It is not the answer a check is
# about: wait out the window the edge names, then ask again, up to three windows. Every check keeps its answer.
wait_429() {
	local reset="$1"
	[[ $reset =~ ^[0-9]+$ ]] && [ "$reset" -le 60 ] || reset=30
	sleep $((reset + 1))
}

status() {
	local expected="$1" desc="$2"; shift 2
	local got reset
	read -r got reset < <(curl -sS -o /dev/null -w '%{http_code} %header{x-ratelimit-reset}\n' \
		--max-time 20 "$@" 2>/dev/null || echo curl-error)
	local tries=0
	while [ "$got" = 429 ] && [ "$expected" != 429 ] && [ $tries -lt 3 ]; do
		wait_429 "$reset"; tries=$((tries + 1))
		read -r got reset < <(curl -sS -o /dev/null -w '%{http_code} %header{x-ratelimit-reset}\n' \
			--max-time 20 "$@" 2>/dev/null || echo curl-error)
	done
	if [ "$got" = "$expected" ]; then ok "$desc ($got)"; else ko "$desc (expected $expected, got $got)"; fi
}

# immutable <description> <url>: the asset answers 200 with the year-long cache of a
# content-hashed file (T-4201). A 429 on the way is waited out as `status` does.
immutable() {
	local desc="$1" url="$2" head tries=0
	case $url in */assets/?*|*/webassets/?*) ;; *) ko "$desc (no asset found on the page)"; return ;; esac
	head=$(curl -sSI --max-time 20 "$url" 2>/dev/null || true)
	while grep -q '^HTTP/[0-9.]* 429' <<<"$head" && [ $tries -lt 3 ]; do
		wait_429 "$(sed -n 's/^x-ratelimit-reset: *\([0-9]*\).*/\1/Ip' <<<"$head")"; tries=$((tries + 1))
		head=$(curl -sSI --max-time 20 "$url" 2>/dev/null || true)
	done
	if grep -q '^HTTP/[0-9.]* 200' <<<"$head" && grep -qi '^cache-control: public, max-age=31536000, immutable' <<<"$head"; then
		ok "$desc"
	else
		ko "$desc ($(head -1 <<<"$head" | tr -d '\r'); $(grep -i '^cache-control' <<<"$head" | tr -d '\r'))"
	fi
}

echo "smoke: $base (realm $realm)"

echo "workloads"
if scripts/wait-rollouts.sh "$slug" "${JC_ROLLOUT_TIMEOUT:-120}" >/tmp/smoke-rollouts.log 2>&1; then
	ok "all workloads ready"
else
	ko "workloads not ready"; tail -20 /tmp/smoke-rollouts.log
fi

echo "mesh"
# Every meshed pod waits on the Linkerd control plane and on the CNI plugin, which live outside
# the instance's namespaces. After a node reboot both sat not Ready for nine hours and nothing
# said so (T-4384, dev 2026-10-17), so each pod there is read on its own Ready condition.
for ns in linkerd linkerd-cni; do
	pods=$(kubectl get pods -n "$ns" -o 'jsonpath={range .items[*]}{.metadata.name}={.status.conditions[?(@.type=="Ready")].status}{"\n"}{end}' 2>/dev/null || true)
	if [ -z "$pods" ]; then
		skip "$ns (no pod in it on this cluster)"
		continue
	fi
	not_ready=$(grep -v '=True$' <<<"$pods" | cut -d= -f1 | tr '\n' ' ')
	if [ -z "$not_ready" ]; then
		ok "every pod in $ns is Ready"
	else
		ko "$ns pods not Ready: ${not_ready% }"
	fi
done

echo "identity"
status 200 "keycloak OIDC discovery over valid TLS" "$idm/realms/$realm/.well-known/openid-configuration"
if curl -sS --max-time 20 "$idm/realms/$realm/.well-known/openid-configuration" 2>/dev/null |
	grep -q '"jwks_uri"'; then ok "realm $realm publishes a JWKS uri"; else ko "realm $realm has no JWKS uri"; fi
status 200 "keycloak JWKS over valid TLS" "$idm/realms/$realm/protocol/openid-connect/certs"

echo "gateway"
# Everything reaches the platform through APISIX; the identity host is the route that
# exists in every instance, even before the portal and the gateway are deployed.
if curl -sSI --max-time 20 "$idm/realms/$realm/" 2>/dev/null | grep -i '^server: *APISIX' >/dev/null; then
	ok "traffic is served by APISIX, not by the ingress controller"
else
	ko "no APISIX Server header on $idm — the route is not going through the gateway"
fi

echo "edge"
# T-4630, EP-128: the data host answers from the gateway (its RFC 9728 document names the host),
# and the address people copied before the move is a 308 to the same path there.
if has_route context-mcp-hub; then
	metadata=$(curl -sS --max-time 20 "$api/api/mcp/.well-known/oauth-protected-resource" 2>/dev/null || true)
	if grep -q "\"resource\":\"$api/api/mcp\"" <<<"$metadata"; then
		ok "the api host serves the hub from the gateway"
	else
		ko "the api host does not serve the hub's metadata naming $api/api/mcp (got: ${metadata:0:160})"
	fi
	read -r got location < <(curl -sS -o /dev/null -w '%{http_code} %{redirect_url}\n' --max-time 20 \
		"$base/api/mcp/.well-known/oauth-protected-resource?x=1" 2>/dev/null || echo curl-error)
	if [ "$got" = 308 ] && [ "$location" = "$api/api/mcp/.well-known/oauth-protected-resource?x=1" ]; then
		ok "the apex sends the old hub address to the api host (308)"
	else
		ko "the apex answers the old hub address $got ${location:-without a Location}, not 308 to $api"
	fi
else
	skip "api host (route context-mcp-hub not configured in this instance)"
fi
# The Portal host is the DEMO entry: a missing Ingress rule for it fails silently as a 404
# from the ingress controller, and a missing SAN as a handshake error. Unauthenticated, the
# edge login answers a 302 to the realm's authorization endpoint with the `edge` client and
# the callback that client lists (ADR-N-019, AP-27); anything else means the openid-connect
# plugin is not in front of the Portal or points at a client Keycloak will refuse. `/` itself
# lets an anonymous visitor through to the catalogue (portal-home, T-5627), so the check asks a
# page that needs a session.
if has_route portal-ui; then
	edge_callback=$(printf '%s' "$portal/callback" | sed 's,:,%3A,g; s,/,%2F,g')
	location=$(curl -sS -o /dev/null -w '%{redirect_url}' --max-time 20 "$portal/projects" 2>/dev/null || true)
	# The query parameters come in whatever order the plugin builds them.
	case "$location" in
		"$idm/realms/$realm/protocol/openid-connect/auth?"*) ;;
		*) location="" ;;
	esac
	if [ -n "$location" ] && grep -q "client_id=edge" <<<"$location" && grep -q "redirect_uri=$edge_callback" <<<"$location"; then
		ok "portal host sends an anonymous visitor to the edge login"
	else
		ko "portal host does not redirect to the edge login with client_id=edge and redirect_uri=$edge_callback (got: ${location:-none})"
	fi
	# The apex still answers: `/` goes to the Portal host, so the one hostname people learn
	# first keeps working.
	location=$(curl -sS -o /dev/null -w '%{redirect_url}' --max-time 20 "$base/" 2>/dev/null || true)
	if [ "$location" = "$portal/" ]; then ok "apex redirects to the portal host"; else ko "apex does not redirect to $portal/ (got: ${location:-none})"; fi
else
	skip "portal host (route portal-ui not configured in this instance)"
fi
# A streamed body reaches the application (T-2263). APISIX inherits `client_max_body_size 0`,
# which nginx reads as "no limit" for a body with a Content-Length and as "zero" in its chunked
# filter, so with the default every `Transfer-Encoding: chunked` write is refused with 413 before
# authentication — measured on dev as a 435-byte Portal write. The probe is unauthenticated on
# purpose: what matters is that the answer comes from the application (401 or 403, whichever the
# route's own refusal is) and not the edge's 413.
if has_route portal-api; then
	chunked=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 20 -X PUT \
		-H 'Content-Type: application/json' -H 'Transfer-Encoding: chunked' \
		--data-binary '{"probe":"chunked"}' \
		"$portal/api/v1/projects/helsinki/pipelines/smoke-probe" 2>/dev/null || true)
	case "$chunked" in
		413) ko "the edge refuses a chunked request body (413): client_max_body_size is 0 again" ;;
		4*|2*) ok "a chunked request body reaches the application ($chunked)" ;;
		*) ko "a chunked request body got ${chunked:-no answer}" ;;
	esac
else
	skip "chunked body (route portal-api not configured in this instance)"
fi
headers=$(curl -sSI --max-time 20 "$idm/realms/$realm/" 2>/dev/null)
missing=""
for header in strict-transport-security x-content-type-options x-frame-options referrer-policy; do
	grep -qi "^$header:" <<<"$headers" || missing="$missing $header"
done
if [ -z "$missing" ]; then ok "security response headers present"; else ko "response headers missing:$missing"; fi
# BSI TR-02102: TLS 1.2 is the floor and the CBC suites are out. Both are asserted against
# the live listener, because the policy that produces them lives in the ingress controller
# and never appears in this repo's rendered output.
if curl -sS -o /dev/null --max-time 20 --tls-max 1.1 "$idm/realms/$realm/" >/dev/null 2>&1; then
	ko "the edge negotiated TLS 1.1"
else
	ok "the edge refuses TLS 1.1"
fi
if command -v openssl >/dev/null 2>&1; then
	host=${idm#https://}; host=${host%%/*}
	# The output is captured before it is matched: a refused handshake makes openssl exit
	# non-zero, and under `pipefail` that would sink the whole pipeline even though the grep
	# found exactly what this check wants to see.
	handshake=$(echo | openssl s_client -connect "$host:443" -servername "$host" -tls1_2 \
		-cipher 'ECDHE-RSA-AES128-SHA256:ECDHE-RSA-AES256-SHA384:DES-CBC3-SHA' 2>&1 || true)
	if grep -qF 'Cipher is (NONE)' <<<"$handshake"; then
		ok "the edge refuses CBC and 3DES suites"
	else
		ko "the edge negotiated a CBC or 3DES suite"
	fi
else
	skip "cipher suite check (openssl not installed)"
fi

echo "token"
# The client secret is never in the repo: CI passes it in, otherwise it is read from the
# cluster the kubeconfig already grants access to.
client_id="${JC_SMOKE_CLIENT_ID:-apisix-gateway}"
client_secret="${JC_SMOKE_CLIENT_SECRET:-$(kubectl get secret "keycloak-client-$client_id" -n "$slug" -o jsonpath='{.data.client-secret}' 2>/dev/null | base64 -d 2>/dev/null || true)}"
token=""
if [ -z "$client_secret" ]; then
	ko "no client secret for $client_id (set JC_SMOKE_CLIENT_SECRET or point KUBECONFIG at the instance)"
else
	# A secret goes to curl through a pipe (`name@<(printf ...)`, printf is a builtin), never as
	# an argv word another process on this host can read in /proc (F-R2DEP3-003).
	token=$(curl -sS --max-time 20 -d grant_type=client_credentials -d "client_id=$client_id" \
		--data-urlencode "client_secret@"<(printf %s "$client_secret") \
		"$idm/realms/$realm/protocol/openid-connect/token" 2>/dev/null |
		sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')
	if [ -n "$token" ]; then ok "client_credentials token from realm $realm"; else ko "no access_token for $client_id in realm $realm"; fi
fi

# DEMO.md step 1 is a human login. The development profile seeds demo.steward and
# demo.viewer with per-cluster passwords (Secret keycloak-user-<name>); a password grant
# through the smoke's confidential client proves the user, the credential and the realm
# policy agree. An instance without the seed (production) has nothing to prove: skip.
demo_user="${JC_SMOKE_DEMO_USER:-demo.viewer}"
demo_password="${JC_SMOKE_DEMO_PASSWORD:-$(kubectl get secret "keycloak-user-${demo_user//./-}" -n "$slug" -o jsonpath='{.data.password}' 2>/dev/null | base64 -d 2>/dev/null || true)}"
# The Secret is named after the short name; the realm stores the email as the username
# (registrationEmailAsUsername), so that is what a password grant has to send.
# The organization's domain is the users' email domain and every URN's second segment; the
# gateway is deployed with it, so it is read from there rather than written here.
org="${JC_SMOKE_ORG:-$(kubectl get deploy context-gateway -n "$slug" \
	-o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="JC_GATEWAY_ORG_DOMAIN")].value}' 2>/dev/null || true)}"
org="${org:-hel.fi}"
demo_login="${demo_user}@${org}"
# What a live journey or a recorded take names as its own: the janitor role's pattern, which
# tests/test_smoke_script.py holds equal to seed/helsinki/helsinki-role-janitor.yaml (T-2817).
RESIDUE_NAMES='t[0-9]{4}[a-z]?-[a-z0-9-]+|newuser-policy-[a-z0-9]+|[a-z0-9-]+-[0-9]{4}|bikes-regional-[a-z0-9]+|large-map-city'
demo_token=""
if [ -z "$demo_password" ]; then
	skip "demo user login (no demo users seeded in this instance)"
elif [ -z "$client_secret" ]; then
	ko "demo user login needs the $client_id client secret"
else
	demo_token=$(curl -sS --max-time 20 -d grant_type=password \
		-d "client_id=$client_id" --data-urlencode "client_secret@"<(printf %s "$client_secret") \
		-d "username=$demo_login" --data-urlencode "password@"<(printf %s "$demo_password") \
		"$idm/realms/$realm/protocol/openid-connect/token" 2>/dev/null |
		sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')
	if [ -n "$demo_token" ]; then ok "demo user $demo_login logs in with a password grant"; else ko "demo user $demo_login cannot log in with a password grant"; fi
fi

echo "edge login"
# The edge (APISIX openid-connect, ADR-N-019) forwards the person's access token and the Portal
# verifies it like a bearer, audience included. A realm whose `edge` client has no audience
# mapper for portal-api mints tokens the Portal refuses: the login page, then a 401, then the
# login page again, for everybody (T-0619). The client's mappers are read through the admin
# API with the instance's own admin secret; direct grants are off for `edge` on purpose, so
# there is no token to inspect from outside.
admin_user="${JC_SMOKE_ADMIN_USER:-admin}"
admin_password="${JC_SMOKE_ADMIN_PASSWORD:-$(kubectl get secret keycloak-admin-user -n "$slug" -o jsonpath='{.data.password}' 2>/dev/null | base64 -d 2>/dev/null || true)}"
if [ -z "$admin_password" ]; then
	skip "edge client audience (no admin secret: set JC_SMOKE_ADMIN_PASSWORD or point KUBECONFIG at the instance)"
else
	admin_token=$(curl -sS --max-time 20 -d grant_type=password -d client_id=admin-cli \
		-d "username=$admin_user" --data-urlencode "password@"<(printf %s "$admin_password") \
		"$idm/realms/master/protocol/openid-connect/token" 2>/dev/null |
		sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')
	edge_client=$(curl -sS --max-time 20 -H "Authorization: Bearer $admin_token" "$idm/admin/realms/$realm/clients?clientId=edge" 2>/dev/null || true)
	if [ -z "$admin_token" ]; then
		ko "no admin token for $admin_user in realm master"
	elif grep -q '"included.client.audience" *: *"portal-api"' <<<"$edge_client"; then
		ok "edge client mints tokens with audience portal-api (the Portal verifies aud)"
	else
		ko "edge client has no portal-api audience mapper: every edge login is a 401 (T-0619)"
	fi
	# The agent proxy reads samples through whatever endpoint a run names, including one approved
	# after the realm was written, so its token carries the gateway-wide audience beside the seeded
	# slugs and the Policy decides per endpoint (T-0666, Architecture/12 §5). Without it the kit
	# pass reads a 401 for every type and the generated app has no data.
	if [ -n "$admin_token" ]; then
		proxy_client=$(curl -sS --max-time 20 -H "Authorization: Bearer $admin_token" "$idm/admin/realms/$realm/clients?clientId=helsinki-agent-proxy" 2>/dev/null || true)
		if [ "$proxy_client" = "[]" ]; then
			skip "agent proxy audience (no helsinki-agent-proxy client in realm $realm)"
		elif grep -q '"included.custom.audience" *: *"context-gateway"' <<<"$proxy_client"; then
			ok "agent proxy client mints tokens with audience context-gateway (reads through any endpoint its Policy grants)"
		else
			ko "agent proxy client has no context-gateway audience: a run reads a 401 through an endpoint approved after the realm (T-0666)"
		fi
		# The proxy's second door: it asks the Portal's internal listener what a run is, and that
		# listener answers this client's token and no other (T-2271, AG-52). Without the mapper the
		# lookup fails, and the proxy answers every model call of every conversation
		# `401 invalid run credentials` — the same sentence four different failures produce
		# (T-2285), so a realm written before the mapper looks exactly like a forged ticket. This
		# is the assistant's first-impression flow, so it is checked here and not only in the
		# render (T-2420; `tests/test_keycloak_clients.py` holds the manifest side).
		if [ "$proxy_client" != "[]" ] && [ -n "$proxy_client" ]; then
			if grep -q '"included.custom.audience" *: *"portal-internal"' <<<"$proxy_client"; then
				ok "agent proxy client mints tokens with audience portal-internal (the Portal answers its run lookups)"
			else
				ko "agent proxy client has no portal-internal audience: every assistant answer fails with 401 invalid run credentials (T-2420, T-2271)"
			fi
		fi
	fi
fi

echo "authenticated routes"
if has_route portal-api; then
	# /api/v1/health is public (probes, uptime checks); /api/v1/auth/me is the smallest route
	# behind the Portal's own token verification. The edge passes a call without a session
	# through untouched (`unauth_action: pass`, no bearer verification: ES256 vs
	# lua-resty-openidc, T-0252), so the 401 and the 200 below are the Portal's own answers.
	status 200 "portal API health is public" "$portal/api/v1/health"
	# The binary and the UI bundle it serves (T-2485, T-5297): `match` when the bundle was built
	# against this API. Anything else still serves pages, so it is a warning: a page may call a
	# route the binary no longer has.
	version=$(curl -sS --max-time 20 "$portal/api/v1/version" 2>/dev/null || true)
	contract=$(grep -o '"contract": *"[a-z]*"' <<<"$version" | sed 's/.*"\([a-z]*\)"$/\1/' || true)
	if [ "$contract" = match ]; then
		ok "portal UI bundle was built against the binary's API (contract match)"
	else
		warn "portal UI bundle contract is ${contract:-unreadable}, not match: ${version:0:200}"
	fi
	status 401 "portal API rejects an unauthenticated call" "$portal/api/v1/auth/me"
	# The Portal's own code flow, the fallback of an installation without the edge, starts
	# here with a 302 to Keycloak. The redirect_uri it carries must be one the portal-api
	# client lists exactly, or Keycloak answers "Invalid parameter: redirect_uri" and no human
	# can log in; that page is a 400 the status checks never see.
	callback=$(printf '%s' "$portal/api/v1/auth/callback" | sed 's,:,%3A,g; s,/,%2F,g')
	location=$(curl -sS -o /dev/null -w '%{redirect_url}' --max-time 20 "$portal/api/v1/auth/login" 2>/dev/null || true)
	case "$location" in
		"$idm/realms/$realm/protocol/openid-connect/auth?"*"redirect_uri=$callback"*) ok "portal login redirects to keycloak with the callback the client lists" ;;
		*) ko "portal login redirect is not keycloak with redirect_uri=$callback (got: ${location:-none})" ;;
	esac
	if [ -n "$demo_token" ]; then
		spaces=$(curl -sS --max-time 20 -H "Authorization: Bearer $demo_token" "$portal/api/v1/projects/helsinki/spaces" 2>/dev/null || true)
		if grep -q '"name": *"helsinki"' <<<"$spaces"; then
			ok "portal lists the seeded helsinki space (the configuration repository is seeded)"
		else
			ko "portal does not list the helsinki space: ${spaces:0:160}"
		fi
		# The pipeline test (PL-43): one message through a candidate pipeline on the project's
		# runner, captured on the Portal's internal listener. A 503 here is the runner URL or
		# the capture URL missing from the Portal, or a NetworkPolicy between the two. The viewer
		# may not test a pipeline (403 is the role, not the wiring), so this runs as the steward.
		steward_password="${JC_SMOKE_STEWARD_PASSWORD:-$(kubectl get secret keycloak-user-demo-steward -n "$slug" -o jsonpath='{.data.password}' 2>/dev/null | base64 -d 2>/dev/null || true)}"
		# demo.steward holds mfa-required (T-5120): the password grant wants a TOTP code of the
		# seed beside the password, RFC 6238 HmacSHA256/30 s/6 digits as the realm's OTP policy.
		steward_totp=""
		steward_seed="${JC_SMOKE_STEWARD_TOTP:-$(kubectl get secret keycloak-user-demo-steward -n "$slug" -o jsonpath='{.data.totp}' 2>/dev/null | base64 -d 2>/dev/null || true)}"
		if [ -n "$steward_seed" ]; then
			steward_totp=$(printf '%s' "$steward_seed" | python3 -c 'import hashlib,hmac,struct,sys,time
m=hmac.new(sys.stdin.read().encode(),struct.pack(">Q",int(time.time())//30),hashlib.sha256).digest()
o=m[-1]&15
print(str((struct.unpack(">I",m[o:o+4])[0]&0x7FFFFFFF)%10**6).zfill(6),end="")')
		fi
		steward_token=$(curl -sS --max-time 20 -d grant_type=password \
			-d "client_id=$client_id" --data-urlencode "client_secret@"<(printf %s "$client_secret") \
			-d "username=demo.steward@${org}" --data-urlencode "password@"<(printf %s "$steward_password") \
			${steward_totp:+-d "totp=$steward_totp"} \
			"$idm/realms/$realm/protocol/openid-connect/token" 2>/dev/null |
			sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')
		# Organization → People lists the demo people (T-2688, DEMO.md): read on Person is the
		# steward's, not the viewer's. A missing one is a seed or a Keycloak search that lost them.
		people=$(curl -sS --max-time 20 -H "Authorization: Bearer $steward_token" \
			"$portal/api/v1/organization/people?search=demo.&max=100" 2>/dev/null || true)
		missing=""
		for who in demo.steward demo.viewer demo.approver; do
			case "$people" in *"\"$who@$org\""*) ;; *) missing="$missing $who@$org" ;; esac
		done
		if [ -z "$missing" ]; then ok "People lists the demo people"; else ko "People does not list:$missing"; fi
		ptest='{"pipeline":{"apiVersion":"joinedcontext.com/v1alpha1","kind":"Pipeline","metadata":{"name":"smoke-test","namespace":"helsinki"},"spec":{"class":"auto","period":"60s","source":{"dataSourceRef":{"kind":"DataSource","name":"hsl-citybikes-gbfs"}},"compute":{"kind":"bloblang","bloblang":"root.id = \"urn:ngsi-ld:SmokeProbe:hel.fi:helsinki:1\"\nroot.type = \"SmokeProbe\""},"output":{"type":"SmokeProbe","mode":"upsert"},"targetEndpoint":"urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-all"}},"sample":{"text":"{\"a\":1}","format":"json"}}'
		status 200 "pipeline test runs a candidate on the runner and captures the output" -X POST -H "Authorization: Bearer $steward_token" \
			-H 'Content-Type: application/json' -d "$ptest" "$portal/api/v1/projects/helsinki/pipelines/test"
		# A Check fetches the URL a person typed on the project's runner (MF-39, T-0752), so an
		# address inside the cluster or the node's metadata service must answer as unreachable:
		# records, a status or a body mean the runner's egress lets a typed URL in. The metadata
		# address goes over 443, which the mesh proxy does not carry, so a refusal is the policy's.
		for inside in "https://kubernetes.default.svc/version" \
			"https://169.254.169.254/hetzner/v1/metadata" \
			"http://keycloak-app-keycloakx-http.${slug}.svc/realms/master" \
			"http://context-broker.${slug}.svc:8080/ngsi-ld/v1/types"; do
			probe=$(curl -sS --max-time 60 -X POST -H "Authorization: Bearer $steward_token" -H 'Content-Type: application/json' \
				-d "{\"apiVersion\":\"joinedcontext.com/v1alpha1\",\"kind\":\"DataSource\",\"metadata\":{\"name\":\"smoke-probe\",\"namespace\":\"helsinki\"},\"spec\":{\"type\":\"http\",\"http\":{\"url\":\"$inside\"}}}" \
				"$portal/api/v1/projects/helsinki/datasources?dryRun=All" 2>/dev/null || true)
			said=$(printf '%s' "$probe" | sed -n 's/.*"skipped": *"\([^"]*\)".*/\1/p')
			# A private or link-local host is refused before anything runs (jc-core, PL-07,
			# T-3162): the dry run is red and names why, which is the strongest refusal there is.
			if grep -q '"valid": *false' <<<"$probe" && grep -q "does not connect to the runner's own pod or a private address" <<<"$probe"; then
				said="refused before it runs: a private address (PL-07)"
			fi
			case "$said" in
			"refused before it runs"* | "the feed could not be reached"* | "the feed did not answer within"* | "the feed answered nothing"*)
				ok "a probe of $inside stays outside the cluster: $said" ;;
			"")
				ko "a probe of $inside answered no refusal: ${probe:0:160}" ;;
			*)
				ko "a probe of $inside reached it: $said" ;;
			esac
		done
		# A take cleans up after itself (T-0667, T-0750): residue is what a journey or a take names as
		# its own, the janitor's pattern (seed/helsinki/helsinki-role-janitor.yaml, T-2627, T-2817),
		# and one of it per kind is a take in flight. What a person made on dev is not residue,
		# whatever its name, so a generated app's endpoint or an upload leaves the check green.
		seed=$(seed_file helsinki)
		for kind in "endpoints Endpoint projects__helsinki__spaces__[a-z0-9-]*__endpoints__[a-z0-9-]*\.yaml" \
			"pipelines Pipeline projects__helsinki__pipelines__[a-z0-9-]*__pipeline\.yaml" \
			"spaces ContextSpace projects__helsinki__spaces__[a-z0-9-]*__space\.yaml"; do
			set -- $kind
			plural=$1 manifest_kind=$2 seed_pattern=$3
			if ! grep -q "$seed_pattern" <<<"$seed"; then
				skip "helsinki $plural residue (no Helsinki seed in this instance)"
				continue
			fi
			names=$(curl -sS --max-time 20 -H "Authorization: Bearer $demo_token" "$portal/api/v1/projects/helsinki/$plural" 2>/dev/null |
				python3 -c 'import json, sys
try:
    items = json.load(sys.stdin).get("items") or []
except ValueError:
    items = []
for item in items:
    if item.get("kind") == sys.argv[1]:
        print((item.get("metadata") or {}).get("name", ""))' "$manifest_kind")
			listed=$(printf '%s\n' "$names" | grep -c .)
			residue=$(printf '%s\n' "$names" | grep -Ex "($RESIDUE_NAMES)" | tr '\n' ' ')
			found=$(printf '%s' "$residue" | wc -w)
			if [ "$listed" -eq 0 ]; then
				ko "helsinki lists no $plural: the list did not answer"
			elif [ "$found" -eq 0 ]; then
				ok "helsinki lists $listed $plural, none of them residue of takes or e2e"
			elif [ "$found" -eq 1 ]; then
				ok "helsinki lists $listed $plural, one a take in flight: ${residue% }"
			else
				ko "helsinki lists residue of takes or e2e among its $plural: ${residue% } (T-0667)"
			fi
		done
	else
		skip "portal space list (no demo user token)"
	fi
	if [ -n "$token" ]; then
		# A bearer caller (CLI, service account) passes the edge login untouched (AP-28).
		status 200 "portal API answers a bearer call through the edge" -H "Authorization: Bearer $token" "$portal/api/v1/auth/me"
		status 401 "portal API refuses a tampered token" -H "Authorization: Bearer ${token%.*}.forged" "$portal/api/v1/auth/me"
		# header {"alg":"ES256","typ":"JWT"}, payload {"exp":1}: expired and unsigned at once.
		# Built at run time: a JWT-shaped literal in the repo trips gitleaks.
		b64url() { printf '%s' "$1" | base64 | tr -d '\n=' | tr '+/' '-_'; }
		expired="$(b64url '{"alg":"ES256","typ":"JWT"}').$(b64url '{"exp":1}').AAAA"
		status 401 "portal API refuses an expired token" -H "Authorization: Bearer $expired" "$portal/api/v1/auth/me"
	else
		skip "portal API bearer call (no token)"
	fi
else
	skip "portal API (route portal-api not configured in this instance)"
fi

echo "context endpoint"
if has_route context-endpoint; then
	# The slug comes from the Endpoint the forge bootstrap seeds (T-0263/T-0277, T-0278: the
	# gateway serves the forge's checkout), the same way the route table comes from APISIX: a
	# slug written into this script would drift from the seed, and a made-up one cannot exist
	# (slugs are 26+ base32 characters). A seed key is the repository path with `/` as `__`.
	ep="${JC_SMOKE_ENDPOINT:-$(seed_file banskabystrica "projects__banskabystrica__spaces__ovzdusie__endpoints__public-air\.yaml" | sed -n 's/^ *slug: *//p' | head -1)}"
	# The writes go to the conformance space's own Endpoint (F-BNT1-002), never to public-air,
	# whose space holds the published EEA readings; public-air is read below, anonymously.
	cep="${JC_SMOKE_WRITE_ENDPOINT:-$(seed_file banskabystrica "projects__banskabystrica__spaces__conformance__endpoints__conformance\.yaml" | sed -n 's/^ *slug: *//p' | head -1)}"
	# Writes are bound to the conformance ServiceAccount (GW22): its client_credentials token
	# carries the slug as audience and an azp the gateway resolves to the manifest (PF-45, PF-46).
	# The apisix-gateway token above is minted for the Portal and names no account here.
	ep_client="${JC_SMOKE_ENDPOINT_CLIENT_ID:-banskabystrica-conformance}"
	ep_secret="${JC_SMOKE_ENDPOINT_CLIENT_SECRET:-$(kubectl get secret "keycloak-client-$ep_client" -n "$slug" -o jsonpath='{.data.client-secret}' 2>/dev/null | base64 -d 2>/dev/null || true)}"
	ep_token=""
	if [ -z "$ep" ] || [ -z "$cep" ]; then
		skip "endpoint round trip (no Endpoint seeded on the gateway in this instance)"
	elif [ -n "${JC_SMOKE_NO_TEST_WRITER:-}" ]; then
		# A production instance holds no test account (T-4678): its pipelines' writes are what the
		# entity checks below read back.
		skip "endpoint round trip (this instance has no test writer by design)"
	elif [ -z "$ep_secret" ]; then
		ko "no client secret for $ep_client (set JC_SMOKE_ENDPOINT_CLIENT_SECRET or point KUBECONFIG at the instance)"
	else
		ep_token=$(curl -sS --max-time 20 -d grant_type=client_credentials -d "client_id=$ep_client" \
			--data-urlencode "client_secret@"<(printf %s "$ep_secret") \
			"$idm/realms/$realm/protocol/openid-connect/token" 2>/dev/null |
			sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')
		if [ -n "$ep_token" ]; then ok "client_credentials token for $ep_client"; else ko "no access_token for $ep_client in realm $realm"; fi
	fi
	if [ -n "$ep_token" ]; then
		# application/json, so the core @context applies; application/ld+json without an @context
		# member is a 400 (CIM 009 6.3.5), which masked every write check as "passing".
		# The first create also creates the tenant on Antares (5.5.10); reads before it are 404.
		urn="urn:ngsi-ld:AirQualityObserved:${org}:conformance:smoke-1"
		body='{"id":"'"$urn"'","type":"AirQualityObserved","dateObserved":{"type":"Property","value":"2026-01-01T00:00:00Z"}}'
		status 201 "entity create through the endpoint" -X POST -H 'Content-Type: application/json' \
			-H "Authorization: Bearer $ep_token" -d "$body" "$api/api/endpoint/$cep/ngsi-ld/v1/entities"
		status 200 "entity read back" -H "Authorization: Bearer $ep_token" "$api/api/endpoint/$cep/ngsi-ld/v1/entities/$urn"
		# An id that is no NGSI-LD URN is refused by the gateway, not stored (PF-43).
		bad='{"id":"smoke-not-a-urn","type":"AirQualityObserved","dateObserved":{"type":"Property","value":"2026-01-01T00:00:00Z"}}'
		status 400 "write with an id that is no NGSI-LD URN is refused" -X POST -H 'Content-Type: application/json' \
			-H "Authorization: Bearer $ep_token" -d "$bad" "$api/api/endpoint/$cep/ngsi-ld/v1/entities"
		# A URN naming another organization is an id like any other since ADR-N-041: it lands in
		# this endpoint's space and is removed again, so dev keeps nothing (a run before the rule
		# changed may have left it: it is deleted first).
		# A delete names why (JC-Reason, T-4666): without one the gateway answers 400.
		why="JC-Reason: smoke%20test%20cleanup"
		other="urn:ngsi-ld:AirQualityObserved:someone-else.sk:conformance:smoke-2"
		curl -sS -o /dev/null --max-time 20 -X DELETE -H "$why" -H "Authorization: Bearer $ep_token" \
			"$api/api/endpoint/$cep/ngsi-ld/v1/entities/$other" 2>/dev/null || true
		status 201 "an id naming another organization lands in this space (ADR-N-041)" -X POST -H 'Content-Type: application/json' \
			-H "Authorization: Bearer $ep_token" -d '{"id":"'"$other"'","type":"AirQualityObserved","dateObserved":{"type":"Property","value":"2026-01-01T00:00:00Z"}}' "$api/api/endpoint/$cep/ngsi-ld/v1/entities"
		status 204 "and is deleted again" -X DELETE -H "$why" -H "Authorization: Bearer $ep_token" "$api/api/endpoint/$cep/ngsi-ld/v1/entities/$other"
		status 204 "entity delete" -X DELETE -H "$why" -H "Authorization: Bearer $ep_token" "$api/api/endpoint/$cep/ngsi-ld/v1/entities/$urn"

		# T-0945: what the space lists, the space serves. An entity whose URN carries another
		# organization's domain lists (the broker holds it) and answers 400 on retrieve (the
		# gateway checks the prefix, GW20), so a steward can neither read nor correct it. A
		# round trip of the script's own entity would never see that: the rows that drift are
		# the ones somebody else wrote. Read as the public reads public-air: the conformance token
		# is bound to its own Endpoint alone (F-BNT1-002).
		listed=$(curl -sS --max-time 20 \
			"$api/api/endpoint/$ep/ngsi-ld/v1/entities?type=AirQualityObserved&limit=50" 2>/dev/null |
			tr ',' '\n' | sed -n 's/.*"id":"\(urn:ngsi-ld:[^"]*\)".*/\1/p' | sort -u)
		if [ -z "$listed" ]; then
			skip "every listed entity retrieves by id (the space holds none)"
		else
			unreadable=""
			for one in $listed; do
				got=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 20 \
					"$api/api/endpoint/$ep/ngsi-ld/v1/entities/$one" 2>/dev/null) || got="curl-error"
				[ "$got" = "200" ] || unreadable="$unreadable $one($got)"
			done
			if [ -z "$unreadable" ]; then
				ok "every listed entity retrieves by id ($(printf '%s\n' "$listed" | wc -l) of them)"
			else
				ko "listed but not retrievable, so no steward can read or correct them:$unreadable"
			fi
		fi
		# T-4372: what the public reads now it reads over time, anonymously, from the history the
		# space keeps; the window is capped to that history by the public-read Policy.
		since=$(date -u -d '-1 day' +%Y-%m-%dT%H:%M:%SZ)
		status 200 "an anonymous visitor reads the air history of public-air" \
			"$api/api/endpoint/$ep/ngsi-ld/v1/temporal/entities?type=AirQualityObserved&timerel=after&timeAt=$since&lastN=1&limit=1"
	fi
else
	skip "endpoint round trip (route context-endpoint not configured in this instance)"
fi

echo "helsinki space"
# DEMO.md steps 3 and 4 on the shared `helsinki` space: four public endpoints over one space,
# three of them bound by policyRef to one entity type, the fourth to everything; every one
# answers NGSI-LD, MCP and the model's LinkML; the streams of the resident runner fill it. The
# slugs come from the forge's seed the way the ovzdusie one does above.
if has_route context-endpoint; then
	slug_of() { seed_file helsinki "projects__helsinki__spaces__helsinki__endpoints__helsinki-$1\.yaml" | sed -n 's/^ *slug: *//p' | head -1; }
	all=$(slug_of all)
	if [ -z "$all" ]; then
		skip "helsinki endpoints (no Helsinki seed on the gateway in this instance)"
	else
		mcp='{"jsonrpc":"2.0","id":1,"method":"tools/list"}'
		for name in events bikes transport all; do
			hs=$(slug_of "$name")
			granted=$(type_of "$hs")
			if [ -z "$granted" ]; then
				ko "helsinki-$name names no type in its access document"
				continue
			fi
			status 200 "helsinki-$name serves NGSI-LD" "$api/api/endpoint/$hs/ngsi-ld/v1/entities?type=$granted&limit=1"
			status 200 "helsinki-$name serves the LinkML model" "$api/api/endpoint/$hs/schema/v1/linkml"
			status 200 "helsinki-$name answers its DCAT record on the api host" "$api/api/endpoint/$hs/"
			status 200 "helsinki-$name answers an MCP tools/list" -X POST -H 'Content-Type: application/json' \
				-H 'Accept: application/json, text/event-stream' -d "$mcp" "$api/api/endpoint/$hs/mcp"
		done
		# EP-51, T-2262: a schema artifact is revalidated, not downloaded again. The gateway sends
		# `private, no-cache` and a strong ETag, and the edge no longer replaces that with
		# `no-store`; the second read with the ETag it answered is a 304 with no body.
		artifact="$api/api/endpoint/$all/schema/v1/linkml"
		first=$(curl -sS -D - -o /dev/null --max-time 20 "$artifact" 2>/dev/null | tr -d '\r' || true)
		etag=$(printf '%s\n' "$first" | sed -n 's/^[Ee][Tt][Aa][Gg]: *//p' | head -1)
		cache=$(printf '%s\n' "$first" | sed -n 's/^[Cc]ache-[Cc]ontrol: *//p' | head -1)
		if [ "$cache" = "private, no-cache" ]; then
			ok "a schema artifact may be kept and revalidated by its reader ($cache)"
		else
			ko "a schema artifact says Cache-Control '${cache:-none}', expected 'private, no-cache'"
		fi
		if [ -n "$etag" ]; then
			status 304 "a second read of a schema artifact with its ETag is a 304" -H "If-None-Match: $etag" "$artifact"
		else
			ko "a schema artifact carries no ETag to revalidate with"
		fi
		# The slice is real: a type the endpoint's Policy does not grant is an empty answer, not
		# the neighbour's data (EP-14, 404-vs-403 narrowing keeps it a 200).
		body=$(curl -sS --max-time 20 "$api/api/endpoint/$(slug_of bikes)/ngsi-ld/v1/entities?type=Event&limit=1" 2>/dev/null || true)
		if [ "$body" = "[]" ]; then ok "helsinki-bikes does not serve events"; else ko "helsinki-bikes served events: ${body:0:120}"; fi
		# T-4372: a type that keeps no history grants no history read; the temporal query is a 404.
		status 404 "helsinki-events serves no history (Event keeps none)" \
			"$api/api/endpoint/$(slug_of events)/ngsi-ld/v1/temporal/entities?type=Event&timerel=after&timeAt=$(date -u -d '-1 day' +%Y-%m-%dT%H:%M:%SZ)"
		# T-4035, EP-106/107: the transport endpoint's WebSocket is public like its REST read. An
		# anonymous client (no Origin, no token) gets its 101 and a subscribe for Vehicle its 201.
		ws=$(python3 "$(dirname "$0")/smoke-ws.py" "$api/api/endpoint/$(slug_of transport)/ws" Vehicle 20 2>&1 || true)
		if [ "$ws" = "101 201" ]; then
			ok "helsinki-transport opens its WebSocket anonymously and subscribes to Vehicle (101 201)"
		else
			ko "helsinki-transport WebSocket answered '${ws:0:160}', expected '101 201'"
		fi
		# Data flows: the bike feed answers within a minute of the runner starting, the events
		# register at once; the buses depend on HSL's timetable, so their absence is a note.
		flowing() { # type seconds
			deadline=$(( $(date +%s) + $2 ))
			while :; do
				body=$(curl -sS --max-time 20 "$api/api/endpoint/$all/ngsi-ld/v1/entities?type=$1&limit=1" 2>/dev/null || true)
				case "$body" in "[{"*) return 0 ;; esac
				[ "$(date +%s)" -ge "$deadline" ] && return 1
				sleep 10
			done
		}
		if flowing BikeHireDockingStation 120; then ok "city bike stations are flowing into the space"; else ko "no BikeHireDockingStation entity after 120 s"; fi
		if flowing Event 60; then ok "events are flowing into the space"; else ko "no Event entity after 60 s"; fi
		if flowing Vehicle 30; then ok "buses are flowing into the space"; else skip "no Vehicle entity yet (HSL's trunk lines may be off the road at this hour)"; fi
		# T-4823: a vehicle and a Fintraffic Alert leave by their own lifetime (`expiresAt`,
		# NGSI-LD 4.22), which the broker hides from every read once it passes; no reaper pipeline
		# deletes them. Each one read carries it: a vehicle's five minutes after its position, an
		# Alert's no later than its validTo. One without it would stay after its feed went quiet.
		lifetime=$(curl -sS --max-time 20 "$api/api/endpoint/$all/ngsi-ld/v1/entities?type=Vehicle,Alert&limit=1000&options=sysAttrs" 2>/dev/null |
			python3 -c '
import json, sys
from datetime import datetime, timedelta
def at(s):
    return datetime.fromisoformat(str(s).replace("Z", "+00:00"))
def value(e, name):
    a = e.get(name)
    return a.get("value") if isinstance(a, dict) else a
try:
    entities = json.load(sys.stdin)
except ValueError:
    print("ko the space did not answer with entities"); sys.exit()
bad, read = [], 0
for e in entities if isinstance(entities, list) else []:
    if e.get("type") == "Alert" and value(e, "alertSource") != "Fintraffic":
        continue
    read += 1
    try:
        if e.get("type") == "Vehicle":
            fine = at(e["expiresAt"]) - at(e["location"]["observedAt"]) <= timedelta(minutes=5)
        else:
            fine = "expiresAt" in e and (value(e, "validTo") is None or at(e["expiresAt"]) <= at(value(e, "validTo")))
    except (KeyError, TypeError, ValueError):
        fine = False
    if not fine:
        bad.append(e.get("id", "?"))
print(f"ko {len(bad)} of {read} carry no lifetime, e.g. {bad[0]}" if bad else f"ok {read}")
' 2>/dev/null || true)
		case "$lifetime" in
			"ok 0") skip "no vehicle or Fintraffic Alert to check the lifetime of" ;;
			ok*) ok "every vehicle and Fintraffic Alert carries its expiresAt (${lifetime#ok })" ;;
			"") ko "the lifetime check of vehicles and Alerts read nothing" ;;
			*) ko "vehicles and Alerts: ${lifetime#ko }" ;;
		esac
	fi
else
	skip "helsinki endpoints (route context-endpoint not configured in this instance)"
fi

echo "ogc api features"
# T-3957 (T-3831's dev row): one row per Endpoint kind of Antares' OGC matrix, through the
# gateway, whose OGC surface is the broker's view since platform T-3752. Each row reads
# /conformance and one page of the first collection, and names the seeded Endpoint it read.
# A kind no seeded Endpoint has is a skip naming it.
if has_route context-endpoint; then
	seed_slug() { # project space endpoint
		seed_file "$1" "projects__$1__spaces__$2__endpoints__$3\.yaml" | sed -n 's/^ *slug: *//p' | head -1
	}
	ogc_row() { # kind endpoint-name slug hidden-attribute|"" bearer|""
		local kind="$1" name="$2" ep="$3" hidden="$4" auth=() root conf coll head type body verdict
		if [ -z "$ep" ]; then skip "ogc $kind ($name is not seeded in this instance)"; return; fi
		[ -n "$5" ] && auth=(-H "Authorization: Bearer $5")
		root="$api/api/endpoint/$ep/ogc/features"
		conf=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 20 "${auth[@]}" "$root/conformance" 2>/dev/null) || conf="curl-error"
		if [ "$conf" != "200" ]; then ko "ogc $kind ($name $ep): /conformance answered $conf"; return; fi
		# The collections are the view's LinkML classes (T-3752), and a class may hold no entity in
		# this space: the row reads the first collection that has a feature, else the first one.
		local ids c
		ids=$(curl -sS --max-time 20 "${auth[@]}" "$root/collections" 2>/dev/null |
			python3 -c 'import json,sys; print("\n".join(c["id"] for c in json.load(sys.stdin).get("collections", [])[:40]))' 2>/dev/null)
		coll=$(printf '%s\n' "$ids" | head -1)
		if [ -z "$coll" ]; then ko "ogc $kind ($name $ep): /collections lists none"; return; fi
		for c in $ids; do
			if curl -sS --max-time 20 "${auth[@]}" "$root/collections/$c/items?limit=1" 2>/dev/null |
				python3 -c 'import json,sys; sys.exit(0 if json.load(sys.stdin).get("features") else 1)' 2>/dev/null; then
				coll=$c
				break
			fi
		done
		head=$(curl -sS -D - -o /dev/null --max-time 20 "${auth[@]}" "$root/collections/$coll/items?limit=1" 2>/dev/null | tr -d '\r')
		type=$(printf '%s\n' "$head" | sed -n 's/^[Cc]ontent-[Tt]ype: *//p' | head -1)
		case "$head" in "HTTP/"*" 200"*) ;; *) ko "ogc $kind ($name $ep): items of $coll answered ${head%%$'\n'*}"; return ;; esac
		case "$type" in application/geo+json*) ;; *) ko "ogc $kind ($name $ep): items of $coll are '${type:-no type}', not application/geo+json"; return ;; esac
		body=$(curl -sS --max-time 20 "${auth[@]}" "$root/collections/$coll/items?limit=1" 2>/dev/null)
		verdict=$(printf '%s' "$body" | python3 -c '
import json, sys
hidden = sys.argv[1]
try:
    features = json.load(sys.stdin).get("features") or []
except (ValueError, AttributeError):
    sys.exit("no FeatureCollection")
if not features:
    sys.exit("no feature")
if hidden and any(hidden in (f.get("properties") or {}) for f in features):
    sys.exit("the hidden " + hidden + " is served")
print(len(features))' "$hidden" 2>&1) || { ko "ogc $kind ($name $ep): items of $coll: $verdict"; return; }
		ok "ogc $kind ($name $ep): conformance, $verdict feature of $coll as geo+json${hidden:+, no $hidden}"
	}
	ogc_row public helsinki-bikes "$(seed_slug helsinki helsinki helsinki-bikes)" "" ""
	if [ -n "$demo_token" ]; then
		ogc_row internal helsinki-bikes-ops "$(seed_slug helsinki helsinki helsinki-bikes-ops)" "" "$demo_token"
	else
		skip "ogc internal (no demo user token to read helsinki-bikes-ops with)"
	fi
	skip "ogc filtered view (no seeded Endpoint narrows its view by a q)"
	ogc_row star public-air "$(seed_slug banskabystrica ovzdusie public-air)" "" ""
	ogc_row projection helsinki-all "$(seed_slug helsinki helsinki helsinki-all)" contactPoint ""
	skip "ogc cube (no seeded Endpoint serves a LinkML cube, T-3805)"
else
	skip "ogc api features (route context-endpoint not configured in this instance)"
fi

echo "forge"
if has_route gitea-forge; then
	# Gitea listens at the root; a 200 here proves both halves of the prefix — the gateway
	# rewrite that takes /git off, and the ROOT_URL that put it there.
	status 200 "git forge answers under the /git prefix" "$base/git/api/healthz"
	# A person signed in with Keycloak reads the configuration repository (PF-79). Three
	# things make that true and each fails silently: the team exists, it reads, and the
	# forge's OpenID Connect source maps the realm group onto it. Checked with the
	# administrator credential the bootstrap Job already uses, in-cluster.
	forge_admin_user=$(kubectl get secret gitea-admin-credentials -n "$slug" -o jsonpath='{.data.username}' 2>/dev/null | base64 -d 2>/dev/null || true)
	forge_admin_pw=$(kubectl get secret gitea-admin-credentials -n "$slug" -o jsonpath='{.data.password}' 2>/dev/null | base64 -d 2>/dev/null || true)
	forge_org=$(kubectl get job gitea-bootstrap -n "$slug" -o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="ORG")].value}' 2>/dev/null || true)
	forge_org="${forge_org:-joinedcontext}"
	if [ -n "$forge_admin_pw" ]; then
		teams=$(curl -sS --max-time 20 -u "$forge_admin_user:$forge_admin_pw" \
			"$base/git/api/v1/orgs/$forge_org/teams" 2>/dev/null || true)
		if grep -q '"name": *"readers"' <<<"$teams"; then
			ok "the forge has the readers team a signed-in person lands in"
		else
			ko "the forge has no readers team: every Keycloak login meets a 404 on the private repository"
		fi
		# What a team may do is its unit map: Gitea computes the summary `permission` from
		# it and answers "none" for any map it did not mint itself, so a team that reads
		# the code unit looks unprivileged there (it read "none" on dev while the readers
		# team was reading perfectly well).
		units=$(printf '%s' "$teams" | tr ',' '\n')
		if grep -q '"repo.code": *"read"' <<<"$units"; then
			ok "a forge team reads the configuration repository (PF-79)"
		else
			ko "no forge team carries read permission"
		fi
		# A team that writes is a project's `{slug}-writers` (PF-87): it holds its project's
		# repository and nothing else. None may write the configuration repository, whose merges
		# are the Portal's (PF-80), nor every repository at once.
		writing=$(printf '%s' "$teams" | python3 -c 'import json, sys
for t in json.load(sys.stdin):
    if (t.get("units_map") or {}).get("repo.code") in ("write", "admin"):
        print(t["id"], "all" if t.get("includes_all_repositories") else "listed", t["name"])' 2>/dev/null || true)
		wrong=""
		while read -r team_id scope team_name; do
			[ -n "$team_id" ] || continue
			if [ "$scope" = all ]; then
				wrong="$wrong $team_name"
				continue
			fi
			team_repos=$(curl -sS --max-time 20 -u "$forge_admin_user:$forge_admin_pw" \
				"$base/git/api/v1/teams/$team_id/repos" 2>/dev/null || true)
			if grep -qE '"name": *"configuration"' <<<"$team_repos"; then
				wrong="$wrong $team_name"
			fi
		done <<<"$writing"
		if [ -n "$wrong" ]; then
			ko "forge team(s)$wrong write the configuration repository: merging is the Portal's (PF-80)"
		else
			ok "no forge team writes the configuration repository, so merging stays the Portal's approval (PF-80)"
		fi
		# The Actions runner (ADR-N-028): registered in the organization and online. It
		# registers again before every job, so an empty list is waited for, and the wait is
		# printed (T-0459's pattern), not hidden.
		# The runners register in the organization that holds the App repositories, its own one
		# when `appsOrganization` is set (T-2969); the bootstrap records it beside the token as
		# `owner` (a name, never the token), and the configuration's organization otherwise.
		runner_org=$(kubectl get secret gitea-runner-registration -n "$slug" -o jsonpath='{.data.owner}' 2>/dev/null | base64 -d 2>/dev/null || true)
		runner_org="${runner_org:-$forge_org}"
		if kubectl get deployments -A --field-selector metadata.name=gitea-runner -o name 2>/dev/null | grep . >/dev/null; then
			waited=0
			while :; do
				runners=$(curl -sS --max-time 20 -u "$forge_admin_user:$forge_admin_pw" \
					"$base/git/api/v1/orgs/$runner_org/actions/runners" 2>/dev/null || true)
				grep -q '"status": *"online"' <<<"$runners" && break
				[ "$waited" -ge "${JC_SMOKE_RUNNER_WAIT:-60}" ] && break
				sleep 5
				waited=$((waited + 5))
			done
			if grep -q '"status": *"online"' <<<"$runners"; then
				ok "an Actions runner is registered in $runner_org and online (waited ${waited}s)"
			else
				ko "no Actions runner of $runner_org is online after ${waited}s: no application builds"
			fi
		else
			skip "actions runner (no gitea-runner in this instance)"
		fi
	else
		skip "forge teams (no gitea-admin-credentials in $slug)"
	fi
	# The repository stays private: without a session the file is not served (PF-79).
	status 404 "the configuration repository is not readable without a session" \
		"$base/git/$forge_org/configuration/raw/branch/main/platform-settings.yaml"
else
	skip "git forge (route gitea-forge not configured in this instance)"
fi

echo "portal bundle"
if has_route portal-public-assets; then
	# T-4201: the public catalogue page names its content-hashed chunks; one is kept a year.
	catalogue_page=$(curl -sS --max-time 20 "$portal/catalogue" 2>/dev/null || true)
	immutable "a Portal chunk is cached for a year" "$portal$(grep -o '/assets/[^"]*\.js' <<<"$catalogue_page" | head -1)"
else
	skip "portal bundle (route portal-public-assets not configured in this instance)"
fi

echo "documentation"
if has_route portal-ui; then
	# T-6200: the API chapter and the User Guide are for signed-in people only; /docs/* has no
	# route of its own, so portal-ui's login answers a visitor before the Portal is reached.
	guide=$(curl -sS --max-time 20 "$portal/docs/API/07-websocket" 2>/dev/null || true)
	case $guide in
	*'<title>Live Updates over WebSocket</title>'*) ko "the WebSocket guide reads without signing in" ;;
	*) ok "the WebSocket guide does not read without signing in" ;;
	esac
	status 302 "a visitor asking for a guide page is sent to sign in" "$portal/docs/User-Guide/01-getting-started"
fi

echo "catalogue"
if has_route ckan; then
	# The catalogue has a host of its own, derived the way the route derives it.
	catalogue="${JC_CATALOGUE_URL:-https://opendata.${base#https://}}"
	status 200 "catalogue front page over valid TLS" "$catalogue/"
	status 200 "catalogue Action API answers" "$catalogue/api/3/action/status_show"
	# The theme is the whole point of OPS-47: the front page has to carry the instance's
	# own name and its own primary colour, and neither may be a literal in any image.
	name=$(kubectl get configmap portal-branding -n "$slug" \
		-o jsonpath='{.data.branding\.yaml}' 2>/dev/null | sed -n 's/^instanceName: *//p' | tr -d "'\"")
	primary=$(kubectl get configmap portal-branding -n "$slug" \
		-o jsonpath='{.data.branding\.yaml}' 2>/dev/null | sed -n 's/^  primary: *//p' | tr -d "'\"")
	page=$(curl -sS --max-time 20 "$catalogue/" 2>/dev/null || true)
	# T-4201: a fingerprinted stylesheet is kept a year, so a revisit costs no request.
	immutable "a catalogue webasset is cached for a year" "$catalogue$(grep -o '/webassets/[^"]*\.css' <<<"$page" | head -1)"
	if [ -n "$name" ] && grep -qF "$name" <<<"$page"; then
		ok "catalogue front page carries the instance name"
	else
		ko "catalogue front page does not carry the instance name (${name:-<no branding configured>})"
	fi
	if [ -n "$primary" ] && grep -qF -- "--jc-primary: $primary" <<<"$page"; then
		ok "catalogue front page carries the branded primary colour"
	else
		ko "catalogue front page does not carry the branded primary colour (${primary:-<none>})"
	fi
	# The one path on the primary host that leads to the catalogue.
	status 302 "/ckan on the primary host redirects to the catalogue" "$base/ckan"
	# T-4679: the old host answers 301 to the same path and query on opendata.{domain}.
	moved=$(curl -sS -o /dev/null -w '%{http_code} %{redirect_url}' --max-time 20 \
		"https://data.${base#https://}/dataset/x?q=1" 2>/dev/null || echo curl-error)
	if [ "$moved" = "301 https://opendata.${base#https://}/dataset/x?q=1" ]; then
		ok "data.{domain} answers 301 to the same path on opendata.{domain}"
	else
		ko "data.{domain} does not redirect to opendata.{domain} with path and query (got: $moved)"
	fi
	# The publisher's token (T-0493): the catalogue's api-token Job keeps one of the site
	# administrator in `ckan-api-token`. Listing that user's tokens is refused to anonymous, and
	# CKAN reads a token signed with a key it no longer has as anonymous, so a 200 here is a
	# token that survived every restart since it was minted.
	ckan_token=$(kubectl get secret ckan-api-token -n "$slug" -o jsonpath='{.data.token}' 2>/dev/null | base64 -d 2>/dev/null || true)
	ckan_admin=$(kubectl get secret ckan-admin-credentials -n "$slug" -o jsonpath='{.data.username}' 2>/dev/null | base64 -d 2>/dev/null || true)
	if [ -z "$ckan_token" ] || [ -z "$ckan_admin" ]; then
		ko "no CKAN API token in Secret ckan-api-token: the catalogue's api-token Job has not written it (T-0493)"
	elif curl -sS --max-time 20 -H "Authorization: $ckan_token" -H 'Content-Type: application/json' \
		-d "{\"user_id\": \"$ckan_admin\"}" "$catalogue/api/3/action/api_token_list" 2>/dev/null |
		grep -q '"success": *true'; then
		ok "the publisher's CKAN API token authenticates as the site administrator"
	else
		ko "the CKAN API token in ckan-api-token no longer authenticates (T-0493)"
	fi
else
	skip "catalogue (route ckan not configured in this instance)"
fi

echo "network policy"
image="docker.io/busybox:1.38@sha256:dc2d74b28e4cf8984fa52af1f39bc7c3d9c73760b41a74d629f5d11b1ab28616"
# One probe pod, parameterised by the command it runs: probe_overrides <pod-name> <command-json>.
# It has to stay OUT of the mesh: a meshed probe only completes a TCP handshake with its
# own outbound proxy on localhost, so `nc -z` reports success no matter what the policy
# does. The opt-out carries the reason the justify-linkerd-inject-opt-out policy asks for, and the
# probe mounts no API token, which justify-api-token-access refuses without a reason (T-6220).
probe_overrides() {
	cat <<JSON
{
  "metadata": {
    "annotations": {
      "linkerd.io/inject": "disabled",
      "mesh.joinedcontext.com/opt-out-reason": "smoke probe: an unmeshed pod is the point of the NetworkPolicy check"
    }
  },
  "spec": {
    "automountServiceAccountToken": false,
    "securityContext": {"runAsNonRoot": true, "runAsUser": 65534, "seccompProfile": {"type": "RuntimeDefault"}},
    "containers": [{
      "name": "$1",
      "image": "$image",
      "command": $2,${3:+
      $3,}
      "securityContext": {"allowPrivilegeEscalation": false, "readOnlyRootFilesystem": true, "capabilities": {"drop": ["ALL"]}},
      "resources": {"requests": {"cpu": "10m", "memory": "16Mi"}, "limits": {"cpu": "100m", "memory": "64Mi"}}
    }]
  }
}
JSON
}

# Ingress: a pod that carries none of the allowed labels must not reach the database at all.
probe="smoke-netpol-$$"
if kubectl run "$probe" -n "$slug" --rm --attach --restart=Never --quiet --timeout=90s \
	--image="$image" \
	--overrides="$(probe_overrides "$probe" '["nc", "-w", "5", "-z", "postgres-cluster-rw", "5432"]')" \
	>/dev/null 2>&1; then
	ko "an unlabelled pod reached postgres-cluster-rw:5432 — the database NetworkPolicy is not holding"
else
	ok "postgres refuses a pod outside the allowed selectors"
fi

# Egress: the other half of the same default-deny, and the half nothing measured until now.
# `podSelector: {}` with both policy types is what the repository renders
# (defaults/environment/networkpolicies.yaml.gotmpl), but a controller that enforces only
# ingress passes every check above while leaving a compromised pod free to call out — which
# is what happened on this cluster long enough for T-0258 to be written against it. The
# rendered manifest cannot tell the two apart: tests/test_networkpolicies.py asserts the same
# YAML either way, so the difference only exists on a running cluster (OPS-38, SEC-GAP-05).
#
# The probe prints one line per destination it reached and one line to prove it ran at all.
# Without that last line a pod that never started looks exactly like a pod the policy stopped,
# and this check would go quietly green the day the image or the namespace changes.
#
# It settles first. The controller programs a NEW pod's egress chains a few seconds after that
# pod is running, so a probe that calls out the instant it starts measures the gap and not the
# policy: measured here, the same pod spec reaches 1.1.1.1:443 immediately and is refused
# twenty-five seconds later. The loop waits for the refusal and reports how long it took, so a
# window that grows is visible rather than silent; if it never comes the measurement below
# runs anyway and fails, which is the case this check exists for. The window itself is real
# and is T-0459 — it is not what this check is about.
egress="smoke-egress-$$"
# nslookup is the discriminating leg: CoreDNS is up, in this cluster, and reachable the moment
# egress stops being enforced. The name is fully qualified: busybox's nslookup does not walk the
# search list, so `kubernetes.default` is NXDOMAIN from any pod and the leg proved nothing. The 1.1.1.1 leg adds the internet, which a firewall outside
# Kubernetes may also be blocking, so it is a second opinion rather than the evidence.
egress_out=$(kubectl run "$egress" -n "$slug" --rm --attach --restart=Never --quiet --timeout=180s \
	--image="$image" \
	--overrides="$(probe_overrides "$egress" '["sh", "-c", "i=0; while [ $i -lt 18 ]; do nc -w 3 -z 1.1.1.1 443 >/dev/null 2>&1 || break; i=$((i+1)); sleep 4; done; echo SETTLED-AFTER=$((i*4))s; timeout 8 nslookup kubernetes.default.svc.cluster.local >/dev/null 2>&1 && echo REACHED-DNS; nc -w 5 -z 1.1.1.1 443 >/dev/null 2>&1 && echo REACHED-NET; echo PROBE-RAN"]')" \
	2>/dev/null)
reached=$(printf '%s' "$egress_out" | sed -n 's/^REACHED-DNS\r*$/CoreDNS/p; s/^REACHED-NET\r*$/1.1.1.1:443/p' | tr '\n' ' ')
settled=$(printf '%s' "$egress_out" | sed -n 's/^SETTLED-AFTER=\([0-9]*\)s\r*$/\1/p' | head -1)
if ! grep -q PROBE-RAN <<<"$egress_out"; then
	ko "the egress probe never ran, so the egress half of the default-deny was not measured"
elif [ -n "$reached" ]; then
	ko "an unlabelled pod reached ${reached}— the egress half of the default-deny is not holding"
elif [ -z "$settled" ]; then
	# The probe ran and reached nothing, but printed no window. Nothing was measured, so the
	# ceiling below would be compared against an empty string and pass by accident.
	ko "the egress probe printed no settle window, so the length of the gap was not measured"
elif [ "$settled" -gt "$settle_ceiling" ]; then
	ko "the egress settle window was ${settled}s, over the ${settle_ceiling}s ceiling (JC_NETPOL_SETTLE): the gap the hardening page accepts is no longer bounded"
else
	ok "an unlabelled pod reaches neither CoreDNS nor the internet (settled after ${settled}s, ceiling ${settle_ceiling}s)"
fi

# The Actions runner runs an application's untrusted build code (T-1707, AP-81, ADR-N-028): a
# pod with the runner's label reaches the forge, the Portal API and DNS, and never the
# internet. DNS is the positive control, so a probe that could not dial at all never reads as
# "refused"; the settle loop is the egress probe's above, for the same controller window (T-0459).
echo "actions runner egress"
runner_ns=$(kubectl get deployments -A --field-selector metadata.name=gitea-runner -o jsonpath='{.items[0].metadata.namespace}' 2>/dev/null || true)
if [ -z "$runner_ns" ]; then
	skip "actions runner egress (no gitea-runner in this instance)"
else
	build_probe="smoke-runner-$$"
	build_out=$(kubectl run "$build_probe" -n "$runner_ns" --rm --attach --restart=Never --quiet --timeout=180s \
		--image="$image" \
		--overrides="$(probe_overrides "$build_probe" '["sh", "-c", "i=0; while [ $i -lt 18 ]; do nc -w 3 -z 1.1.1.1 443 >/dev/null 2>&1 || break; i=$((i+1)); sleep 4; done; echo SETTLED-AFTER=$((i*4))s; timeout 8 nslookup kubernetes.default.svc.cluster.local >/dev/null 2>&1 && echo REACHED-DNS; nc -w 5 -z 1.1.1.1 443 >/dev/null 2>&1 && echo REACHED-NET; echo PROBE-RAN"]' |
			sed 's/"metadata": {/"metadata": {\n    "labels": {"app.kubernetes.io\/name": "gitea-runner"},/')" \
		2>/dev/null)
	build_settled=$(printf '%s' "$build_out" | sed -n 's/^SETTLED-AFTER=\([0-9]*\)s\r*$/\1/p' | head -1)
	if ! grep -q PROBE-RAN <<<"$build_out"; then
		ko "the runner probe never ran, so the runner's egress was not measured"
	elif grep -q '^REACHED-NET' <<<"$build_out"; then
		ko "a pod with the runner's label reached 1.1.1.1:443: an application's build can reach the internet"
	elif ! grep -q '^REACHED-DNS' <<<"$build_out"; then
		ko "a pod with the runner's label cannot resolve names, so the refusal of the internet proves nothing"
	else
		ok "a runner pod resolves names and reaches no internet address (settled after ${build_settled:-?}s)"
	fi
	# The walls of the runner itself, read from the running pod rather than the render (T-1707
	# steps 2 and 4): an application's build runs as this uid, with this filesystem.
	runner_walls=$(kubectl exec -n "$runner_ns" deploy/gitea-runner -c gitea-runner -- \
		sh -c 'echo UID=$(id -u); [ -e /var/run/secrets/kubernetes.io/serviceaccount/token ] && echo SA-TOKEN; echo WALLS-READ' 2>/dev/null || true)
	runner_uid=$(printf '%s' "$runner_walls" | sed -n 's/^UID=\([0-9]*\)\r*$/\1/p')
	if ! grep -q WALLS-READ <<<"$runner_walls"; then
		ko "the runner could not be asked for its uid and token, so its walls were not measured"
	elif [ -z "$runner_uid" ] || [ "$runner_uid" = 0 ]; then
		ko "the runner runs as uid ${runner_uid:-?}: an application's build runs as root"
	elif grep -q '^SA-TOKEN' <<<"$runner_walls"; then
		ko "the runner holds a Kubernetes ServiceAccount token an application's build can read"
	else
		ok "the runner runs as uid $runner_uid and holds no Kubernetes token"
	fi
fi

# The edge's data plane admits the ingress controller alone (T-0939, EP-20). APISIX trusts
# X-Real-IP from the whole pod network, so any other pod reaching 9080 — or 4143, where meshed
# traffic lands — could name itself any caller and spend that caller's rate limit. The probe
# runs in `default`, a namespace no policy of ours governs, so its own egress lets it try: what
# refuses it can only be APISIX's ingress.
edge_peer="smoke-edgepeer-$$"
# The pod's own address: the Service publishes only 80, and an unlabelled pod in `default` is
# refused CoreDNS on purpose (above), so a name would never resolve and the policy on the pod's
# ports 9080 and 4143 is what this measures.
# The sleep keeps the pod alive until kubectl run has attached; a probe that prints and exits
# at once leaves --attach nothing to read.
edge_host=$(kubectl get pods -n "$slug" -l app.kubernetes.io/name=apisix -o jsonpath='{.items[0].status.podIP}' 2>/dev/null)
edge_out=$(kubectl run "$edge_peer" -n default --rm --attach --restart=Never --quiet --timeout=90s \
	--image="$image" \
	--overrides="$(probe_overrides "$edge_peer" "[\"sh\", \"-c\", \"sleep 3; echo PROBE-RAN; nc -w 5 -z $edge_host 9080 >/dev/null 2>&1 && echo REACHED-9080; nc -w 5 -z $edge_host 4143 >/dev/null 2>&1 && echo REACHED-4143; true\"]")" \
	2>/dev/null)
edge_reached=$(printf '%s' "$edge_out" | sed -n 's/^REACHED-\([0-9]*\)\r*$/\1/p' | tr '\n' ' ')
if [ -z "$edge_host" ]; then
	ko "no APISIX pod in $slug, so who may reach APISIX was not measured"
elif ! grep -q PROBE-RAN <<<"$edge_out"; then
	ko "the edge-peer probe never ran, so who may reach APISIX was not measured"
elif [ -n "$edge_reached" ]; then
	ko "a pod outside the ingress controller reached APISIX on port ${edge_reached} — it could set X-Real-IP and spend another caller's rate limit (T-0939)"
else
	ok "APISIX refuses a pod that is not the ingress controller on 9080 and 4143"
fi

# A pod-backed App (AP-26, AP-108): the Portal's reconciler writes one NetworkPolicy per App
# that admits APISIX on the app port and the mesh's inbound port, and nothing else. The first
# running App pod stands for all of them (every App gets the same policy); its own address,
# so neither probe needs DNS. The APISIX-labelled probe is the positive control, retried for
# the half minute the controller takes to admit a new pod (T-0459); it carries both labels the
# edge's egress policy selects (components/apisix/networkpolicies.yaml), since with the name alone
# the namespace's default-deny drops it before it leaves (T-2667); the one in `default` is
# the refusal, on both ports, as for the edge above.
echo "apps"
app_ips=$(kubectl get pods -A -l joinedcontext.com/app=true --field-selector=status.phase=Running \
	-o jsonpath='{range .items[*]}{.status.podIP}{" "}{end}' 2>/dev/null)
app_count=$(printf '%s' "$app_ips" | wc -w)
if [ "$app_count" -eq 0 ]; then
	skip "App pods (0 pod-backed Apps run in this instance)"
else
	app_ip=${app_ips%% *}
	app_probe="smoke-app-$$"
	# T-6594: APISIX's two labels are also the selector of Service apisix-gateway, so a Ready
	# probe became one of its endpoints and Traefik sent visitors to busybox, which answered
	# nothing on 9080: the walk's load of / met a 502 at 08:06Z, one second after the probe
	# started. A readiness probe that never passes keeps it out of every Service; the
	# NetworkPolicies select by labels and admit it as before.
	app_edge=$(probe_overrides "$app_probe-edge" "[\"sh\", \"-c\", \"i=0; while [ \$i -lt 10 ]; do nc -w 3 -z $app_ip 8080 && exit 0; i=\$((i+1)); sleep 3; done; exit 1\"]" \
		'"readinessProbe": {"exec": {"command": ["false"]}, "periodSeconds": 300}' |
		sed 's/"metadata": {/"metadata": {\n    "labels": {"app.kubernetes.io\/name": "apisix", "app.kubernetes.io\/instance": "apisix-apisix"},/')
	if kubectl run "$app_probe-edge" -n "$slug" --rm --attach --restart=Never --quiet --timeout=90s \
		--image="$image" --overrides="$app_edge" >/dev/null 2>&1; then
		ok "a pod with APISIX's label reaches an App pod on 8080 (first of $app_count)"
	else
		ko "a pod with APISIX's label cannot reach the App pod at $app_ip:8080, so no App is served and the refusal below proves nothing"
	fi
	app_out=$(kubectl run "$app_probe" -n default --rm --attach --restart=Never --quiet --timeout=90s \
		--image="$image" \
		--overrides="$(probe_overrides "$app_probe" "[\"sh\", \"-c\", \"sleep 3; echo PROBE-RAN; nc -w 5 -z $app_ip 8080 >/dev/null 2>&1 && echo REACHED-8080; nc -w 5 -z $app_ip 4143 >/dev/null 2>&1 && echo REACHED-4143; true\"]")" \
		2>/dev/null)
	app_reached=$(printf '%s' "$app_out" | sed -n 's/^REACHED-\([0-9]*\)\r*$/\1/p' | tr '\n' ' ')
	if ! grep -q PROBE-RAN <<<"$app_out"; then
		ko "the App-peer probe never ran, so who may reach an App pod was not measured"
	elif [ -n "$app_reached" ]; then
		ko "a pod outside APISIX reached the App pod at $app_ip on port ${app_reached} — the App's NetworkPolicy is not holding and its login front can be skipped (AP-26)"
	else
		ok "an App pod refuses a pod that is not APISIX on 8080 and 4143"
	fi
fi

# A forged caller at the broker (T-4875): only the gateway's and the Portal's mesh identities may
# reach its port. The probe
# wears the Portal's pod label, which the NetworkPolicies admit, and stays out of the mesh: the
# Linkerd Server on 9090 is then the only thing that can refuse it. An HTTP status other than 403
# or 101/200 means the proxy did not answer and nothing was measured.
echo "broker callers"
cb_ip=$(kubectl get pods -n "$slug" -l app.kubernetes.io/name=context-broker-broker -o jsonpath='{.items[0].status.podIP}' 2>/dev/null)
if [ -z "$cb_ip" ] || ! kubectl get servers.policy.linkerd.io context-broker-api -n "$slug" >/dev/null 2>&1; then
	skip "broker callers (no meshed context broker with its Server in this instance)"
else
	forger="smoke-forger-$$"
	forge="wget -S -q -O /dev/null -T 5 --header 'x-jc-subject: smoke-forged' --header 'x-jc-roles: admin'"
	# Any 16 bytes in base64 make a handshake key (RFC 6455 4.1); built here, because as a
	# literal gitleaks reads it as an API key.
	ws_key=$(printf '%s' smoke-forged-key | base64)
	forge_overrides=$(probe_overrides "$forger" "[\"sh\", \"-c\", \"sleep 3; echo PROBE-RAN; $forge http://$cb_ip:9090/ngsi-ld/v1/types 2>&1 | sed -n 's/^ *HTTP[^ ]* \\\\([0-9]*\\\\).*/HTTP-\\\\1/p'; $forge --header 'Connection: Upgrade' --header 'Upgrade: websocket' --header 'Sec-WebSocket-Version: 13' --header 'Sec-WebSocket-Key: $ws_key' http://$cb_ip:9090/x/ws 2>&1 | sed -n 's/^ *HTTP[^ ]* \\\\([0-9]*\\\\).*/WS-\\\\1/p'; true\"]" |
		sed 's/"metadata": {/"metadata": {\n    "labels": {"app.kubernetes.io\/name": "portal-portal"},/')
	forge_out=$(kubectl run "$forger" -n "$slug" --rm --attach --restart=Never --quiet --timeout=90s \
		--image="$image" --overrides="$forge_overrides" 2>/dev/null)
	http_code=$(printf '%s' "$forge_out" | sed -n 's/^HTTP-\([0-9]*\)\r*$/\1/p' | head -1)
	ws_code=$(printf '%s' "$forge_out" | sed -n 's/^WS-\([0-9]*\)\r*$/\1/p' | head -1)
	if ! grep -q PROBE-RAN <<<"$forge_out"; then
		ko "the forged-caller probe never ran, so who may speak to the broker was not measured"
	elif [ "$http_code" = 403 ] && [ "$ws_code" = 403 ]; then
		ok "the broker's proxy refuses a forged x-jc-subject from a pod outside the gateway and the Portal (HTTP and WebSocket: 403)"
	elif [ -z "$http_code" ] || [ -z "$ws_code" ]; then
		ko "the forged-caller probe got no answer from the broker at $cb_ip:9090 (HTTP '${http_code}', WebSocket '${ws_code}'), so the refusal was not measured"
	else
		ko "a pod outside the gateway and the Portal reached the broker with a forged subject (HTTP $http_code, WebSocket $ws_code): it can read as anyone (T-4875)"
	fi

	# The second layer (T-6433): the one identity past that proxy outside ANTARES_POLICY_SUBJECT_FROM
	# is the broker's own, admitted for its self-calls. A meshed pod wearing it forges a subject; the
	# broker must drop the headers, so the read names nobody and gets 401, where a believed subject
	# gets 200 or a policy 403. Its proxy never exits, so the pod is run detached, read and deleted.
	forger="smoke-meshforge-$$"
	cb_account=$(kubectl get pods -n "$slug" -l app.kubernetes.io/name=context-broker-broker -o jsonpath='{.items[0].spec.serviceAccountName}' 2>/dev/null)
	meshed_overrides=$(probe_overrides "$forger" "[\"sh\", \"-c\", \"sleep 3; echo PROBE-RAN; $forge 'http://$cb_ip:9090/ngsi-ld/v1/entities?type=Smoke' 2>&1 | sed -n 's/^ *HTTP[^ ]* \\\\([0-9]*\\\\).*/HTTP-\\\\1/p'; echo PROBE-DONE\"]" |
		sed -e '/opt-out-reason/d' -e 's/"linkerd.io\/inject": "disabled",/"linkerd.io\/inject": "enabled"/' \
			-e "s/\"spec\": {/\"spec\": {\n    \"serviceAccountName\": \"$cb_account\",/" \
			-e 's/"metadata": {/"metadata": {\n    "labels": {"app.kubernetes.io\/name": "portal-portal"},/')
	kubectl run "$forger" -n "$slug" --restart=Never --image="$image" --overrides="$meshed_overrides" >/dev/null 2>&1 || true
	forge_out=""
	meshed_deadline=$(($(date +%s) + ${JC_SMOKE_MESHED_WAIT:-90}))
	while :; do
		forge_out=$(kubectl logs "$forger" -n "$slug" -c "$forger" 2>/dev/null || true)
		grep -q PROBE-DONE <<<"$forge_out" && break
		[ "$(date +%s)" -lt "$meshed_deadline" ] || break
		sleep 3
	done
	kubectl delete pod "$forger" -n "$slug" --wait=false >/dev/null 2>&1 || true
	http_code=$(printf '%s' "$forge_out" | sed -n 's/^HTTP-\([0-9]*\)\r*$/\1/p' | head -1)
	if ! grep -q PROBE-DONE <<<"$forge_out"; then
		ko "the meshed forged-caller probe never finished, so who the broker believes was not measured"
	elif [ "$http_code" = 401 ]; then
		ok "the broker drops a forged x-jc-subject from a meshed identity outside its list (401)"
	elif [ -z "$http_code" ] || [ "$http_code" = 403 ]; then
		ko "the meshed forged-caller probe was not admitted past the proxy (HTTP '${http_code}'), so who the broker believes was not measured"
	else
		ko "the broker believed a forged x-jc-subject from a meshed identity outside its list (HTTP $http_code): check ANTARES_POLICY_SUBJECT_FROM (T-6433)"
	fi
fi

# The sample applications (T-2599, AP-75, AP-80): the forge bootstrap pushed each into its own
# repository and committed its manifest, and the build lane builds it on that push. Built means
# the Portal accepted `status.build` for the repository's head. A fresh seed takes minutes, so
# this waits up to JC_SMOKE_APP_WAIT seconds (default 600) and says how long it took (T-0459).
echo "sample apps"
sample_apps=$(kubectl get configmaps -n "$slug" -o name 2>/dev/null | sed -n 's,^configmap/gitea-sample-apps-,,p' | tr '\n' ' ')
app_wait=${JC_SMOKE_APP_WAIT:-600}
if [ -z "${sample_apps% }" ]; then
	skip "sample apps (none is seeded in this instance)"
elif [ -z "$demo_token" ]; then
	skip "sample apps (no demo user token to read them with)"
else
	app_started=$(date +%s)
	pending=${sample_apps% }
	while :; do
		still=""
		for app in $pending; do
			answer=$(curl -sS --max-time 20 -H "Authorization: Bearer $demo_token" \
				"$portal/api/v1/projects/helsinki/apps/$app" 2>/dev/null | tr -d '\n ' || true)
			commit=$(printf '%s' "$answer" | sed -n 's/.*"build":{[^}]*"commit":"\([0-9a-f]\{40\}\)".*/\1/p')
			if [ -n "$commit" ]; then
				ok "sample app $app is built at $(printf '%.12s' "$commit") after $(($(date +%s) - app_started)) s"
			else
				still="$still $app"
			fi
		done
		pending=${still# }
		[ -n "$pending" ] && [ $(($(date +%s) - app_started)) -lt "$app_wait" ] || break
		sleep 10
	done
	for app in $pending; do
		ko "sample app $app has no status.build after $app_wait s: the lane did not build its repository's head"
	done
fi

# A pod-backed App the Portal published but whose pod never became ready (T-2665: every
# fullstack pull failed at the registry's token realm, and dev looked green for eleven hours).
echo "App deployments"
app_deployments=$(kubectl get deployments -A -l joinedcontext.com/app=true \
	-o jsonpath='{range .items[*]}{.metadata.namespace}/{.metadata.name}|{.status.readyReplicas}|{.spec.replicas}{"\n"}{end}' 2>/dev/null || true)
if [ -z "$app_deployments" ]; then
	skip "App deployments (no pod-backed App is published in this instance)"
else
	# '|', not a space: read would fold an absent readyReplicas into the next field.
	while IFS='|' read -r name ready wanted; do
		[ -n "$name" ] || continue
		# readyReplicas is absent, not 0, while no pod is ready; a scaled-down App wants 0.
		case "$wanted" in '' | *[!0-9]*) ready_want=1 ;; *) ready_want=$wanted ;; esac
		case "$ready" in '' | *[!0-9]*) ready=0 ;; esac
		if [ "$ready" -ge "$ready_want" ]; then
			ok "App deployment $name has $ready of $ready_want pods ready"
		else
			ko "App deployment $name has $ready of $ready_want pods ready (kubectl describe pod -l app.kubernetes.io/name=${name#*/} -n ${name%%/*})"
		fi
	done <<<"$app_deployments"
fi

# The routes the Portal adds to the edge file per published App (T-2668, ADR-N-030, ADR-N-037,
# AP-28, AP-29, AP-133). Each App is served on its own host: a public App serves an anonymous
# visitor there; any other sends the visitor to the realm's login with the App's own client
# `app-{name}`. The old path on the apex answers a 308 to the host and sets no cookie.
echo "App routes"
if ! has_route portal-ui; then
	skip "App routes (route portal-ui not configured in this instance)"
elif [ -z "$demo_token" ]; then
	skip "App routes (no demo user token to list the published Apps with)"
else
	published=$(curl -sS --max-time 20 -H "Authorization: Bearer $demo_token" \
		"$portal/api/v1/projects/helsinki/apps" 2>/dev/null | python3 -c '
import json, sys
try:
    items = json.load(sys.stdin).get("items") or []
except Exception:
    sys.exit(0)
for app in items:
    spec = app.get("spec") or {}
    if spec.get("lifecycle") == "published":
        print(app["metadata"]["name"], spec.get("visibility") or "internal")' || true)
	if [ -z "$published" ]; then
		skip "App routes (no App is published in helsinki)"
	fi
	# A public App's map draws on the Portal's basemap route and nothing else (AP-67, AP-12):
	# its page policy has to let the tiles in, or the map is blank without a word (T-3014).
	basemap="$portal/api/v1/projects/helsinki/basemap/"
	basemap_code=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 20 "${basemap}default/style.json" 2>/dev/null || true)
	# An apply that re-vendored an App makes the lane build it again, and its host answers 404 until
	# the Portal fetched that build (T-3424): the host checks wait out a 404, within the sample apps'
	# bound (JC_SMOKE_APP_WAIT, shared by every App), then run as they always did. Any other answer
	# (a login wall's 302, a 5xx) is no publish still to come, and is checked at once.
	host_deadline=$(($(date +%s) + app_wait))
	wait_host() {
		local code
		while :; do
			code=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 20 "$1" 2>/dev/null || true)
			[ "$code" = 404 ] && [ "$(date +%s)" -lt "$host_deadline" ] || return 0
			sleep "${JC_SMOKE_POLL:-10}"
		done
	}
	# csp_allows <policy> <directive> <source>: the directive lists the source as one of its own.
	csp_allows() { tr ';' '\n' <<<"$1" | sed 's/^ *//' | grep -E "^$2( |\$)" | tr ' ' '\n' | grep -xF "$3" >/dev/null; }
	while read -r app visibility; do
		[ -n "$app" ] || continue
		host="https://$app.apps.${base#https://}"
		moved=$(curl -sS -o /dev/null -D - --max-time 20 "$base/apps/$app/" 2>/dev/null | tr -d '\r' || true)
		if grep -qE '^HTTP/[0-9.]+ 308' <<<"$moved" && grep -qix "location: $host/" <<<"$moved" &&
			! grep -qi '^set-cookie:' <<<"$moved"; then
			ok "App $app's old path answers 308 to $host/ with no cookie"
		else
			ko "App $app's old path does not move to its host without a cookie (got: $(head -1 <<<"$moved"))"
		fi
		if [ "$visibility" = public ]; then
			wait_host "$host/"
			status 200 "public App $app answers an anonymous visitor on its own host" "$host/"
			if [ "$basemap_code" = 200 ]; then
				csp=$(curl -sS -o /dev/null -D - --max-time 20 "$host/" 2>/dev/null | tr -d '\r' |
					sed -n 's/^[Cc]ontent-[Ss]ecurity-[Pp]olicy: *//p' | head -1)
				if csp_allows "$csp" img-src "$basemap" && csp_allows "$csp" connect-src "$basemap"; then
					ok "public App $app's policy lets the basemap tiles in ($basemap)"
				else
					ko "public App $app's Content-Security-Policy does not list $basemap in img-src and connect-src, so its map draws no tiles (got: ${csp:-none})"
				fi
			fi
			continue
		fi
		location=$(curl -sS -o /dev/null -w '%{redirect_url}' --max-time 20 "$host/" 2>/dev/null || true)
		case "$location" in
			"$idm/realms/$realm/protocol/openid-connect/auth?"*) ;;
			*) location="" ;;
		esac
		if [ -n "$location" ] && grep -E "[?&]client_id=app-$app(&|\$)" >/dev/null <<<"$location"; then
			ok "$visibility App $app sends an anonymous visitor to its own login (app-$app)"
		else
			ko "$visibility App $app has no route of its own at the edge: an anonymous visit is not sent to the login of client app-$app (got: ${location:-none})"
		fi
	done <<<"$published"
fi

echo "broker surfaces"
# The surfaces the broker mounts (T-3905, T-3922): `/q/health` reports each one the deployment selected,
# so a `ws` missing from ANTARES_API_SURFACES or an image built without it shows here. Health
# needs no credential; the broker is reached over a port-forward like jc-functions below.
if ! kubectl get deployment context-broker -n "$slug" >/dev/null 2>&1; then
	skip "broker surfaces (the context broker is not deployed in this instance)"
else
	cb_port=$((30000 + $$ % 10000))
	kubectl port-forward -n "$slug" svc/context-broker "$cb_port:8080" >/dev/null 2>&1 &
	cb_forward=$!
	cb_health=""
	for _ in 1 2 3 4 5 6 7 8 9 10; do
		cb_health=$(curl -s --max-time 2 "http://127.0.0.1:$cb_port/q/health" 2>/dev/null) && [ -n "$cb_health" ] && break
		sleep 1
	done
	for surface in views mcp access ws ogc ical; do
		if printf '%s' "$cb_health" | python3 -c 'import json,sys; sys.exit(0 if sys.argv[1] in (json.load(sys.stdin).get("surfaces") or {}) else 1)' "$surface" 2>/dev/null; then
			ok "the broker mounts the $surface surface"
		else
			ko "the broker's /q/health lists no $surface surface: ${cb_health:0:160}"
		fi
	done
	kill "$cb_forward" 2>/dev/null
fi

echo "events calendar"
# T-4895, T-5047: the region's and the country's events as a calendar a person subscribes to, read
# anonymously, and no write for anyone but the pipelines. The slugs are the seed's own files.
events_seed="$(dirname "$0")/../components/context-gateway/seed"
for ev_file in bbsk/bbsk-endpoint-podujatia.yaml slovensko/slovensko-endpoint-podujatia.yaml; do
	ev=$(sed -n 's/^  name: *//p' "$events_seed/$ev_file" 2>/dev/null | head -1)
	ev_slug=$(sed -n 's/^ *slug: *//p' "$events_seed/$ev_file" 2>/dev/null | head -1)
	if [ -z "$ev_slug" ]; then
		skip "events calendar $ev_file (no seed manifest beside this script)"
		continue
	fi
	ics=$(curl -sS --max-time 30 "$api/api/endpoint/$ev_slug/ical/v1/events.ics" 2>/dev/null || true)
	if [ "${ics:0:17}" = $'BEGIN:VCALENDAR\r\n' ] && [[ $ics == *$'END:VCALENDAR\r' ]]; then
		ok "$ev serves its events as an iCalendar feed"
	else
		ko "$ev serves no iCalendar feed at /ical/v1/events.ics: ${ics:0:120}"
	fi
	status 403 "$ev refuses an anonymous write" -X POST -H 'Content-Type: application/json' \
		-d '{"id":"urn:ngsi-ld:Event:smoke-anonymous-write","type":"Event","name":{"type":"Property","value":"x"}}' \
		"$api/api/endpoint/$ev_slug/ngsi-ld/v1/entities"
done

echo "functions"
# jc-functions (SDK-22, SDK-23) takes the Portal alone, so the smoke reaches its Service over a
# port-forward and presents what the Portal presents: portal-api's client_credentials token,
# whose audience jc-functions is the realm's mapper. The one function sent reads a public
# Helsinki endpoint anonymously, so its answer is also the runtime's egress to the gateway.
if ! kubectl get deployment jc-functions -n "$slug" >/dev/null 2>&1; then
	skip "functions (jc-functions is not deployed in this instance)"
else
	fn_port=$((20000 + $$ % 10000))
	kubectl port-forward -n "$slug" svc/jc-functions "$fn_port:8080" >/dev/null 2>&1 &
	fn_forward=$!
	for _ in 1 2 3 4 5 6 7 8 9 10; do
		curl -s -o /dev/null --max-time 2 "http://127.0.0.1:$fn_port/healthz" && break
		sleep 1
	done
	status 401 "jc-functions refuses an invocation without the Portal's token" -X POST \
		-H 'Content-Type: application/json' -d '{}' "http://127.0.0.1:$fn_port/invoke"
	# portal-api is federated (PF-47, T-2868): its proof is a token of the Portal pod's own
	# ServiceAccount with the realm as audience, sent without client_id, exactly what the Portal
	# sends. Ten minutes, never stored.
	fn_assertion=$(kubectl create token portal -n "$slug" --audience "$idm/realms/$realm" --duration 10m 2>/dev/null || true)
	fn_token=""
	if [ -n "$fn_assertion" ]; then
		fn_token=$(curl -sS --max-time 20 -d grant_type=client_credentials \
			-d client_assertion_type=urn:ietf:params:oauth:client-assertion-type:jwt-bearer \
			--data-urlencode "client_assertion@"<(printf %s "$fn_assertion") \
			"$idm/realms/$realm/protocol/openid-connect/token" 2>/dev/null |
			sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')
	fi
	fn_slug=$(seed_file helsinki "projects__helsinki__spaces__helsinki__endpoints__helsinki-all\.yaml" | sed -n 's/^ *slug: *//p' | head -1)
	if [ -z "$fn_token" ]; then
		ko "no client_credentials token for portal-api, so no function was invoked"
	elif [ -z "$fn_slug" ]; then
		skip "function invocation (no Helsinki seed on the gateway in this instance)"
	else
		# The same rule as the endpoint probes above: a read names a type, and the type comes
		# from the endpoint's access document rather than from this script's memory (GW33).
		fn_type=$(type_of "$fn_slug")
		fn_type=${fn_type:-Event}
		fn_body=$(printf '{"files":{"@app/functions/smoke.ts":"export default async (request, ctx) => { ctx.log(\\"smoke\\"); const r = await ctx.jc.get(\\"/api/endpoint/%s/ngsi-ld/v1/entities?type='"$fn_type"'&limit=1\\"); return { body: { gateway: r.status } }; };","@joinedcontext/sdk/server":"export const createClient = (config, transport) => ({ get: (path) => transport({ method: \\"GET\\", path }) });"},"entry":"@app/functions/smoke.ts","request":{"method":"POST","query":{},"body":null,"user":null},"config":{"slug":"%s","orgDomain":"smoke","space":"helsinki"}}' "$fn_slug" "$fn_slug")
		fn_answer=$(curl -sS --max-time 20 -X POST -H 'Content-Type: application/json' \
			-H "Authorization: Bearer $fn_token" -d "$fn_body" "http://127.0.0.1:$fn_port/invoke" 2>/dev/null)
		case "$fn_answer" in
			*'"body":{"gateway":200}'*) ok "jc-functions runs a function for the Portal's token and the function reads the gateway" ;;
			*) ko "jc-functions did not answer the smoke function with the gateway's 200: ${fn_answer:-no answer}" ;;
		esac
	fi
	kill "$fn_forward" 2>/dev/null
	# Ingress: the Service's address, so neither probe needs DNS. A probe carrying the Portal's
	# label connects (the policy is what decides, not a pod that cannot dial at all) and an
	# unlabelled one does not.
	fn_ip=$(kubectl get service jc-functions -n "$slug" -o jsonpath='{.spec.clusterIP}' 2>/dev/null)
	fn_probe="smoke-functions-$$"
	# The controller adds a new pod to the allowed sources a few seconds after it starts (T-0459),
	# so the labelled probe retries for half a minute before it counts as refused.
	fn_retry='["sh", "-c", "i=0; while [ $i -lt 10 ]; do nc -w 3 -z IP 8080 && exit 0; i=$((i+1)); sleep 3; done; exit 1"]'
	portal_probe=$(probe_overrides "$fn_probe-portal" "${fn_retry/IP/$fn_ip}" |
		sed 's/"metadata": {/"metadata": {\n    "labels": {"app.kubernetes.io\/name": "portal-portal"},/')
	if kubectl run "$fn_probe-portal" -n "$slug" --rm --attach --restart=Never --quiet --timeout=90s \
		--image="$image" --overrides="$portal_probe" >/dev/null 2>&1; then
		ok "a pod with the Portal's label reaches jc-functions:8080"
	else
		ko "a pod with the Portal's label cannot reach jc-functions:8080, so the refusal below proves nothing"
	fi
	if kubectl run "$fn_probe" -n "$slug" --rm --attach --restart=Never --quiet --timeout=90s \
		--image="$image" \
		--overrides="$(probe_overrides "$fn_probe" "[\"nc\", \"-w\", \"5\", \"-z\", \"$fn_ip\", \"8080\"]")" \
		>/dev/null 2>&1; then
		ko "an unlabelled pod reached jc-functions:8080 — the functions NetworkPolicy is not holding"
	else
		ok "jc-functions refuses a pod that is not the Portal"
	fi
fi

# Who the edge thinks the caller is (T-0929, EP-20, SP-22). APISIX sits behind the ingress
# controller as a ClusterIP, and its chart trusts only 127.0.0.1 for `X-Real-IP`, so until the
# nginx snippet was added every caller on the internet arrived as the ingress pod: one address in
# the access log, and one `limit-count` bucket the first caller emptied for everybody. The access
# log is where that shows, so that is what this reads.
echo "edge caller"
if ! kubectl get deployment apisix -n "$slug" >/dev/null 2>&1; then
	skip "edge caller (no APISIX in this instance)"
else
	# A fixed path, not a per-run one: the access log is read back through `kubectl logs`,
	# and `tail -1` of the matches is this run's.
	beacon="/smoke-caller"
	curl -s -o /dev/null --max-time 10 "$base$beacon" || true
	logged=$(kubectl logs deployment/apisix -n "$slug" -c apisix --tail=200 2>/dev/null |
		grep -F "$beacon" | tail -1 | awk '{print $1}')
	case "$logged" in
	"")
		skip "edge caller (the request did not reach the edge's access log)"
		;;
	10.42.* | 10.43.*)
		ko "the edge logged the caller as $logged, a pod address: every caller shares one rate-limit bucket"
		;;
	*)
		ok "the edge names the caller, not the hop in front of it ($logged)"
		;;
	esac
fi

# The artifact store's per-organization credentials (PF-32, T-0422, T-0925, T-0933). The Secret
# is the one object that proves the whole path: the reconciler reached the store's admin API with
# the root credential, the store accepted the policy and the user, and the reader was handed to
# the namespace that serves. It was missing for as long as the reconciler's egress named the
# store's service port and not its Linkerd inbound port, and nothing here noticed.
echo "artifact store"
if ! kubectl get statefulset artifact-store -n "$slug" >/dev/null 2>&1; then
	skip "artifact store (the component is not deployed in this instance)"
else
	readers=$(kubectl get secret -n "$slug" -l joinedcontext.com/artifact-store-role=reader \
		-o jsonpath='{range .items[*]}{.metadata.name}{" "}{end}' 2>/dev/null || true)
	if [ -z "$readers" ]; then
		ko "no organization has a reader credential: the reconciler minted none or handed none over"
	else
		missing=""
		for reader in $readers; do
			for key in ACCESS_KEY_ID ACCESS_SECRET_KEY; do
				value=$(kubectl get secret "$reader" -n "$slug" -o "jsonpath={.data.$key}" 2>/dev/null || true)
				[ -n "$value" ] || missing="$missing $reader/$key"
			done
		done
		if [ -n "$missing" ]; then
			ko "a reader Secret is missing a key:$missing"
		else
			ok "every organization has its artifact-store reader credential ($readers)"
		fi
	fi
	# The writer publishes; a Secret carrying it would hand a serving pod the power to replace
	# an artifact, which is the one thing the two roles exist to keep apart.
	writers=$(kubectl get secret -n "$slug" -l joinedcontext.com/artifact-store-role=writer \
		-o jsonpath='{range .items[*]}{.metadata.name}{" "}{end}' 2>/dev/null || true)
	if [ -n "$writers" ]; then
		ko "a writer credential was written into the cluster:$writers"
	else
		ok "no writer credential is written anywhere in the cluster"
	fi
fi

# A pipeline's credential (PL-15, CC-06, T-0927, T-0935). The Portal decrypts every `secretRef`
# of every Pipeline with the backend it was given and writes one `pipeline-secrets` Secret into
# the runner's namespace; the runner takes the whole Secret as environment, so a stream reads
# `${MQTT_PASSWORD}` and nothing else ever holds the value. Two properties are checked here and
# nowhere else: the environment reaches the process, and the plaintext is in no ConfigMap.
echo "pipeline credentials"
if ! kubectl get secret pipeline-secrets -n "$slug" >/dev/null 2>&1; then
	skip "pipeline credentials (no pipeline on this instance declares a secretRef)"
else
	variables=$(kubectl get secret pipeline-secrets -n "$slug" \
		-o go-template='{{range $name, $_ := .data}}{{$name}}{{"\n"}}{{end}}' 2>/dev/null || true)
	if [ -z "$variables" ]; then
		ko "pipeline-secrets is empty: every reference was refused, and the runner has nothing"
	else
		missing=""
		for variable in $variables; do
			# The value is never printed: the test is whether the process has it, not what it is.
			kubectl exec -n "$slug" deploy/pipeline-runner -c pipeline-runner-runner -- \
				sh -c "[ -n \"\$$variable\" ]" >/dev/null 2>&1 || missing="$missing $variable"
		done
		if [ -n "$missing" ]; then
			ko "the runner's environment is missing a resolved credential:$missing"
		else
			ok "the runner reads every resolved credential from its environment ($(echo $variables | tr '\n' ' '))"
		fi
		# The other half: what the Secret holds appears in no ConfigMap of the namespace. A
		# credential copied into a rendered config is a credential in `kubectl get -o yaml`.
		leaked=""
		for variable in $variables; do
			value=$(kubectl get secret pipeline-secrets -n "$slug" -o "jsonpath={.data.$variable}" 2>/dev/null | base64 -d 2>/dev/null || true)
			[ -n "$value" ] || continue
			if kubectl get configmap -n "$slug" -o yaml 2>/dev/null | grep -F -- "$value" >/dev/null; then
				leaked="$leaked $variable"
			fi
		done
		if [ -n "$leaked" ]; then
			ko "a resolved credential is written in plaintext into a ConfigMap:$leaked"
		else
			ok "no resolved credential appears in any ConfigMap of $slug"
		fi
	fi
fi

echo "pipeline runner streams (T-4037)"
if [ -z "$runner_restarts_before" ]; then
	skip "pipeline runner streams (no pipeline-runner in this instance)"
else
	# A stream the runner refused at init (a step that does not build, the GTFS-RT schema read
	# twice) never runs and says so only in this log, without its name. The Portal sends a
	# refused render again on every 60 s sync tick, so a refusal that stands is in the last two
	# minutes; one from the apply (the new pod given the old render before the seed converged,
	# T-4055) is older once the smoke has run that long.
	window=${JC_SMOKE_RUNNER_WINDOW:-120}
	ran=$(($(date +%s) - smoke_started))
	[ "$ran" -lt "$window" ] && sleep $((window - ran))
	refused=$(kubectl logs -n "$slug" -l app.kubernetes.io/name=pipeline-runner-runner -c pipeline-runner-runner --since="${window}s" --tail=-1 2>/dev/null |
		grep -oE 'Failed to create stream \([^)]*\)|Streams CRUD Error[^"]{0,80}' | sort -u | head -n 5 || true)
	if [ -z "$refused" ]; then
		ok "the pipeline runner refused no stream in the last ${window} s"
	else
		ko "the pipeline runner refused streams: $(echo $refused)"
	fi
fi

echo "pipeline runner memory (T-3537)"
if [ -z "$runner_restarts_before" ]; then
	skip "pipeline runner memory (no pipeline-runner in this instance)"
else
	runner_restarts_after=$(runner_restarts)
	if [ "$runner_restarts_after" = "$runner_restarts_before" ]; then
		ok "the pipeline runner did not restart during the smoke run ($runner_restarts_before restarts)"
	else
		ko "the pipeline runner restarted during the smoke run ($runner_restarts_before -> ${runner_restarts_after:-gone}); kubectl describe its pod for OOMKilled"
	fi
	# The working set (what the kernel's OOM killer counts) against the container's limit,
	# both in MiB; Bento's GOMEMLIMIT sits at 85 %, so 80 % is the line before the GC fights.
	used=$(kubectl top pod -n "$slug" -l app.kubernetes.io/name=pipeline-runner-runner --containers --no-headers 2>/dev/null |
		awk '$2 == "pipeline-runner-runner" { sub(/Mi$/, "", $4); total += $4 } END { if (NR) print total }')
	limit=$(kubectl get deployment pipeline-runner -n "$slug" \
		-o 'jsonpath={.spec.template.spec.containers[?(@.name=="pipeline-runner-runner")].resources.limits.memory}' 2>/dev/null)
	case "$limit" in
	*Gi) limit_mib=$((${limit%Gi} * 1024)) ;;
	*Mi) limit_mib=${limit%Mi} ;;
	*) limit_mib="" ;;
	esac
	case "$used:$limit_mib" in
	'':* | *: | *[!0-9:]*)
		skip "pipeline runner memory (working set \"${used}\" or limit \"${limit}\" not readable in MiB)"
		;;
	*)
		if [ $((used * 100)) -lt $((limit_mib * 80)) ]; then
			ok "the pipeline runner uses ${used}Mi of its ${limit_mib}Mi limit, under 80 %"
		else
			ko "the pipeline runner uses ${used}Mi of its ${limit_mib}Mi limit, 80 % or more (T-3537)"
		fi
		;;
	esac
fi

# The MQTT edge brokers (T-4857, ADR-N-048): every broker the Portal stamped runs, answers TLS on
# its address with its own host's certificate, and refuses a CONNECT without a token. Plain 1883
# is never reachable from outside. A broker is its Deployment and its host its Certificate's name
# (a custom hostname with the admin's certificate: its route's `HostSNI`). A broker with a
# dedicated port has only its `-port` route, which answers a client that sends no SNI with the same
# certificate (T-5856, T-5871); the others have the 8883 route by `HostSNI`. Certificate doors
# (`-cert`, T-5875) need a client certificate and are not probed here.
# The brokers live where the brokers chart put its `broker-template` (the instance's namespace on
# dev). Fewer running than the seed declares (gitea-seed-* ConfigMaps) fails; neither skips.
echo "mqtt edge brokers (T-4857)"
mqtt_ns="${JC_MQTT_NAMESPACE:-$(kubectl get configmaps -A --field-selector metadata.name=broker-template \
	-o 'jsonpath={.items[*].metadata.namespace}' 2>/dev/null | awk '{ print $1 }' || true)}"
mqtt_seeded=$(kubectl get configmaps -n "$slug" \
	-o go-template='{{range .items}}==cm {{.metadata.name}}{{"\n"}}{{range .data}}{{.}}{{"\n"}}{{end}}{{end}}' 2>/dev/null |
	awk '/^==cm / { seed = ($2 ~ /^gitea-seed-/) } seed && $0 == "kind: MqttBroker" { n++ } END { print n + 0 }' || true)
mqtt_brokers=""
mqtt_routes=""
mqtt_certs=""
if [ -n "$mqtt_ns" ]; then
	mqtt_portal=app.kubernetes.io/managed-by=joinedcontext-portal
	mqtt_brokers=$(kubectl get deployments -n "$mqtt_ns" -l "app.kubernetes.io/name=mqtt-broker,$mqtt_portal" \
		-o go-template='{{range .items}}{{.metadata.name}} {{with .status.readyReplicas}}{{.}}{{else}}0{{end}}{{"\n"}}{{end}}' 2>/dev/null |
		awk '$1 != "" && $1 !~ /-cert$/' || true)
	mqtt_routes=$(kubectl get ingressroutetcp -n "$mqtt_ns" -l "$mqtt_portal" \
		-o go-template='{{range .items}}{{.metadata.name}} {{index .spec.entryPoints 0}} {{(index .spec.routes 0).match}}{{"\n"}}{{end}}' 2>/dev/null |
		tr -d '\140' || true)
	mqtt_certs=$(kubectl get certificates -n "$mqtt_ns" -l "$mqtt_portal" \
		-o go-template='{{range .items}}{{.metadata.name}} {{index .spec.dnsNames 0}}{{"\n"}}{{end}}' 2>/dev/null || true)
fi
mqtt_running=$(printf '%s\n' "$mqtt_brokers" | awk 'NF { n++ } END { print n + 0 }')
if [ "${mqtt_seeded:-0}" -gt "$mqtt_running" ]; then
	ko "mqtt edge brokers: the seed declares ${mqtt_seeded} MqttBroker(s), ${mqtt_running} run in ${mqtt_ns:-no namespace (no broker-template ConfigMap: components/mqtt-brokers is not installed)}"
fi
if [ -z "$mqtt_brokers" ]; then
	[ "${mqtt_seeded:-0}" -gt 0 ] || skip "mqtt edge brokers (no MqttBroker seeded or stamped in this instance)"
else
	while read -r release ready; do
		if [ "${ready:-0}" -ge 1 ]; then ok "mqtt $release runs"; else ko "mqtt $release has no ready replica"; fi
		host=$(printf '%s\n' "$mqtt_certs" | awk -v r="$release" '$1 == r { print $2 }')
		[ -n "$host" ] || host=$(printf '%s\n' "$mqtt_routes" |
			sed -n "s/^$release [^ ]* HostSNI(\([^*)]*\))\$/\1/p")
		dedicated=$(printf '%s\n' "$mqtt_routes" | awk -v r="$release-port" '$1 == r { print $2 }')
		on8883=$(printf '%s\n' "$mqtt_routes" | awk -v r="$release" '$1 == r { print $1 }')
		if [ -z "$host" ]; then
			ko "mqtt $release: neither a Certificate nor a HostSNI route names the broker's host"
			continue
		fi
		if [ -n "$dedicated" ]; then
			port=${dedicated#mqtt-}
			sni=(-noservername)
		elif [ -n "$on8883" ]; then
			port=8883
			sni=(-servername "$host")
		else
			ko "mqtt $release: no route on 8883 or a dedicated port reaches the broker"
			continue
		fi
		# A verified chain for the broker's own host name, with SNI on 8883 and without it on a
		# dedicated port: the edge's default certificate would fail -verify_hostname.
		if openssl s_client -connect "$host:$port" "${sni[@]}" -verify_hostname "$host" -verify_return_error \
			-brief </dev/null >/dev/null 2>&1; then
			ok "mqtt $host:$port serves a valid certificate for $host"
		else
			ko "mqtt $host:$port: no verified TLS for $host"
		fi
		# No token, no session: the gateway refuses the CONNECT (ADR-N-048 §2.3), so mosquitto
		# exits non-zero with a refusal, never a publish.
		refused=$(timeout 20 mosquitto_pub -h "$host" -p "$port" --capath /etc/ssl/certs -i "smoke-$RANDOM" \
			-t smoke/none -m x -q 1 2>&1 || true)
		case "$refused" in
		*"ot authori"* | *"efused"* | *"Bad user name or password"*) ok "mqtt $host:$port refuses a CONNECT without a token" ;;
		*) ko "mqtt $host:$port: a CONNECT without a token was not refused (${refused:-published})" ;;
		esac
		if timeout 5 bash -c "exec 3<>/dev/tcp/$host/1883" 2>/dev/null; then
			ko "mqtt $host:1883 (plain MQTT) answers from outside the cluster"
		else
			ok "mqtt $host:1883 (plain MQTT) is closed from outside"
		fi
	done <<<"$mqtt_brokers"
fi

printf '\n%s passed, %s failed, %s skipped\n' "$pass" "$fail" "$skipped"
[ "$warned" -eq 0 ] || printf '%s warnings\n' "$warned"
[ "$fail" -eq 0 ]
