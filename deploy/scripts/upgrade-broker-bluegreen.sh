#!/usr/bin/env bash
#
# Blue/green replacement of the context broker, with no in-place data migration (OPS-13,
# OPS-14, CC-51, T-0047).
#
# The platform's configuration lives in Git and the broker's entities are replayed from it, so
# a major broker upgrade does not migrate data: it fills a new broker from the repository and
# moves the traffic once the new one holds what Git declares.
#
#   1. a green database beside the blue one, so the replay cannot write into live data,
#   2. a green broker release on the new image, pointed at that database,
#   3. a green gateway release in front of it, because nothing writes to a broker directly
#      (CC-04) and the replay must pass the same policy layer production uses,
#   4. `jcctl apply` to replay the repository's seed entities through the green gateway,
#   5. `jcctl plan` to prove the green stack holds what Git declares, zero diff,
#   6. a smoke read through the green gateway,
#   7. the cut-over: the production gateway's broker URL becomes the green broker, as a rolling
#      update with `maxUnavailable: 0`, so no request is dropped,
#   8. the green gateway goes away — it existed only to carry the replay.
#
# The blue broker and its database are left running and untouched: they are the rollback, and
# removing them is the operator's own decision once the new one has proven itself.
#
#   scripts/upgrade-broker-bluegreen.sh --check
#   scripts/upgrade-broker-bluegreen.sh --image ghcr.io/x/antares@sha256:… --repo-dir /var/git/city-config
#
# Every step prints what it runs. `--check` asserts the preconditions and writes nothing.
set -euo pipefail

cd "$(dirname "$0")/.."

IMAGE=""
REPO_DIR="${JC_UPGRADE_REPO_DIR:-}"
TOKEN_FILE="${JC_UPGRADE_TOKEN_FILE:-}"
CHECK_ONLY=false
KEEP_GREEN_GATEWAY=false
# The green stack's names. They are release names, not permanent parts of the deployment: the
# gateway is removed at the end and the broker is renamed back by the Git change step 9 prints.
GREEN_BROKER="context-broker-green"
GREEN_GATEWAY="context-gateway-green"
GREEN_DATABASE="${JC_UPGRADE_GREEN_DATABASE:-antares_green}"
ROLLOUT_TIMEOUT="${JC_UPGRADE_ROLLOUT_TIMEOUT:-600}"

usage() {
	sed -n '3,27p' "$0" | sed 's/^# \{0,1\}//'
	exit "${1:-0}"
}

while [ $# -gt 0 ]; do
	case "$1" in
	--check) CHECK_ONLY=true ;;
	--keep-green-gateway) KEEP_GREEN_GATEWAY=true ;;
	--image)
		IMAGE="${2:?--image needs the green broker image, pinned by digest}"
		shift
		;;
	--repo-dir)
		REPO_DIR="${2:?--repo-dir needs the configuration repository}"
		shift
		;;
	--token-file)
		TOKEN_FILE="${2:?--token-file needs a path}"
		shift
		;;
	-h | --help) usage 0 ;;
	*)
		echo "unknown argument: $1" >&2
		usage 1
		;;
	esac
	shift
done

step() { printf '\n=== %s\n' "$1"; }
run() {
	printf '+ %s\n' "$*"
	"$@"
}
fail() {
	printf 'FAIL  %s\n' "$1" >&2
	exit 1
}

# The namespace a helm release lives in, or empty when there is no such release. Read rather
# than computed: `jc.namespace` depends on `global.singleNamespace`, and a script that guessed
# it wrong would install the green stack somewhere the blue one cannot be compared with.
release_namespace() {
	helm list --all-namespaces --filter "^$1\$" --output json 2>/dev/null |
		sed -n 's/.*"namespace": *"\([^"]*\)".*/\1/p' | head -1
}

