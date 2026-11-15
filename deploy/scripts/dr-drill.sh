#!/usr/bin/env bash
#
# The restore half of the disaster recovery drill (OPS-11, OPS-12, CC-50, T-0046).
#
# The deployment half is not here: the k3d lane of `ci-full` already brings a whole instance up
# with `scripts/test-deployment-variants.sh`, and duplicating that would be a second harness to
# keep in step with the first. This script is what happens to an instance that is already
# running, in the order Runbook 6 and `Deployment/07 §3` give it:
#
#   1. back the database up into the object store the instance carries,
#   2. lose the database, the way an incident loses it,
#   3. bring it back from the archive at an instant inside the retention window (OPS-10),
#   4. replay the repository's seed entities through the gateway (CC-72), and
#   5. assert the platform holds what Git declares, by asking `jcctl plan` for a diff.
#
# A drill that only proves step 3 proves the database came back. The point of 4 and 5 is that
# the *platform* came back: the manifests are read from Git by every component, and the seed
# entities are the one state the repository cannot project by itself.
#
#   scripts/dr-drill.sh --check                 # preconditions only, changes nothing
#   scripts/dr-drill.sh --env local --slug local
#
# Every step prints what it runs. `--check` is what CI runs before the cluster exists and what
# an operator runs against a live instance to see whether a drill would work there — it reads
# and asserts, and writes nothing.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

ENVIRONMENT="local"
SLUG=""
CHECK_ONLY=false
# How long a restore may take before the drill calls it a failure. CNPG downloads a base backup
# and replays WAL; on a runner with the archive in-cluster this is a minute, and the margin is
# for a disk that is busy rather than for a restore that is stuck.
RESTORE_TIMEOUT="${JC_DR_RESTORE_TIMEOUT:-900}"
BACKUP_TIMEOUT="${JC_DR_BACKUP_TIMEOUT:-600}"

usage() {
	sed -n '3,25p' "$0" | sed 's/^# \{0,1\}//'
	exit "${1:-0}"
}

while [ $# -gt 0 ]; do
	case "$1" in
	--check) CHECK_ONLY=true ;;
	--env)
		ENVIRONMENT="${2:?--env needs a helmfile environment}"
		shift
		;;
	--slug)
		SLUG="${2:?--slug needs an instance slug}"
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
[ -n "$SLUG" ] || SLUG="$ENVIRONMENT"

step() { printf '\n=== %s\n' "$1"; }
run() {
	printf '+ %s\n' "$*"
	"$@"
}
fail() {
	printf 'FAIL  %s\n' "$1" >&2
	exit 1
}

# --- preconditions ----------------------------------------------------------
# Every one of these is a thing the drill cannot do without, checked before anything is deleted:
# a drill that destroys the database and then finds it has no `jcctl` has made an incident
# rather than rehearsed one.
step "preconditions"
for tool in kubectl helmfile jcctl; do
	command -v "$tool" >/dev/null || fail "$tool is not on PATH"
	printf '  ok    %s\n' "$tool"
done

kubectl get ns "$SLUG" >/dev/null 2>&1 || fail "no namespace '$SLUG': deploy the instance first"
printf '  ok    namespace %s\n' "$SLUG"

kubectl get clusters.postgresql.cnpg.io -n "$SLUG" postgres-cluster >/dev/null 2>&1 ||
	fail "no CNPG Cluster 'postgres-cluster' in '$SLUG'"
printf '  ok    postgres-cluster\n'

# The archive is what the whole drill turns on. Without `backup.barmanObjectStore` the Cluster
# has no WAL archive, so there is nothing to recover from and the drill would prove the opposite
# of what it claims: that a restore "worked" by re-initialising an empty database.
archive="$(kubectl get clusters.postgresql.cnpg.io -n "$SLUG" postgres-cluster \
	-o jsonpath='{.spec.backup.barmanObjectStore.destinationPath}' 2>/dev/null || true)"
[ -n "$archive" ] ||
	fail "postgres-cluster archives no WAL: set postgres.cluster.backups.enabled in the '$ENVIRONMENT' environment"
printf '  ok    archive %s\n' "$archive"

recovery_env="deployment/environments/${ENVIRONMENT}-recovery/global.yaml.gotmpl"
[ -f "$recovery_env" ] ||
	fail "no recovery environment at $recovery_env: the restore pass is a values change (Deployment/07 §3 step 2)"
printf '  ok    recovery environment %s\n' "$recovery_env"

repo_dir="${JC_DR_REPO_DIR:-}"
if [ -n "$repo_dir" ]; then
	[ -d "$repo_dir" ] || fail "JC_DR_REPO_DIR '$repo_dir' is not a directory"
	printf '  ok    repository %s\n' "$repo_dir"
