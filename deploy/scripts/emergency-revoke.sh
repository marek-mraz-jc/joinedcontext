#!/usr/bin/env bash
# Emergency revocation of a compromised credential or policy (OPS-45, R48, PF-38).
#
#   scripts/emergency-revoke.sh --instance dev --policy public-read \
#       --project helsinki --space air --forge https://dev.example.org/git \
#       --verify-url https://host/api/endpoint/<slug>/ngsi-ld/v1/entities?type=X \
#       --verify-token-file .secrets/compromised-token \
#       [--user jana.kovacova | --service-account banskabystrica-conformance] [--bound 5]
#
# Three things have to happen, in this order, and the order is the point:
#
#   1. the Policy that grants the compromised subject leaves the repository the gateway reads,
#      because an access token already in an attacker's hands stays cryptographically
#      valid until it expires; only the PDP can stop honouring it. That repository is the
#      forge's (T-5650): the script deletes the manifest with a commit on `main` as the
#      Portal's machine user, whose token it reads from the Secret gitea-token-portal;
#   2. each gateway pod fetches that project now instead of at its next 10 s pass: the script
#      runs one `jcctl checkouts --once --only <project>` in the pod's checkouts sidecar, and
#      the reaper swaps the tables within a second (platform T-0177), so nothing restarts the
#      gateway every tenant shares;
#   3. Keycloak stops minting new tokens for the subject: sessions logged out for a
#      user, client secret rotated for a service account.
#
# Then the script MEASURES the propagation instead of claiming it: it calls the endpoint
# with the compromised token until the answer is 401 or 403 and fails if that takes longer
# than the bound (5 s per OPS-45). A Policy is never removed without that measurement.
#
# The script never prints a token, a secret or a password, and never puts one on a command line.
set -uo pipefail

instance=""
policy=""
project=""
space=""
forge=""
forge_repo=""
policy_path=""
forge_token_secret="gitea-token-portal"
user=""
service_account=""
verify_url=""
verify_token_file=""
bound=5
namespace=""
gateway="context-gateway"
idm=""

die() { printf 'emergency-revoke: %s\n' "$1" >&2; exit 2; }

while [ $# -gt 0 ]; do
	case "$1" in
	--instance) instance="${2:?}"; shift 2 ;;
	--namespace) namespace="${2:?}"; shift 2 ;;
	--policy) policy="${2:?}"; shift 2 ;;
	--project) project="${2:?}"; shift 2 ;;
	--space) space="${2:?}"; shift 2 ;;
	--forge) forge="${2:?}"; shift 2 ;;
	--forge-repo) forge_repo="${2:?}"; shift 2 ;;
	--policy-path) policy_path="${2:?}"; shift 2 ;;
	--forge-token-secret) forge_token_secret="${2:?}"; shift 2 ;;
	--gateway) gateway="${2:?}"; shift 2 ;;
	--user) user="${2:?}"; shift 2 ;;
	--service-account) service_account="${2:?}"; shift 2 ;;
	--verify-url) verify_url="${2:?}"; shift 2 ;;
	--verify-token-file) verify_token_file="${2:?}"; shift 2 ;;
	--bound) bound="${2:?}"; shift 2 ;;
	--idm) idm="${2:?}"; shift 2 ;;
	-h | --help) sed -n '2,29p' "$0"; exit 0 ;;
	*) die "unknown argument: $1" ;;
	esac
done

[ -n "$instance" ] || die "usage: emergency-revoke.sh --instance <slug> [--policy NAME --project P --space S --forge URL --verify-url URL --verify-token-file F] [--user NAME | --service-account CLIENT]"
[ -n "$policy" ] || [ -n "$user" ] || [ -n "$service_account" ] ||
	die "nothing to revoke: give --policy, --user or --service-account"
