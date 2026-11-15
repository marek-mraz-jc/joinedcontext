#!/usr/bin/env bash
# `just upgrade` end to end (T-3997, T-3794), what ci-full's `upgrade` job runs:
#
#   1. the previous release tag (the newest `v*` below HEAD) is installed into a fresh k3d cluster
#      by its own harness (the 0,0,0 variant k3d-deploy runs: one namespace, no mesh), kept up;
#   2. HEAD becomes a pre-release `v<next patch>-ci.<run>`: its notes written, committed and tagged
#      in this checkout only, never pushed;
#   3. every CloudNativePG cluster gets a backup to an S3 store beside the k3d nodes, so the
#      upgrade takes a real one rather than being waved through by --i-have-a-backup;
#   4. `just upgrade` to that tag must end green and write it into the ConfigMap jc-release;
#   5. the same upgrade again against a domain no smoke can reach must end red, every release it
#      advanced must be rolled back to the revision it had, nothing may be deleted, and jc-release
#      must still name the green run's version.
#
# Needs docker, k3d, helmfile, helm, kubectl, just and python3 with PyYAML. Exit 0 when all five
# hold. The cluster and the store stay up on failure, for the job's diagnostics step.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
ENV=testing                 # the scratch environment the harness writes
CLUSTER=jc-local            # the harness's cluster, its docker network k3d-jc-local
DOMAIN=joinedcontext.test   # what the harness installs under
WORK="$(mktemp -d)"

git fetch --quiet --tags origin 2>/dev/null || true
PREV="$(git tag --list 'v*' --sort=-v:refname --merged HEAD | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' | head -1 || true)"
if [ -z "$PREV" ]; then
	echo "no release tag v<major>.<minor>.<patch> below HEAD to upgrade from: the first one is cut" \
		"by the integrator (just release-notes v0.1.0, commit, tag)" >&2
	exit 1
fi
IFS=. read -r major minor patch <<<"${PREV#v}"
NEXT="v${major}.${minor}.$((patch + 1))-ci.${GITHUB_RUN_ID:-local}"
echo "upgrade: ${PREV} -> ${NEXT} (HEAD $(git rev-parse --short HEAD))"

# 1. the previous release, by its own harness
git worktree add --detach --quiet "$WORK/prev" "$PREV"
(cd "$WORK/prev" && ./scripts/test-deployment-variants.sh --keep 0,0,0)

# The upgrade runs from this checkout and copies its deployment/ into the release's tree; the
# environment is the one the previous release was installed with. Its harness removes that
# scratch file on exit, --keep included (ci-full 39335432708, T-4784), so it is written again
# here by the previous release's own write_env_overlay for variant 0,0,0.
JUST_NO_DOTENV=true just _dev-assemble >/dev/null
rm -rf "deployment/environments/${ENV}"
./scripts/prev-env-overlay.sh "$WORK/prev/scripts/test-deployment-variants.sh" \
	"deployment/environments/${ENV}/global.yaml.gotmpl"

# 2. HEAD as a pre-release
./scripts/release-notes.py "$NEXT"
git add releases
git -c user.name=ci -c user.email=ci@joinedcontext.invalid commit --quiet -m "release notes ${NEXT} (ci only, not pushed)"
git tag "$NEXT"

# 3. the backups: the platform's own pinned S3 store (components/artifact-store) on the k3d
# network, on 443, the one port the databases' NetworkPolicies let them reach off-cluster.
image() {
	python3 -c 'import sys, yaml
p = yaml.safe_load(open("components/artifact-store/images.yaml"))["artifact-store"][sys.argv[1]]
print("%s:%s@%s" % (p["repository"], p["tag"], p["digest"]))' "$1"
}
KEY="$(openssl rand -hex 12)"
SECRET="$(openssl rand -hex 24)"
docker run -d --name jc-upgrade-s3 --network "k3d-${CLUSTER}" \
	--sysctl net.ipv4.ip_unprivileged_port_start=443 --tmpfs /data:uid=10001 \
	-e RUSTFS_VOLUMES=/data -e RUSTFS_ADDRESS=0.0.0.0:443 \
	-e RUSTFS_ACCESS_KEY="$KEY" -e RUSTFS_SECRET_KEY="$SECRET" "$(image store)" >/dev/null
S3="http://$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' jc-upgrade-s3):443"
for _ in $(seq 30); do
	docker run --rm --network "k3d-${CLUSTER}" -e AWS_ACCESS_KEY_ID="$KEY" -e AWS_SECRET_ACCESS_KEY="$SECRET" \
		-e AWS_DEFAULT_REGION=us-east-1 "$(image bootstrap)" --endpoint-url "$S3" s3api create-bucket \
		--bucket backups >/dev/null 2>&1 && break
	sleep 2