# --- 1. preconditions -------------------------------------------------------
# Everything the upgrade cannot do without, asserted before anything is created: a run that
# installs a green broker and then finds it has no `jcctl` has doubled the load on the node and
# rehearsed nothing.
step "1/9 preconditions"
for tool in kubectl helm jcctl curl; do
	command -v "$tool" >/dev/null || fail "$tool is not on PATH"
	printf '  ok    %s\n' "$tool"
done

BROKER_NS="$(release_namespace context-broker-broker)"
[ -n "$BROKER_NS" ] || fail "no 'context-broker-broker' release: deploy the instance first"
printf '  ok    blue broker in %s\n' "$BROKER_NS"

GATEWAY_NS="$(release_namespace context-gateway-gateway)"
[ -n "$GATEWAY_NS" ] || fail "no 'context-gateway-gateway' release: deploy the instance first"
printf '  ok    blue gateway in %s\n' "$GATEWAY_NS"

# The cut-over is a rolling update of the live gateway, and it is only zero-downtime because
# the workload chart never takes a pod away before its replacement is ready. Asserted rather
# than trusted: a values override could have set it, and a dropped request during a cut-over
# is exactly what OPS-13 is about.
unavailable="$(kubectl get deployment -n "$GATEWAY_NS" context-gateway \
	-o jsonpath='{.spec.strategy.rollingUpdate.maxUnavailable}' 2>/dev/null || true)"
[ "$unavailable" = "0" ] ||
	fail "the gateway's rollingUpdate.maxUnavailable is '${unavailable:-unset}', not 0: a cut-over would drop requests"
printf '  ok    gateway rolls over with maxUnavailable=0\n'

[ -d charts/workload ] || fail "no charts/workload: run this from the deployment repository"
printf '  ok    charts/workload\n'

if [ "$CHECK_ONLY" = false ]; then
	[ -n "$IMAGE" ] || fail "--image is required: the green broker's image, pinned by digest"
	case "$IMAGE" in
	*@sha256:*) ;;
	*) fail "--image must be pinned by digest (CC-35), not by tag: $IMAGE" ;;
	esac
	[ -n "$REPO_DIR" ] || fail "--repo-dir is required: the replay reads the repository Git holds"
	[ -d "$REPO_DIR" ] || fail "--repo-dir '$REPO_DIR' is not a directory"
	[ -z "$(release_namespace "$GREEN_BROKER")" ] ||
		fail "a '$GREEN_BROKER' release already exists: an earlier upgrade did not finish"
	printf '  ok    green image %s\n  ok    repository %s\n' "$IMAGE" "$REPO_DIR"
fi

# The database the blue broker reads, so the green one can be told to read a different one and
# the replay cannot touch live data.
BLUE_DB_URL="$(kubectl get deployment -n "$BROKER_NS" context-broker \
	-o jsonpath='{.spec.template.spec.containers[0].env[?(@.name=="ANTARES_DATABASE_URL")].value}' 2>/dev/null || true)"
[ -n "$BLUE_DB_URL" ] || fail "the blue broker has no ANTARES_DATABASE_URL: nothing to derive a green database from"
GREEN_DB_URL="${BLUE_DB_URL%/*}/${GREEN_DATABASE}"
[ "$GREEN_DB_URL" != "$BLUE_DB_URL" ] ||
	fail "the green database would be the blue one: refusing to replay into live data"
printf '  ok    green database %s (blue keeps %s)\n' "$GREEN_DATABASE" "${BLUE_DB_URL##*/}"

PG_PRIMARY="$(kubectl get pods --all-namespaces -l cnpg.io/instanceRole=primary \
	-o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)"
PG_NS="$(kubectl get pods --all-namespaces -l cnpg.io/instanceRole=primary \
	-o jsonpath='{.items[0].metadata.namespace}' 2>/dev/null || true)"
[ -n "$PG_PRIMARY" ] || fail "no CNPG primary pod: the green database cannot be created"
printf '  ok    postgres primary %s/%s\n' "$PG_NS" "$PG_PRIMARY"

if [ "$CHECK_ONLY" = true ]; then
	printf '\nchecked only; nothing was written\n'
	exit 0
fi

