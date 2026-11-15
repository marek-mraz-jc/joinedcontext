#!/usr/bin/env bash
# Each Argo CD Application with its sync and health status, revision and last sync, failed and
# stuck ones first (docs Deployment/15-gitops.md §4, ADR-N-056 §2.9). Exit 1 when any is not
# Synced and Healthy, so the deployer's round can act on the code alone.
#   KUBECONFIG=.secrets/kubeconfig-dev.yaml scripts/gitops-status.sh [namespace]
set -euo pipefail

namespace="${1:-argocd}"
table=$(kubectl get applications.argoproj.io -n "$namespace" -o json | jq -r '
	[.items[] | {
		name: .metadata.name,
		sync: (.status.sync.status // "Unknown"),
		health: (.status.health.status // "Unknown"),
		revision: ((.status.sync.revision // "-")[0:7]),
		phase: (.status.operationState.phase // "-"),
		finished: (.status.operationState.finishedAt // "-"),
		message: ((.status.operationState.message // "") | gsub("\n"; " ") | .[0:120])
	}]
	| sort_by((if .sync == "Synced" and .health == "Healthy" then 1 else 0 end), .name)
	| (["APPLICATION", "SYNC", "HEALTH", "REVISION", "LAST SYNC", "FINISHED", "MESSAGE"] | @tsv),
	  (.[] | [.name, .sync, .health, .revision, .phase, .finished, .message] | @tsv),
	  (if all(.sync == "Synced" and .health == "Healthy") then empty else "NOT GREEN" end)
')
printf '%s\n' "$table"
if grep -qx "NOT GREEN" <<<"$table"; then exit 1; fi
