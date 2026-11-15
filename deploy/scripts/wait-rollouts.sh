#!/usr/bin/env bash
# Wait for every workload of a deployed instance to become Ready and report what did not.
#   scripts/wait-rollouts.sh <instance-slug> [timeout-seconds] [release...]
# With release names (`just dev-apply-pins`, T-6155) it waits only for the Deployments, StatefulSets
# and DaemonSets those Helm releases own (annotation meta.helm.sh/release-name) and fails when one
# does not roll out: a pin of the Portal waits neither on CNPG nor on an unrelated pod.
set -euo pipefail

slug="${1:?usage: wait-rollouts.sh <instance-slug> [timeout] [release...]}"
timeout="${2:-600}"
shift $(( $# < 2 ? $# : 2 ))

namespaces=$(kubectl get ns -o name | sed 's|namespace/||' | grep -E "^(${slug}|${slug}-.+|jc-operators)$" || true)
[ -n "$namespaces" ] || { echo "no namespaces for instance '${slug}'" >&2; exit 1; }

deadline=$(( $(date +%s) + timeout ))

if [ "$#" -gt 0 ]; then
	echo "waiting up to ${timeout}s for the rollouts of: $*"
	failed=0 n=0
	for ns in $namespaces; do
		# a plain assignment, so an unreadable listing stops the wait (a loop list hides its exit)
		owned=$(kubectl get deploy,statefulset,daemonset -n "$ns" -o json | python3 -c '
import json, sys
wanted = set(sys.argv[1:])
for o in json.load(sys.stdin)["items"]:
    if (o["metadata"].get("annotations") or {}).get("meta.helm.sh/release-name") in wanted:
        print(o["kind"].lower() + "/" + o["metadata"]["name"])' "$@")
		for res in $owned; do
			n=$((n + 1))
			left=$(( deadline - $(date +%s) )); [ "$left" -gt 0 ] || left=1
			kubectl rollout status "$res" -n "$ns" --timeout="${left}s" || { failed=1; echo "--- not rolled out: ${ns}/${res}"; }
		done
	done
	[ "$failed" -eq 0 ] && echo "${n} workload(s) of $* rolled out"
	exit "$failed"
fi

echo "waiting up to ${timeout}s for rollouts in: $(echo "$namespaces" | tr '\n' ' ')"
for ns in $namespaces; do
	for res in $(kubectl get deploy,statefulset,daemonset -n "$ns" -o name 2>/dev/null); do
		left=$(( deadline - $(date +%s) ))
		[ "$left" -gt 0 ] || { left=1; }
		kubectl rollout status "$res" -n "$ns" --timeout="${left}s" || true
	done
done

# CloudNativePG clusters are not rollouts; wait for the operator to report them ready.
for ns in $namespaces; do
	for cl in $(kubectl get cluster.postgresql.cnpg.io -n "$ns" -o name 2>/dev/null || true); do
		left=$(( deadline - $(date +%s) )); [ "$left" -gt 0 ] || left=1
		kubectl wait "$cl" -n "$ns" --for=condition=Ready --timeout="${left}s" || true
	done
done

failed=0
for ns in $namespaces; do
	# A pod already being deleted belongs to a rollout that finished; once its containers exit it
	# shows Error or Completed rather than Terminating, so skip it by its deletionTimestamp.
	deleting=$(kubectl get pods -n "$ns" -o jsonpath='{range .items[?(@.metadata.deletionTimestamp)]}{.metadata.name}{"\n"}{end}' 2>/dev/null || true)
	# A Job's pod in Error is one attempt; the Job retries it and removes it (a CronJob's pod
	# did, mid-apply). Only a Job whose `Failed` condition is True is a failure of the apply, unless
	# its CronJob ran again later and that run completed: the CronJob kept the failed run's pod
	# (failedJobsHistoryLimit) but works now (a lane-token run while the upgrade restarted Gitea).
	owners=$(kubectl get pods -n "$ns" -o jsonpath='{range .items[*]}{.metadata.name}{" "}{.metadata.ownerReferences[?(@.kind=="Job")].name}{"\n"}{end}' 2>/dev/null || true)
	failed_jobs=$(kubectl get jobs -n "$ns" -o jsonpath='{range .items[*]}{.metadata.name}{"|"}{.status.conditions[?(@.type=="Failed")].status}{"|"}{.status.conditions[?(@.type=="Complete")].status}{"|"}{.metadata.ownerReferences[?(@.kind=="CronJob")].name}{"|"}{.metadata.creationTimestamp}{"\n"}{end}' 2>/dev/null | awk -F'|' '
		{ name[NR] = $1; failed[NR] = $2; cron[NR] = $4; at[NR] = $5 }
		$3 == "True" && $4 != "" && $5 > done[$4] { done[$4] = $5 }
		END { for (i = 1; i <= NR; i++) if (failed[i] == "True" && !(cron[i] != "" && done[cron[i]] > at[i])) print name[i] }' || true)
	while read -r pod; do
		[ -n "$pod" ] || continue
		grep -qxF "$pod" <<<"$deleting" && continue
		job=$(awk -v pod="$pod" '$1 == pod {print $2}' <<<"$owners")
		# A CronJob may start a pod after the snapshot above: read that pod's owner now, and a
		# pod already gone was one of those.
		if ! grep -q "^${pod} " <<<"$owners"; then
			job=$(kubectl get pod "$pod" -n "$ns" -o jsonpath='{.metadata.ownerReferences[?(@.kind=="Job")].name}' 2>/dev/null) || continue
		fi
		if [ -n "$job" ] && ! grep -qxF "$job" <<<"$failed_jobs"; then continue; fi
		failed=1
		echo "--- not ready: ${ns}/${pod}"
		kubectl get pod "$pod" -n "$ns" -o wide
		kubectl get events -n "$ns" --field-selector "involvedObject.name=${pod}" \
			--sort-by=.lastTimestamp -o wide 2>/dev/null | tail -5
	done < <(kubectl get pods -n "$ns" --no-headers 2>/dev/null | awk '
		$3 == "Completed" || $3 == "Succeeded" { next }        # finished Jobs, not workloads
		$3 == "Terminating" { next }                           # the old pod of a rollout that already succeeded
		{ split($2, ready, "/"); if ($3 != "Running" || ready[1] != ready[2]) print $1 }')
done

[ "$failed" -eq 0 ] && echo "all workloads ready in: $(echo "$namespaces" | tr '\n' ' ')"
exit "$failed"
