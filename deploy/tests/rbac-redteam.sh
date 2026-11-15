#!/usr/bin/env bash
# T-4775: replays the escalation attempts against the Portal's and the sandbox reaper's
# cluster-wide rights on a throwaway kind cluster, where the API server itself judges them.
# The kyverno CLI tests (test_portal_namespaces_policy.py, test_sandbox_reaper_policy.py) judge
# the same policies offline; this adds what only a live API server does: RBAC, Pod Security
# admission, impersonated identities and the namespaceObject a RoleBinding lands in.
#
# Never point it at dev or prod: it creates and deletes namespaces. It refuses a kubeconfig
# context that is not its own kind cluster.
#
# Usage: tests/rbac-redteam.sh            (creates and deletes kind cluster jc-rbac-redteam)
#        KEEP_CLUSTER=1 tests/rbac-redteam.sh
#        REDTEAM_KUBECONFIG=<kubeconfig of a local throwaway API server> tests/rbac-redteam.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CLUSTER="${CLUSTER:-jc-rbac-redteam}"
# Validating admission policies are GA since Kubernetes 1.30; kind's default node image is newer.
NODE_IMAGE="${NODE_IMAGE:-}"
WORK="$(mktemp -d)"
trap 'rm -rf "${WORK:?}"; [ -n "${KEEP_CLUSTER:-}" ] || [ -n "${REDTEAM_KUBECONFIG:-}" ] || kind delete cluster --name "$CLUSTER" >/dev/null 2>&1 || true' EXIT

export KUBECONFIG="$WORK/kubeconfig"
if [ -n "${REDTEAM_KUBECONFIG:-}" ]; then
	# A throwaway API server of the caller's (a sandbox where kind cannot boot runs etcd and
	# kube-apiserver as two containers). Only one on this machine: never dev, never prod.
	cp "$REDTEAM_KUBECONFIG" "$KUBECONFIG"
	KEEP_CLUSTER=1
	case "$(kubectl config view --minify -o jsonpath='{.clusters[0].cluster.server}')" in
	https://127.0.0.1:* | https://localhost:*) ;;
	*) echo "REDTEAM_KUBECONFIG is not a local API server, refusing" >&2; exit 2 ;;
	esac
else
	if ! kind get clusters 2>/dev/null | grep -qx "$CLUSTER"; then
		kind create cluster --name "$CLUSTER" ${NODE_IMAGE:+--image "$NODE_IMAGE"} --wait 120s >/dev/null
	fi
	kind export kubeconfig --name "$CLUSTER" --kubeconfig "$KUBECONFIG" >/dev/null
	[ "$(kubectl config current-context)" = "kind-$CLUSTER" ] || { echo "not the kind cluster, refusing" >&2; exit 2; }
fi

PORTAL=system:serviceaccount:dev:portal
REAPER=system:serviceaccount:ops:sandbox-reaper
k() { kubectl "$@"; }
# as <identity> <kubectl args...>: `-` is the cluster's own administrator, anything else impersonated.
as() {
	local who="$1"
	shift
	if [ "$who" = - ]; then k "$@"; else k --as="$who" "$@"; fi
}

# What the deployment installs, rendered from the charts as they are in this tree.
k create namespace dev --dry-run=client -o yaml | k apply -f - >/dev/null
k create namespace ops --dry-run=client -o yaml | k apply -f - >/dev/null
helm template portal-namespaces "$ROOT/components/portal/charts/namespaces" \
	--set release=dev --set serviceAccount.namespace=dev | k apply -f - >/dev/null
helm template sandbox-reaper "$ROOT/components/sandbox-reaper/charts/reaper" --namespace ops \
	--show-only templates/rbac.yaml --show-only templates/policy.yaml | k apply -f - >/dev/null