namespace="${namespace:-$instance}"
if [ -n "$policy" ]; then
	[ -n "$forge" ] || die "--forge <https://host/prefix of the forge> is needed to remove a policy"
	[ -n "$verify_url" ] && [ -n "$verify_token_file" ] ||
		die "a policy is removed only with --verify-url and --verify-token-file, so the refusal is measured"
	# Layout 2 (CC-85): each project is its own repository `{owner}/{project}` with the Policy at
	# spaces/{space}/policies/{name}.yaml (MF-06). Layout 1 names both with the two overrides.
	if [ -z "$policy_path" ]; then
		[ -n "$project" ] && [ -n "$space" ] || die "--project and --space (or --forge-repo and --policy-path) name the policy's file"
		policy_path="spaces/$space/policies/$policy.yaml"
	fi
	[ -n "$forge_repo" ] || { [ -n "$project" ] || die "--project or --forge-repo names the policy's repository"; forge_repo="joinedcontext/$project"; }
	case "$forge_repo/$policy_path" in
	*..* | /* | *[!A-Za-z0-9._/-]*) die "the policy's repository and path take letters, digits, . _ - and / only" ;;
	esac
fi

started=$(date +%s)
elapsed() { printf '%ss' "$(($(date +%s) - started))"; }
step() { printf '[%6s] %s\n' "$(elapsed)" "$1"; }
fail=0
ko() { printf '[%6s] FAIL  %s\n' "$(elapsed)" "$1" >&2; fail=1; }

step "revoking on instance $instance (namespace $namespace), bound ${bound}s"

# 1. The Policy leaves the repository the gateway reads: a commit on the forge's `main`.
#    The token rides in a curl config on stdin, never on a command line.
forge_api() {
	local method="$1" url="$2" body="${3:-}"
	if [ -n "$body" ]; then
		curl -sS --max-time 10 -K - -X "$method" -H "Content-Type: application/json" \
			--data-binary "$body" -w '\n%{http_code}' "$url" <<<"header = \"Authorization: token $forge_token\""
	else
		curl -sS --max-time 10 -K - -X "$method" -w '\n%{http_code}' "$url" <<<"header = \"Authorization: token $forge_token\""
	fi
}

policy_removed=0
if [ -n "$policy" ]; then
	forge_token=$(kubectl -n "$namespace" get secret "$forge_token_secret" -o 'jsonpath={.data.token}' 2>/dev/null | base64 -d 2>/dev/null)
	contents="${forge%/}/api/v1/repos/$forge_repo/contents/$policy_path"
	if [ -z "$forge_token" ]; then
		ko "no forge token in Secret $forge_token_secret of $namespace: the policy was not removed"
	else
		answer=$(forge_api GET "$contents?ref=main")
		code=${answer##*$'\n'}
		sha=$(printf '%s' "${answer%$'\n'*}" | sed -n 's/.*"sha":"\([0-9a-f]*\)".*/\1/p' | head -1)
		if [ "$code" != 200 ] || [ -z "$sha" ]; then
			ko "policy $policy is not at $forge_repo:$policy_path on main (forge answered $code): check --project and --space"
		elif [[ "${answer%$'\n'*}" != *'"type":"file"'* ]]; then
			ko "$forge_repo:$policy_path is not a file"
		else
			answer=$(forge_api DELETE "$contents" \
				"{\"sha\":\"$sha\",\"branch\":\"main\",\"message\":\"Emergency revocation of Policy $policy (OPS-45)\"}")
			code=${answer##*$'\n'}
			case "$code" in
			200) policy_removed=1; step "policy $policy removed from $forge_repo:$policy_path on main" ;;
			*) ko "the forge refused to remove $forge_repo:$policy_path ($code): the policy still grants" ;;
			esac
		fi
	fi
	unset forge_token answer
fi

# 2. Each gateway pod fetches the project now. The checkouts sidecar's own loop fetches every
#    10 s, over the 5 s bound; one pass with --only takes its turn on the sidecar's lock and
#    links the new commit, and the reaper swaps the tables within the next second.
if [ "$policy_removed" -eq 1 ]; then
	selector=$(kubectl -n "$namespace" get deployment "$gateway" \
		-o 'go-template={{range $k, $v := .spec.selector.matchLabels}}{{$k}}={{$v}},{{end}}' 2>/dev/null)
	pods=$(kubectl -n "$namespace" get pods -l "${selector%,}" --field-selector status.phase=Running \
		-o 'jsonpath={range .items[*]}{.metadata.name}{"\n"}{end}' 2>/dev/null)
	[ -n "$selector" ] && [ -n "$pods" ] || ko "no running pod of deployment $gateway in $namespace to fetch the revocation"
	for pod in $pods; do
		mapfile -t args < <(kubectl -n "$namespace" get pod "$pod" \
			-o 'jsonpath={range .spec.containers[?(@.name=="checkouts")].args[*]}{@}{"\n"}{end}')
		if [ "${#args[@]}" -eq 0 ]; then
			# Layout 1: git-sync reads the organization repository every 10 s; the measurement
			# below says whether that held the bound.
			step "$pod has no checkouts sidecar (layout 1): its git-sync fetches within its period"
			continue
		fi
		pass=()
		while [ "${#args[@]}" -gt 0 ]; do
			case "${args[0]}" in
			--interval | --only) args=("${args[@]:2}") ;;
			--once) args=("${args[@]:1}") ;;
			*) pass+=("${args[0]}"); args=("${args[@]:1}") ;;
			esac
		done
		if kubectl -n "$namespace" exec "$pod" -c checkouts -- /usr/local/bin/jcctl checkouts \
			"${pass[@]}" --once --only "${project:-${forge_repo#*/}}" >/dev/null; then
			step "$pod fetched ${project:-${forge_repo#*/}} without the policy"
		else
			ko "$pod could not fetch ${project:-${forge_repo#*/}}: it serves the policy until its next pass"
		fi
	done
