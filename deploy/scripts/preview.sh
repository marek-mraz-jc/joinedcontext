#!/usr/bin/env bash
# Code-branch previews on the dev node (ADR-N-062, docs Deployment/17, T-5548).
#
#   scripts/preview.sh check <slug>                        the guards alone; changes nothing
#   scripts/preview.sh up <slug> [portal-image] [ui-image]  start or update preview-<slug>.<domain>
#   scripts/preview.sh down <slug>                         delete it and list what is left (nothing)
#   scripts/preview.sh left <slug>                         list what is left of it
#   scripts/preview.sh gc                                  clean up after namespaces the reaper deleted
#   scripts/preview.sh isolation <slug> <runner-image>     run the isolation suite against it
#
# `just dev-preview`, `just dev-preview-down` and `just dev-preview-isolation` call this after
# `just _dev-guard`, so it only ever runs against the dev cluster. Images are repository@sha256
# references of our own registry and must carry our workflows' signature
# (scripts/verify-pinned-images.py); without them the Portal and its UI run dev's pinned digests.
#
# A preview is a namespace pv-<slug> (the chart components/preview/charts/preview), a database
# and role pv_<slug> in dev's postgres-cluster (pg_hba holds the role to that database), and a
# confidential client pv-<slug>-portal in the Keycloak realm `preview`. The namespace goes with
# the reaper after its TTL; `gc` (run by every `up`) removes the database, role and client of a
# namespace that is gone. No secret is printed: passwords travel on stdin and in a mode-700
# temporary directory, and only the Secret preview-credentials in pv-<slug> holds them.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CHART="$ROOT/components/preview/charts/preview"
DEV_NS="${PREVIEW_DEV_NAMESPACE:-dev}"
DOMAIN="${JC_DEV_DOMAIN:-dev.joinedcontext.com}"
MAX_MEMORY="${PREVIEW_MAX_MEMORY_PERCENT:-90}"
MAX_PREVIEWS="${PREVIEW_MAX:-2}"
MAX_CERTIFICATES="${PREVIEW_MAX_CERTIFICATES_PER_WEEK:-10}"
LABEL="sandbox.joinedcontext.com/lifecycle"
REALM="preview"

die() {
	echo "refusing: $*" >&2
	exit 1
}

valid_slug() {
	[[ "${1:-}" =~ ^[a-z0-9]([a-z0-9-]{0,38}[a-z0-9])?$ ]] ||
		die "'${1:-}' is not a preview slug: 1 to 40 lowercase letters, digits and dashes, such as fix-login (scripts/ci/preview-tag.sh computes it from a branch)"
}

role_of() { echo "pv_${1//-/_}"; }

# ---- guards ---------------------------------------------------------------------------------

check() {
	local slug="$1" roles cronjobs nodes over others certs
	valid_slug "$slug"
	roles=$(kubectl -n "$DEV_NS" get clusters.postgresql.cnpg.io postgres-cluster -o 'jsonpath={.spec.managed.roles[*].name}')
	[[ " $roles " == *" pv_previews "* ]] ||
		die "branch previews are off on this cluster: postgres-cluster has no role pv_previews. Apply dev with global.branchPreviews.enabled (just dev-apply) first."
	cronjobs=$(kubectl get cronjobs -A -o name)
	[[ $'\n'"$cronjobs"$'\n' == *$'/sandbox-reaper-reaper\n'* ]] ||
		die "the sandbox reaper is not deployed, so nothing would delete the preview after its TTL. Apply dev (sandbox-reaper is in its components) first."
	nodes=$(kubectl top nodes --no-headers)
	[ -n "$nodes" ] || die "kubectl top nodes answered nothing: without the node's memory a preview does not start"
	over=$(awk -v max="$MAX_MEMORY" '{ m = $5; sub("%", "", m); if (m !~ /^[0-9]+$/) print $1 " reports no memory figure (" $5 ")"; else if (m + 0 >= max + 0) print $1 " uses " m "% of its memory" }' <<<"$nodes")
	[ -z "$over" ] || die "node ${over}; a preview starts only below ${MAX_MEMORY}% (PREVIEW_MAX_MEMORY_PERCENT, Deployment/17 section 3)"
	others=$(kubectl get namespaces -l "$LABEL=preview" -o name | { grep -c -v -x "namespace/pv-$slug" || true; })
	[ "$others" -lt "$MAX_PREVIEWS" ] ||
		die "$others branch previews run already, the cap is $MAX_PREVIEWS (PREVIEW_MAX): stop one with just dev-preview-down <slug>"
	certs=$(kubectl get certificates.cert-manager.io -A -o 'jsonpath={range .items[*]}{.metadata.namespace} {.metadata.creationTimestamp}{"\n"}{end}' |
		awk -v since="$(date -u -d '7 days ago' +%Y-%m-%dT%H:%M:%SZ)" -v own="pv-$slug" '$1 ~ /^pv-/ && $1 != own && $2 >= since' | wc -l)
	[ "$certs" -lt "$MAX_CERTIFICATES" ] ||
		die "$certs preview certificates were issued in the last 7 days, the budget is $MAX_CERTIFICATES (Let's Encrypt's 50 per week are shared with dev and prod, Deployment/17 section 4)"
	echo "ok: pv-$slug may start (memory below ${MAX_MEMORY}%, $others of $MAX_PREVIEWS previews, $certs of $MAX_CERTIFICATES certificates this week)"
}