# --- 2. the green database --------------------------------------------------
# Created, never dropped: this script does not delete a database, and the blue one it is beside
# is the rollback. `IF NOT EXISTS` has no equivalent for CREATE DATABASE, so an existing one is
# left alone rather than recreated.
step "2/9 green database"
owner="$(printf '%s' "$BLUE_DB_URL" | sed -n 's|^postgresql://\([^:]*\):.*|\1|p')"
[ -n "$owner" ] || fail "cannot read the broker's database role from its URL"
exists="$(kubectl exec -n "$PG_NS" "$PG_PRIMARY" -c postgres -- \
	psql -tAc "select 1 from pg_database where datname = '${GREEN_DATABASE}'" 2>/dev/null || true)"
if [ "$exists" = "1" ]; then
	printf '  %s already exists; left as it is\n' "$GREEN_DATABASE"
else
	run kubectl exec -n "$PG_NS" "$PG_PRIMARY" -c postgres -- \
		psql -c "CREATE DATABASE ${GREEN_DATABASE} OWNER ${owner}"
fi
# The broker's first migration needs both, and its role is deliberately not a superuser, so
# they are created here exactly as CNPG creates them for the blue database.
run kubectl exec -n "$PG_NS" "$PG_PRIMARY" -c postgres -- \
	psql -d "$GREEN_DATABASE" -c "CREATE EXTENSION IF NOT EXISTS postgis; CREATE EXTENSION IF NOT EXISTS btree_gin"

# --- 3. the green broker ----------------------------------------------------
# The blue release's own values, with three things changed: the name, the image and the
# database. Reading them from the live release rather than rendering them again is what makes
# green the same deployment as blue — anything the environment set, green has too.
step "3/9 green broker"
blue_values="$(mktemp)"
trap 'rm -f "$blue_values" "${green_values:-}"' EXIT
helm get values context-broker-broker -n "$BROKER_NS" --all -o yaml >"$blue_values"
run helm upgrade --install "$GREEN_BROKER" ./charts/workload \
	--namespace "$BROKER_NS" \
	--values "$blue_values" \
	--set "fullnameOverride=$GREEN_BROKER" \
	--set "nameOverride=$GREEN_BROKER" \
	--set "image.digest=${IMAGE#*@}" \
	--set "image.repository=${IMAGE%@*}" \
	--set "env.ANTARES_DATABASE_URL=$GREEN_DB_URL" \
	--wait --timeout "${ROLLOUT_TIMEOUT}s"

# --- 4. the green gateway ---------------------------------------------------
# Stateless, and it reads its tables from the same configuration repository the blue one does,
# so the only thing that differs is the broker it points at (CC-04: the replay goes through a
# gateway, never at the broker).
step "4/9 green gateway"
green_values="$(mktemp)"
helm get values context-gateway-gateway -n "$GATEWAY_NS" --all -o yaml >"$green_values"
GREEN_BROKER_URL="http://${GREEN_BROKER}.${BROKER_NS}.svc.cluster.local:8080"
run helm upgrade --install "$GREEN_GATEWAY" ./charts/workload \
	--namespace "$GATEWAY_NS" \
	--values "$green_values" \
	--set "fullnameOverride=$GREEN_GATEWAY" \
	--set "nameOverride=$GREEN_GATEWAY" \
	--set "env.JC_GATEWAY_BROKER_URL=$GREEN_BROKER_URL" \
	--wait --timeout "${ROLLOUT_TIMEOUT}s"

GREEN_GATEWAY_URL="${JC_UPGRADE_GREEN_GATEWAY_URL:-http://${GREEN_GATEWAY}.${GATEWAY_NS}.svc.cluster.local:8080}"
token_args=()
if [ -n "$TOKEN_FILE" ]; then
	token_args=(--token-file "$TOKEN_FILE")
fi

# --- 5. the replay ----------------------------------------------------------
step "5/9 replay the repository's seed entities into green"
run jcctl apply --repo-dir "$REPO_DIR" --gateway-url "$GREEN_GATEWAY_URL" "${token_args[@]}"