fi

# 3. Keycloak stops issuing new tokens for the subject. The admin token comes from the
#    environment (KEYCLOAK_ADMIN_TOKEN) or from the master-realm password grant with
#    KEYCLOAK_ADMIN_USER / KEYCLOAK_ADMIN_PASSWORD; neither is ever echoed.
admin_token() {
	if [ -n "${KEYCLOAK_ADMIN_TOKEN:-}" ]; then
		printf '%s' "$KEYCLOAK_ADMIN_TOKEN"
		return 0
	fi
	[ -n "${KEYCLOAK_ADMIN_USER:-}" ] && [ -n "${KEYCLOAK_ADMIN_PASSWORD:-}" ] || return 1
	# printf is a builtin: the password reaches curl's stdin without a process argv.
	printf '%s' "$KEYCLOAK_ADMIN_PASSWORD" | curl -sS --max-time 10 \
		-d "client_id=admin-cli" -d "grant_type=password" \
		--data-urlencode "username=$KEYCLOAK_ADMIN_USER" \
		--data-urlencode "password@-" \
		"$idm/realms/master/protocol/openid-connect/token" |
		sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p'
}

# A Keycloak admin call: the bearer rides in a curl config on stdin, as forge_api's token does.
kc_api() {
	curl -sS --max-time 10 -K - "$@" <<<"header = \"Authorization: Bearer $token\""
}

# The id of the first object of a Keycloak list answer. Keycloak answers on one line with nested
# objects (a client's protocolMappers, a user's federatedIdentities) that carry ids of their own,
# so a text match picks the wrong one: JSON is read as JSON.
first_id() {
	python3 -c 'import json, sys
try:
    found = json.load(sys.stdin)
    print(found[0]["id"] if isinstance(found, list) and found else "")
except (ValueError, KeyError, TypeError):
    pass'
}

# A client the platform declares (edge, portal-api, gateway, grafana, gitea …) has its secret in
# the generated Secret `keycloak-client-<client>`, copied into every namespace that reads it.
# keycloak-config-cli imports the client's secret from that Secret on every apply, so a rotation
# only in Keycloak would be undone by the next apply, and the readers would keep the old value
# until then. The new secret goes into every copy (the generator keeps an existing value), and
# the workloads that read it restart. A client the Portal creates (a department's ServiceAccount,
# `app-*`) has no such Secret and is rotated in Keycloak alone.
sync_platform_copies() {
	local client="$1" secret="$2" name="keycloak-client-$1" copies ns b64 workload
	copies=$(kubectl get secret -A --field-selector "metadata.name=$name" \
		-o jsonpath='{range .items[*]}{.metadata.namespace}{"\n"}{end}') || {
		ko "cannot list the copies of Secret $name: Keycloak holds a new secret the platform may not know, run this again or apply the platform"
		return
	}
	[ -n "$copies" ] || { step "$client is not a platform client: rotated in Keycloak only"; return; }
	if [ -z "$secret" ]; then
		ko "Keycloak did not answer the new secret of $client, so Secret $name still holds the revoked one: rotate it again"
		return
	fi
	b64=$(printf '%s' "$secret" | base64 | tr -d '\n')
	for ns in $copies; do
		# The patch travels on stdin, never on a command line another process can read.
		if kubectl -n "$ns" patch secret "$name" --type merge --patch-file /dev/stdin \
			<<<"{\"data\":{\"client-secret\":\"$b64\"}}" >/dev/null; then
			step "Secret $name in $ns holds the new secret"
		else
			ko "Secret $name in $ns still holds the revoked secret: the next apply writes it back to Keycloak"
			continue
		fi
		for workload in $(kubectl -n "$ns" get deployments,statefulsets \
			-o jsonpath='{range .items[*]}{.kind}/{.metadata.name}{" "}{.spec.template.spec}{"\n"}{end}' |
			grep -F "\"$name\"" | cut -d' ' -f1); do
			if kubectl -n "$ns" rollout restart "${workload,,}" >/dev/null; then
				step "restarting ${workload,,} in $ns, which reads $name"
			else
				ko "${workload,,} in $ns could not be restarted and still uses the revoked secret"
			fi
		done
	done
	step "apply the platform soon: a component that copied the secret elsewhere (the forge's login source) reads it again only then"
}