# ---- the database ---------------------------------------------------------------------------

primary() {
	local pod
	pod=$(kubectl -n "$DEV_NS" get pod -l cnpg.io/cluster=postgres-cluster,cnpg.io/instanceRole=primary -o 'jsonpath={.items[0].metadata.name}')
	[ -n "$pod" ] || die "postgres-cluster has no primary in $DEV_NS"
	echo "$pod"
}

psql_dev() { # SQL on stdin, as the instance's superuser over its local socket
	kubectl -n "$DEV_NS" exec -i "$(primary)" -c postgres -- psql -X -q -v ON_ERROR_STOP=1 -tA -d postgres
}

# A SCRAM-SHA-256 verifier of the password on stdin, so the cleartext never reaches the server's
# statement log; Postgres stores exactly this string.
scram() {
	python3 -c '
import base64, hashlib, hmac, os, sys
password = sys.stdin.read().encode()
salt, rounds = os.urandom(16), 4096
salted = hashlib.pbkdf2_hmac("sha256", password, salt, rounds)
client = hmac.new(salted, b"Client Key", "sha256").digest()
server = hmac.new(salted, b"Server Key", "sha256").digest()
b64 = lambda b: base64.b64encode(b).decode()
print(f"SCRAM-SHA-256${rounds}:{b64(salt)}${b64(hashlib.sha256(client).digest())}:{b64(server)}")'
}

database_up() { # <slug> <password file>
	local role verifier
	role=$(role_of "$1")
	verifier=$(scram <"$2")
	psql_dev <<SQL
SELECT format('CREATE ROLE %I', '$role') WHERE NOT EXISTS (SELECT FROM pg_roles WHERE rolname = '$role')\gexec
ALTER ROLE "$role" LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS NOINHERIT CONNECTION LIMIT 5 PASSWORD '$verifier';
GRANT pv_previews TO "$role";
SELECT format('CREATE DATABASE %I OWNER %I', '$role', '$role') WHERE NOT EXISTS (SELECT FROM pg_database WHERE datname = '$role')\gexec
REVOKE ALL ON DATABASE "$role" FROM PUBLIC;
SQL
}

database_down() {
	local role
	role=$(role_of "$1")
	psql_dev <<SQL
DROP DATABASE IF EXISTS "$role" WITH (FORCE);
DROP ROLE IF EXISTS "$role";
SQL
}

# ---- the Keycloak client --------------------------------------------------------------------

# Runs the sh script $1 inside the Keycloak pod with kcadm logged in to the master realm. The
# admin password is the first line of stdin, the rest of stdin is the script's own.
kcadm() {
	local pod
	pod=$(kubectl -n "$DEV_NS" get pod -l app.kubernetes.io/name=keycloakx,app.kubernetes.io/instance=keycloak-app -o 'jsonpath={.items[0].metadata.name}')
	[ -n "$pod" ] || die "no Keycloak pod in $DEV_NS"
	{
		kubectl -n "$DEV_NS" get secret keycloak-admin-user -o 'jsonpath={.data.password}' | base64 -d
		echo
		cat
	} | kubectl -n "$DEV_NS" exec -i "$pod" -c keycloak -- sh -c '
set -eu
kc() { /opt/keycloak/bin/kcadm.sh "$@" --config /tmp/kcadm-preview.config; }
trap "rm -f /tmp/kcadm-preview.config" EXIT
IFS= read -r password
kc config credentials --server http://localhost:8080 --realm master --user admin --password "$password" >/dev/null
'"$1"
}