# The Portal's Role in its own namespace (components/portal/values/portal/base-values.yaml.gotmpl
# rbac.rules, and gitea-runner's portal-build-pods for Jobs), a platform credential beside it, and two ServiceAccounts that share
# that namespace on dev: the restore drill's and a token minter's.
k apply -f - >/dev/null <<-'EOF'
	apiVersion: v1
	kind: ServiceAccount
	metadata: {name: portal, namespace: dev}
	---
	apiVersion: v1
	kind: ServiceAccount
	metadata: {name: postgres-cluster-dr-drill, namespace: dev}
	---
	apiVersion: rbac.authorization.k8s.io/v1
	kind: Role
	metadata: {name: portal, namespace: dev}
	rules:
	  - apiGroups: [apps]
	    resources: [deployments]
	    resourceNames: [pipeline-runner]
	    verbs: [get, patch]
	  - apiGroups: [batch]
	    resources: [jobs]
	    verbs: [get, list, create, patch, delete]
	  - apiGroups: [""]
	    resources: [secrets]
	    resourceNames: [app-registry]
	    verbs: [get]
	  - apiGroups: [""]
	    resources: [secrets]
	    verbs: [create, patch]
	---
	apiVersion: rbac.authorization.k8s.io/v1
	kind: RoleBinding
	metadata: {name: portal, namespace: dev}
	roleRef: {apiGroup: rbac.authorization.k8s.io, kind: Role, name: portal}
	subjects: [{kind: ServiceAccount, name: portal, namespace: dev}]
	---
	apiVersion: v1
	kind: Secret
	metadata: {name: db-keycloak, namespace: dev}
	stringData: {password: platform-credential}
	---
	apiVersion: v1
	kind: Service
	metadata: {name: broker, namespace: dev}
	spec: {ports: [{port: 80}]}
	---
	apiVersion: v1
	kind: Namespace
	metadata: {name: victim}
	---
	apiVersion: v1
	kind: Namespace
	metadata:
	  name: sandbox-old
	  labels: {sandbox.joinedcontext.com/lifecycle: preview}
	EOF
# Admission policies are cached by the API server; give it a moment to load the new ones.
sleep 5

failures=0
# refused <name> <as> <kubectl args...>: the request must be refused; prints the refusal.
# EXPECT=<text> refused ...: and the refusal must name <text>, the guard under test.
refused() {
	local name="$1" as="$2"
	shift 2
	if out=$(as "$as" "$@" 2>&1); then
		echo "FAIL  $name: allowed: $out"
		failures=$((failures + 1))
	elif [ -n "${EXPECT:-}" ] && ! grep -q -- "$EXPECT" <<<"$out"; then
		echo "FAIL  $name: refused, but not by $EXPECT: $out"
		failures=$((failures + 1))
	else
		echo "ok    $name: $(echo "$out" | tail -n1 | cut -c1-240)"
	fi
}
allowed() {
	local name="$1" as="$2"
	shift 2
	if out=$(as "$as" "$@" 2>&1); then
		echo "ok    $name: allowed"
	else
		echo "FAIL  $name: refused: $out"
		failures=$((failures + 1))
	fi
}
cannot() {
	local name="$1" as="$2"
	shift 2
	if [ "$(k auth can-i --as="$as" "$@" 2>/dev/null)" = "no" ]; then
		echo "ok    $name: can-i no"
	else
		echo "FAIL  $name: can-i yes"
		failures=$((failures + 1))
	fi
}
ns() { # ns <name> <project> [extra label lines]
	printf 'apiVersion: v1\nkind: Namespace\nmetadata:\n  name: %s\n  labels:\n    joinedcontext.com/managed-by: joinedcontext-portal\n    joinedcontext.com/project: "%s"\n' "$1" "$2"
	shift 2
	for l in "$@"; do printf '    %s\n' "$l"; done
}
RESTRICTED=("pod-security.kubernetes.io/enforce: restricted" "pod-security.kubernetes.io/audit: restricted" "pod-security.kubernetes.io/warn: restricted")
binding() { # binding <namespace> <role kind> <role name> <subject sa namespace> <subject sa name>
	printf 'apiVersion: rbac.authorization.k8s.io/v1\nkind: RoleBinding\nmetadata: {name: joinedcontext-portal, namespace: %s}\nroleRef: {apiGroup: rbac.authorization.k8s.io, kind: %s, name: %s}\nsubjects: [{kind: ServiceAccount, namespace: %s, name: %s}]\n' "$@"
}
deployment() { # deployment <pod spec fields as one YAML flow map body> [name [namespace]]
	printf 'apiVersion: apps/v1\nkind: Deployment\nmetadata: {name: %s, namespace: %s}\nspec:\n  selector: {matchLabels: {app: probe}}\n  template:\n    metadata: {labels: {app: probe}}\n    spec: {%s, containers: [{name: c, image: registry.k8s.io/pause:3.10}]}\n' "${2:-probe}" "${3:-dev-ovzdusie-apps}" "$1"
}