if [ -n "$user" ] || [ -n "$service_account" ]; then
	[ -n "$idm" ] || die "--idm <https://idm.host> is needed to reach Keycloak"
	token=$(admin_token)
	if [ -z "$token" ]; then
		ko "no Keycloak admin token: set KEYCLOAK_ADMIN_TOKEN, or KEYCLOAK_ADMIN_USER and KEYCLOAK_ADMIN_PASSWORD"
	else
		api="$idm/admin/realms/$instance"
		if [ -n "$user" ]; then
			id=$(kc_api "$api/users?username=$user&exact=true" | first_id)
			if [ -z "$id" ]; then
				ko "user $user not found in realm $instance"
			else
				code=$(kc_api -o /dev/null -w '%{http_code}' -X POST "$api/users/$id/logout")
				case "$code" in
				20*) step "sessions of $user logged out" ;;
				*) ko "logout of $user answered $code" ;;
				esac
			fi
		fi
		if [ -n "$service_account" ]; then
			id=$(kc_api "$api/clients?clientId=$service_account" | first_id)
			if [ -z "$id" ]; then
				ko "client $service_account not found in realm $instance"
			else
				# Rotating the secret is what stops the client credentials grant. The new
				# secret is never printed: it lives in a variable until it is written into
				# the platform's copies of the client's Secret, if there are any.
				answer=$(kc_api -w '\n%{http_code}' -X POST "$api/clients/$id/client-secret")
				code=${answer##*$'\n'}
				case "$code" in
				20*)
					step "client secret of $service_account rotated"
					secret=$(printf '%s' "${answer%$'\n'*}" | sed -n 's/.*"value":"\([^"]*\)".*/\1/p')
					sync_platform_copies "$service_account" "$secret"
					unset secret answer
					;;
				*) ko "secret rotation of $service_account answered $code" ;;
				esac
			fi
		fi
	fi
fi

# 4. Measure what actually happened at the enforcement point (OPS-45: hard 5 s).
if [ -n "$verify_url" ] && [ -n "$verify_token_file" ]; then
	[ -r "$verify_token_file" ] || die "cannot read $verify_token_file"
	deadline=$((started + bound))
	code=""
	while :; do
		code=$(curl -sS -o /dev/null -w '%{http_code}' --max-time 5 -K - "$verify_url" 2>/dev/null \
			<<<"header = \"Authorization: Bearer $(<"$verify_token_file")\"") || code="curl-error"
		case "$code" in
		401 | 403) break ;;
		esac
		[ "$(date +%s)" -lt "$deadline" ] || break
		sleep 1
	done
	took=$(($(date +%s) - started))
	case "$code" in
	401 | 403)
		if [ "$took" -le "$bound" ]; then
			step "the compromised credential is refused ($code) after ${took}s, within the ${bound}s bound"
		else
			ko "the compromised credential is refused ($code) only after ${took}s, over the ${bound}s bound (OPS-45)"
		fi
		;;
	*) ko "the compromised credential still answers $code after ${took}s (OPS-45)" ;;
	esac
else
	step "no --verify-url given: revocation is not measured, only performed"
fi

if [ "$fail" -eq 0 ]; then
	step "revocation complete"
	exit 0
fi
printf 'emergency-revoke: revocation did NOT complete cleanly — read the FAIL lines above\n' >&2
exit 1