client_up() { # <slug>; prints the client secret
	local slug="$1" host="preview-$1.$DOMAIN"
	python3 -c '
import json, sys
slug, host = sys.argv[1], sys.argv[2]
print(json.dumps({
    "clientId": f"pv-{slug}-portal", "enabled": True, "protocol": "openid-connect",
    "description": f"Branch preview pv-{slug} (T-5548); deleted with it",
    "publicClient": False, "standardFlowEnabled": True, "directAccessGrantsEnabled": False,
    "serviceAccountsEnabled": True, "implicitFlowEnabled": False,
    "redirectUris": [f"https://{host}/api/v1/auth/callback"], "webOrigins": [f"https://{host}"],
    "attributes": {"managed-by": "dev-preview", "pkce.code.challenge.method": "S256"},
    "protocolMappers": [{"name": "audience", "protocol": "openid-connect", "protocolMapper": "oidc-audience-mapper",
                         "config": {"included.custom.audience": f"pv-{slug}", "access.token.claim": "true",
                                    "id.token.claim": "false"}}],
}))' "$slug" "$host" | kcadm '
kc get realms/'"$REALM"' >/dev/null 2>&1 || kc create realms -s realm='"$REALM"' -s enabled=true \
  -s registrationAllowed=false -s resetPasswordAllowed=false -s rememberMe=false >/dev/null
id=$(kc get clients -r '"$REALM"' -q clientId=pv-'"$slug"'-portal --fields id --format csv --noquotes | tr -d "\r")
if [ -z "$id" ]; then
  kc create clients -r '"$REALM"' -f - >/dev/null
  id=$(kc get clients -r '"$REALM"' -q clientId=pv-'"$slug"'-portal --fields id --format csv --noquotes | tr -d "\r")
else
  kc update clients/$id -r '"$REALM"' -f - >/dev/null
fi
kc get clients/$id/client-secret -r '"$REALM"' --fields value --format csv --noquotes | tr -d "\r"'
}

client_down() {
	kcadm '
kc get realms/'"$REALM"' >/dev/null 2>&1 || exit 0
id=$(kc get clients -r '"$REALM"' -q clientId=pv-'"$1"'-portal --fields id --format csv --noquotes | tr -d "\r")
[ -z "$id" ] || kc delete clients/$id -r '"$REALM"'' </dev/null
}

# ---- what is left ---------------------------------------------------------------------------