# --- 6. parity --------------------------------------------------------------
# A second pass writes nothing when the platform holds what Git declares. The words are
# asserted, not the exit code: a plan with changes in it also exits zero.
step "6/9 parity"
plan="$(jcctl plan --repo-dir "$REPO_DIR" --gateway-url "$GREEN_GATEWAY_URL" "${token_args[@]}")"
printf '%s\n' "$plan"
case "$plan" in
*"0 to add, 0 to change, 0 to delete"*) ;;
*) fail "green does not match the repository; blue is still serving and nothing was switched" ;;
esac

# --- 7. smoke ---------------------------------------------------------------
# Ready is not the same as answering: the deployment rolled out, and this asks the green
# gateway for something over the green broker before any traffic depends on it.
step "7/9 smoke"
# Through a port-forward rather than a pod of its own: a cluster with the platform's own
# Kyverno policies on it refuses an ad-hoc `kubectl run` (no dropped capabilities, no
# read-only root, runs as root), so a probe that needed one would fail every upgrade on a
# hardened cluster and pass only on an unprotected one.
smoke_port="${JC_UPGRADE_SMOKE_PORT:-18080}"
kubectl port-forward --namespace "$GATEWAY_NS" "deploy/$GREEN_GATEWAY" "${smoke_port}:8080" \
	>/dev/null 2>&1 &
forward=$!
trap 'kill "$forward" 2>/dev/null || true; rm -f "$blue_values" "${green_values:-}"' EXIT
code=""
for _ in 1 2 3 4 5 6 7 8 9 10; do
	code="$(curl -s -o /dev/null -m 3 -w '%{http_code}' "http://127.0.0.1:${smoke_port}/healthz" || true)"
	[ "$code" = "200" ] && break
	sleep 1
done
kill "$forward" 2>/dev/null || true
trap 'rm -f "$blue_values" "${green_values:-}"' EXIT
[ "$code" = "200" ] ||
	fail "the green gateway answered '${code:-nothing}' on /healthz; blue is still serving and nothing was switched"
printf '  ok    green answers /healthz\n'

# --- 8. the cut-over --------------------------------------------------------
# The production gateway keeps its name, its Service and its place behind APISIX; what changes
# is the broker it dials. Nothing at the edge moves, so there is no route to flip and no window
# in which APISIX holds a stale upstream.
step "8/9 cut-over"
run helm upgrade "context-gateway-gateway" ./charts/workload \
	--namespace "$GATEWAY_NS" --reuse-values \
	--set "env.JC_GATEWAY_BROKER_URL=$GREEN_BROKER_URL" \
	--wait --timeout "${ROLLOUT_TIMEOUT}s"
run kubectl rollout status deployment/context-gateway -n "$GATEWAY_NS" --timeout "${ROLLOUT_TIMEOUT}s"

# --- 9. tidy up and say what Git still owes ---------------------------------
step "9/9 the green gateway"
if [ "$KEEP_GREEN_GATEWAY" = true ]; then
	printf '  kept %s at the operator\x27s request\n' "$GREEN_GATEWAY"
else
	run helm uninstall "$GREEN_GATEWAY" --namespace "$GATEWAY_NS" --wait
fi

cat <<NOTE

The traffic is on the green broker. The blue broker and its database are untouched: roll back
by running the step 8 command again with the blue URL
(http://context-broker.${BROKER_NS}.svc.cluster.local:8080).

The cluster is now ahead of Git, and the next helmfile apply would undo the cut-over. Commit
these two changes, then apply, then uninstall '${GREEN_BROKER}':

  components/context-broker/images.yaml   digest: ${IMAGE#*@}
  components/context-broker/values/broker/base-values.yaml.gotmpl
                                          ANTARES_DATABASE_URL -> /${GREEN_DATABASE}

After that apply, the release named 'context-broker-broker' is the upgraded broker on the green
database, the gateway dials it by its ordinary name again, and nothing carries the word green.
NOTE
