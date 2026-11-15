# The four properties every workload of the platform has to declare, asserted before the
# manifest reaches a cluster (TS-18, TS-19, CC-12). Kyverno asserts the same four at
# admission; this is the shift-left copy that fails a pull request instead of a rollout.
package main

workload_kinds := {"Deployment", "StatefulSet", "DaemonSet", "Job", "ReplicaSet"}

pod_spec := input.spec.template.spec if input.kind in workload_kinds

pod_spec := input.spec.jobTemplate.spec.template.spec if input.kind == "CronJob"

pod_spec := input.spec if input.kind == "Pod"

# A Helm *test* hook is never applied by `helmfile sync`, so it is not a workload of the
# deployment; `scripts/render.sh --deployed` and the Kyverno gate drop it for the same reason.
# Every other hook (post-install and friends) is applied and is held to all of this.
helm_test_hook if "test" in split(object.get(input, ["metadata", "annotations", "helm.sh/hook"], ""), ",")

containers contains container if some container in pod_spec.containers

containers contains container if some container in pod_spec.initContainers

subject := sprintf("%s/%s", [input.kind, object.get(input, ["metadata", "name"], "<unnamed>")])

# 1. Non-root. A container that never says so runs as whatever the image's USER is, which for
#    most upstream images is uid 0.
non_root(container) if container.securityContext.runAsNonRoot == true

# The pod's value holds for a container that does not set its own: Kubernetes lets the container's
# runAsNonRoot override the pod's, so a container saying false runs as root under a pod saying true.
non_root(container) if {
	object.get(container, ["securityContext", "runAsNonRoot"], null) == null
	pod_spec.securityContext.runAsNonRoot == true
}

deny contains msg if {
	not helm_test_hook
	some container in containers
	not non_root(container)
	msg := sprintf("%s: container %q does not set runAsNonRoot", [subject, container.name])
}

deny contains msg if {
	not helm_test_hook
	some container in containers
	container.securityContext.runAsUser == 0
	msg := sprintf("%s: container %q asks for uid 0", [subject, container.name])
}

# 2. Every capability dropped. `drop: [ALL]` is the whole of the PSS restricted requirement;
#    an `add` beside it is a deliberate exception and belongs in a policy exception, not here.
deny contains msg if {
	not helm_test_hook
	some container in containers
	not "ALL" in object.get(container, ["securityContext", "capabilities", "drop"], [])
	msg := sprintf("%s: container %q does not drop ALL capabilities", [subject, container.name])
}

# 3. A memory limit. Without one, a single leaking pod evicts its neighbours on the node.
deny contains msg if {
	not helm_test_hook
	some container in containers
	not object.get(container, ["resources", "limits", "memory"], false)
	msg := sprintf("%s: container %q declares no memory limit", [subject, container.name])
}

# 4. Its own ServiceAccount. The `default` one is shared by everything in the namespace, so a
#    RoleBinding meant for one workload reaches all of them.
deny contains msg if {
	not helm_test_hook
	object.get(pod_spec, "serviceAccountName", "default") == "default"
	# The default account is shared by everything in the namespace, so a RoleBinding meant for
	# one workload reaches all of them. A pod that is handed no token cannot use the identity,
	# which is the whole of the risk, so saying so is the other way to satisfy this.
	object.get(pod_spec, "automountServiceAccountToken", true) == true
	msg := sprintf("%s: runs under the default ServiceAccount with its token mounted", [subject])
}