echo "== Portal ServiceAccount: what RBAC alone grants"
cannot "list secrets cluster-wide" "$PORTAL" list secrets -A
cannot "get secrets in kube-system" "$PORTAL" get secrets -n kube-system
cannot "create pods/exec in dev" "$PORTAL" create pods/exec -n dev
cannot "impersonate users" "$PORTAL" impersonate users
cannot "escalate clusterroles" "$PORTAL" escalate clusterroles
cannot "bind cluster-admin" "$PORTAL" bind clusterroles/cluster-admin
cannot "create clusterrolebindings" "$PORTAL" create clusterrolebindings
cannot "create serviceaccounts/token in dev" "$PORTAL" create serviceaccounts/token -n dev
cannot "delete validatingadmissionpolicies" "$PORTAL" delete validatingadmissionpolicies

echo "== Portal ServiceAccount: namespaces"
export EXPECT=dev-portal-apps-namespaces
refused "namespace kube-system-apps" "$PORTAL" apply -f <(ns kube-system-apps system "${RESTRICTED[@]}")
refused "another project's name" "$PORTAL" apply -f <(ns dev-otherproject-apps ovzdusie "${RESTRICTED[@]}")
refused "apps namespace without Pod Security" "$PORTAL" apply -f <(ns dev-attack-apps attack)
refused "apps namespace enforcing privileged" "$PORTAL" apply -f <(ns dev-attack-apps attack "pod-security.kubernetes.io/enforce: privileged")
allowed "its own apps namespace" "$PORTAL" apply -f <(ns dev-ovzdusie-apps ovzdusie "${RESTRICTED[@]}")
refused "lower its own apps namespace to privileged" "$PORTAL" label --overwrite namespace dev-ovzdusie-apps pod-security.kubernetes.io/enforce=privileged
refused "delete a namespace not its own" "$PORTAL" delete namespace victim
refused "adopt a namespace not its own" "$PORTAL" label namespace victim joinedcontext.com/managed-by=joinedcontext-portal joinedcontext.com/project=victim

echo "== Portal ServiceAccount: RoleBindings"
unset EXPECT
k create namespace dev-foreign-apps --dry-run=client -o yaml | k apply -f - >/dev/null
EXPECT=dev-portal-apps-bindings refused "bind portal-apps in a dev-*-apps namespace it did not create" "$PORTAL" apply -f <(binding dev-foreign-apps ClusterRole dev-portal-apps dev portal)
refused "bind portal-apps in kube-system" "$PORTAL" apply -f <(binding kube-system ClusterRole dev-portal-apps dev portal)
refused "bind cluster-admin in its apps namespace" "$PORTAL" apply -f <(binding dev-ovzdusie-apps ClusterRole cluster-admin dev portal)
refused "bind portal-apps to another subject" "$PORTAL" apply -f <(binding dev-ovzdusie-apps ClusterRole dev-portal-apps dev-ovzdusie-apps default)
allowed "bind portal-apps to itself in its apps namespace" "$PORTAL" apply -f <(binding dev-ovzdusie-apps ClusterRole dev-portal-apps dev portal)

echo "== Portal ServiceAccount: platform workloads in the release namespace (T-5094)"
cannot "create deployments in dev" "$PORTAL" create deployments -n dev
cannot "patch another Deployment in dev" "$PORTAL" patch deployments/broker -n dev
cannot "delete the pipeline runner" "$PORTAL" delete deployments/pipeline-runner -n dev
cannot "delete services in dev" "$PORTAL" delete services -n dev
cannot "patch services in dev" "$PORTAL" patch services/broker -n dev
cannot "delete networkpolicies in dev" "$PORTAL" delete networkpolicies -n dev
cannot "create networkpolicies in dev" "$PORTAL" create networkpolicies -n dev
deployment "automountServiceAccountToken: false" pipeline-runner dev | k apply -f - >/dev/null
EXPECT=forbidden refused "create a Deployment in dev" "$PORTAL" apply --server-side -f <(deployment "automountServiceAccountToken: false" probe dev)
allowed "roll the pipeline runner" "$PORTAL" apply --server-side --force-conflicts --field-manager=portal-app-reconciler -f <(printf 'apiVersion: apps/v1\nkind: Deployment\nmetadata: {name: pipeline-runner, namespace: dev}\nspec: {template: {metadata: {annotations: {joinedcontext.com/pipeline-secrets: f1}}}}\n')