else
	printf '  note  JC_DR_REPO_DIR unset: the seed replay and the parity check are skipped\n'
fi

if [ "$CHECK_ONLY" = true ]; then
	printf '\nchecked only; nothing was written\n'
	exit 0
fi

# --- 1. back it up ----------------------------------------------------------
step "1/5 backup"
backup="dr-drill-$(date -u +%Y%m%d-%H%M%S)"
kubectl apply -f - <<EOF
apiVersion: postgresql.cnpg.io/v1
kind: Backup
metadata:
  name: ${backup}
  namespace: ${SLUG}
spec:
  cluster:
    name: postgres-cluster
  method: barmanObjectStore
EOF

deadline=$(($(date +%s) + BACKUP_TIMEOUT))
while :; do
	phase="$(kubectl get backup -n "$SLUG" "$backup" -o jsonpath='{.status.phase}' 2>/dev/null || true)"
	case "$phase" in
	completed) break ;;
	failed) fail "the backup failed: $(kubectl get backup -n "$SLUG" "$backup" -o jsonpath='{.status.error}')" ;;
	esac
	[ "$(date +%s)" -lt "$deadline" ] || fail "the backup did not complete in ${BACKUP_TIMEOUT}s (phase: ${phase:-none})"
	sleep 5
done
# The instant to recover to: after the backup, before the loss. A drill that recovers to "now"
# proves nothing about the target time an operator has to choose during an incident.
target="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
printf '  backup %s completed; target time %s\n' "$backup" "$target"

# --- 2. lose it -------------------------------------------------------------
# The Cluster object and its PVCs, which is what an incident takes: the data is gone and the
# archive is not. Everything else in the namespace stays up, so the drill also shows what the
# rest of the platform does while its database is missing.
step "2/5 the loss"
run kubectl delete clusters.postgresql.cnpg.io -n "$SLUG" postgres-cluster --wait=true
run kubectl delete pvc -n "$SLUG" -l cnpg.io/cluster=postgres-cluster --wait=true

# --- 3. bring it back -------------------------------------------------------
step "3/5 restore from the archive"
# The recovery environment reads the instant from the environment rather than carrying one:
# a drill's target is the run's own, and a committed timestamp would be the last drill's.
export JC_DR_TARGET_TIME="$target"
run helmfile -f ./deployment/helmfile-instance.yaml.gotmpl \
	sync -e "${ENVIRONMENT}-recovery" --selector component=postgres

deadline=$(($(date +%s) + RESTORE_TIMEOUT))
while :; do
	ready="$(kubectl get clusters.postgresql.cnpg.io -n "$SLUG" postgres-cluster \
		-o jsonpath='{.status.readyInstances}' 2>/dev/null || true)"
	# `readyInstances` is absent while the Cluster is bootstrapping, so it is compared as text
	# until it is a number: `[ "" -ge 1 ]` is an error, not a false.
	case "$ready" in
	'' | *[!0-9]*) ;;
	*) if [ "$ready" -ge 1 ]; then break; fi ;;
	esac
	[ "$(date +%s)" -lt "$deadline" ] || fail "the restored cluster had no ready instance after ${RESTORE_TIMEOUT}s"
	sleep 10
done
printf '  restored, %s instance(s) ready\n' "$ready"

run ./scripts/wait-rollouts.sh "$SLUG" "$RESTORE_TIMEOUT"

# --- 4 and 5. the platform, not just the database ---------------------------
if [ -z "$repo_dir" ]; then
	step "4/5 seed replay — skipped (JC_DR_REPO_DIR unset)"
	printf '\nthe database came back; the platform half of the drill did not run\n'
	exit 0
fi

gateway="${JC_DR_GATEWAY_URL:-http://context-gateway.${SLUG}.svc.cluster.local:9090}"
token_file="${JC_DR_TOKEN_FILE:-}"
token_args=()
if [ -n "$token_file" ]; then
	token_args=(--token-file "$token_file")
fi

step "4/5 replay the repository's seed entities"
run jcctl apply --repo-dir "$repo_dir" --gateway-url "$gateway" "${token_args[@]}"

step "5/5 parity"
# A second pass writes nothing when the platform holds what Git declares. The drill asserts the
# words `jcctl plan` uses for that, because an exit code of zero is also what a plan with
# changes in it returns.
plan="$(jcctl plan --repo-dir "$repo_dir" --gateway-url "$gateway" "${token_args[@]}")"
printf '%s\n' "$plan"
case "$plan" in
*"0 to add, 0 to change, 0 to delete"*) ;;
*) fail "the restored platform does not match the repository" ;;
esac

printf '\nthe instance came back from the archive and matches the repository\n'