left() {
	local slug="$1" role found=0 clients
	valid_slug "$slug"
	role=$(role_of "$slug")
	if kubectl get namespace "pv-$slug" -o name >/dev/null 2>&1; then
		echo "left: namespace pv-$slug"
		kubectl -n "pv-$slug" get certificates.cert-manager.io -o name 2>/dev/null | sed 's/^/left: /'
		found=1
	fi
	while read -r what; do
		[ -n "$what" ] && { echo "left: $what"; found=1; }
	done < <(psql_dev <<SQL
SELECT 'database ' || datname FROM pg_database WHERE datname = '$role';
SELECT 'role ' || rolname FROM pg_roles WHERE rolname = '$role';
SQL
	)
	clients=$(kcadm '
kc get realms/'"$REALM"' >/dev/null 2>&1 || exit 0
kc get clients -r '"$REALM"' -q clientId=pv-'"$slug"'-portal --fields clientId --format csv --noquotes' </dev/null | tr -d '\r')
	[ -z "$clients" ] || { echo "left: Keycloak client $clients in realm $REALM"; found=1; }
	[ "$found" = 0 ] && echo "nothing left of pv-$slug: no namespace, Certificate, database, role or Keycloak client"
	[ "$found" = 0 ]
}

down() {
	local slug="$1"
	valid_slug "$slug"
	# The namespace first: its pods hold the database connections and the client's sessions.
	kubectl delete namespace "pv-$slug" --ignore-not-found --wait=true --timeout=300s
	database_down "$slug"
	client_down "$slug"
	left "$slug"
}

gc() {
	local role slug
	for role in $(psql_dev <<<"SELECT rolname FROM pg_roles WHERE rolname LIKE 'pv\_%' AND rolname <> 'pv_previews';"); do
		slug="${role#pv_}"
		slug="${slug//_/-}"
		if ! kubectl get namespace "pv-$slug" -o name >/dev/null 2>&1; then
			echo "gc: pv-$slug is gone (reaped or deleted); removing its database, role and client"
			database_down "$slug"
			client_down "$slug"
		fi
	done
}

# ---- up -------------------------------------------------------------------------------------

pinned() { # <images.yaml key path> -> repository@digest of dev's pin
	local file="$ROOT/components/portal/images.yaml"
	echo "$(yq -r ".portal.$1.repository" "$file")@$(yq -r ".portal.$1.digest" "$file")"
}

up() {
	local slug="$1" portal="${2:-}" ui="${3:-}" existed=0 ns="pv-$1"
	check "$slug"
	gc
	[ -n "$portal" ] || portal=$(pinned portal)
	[ -n "$ui" ] || ui=$(pinned ui)
	work=$(mktemp -d)
	chmod 700 "$work"
	trap 'rm -rf "${work:?}"' EXIT
	helm template "$ns" "$CHART" --set "slug=$slug" --set "domain=$DOMAIN" --set "devNamespace=$DEV_NS" \
		--set-string "images.portal=$portal" --set-string "images.portalUi=$ui" >"$work/preview.yaml"
	"$ROOT/scripts/verify-pinned-images.py" dev --render "$work/preview.yaml"
	kubectl get namespace "$ns" -o name >/dev/null 2>&1 && existed=1

	python3 -c 'import secrets; print(secrets.token_urlsafe(32), end="")' >"$work/pgpassword"
	python3 -c 'import secrets; print(secrets.token_urlsafe(48), end="")' >"$work/cookie-key"
	database_up "$slug" "$work/pgpassword"
	client_up "$slug" </dev/null | tr -d '\n' >"$work/oidc-client-secret"
	[ -s "$work/oidc-client-secret" ] || die "Keycloak returned no secret for pv-$slug-portal"

	yq 'select(.kind == "Namespace")' "$work/preview.yaml" | kubectl apply -f -
	kubectl -n "$ns" create secret generic preview-credentials --dry-run=client -o yaml \
		--from-file=pgpassword="$work/pgpassword" --from-file=cookie-key="$work/cookie-key" \
		--from-file=oidc-client-secret="$work/oidc-client-secret" | kubectl apply -f - >/dev/null
	kubectl apply -f "$work/preview.yaml"
	# A second run wrote a new database password and client secret: the pods read them at start.
	[ "$existed" = 0 ] || kubectl -n "$ns" rollout restart deployment/portal deployment/portal-ui
	kubectl -n "$ns" rollout status deployment/portal-ui --timeout=300s
	kubectl -n "$ns" rollout status deployment/portal --timeout=300s
	echo "preview: https://preview-$slug.$DOMAIN (namespace $ns, deleted by the reaper after $(kubectl get namespace "$ns" -o 'jsonpath={.metadata.annotations.sandbox\.joinedcontext\.com/ttl}'))"
	kubectl -n "$ns" top pods --containers 2>/dev/null || true
}

# ---- isolation ------------------------------------------------------------------------------

isolation() {
	local slug="$1" runner="${2:-}" ns="pv-$1" suite
	valid_slug "$slug"
	[[ "$runner" =~ ^ghcr\.io/marek-mraz-jc/[a-z0-9./_-]+@sha256:[0-9a-f]{64}$ ]] ||
		die "the isolation runner is the conformance image by digest, ghcr.io/marek-mraz-jc/joinedcontext-conformance@sha256:<digest>"
	suite="${PREVIEW_SUITE_DIR:-$ROOT/../joinedcontext-conformance/tests/security}"
	[ -f "$suite/test_preview_isolation.py" ] || die "no $suite/test_preview_isolation.py: set PREVIEW_SUITE_DIR to the conformance clone's tests/security"
	kubectl get namespace "$ns" -o name >/dev/null || die "$ns does not exist: just dev-preview $slug first"
	# Properties 4 and 5 from here, before the Job adds a pod of its own to the namespace.
	PV_SLUG="$slug" PV_KUBECTL=1 python3 -m pytest -q -p no:cacheprovider "$suite/test_preview_isolation.py" -k "pods_name or quota"
	work=$(mktemp -d)
	trap 'rm -rf "${work:?}"; kubectl -n "$ns" delete job/isolation configmap/isolation-suite --ignore-not-found >/dev/null' EXIT
	kubectl -n "$ns" create configmap isolation-suite --from-file="$suite/test_preview_isolation.py" \
		--from-file="$suite/preview_isolation.py" --dry-run=client -o yaml | kubectl apply -f - >/dev/null
	python3 - "$slug" "$runner" "$DOMAIN" "$DEV_NS" >"$work/job.json" <<'PY'
import json, sys
slug, image, domain, dev = sys.argv[1:]
env = [{"name": k, "value": v} for k, v in
       {"PV_SLUG": slug, "PV_IN_NAMESPACE": "1", "PV_DOMAIN": domain, "PV_DEV_NAMESPACE": dev, "HOME": "/tmp",
        "PYTHONDONTWRITEBYTECODE": "1"}.items()]
env.append({"name": "PV_CLIENT_SECRET", "valueFrom": {"secretKeyRef": {"name": "preview-credentials", "key": "oidc-client-secret"}}})
print(json.dumps({"apiVersion": "batch/v1", "kind": "Job", "metadata": {"name": "isolation", "namespace": f"pv-{slug}"},
    "spec": {"backoffLimit": 0, "ttlSecondsAfterFinished": 600, "template": {"metadata": {"labels": {"app.kubernetes.io/name": "isolation"}, "annotations": {
        # The proxy as a native sidecar, so the Job completes; 8080 and 443 bypass it, so a TCP probe
        # meets the NetworkPolicy itself and not the local proxy, which accepts every connection.
        # Postgres (5432) stays meshed: dev's database admits cluster-authenticated callers only.
        "config.alpha.linkerd.io/proxy-enable-native-sidecar": "true",
        "config.linkerd.io/skip-outbound-ports": "443,8080"}},
        "spec": {"restartPolicy": "Never", "automountServiceAccountToken": False,
            "securityContext": {"runAsNonRoot": True, "runAsUser": 1000, "seccompProfile": {"type": "RuntimeDefault"}},
            "containers": [{"name": "suite", "image": image, "env": env, "workingDir": "/suite",
                "command": ["python3", "-m", "pytest", "-q", "-p", "no:cacheprovider", "/suite/test_preview_isolation.py", "-k", "not pods_name and not quota"],
                "resources": {"requests": {"cpu": "50m", "memory": "128Mi"}, "limits": {"cpu": "500m", "memory": "256Mi"}},
                "securityContext": {"allowPrivilegeEscalation": False, "readOnlyRootFilesystem": True, "capabilities": {"drop": ["ALL"]}},
                "volumeMounts": [{"name": "suite", "mountPath": "/suite"}, {"name": "tmp", "mountPath": "/tmp"}]}],
            "volumes": [{"name": "suite", "configMap": {"name": "isolation-suite"}}, {"name": "tmp", "emptyDir": {}}]}}}}))
PY
	"$ROOT/scripts/verify-pinned-images.py" dev --render "$work/job.json"
	kubectl -n "$ns" delete job/isolation --ignore-not-found >/dev/null
	kubectl apply -f "$work/job.json" >/dev/null
	for _ in $(seq 60); do
		[ -n "$(kubectl -n "$ns" get job isolation -o 'jsonpath={.status.succeeded}{.status.failed}')" ] && break
		sleep 5
	done
	kubectl -n "$ns" logs job/isolation -c suite
	[ "$(kubectl -n "$ns" get job isolation -o 'jsonpath={.status.succeeded}')" = 1 ]
}

command="${1:-}"
shift || true
case "$command" in
check | left | down) [ $# -eq 1 ] || die "usage: $0 $command <slug>"; "$command" "$1" ;;
up)
	if [ $# -lt 1 ] || [ $# -gt 3 ]; then die "usage: $0 up <slug> [portal-image] [ui-image]"; fi
	up "$@"
	;;
isolation) [ $# -eq 2 ] || die "usage: $0 isolation <slug> <runner-image>"; isolation "$@" ;;
gc) gc ;;
*) die "usage: $0 check|up|down|left|isolation <slug> ..., or $0 gc" ;;
esac