echo "== Portal ServiceAccount: workloads in its apps namespace"
export EXPECT=dev-portal-workloads
refused "Deployment as the restore drill with its token" "$PORTAL" apply -f <(deployment "serviceAccountName: postgres-cluster-dr-drill, automountServiceAccountToken: true")
refused "Deployment with a projected token" "$PORTAL" apply -f <(deployment "automountServiceAccountToken: false, volumes: [{name: t, projected: {sources: [{serviceAccountToken: {path: t}}]}}]")
refused "Deployment with the node's root" "$PORTAL" apply -f <(deployment "automountServiceAccountToken: false, volumes: [{name: r, hostPath: {path: /}}]")
refused "Deployment on the host network" "$PORTAL" apply -f <(deployment "automountServiceAccountToken: false, hostNetwork: true")
allowed "Deployment without a token" "$PORTAL" apply -f <(deployment "automountServiceAccountToken: false")
# A bare API server has no ServiceAccount controller; kind does, and the apply is a no-op there.
k create serviceaccount default -n dev-ovzdusie-apps --dry-run=client -o yaml | k apply -f - >/dev/null
# As the administrator: whoever creates the pod (the ReplicaSet controller for the Portal's
# Deployments), Pod Security in the apps namespace refuses one that is not restricted.
EXPECT=PodSecurity refused "a root pod in its apps namespace (Pod Security)" - run probe -n dev-ovzdusie-apps --image=registry.k8s.io/pause:3.10 --dry-run=server

unset EXPECT
echo "== Portal ServiceAccount: Secrets in the release namespace (T-4798)"
secret() { # secret <name> [type [annotations flow map]]
	printf 'apiVersion: v1\nkind: Secret\nmetadata: {name: %s, namespace: dev, annotations: {%s}}\ntype: %s\nstringData: {k: v}\n' "$1" "${3:-}" "${2:-Opaque}"
}
cannot "get the database secret" "$PORTAL" get secrets/db-keycloak -n dev
cannot "get the forge admin secret" "$PORTAL" get secrets/gitea-admin-credentials -n dev
cannot "get a build token secret" "$PORTAL" get secrets/build-bikes-42 -n dev
cannot "delete secrets in dev" "$PORTAL" delete secrets -n dev
export EXPECT=dev-portal-secrets
refused "overwrite the database secret by server-side apply" "$PORTAL" apply --server-side --force-conflicts -f <(secret db-keycloak)
refused "create a Secret under a platform name" "$PORTAL" apply --server-side -f <(secret keycloak-admin-user)
refused "a token-typed Secret under a handed-over name" "$PORTAL" apply --server-side -f <(secret artifact-store-reader-x kubernetes.io/service-account-token "kubernetes.io/service-account.name: postgres-cluster-dr-drill")
unset EXPECT
allowed "apply the pipeline runner's Secret" "$PORTAL" apply --server-side -f <(secret pipeline-secrets)
allowed "apply an organization's reader Secret" "$PORTAL" apply --server-side -f <(secret artifact-store-reader-hel)
if [ "$(k get secret db-keycloak -n dev -o jsonpath='{.data.password}' | base64 -d)" = platform-credential ]; then
	echo "ok    the database secret is unchanged"
else
	echo "FAIL  the database secret was changed"
	failures=$((failures + 1))
fi

echo "== sandbox reaper ServiceAccount"
export EXPECT=sandbox-reaper
refused "delete kube-node-lease" "$REAPER" delete namespace kube-node-lease --dry-run=server
refused "delete an unlabelled namespace" "$REAPER" delete namespace victim --dry-run=server
allowed "delete a preview sandbox" "$REAPER" delete namespace sandbox-old --dry-run=server
cannot "get secrets anywhere" "$REAPER" get secrets -A

if [ "$failures" -ne 0 ]; then
	echo "$failures attack(s) not refused" >&2
	exit 1
fi
echo "every attack refused"