done
kubectl get clusters.postgresql.cnpg.io -A -o jsonpath='{range .items[*]}{.metadata.namespace} {.metadata.name}{"\n"}{end}' |
	while read -r ns name; do
		kubectl -n "$ns" create secret generic jc-upgrade-s3 --from-literal=ACCESS_KEY_ID="$KEY" \
			--from-literal=ACCESS_SECRET_KEY="$SECRET" --dry-run=client -o yaml | kubectl apply -f - >/dev/null
		kubectl -n "$ns" patch clusters.postgresql.cnpg.io "$name" --type merge -p "{\"spec\":{\"backup\":{\"barmanObjectStore\":{
			\"destinationPath\":\"s3://backups/${ns}/${name}\",\"endpointURL\":\"${S3}\",\"s3Credentials\":{
			\"accessKeyId\":{\"name\":\"jc-upgrade-s3\",\"key\":\"ACCESS_KEY_ID\"},
			\"secretAccessKey\":{\"name\":\"jc-upgrade-s3\",\"key\":\"ACCESS_SECRET_KEY\"}}}}}}"
		kubectl -n "$ns" wait "clusters.postgresql.cnpg.io/${name}" --for=condition=ContinuousArchiving --timeout=300s
	done

# The upgrade's smoke runs scripts/smoke.sh from here, which checks every host over a verified
# chain (ci-full 39347776680: 39 checks answered 000). Each host the edge serves resolves to the
# k3d edge, and the harness's self-signed CA (cert-manager/ca-secret) is trusted beside the
# system roots, for curl, Python and helm alike; verification stays on.
./scripts/k3d-edge-hosts.sh "$DOMAIN" | sed 's/^/127.0.0.1 /' | sudo tee -a /etc/hosts >/dev/null
kubectl -n cert-manager get secret ca-secret -o 'jsonpath={.data.tls\.crt}' | base64 -d >"$WORK/edge-ca.pem"
[ -s "$WORK/edge-ca.pem" ] || { echo "no CA certificate in cert-manager/ca-secret" >&2; exit 1; }
cat /etc/ssl/certs/ca-certificates.crt "$WORK/edge-ca.pem" >"$WORK/ca-bundle.pem"
export CURL_CA_BUNDLE="$WORK/ca-bundle.pem" SSL_CERT_FILE="$WORK/ca-bundle.pem"

record() { kubectl get configmap -A --field-selector metadata.name=jc-release -o jsonpath='{.items[0].data.version}'; }

# The gate is scripts/edge-smoke.sh, what k3d-deploy proves after a fresh install: smoke.sh is the
# live installation's smoke and needs feeds, an Actions runner and APISIX as the outer hop, none of
# which k3d has (ci-full 39353463338, 39362707152: 35 red, every one of those classes; T-4795).
SMOKE=scripts/edge-smoke.sh

# 4. green
just upgrade "$ENV" "$NEXT" --domain "$DOMAIN" --smoke "$SMOKE"
echo "jc-release after the green upgrade: $(record)"
[ "$(record)" = "$NEXT" ]

# 5. red: the smoke cannot reach a domain under .invalid (RFC 2606), so the run must roll back
objects() {
	kubectl get deployments,statefulsets,services,configmaps,secrets,pvc,clusters.postgresql.cnpg.io -A \
		-o jsonpath='{range .items[*]}{.kind}/{.metadata.namespace}/{.metadata.name}{"\n"}{end}' | sort
}
objects >"$WORK/objects-before"
helm list -A -o json >"$WORK/helm-before.json"
if JC_EDGE_SMOKE_SECONDS=30 just upgrade "$ENV" "$NEXT" --domain smoke-red.invalid --smoke "$SMOKE" 2>&1 | tee "$WORK/red.log"; then
	echo "the upgrade against an unreachable domain ended green" >&2
	exit 1
fi
grep -q "RED: rolling back every helm release this run changed" "$WORK/red.log"
helm list -A -o json >"$WORK/helm-after.json"
python3 - "$WORK/helm-before.json" "$WORK/helm-after.json" <<'PY'
import json, subprocess, sys
before = {(r["namespace"], r["name"]): int(r["revision"]) for r in json.load(open(sys.argv[1]))}
after = {(r["namespace"], r["name"]): r for r in json.load(open(sys.argv[2]))}
wrong = []
for (ns, name), revision in sorted(before.items()):
    if int(after[(ns, name)]["revision"]) == revision:
        continue  # not advanced by the red run, nothing to roll back
    last = json.loads(subprocess.run(["helm", "history", name, "-n", ns, "--max", "1", "-o", "json"],
                                     check=True, capture_output=True, text=True).stdout)[-1]
    print(f"rollback {ns}/{name}: {last['description']}")
    if last["description"] != f"Rollback to {revision}" or last["status"] != "deployed":
        wrong.append(f"{ns}/{name}: {last['status']} {last['description']!r}, wanted 'Rollback to {revision}'")
if wrong:
    sys.exit("not rolled back:\n  " + "\n  ".join(wrong))
PY
objects >"$WORK/objects-after"
deleted="$(comm -23 "$WORK/objects-before" "$WORK/objects-after")"
if [ -n "$deleted" ]; then
	echo "the red run deleted:" >&2
	echo "$deleted" >&2
	exit 1
fi
echo "jc-release after the red upgrade: $(record)"
[ "$(record)" = "$NEXT" ]
echo "upgrade ${PREV} -> ${NEXT}: green applied and recorded, red rolled back, nothing deleted"
