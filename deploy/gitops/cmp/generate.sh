#!/usr/bin/env bash
# Argo CD's helmfile plugin (ADR-N-056 §2.2, OPS-54, OPS-55): what one component of one
# environment renders to, as `just dev-apply` would apply it.
#
# Argo CD runs it in the plugin sidecar, in its own copy of the repository, with the
# Application's plugin env as ARGOCD_ENV_ENVIRONMENT, ARGOCD_ENV_COMPONENT and, for a component
# whose releases span namespaces, ARGOCD_ENV_NAMESPACE; the sidecar is images/gitops-helmfile
# (helmfile, helm, yq). Locally:
#   ARGOCD_ENV_ENVIRONMENT=dev ARGOCD_ENV_COMPONENT=viz gitops/cmp/generate.sh
set -euo pipefail

env="${ARGOCD_ENV_ENVIRONMENT:?the Application names no environment}"
component="${ARGOCD_ENV_COMPONENT:?the Application names no component}"
namespace="${ARGOCD_ENV_NAMESPACE:-}"
[[ "$env" =~ ^[a-z0-9-]+$ && "$component" =~ ^[a-z0-9-]+$ && "$namespace" =~ ^[a-z0-9-]*$ ]] || {
	echo "refusing environment '$env', component '$component' or namespace '$namespace'" >&2
	exit 2
}
selector="component=$component${namespace:+,namespace=$namespace}"

# The smoke gate (ADR-N-056 §2.6): its own chart, carrying the revision it runs for, in the
# plugin's own image.
if [ "$component" = smoke ]; then
	image=$(yq '.gitops.helmfile | .repository + ":" + .tag + "@" + .digest' components/gitops/images.yaml)
	exec helm template "$env-smoke" gitops/smoke --namespace "${ARGOCD_APP_NAMESPACE:-argocd}" \
		-f "gitops/$env.yaml" --set-string "revision=${ARGOCD_APP_REVISION:?}" --set-string "image=$image"
fi

# The assemble step of `just _dev-assemble`, on this copy of the repository.
rm -rf deployment
cp -r defaults/deployment deployment
cp ci/example-deployments/helmfile.yaml deployment/helmfile.yaml
cp -r ci/example-deployments/environments/. deployment/environments/

# GITOPS_SKIP_DEPS=1 only where the chart repositories are already added (the tests); the
# sidecar starts with none, so it lets helmfile add them.
skip=()
[ "${GITOPS_SKIP_DEPS:-}" = 1 ] && skip=(--skip-deps)

# Helm test hooks never run on a sync, so they are no part of what Argo CD applies. Every
# object of a kind that holds data or owns other objects is never pruned or deleted (OPS-55).
helmfile -f deployment/helmfile.yaml -e "$env" -l "$selector" template \
	"${skip[@]+"${skip[@]}"}" -q --include-crds |
	yq eval-all '
		select(. != null and (.kind // "") != "")
		| select(((.metadata.annotations["helm.sh/hook"] // "") | test("(^|,)test(-success|-failure)?(,|$)")) | not)
		| (select(.kind == "PersistentVolumeClaim" or .kind == "Secret" or .kind == "Namespace"
			or .kind == "CustomResourceDefinition"
			or (.kind == "Cluster" and .apiVersion == "postgresql.cnpg.io/v1"))
			| .metadata.annotations["argocd.argoproj.io/sync-options"]) = "Prune=false,Delete=false"
	'
