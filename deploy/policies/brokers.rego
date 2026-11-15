# Each ContextBroker is its own island (ADR-N-046 §2.5, T-3989): a broker's database admits its
# own broker and no other, and a broker reaches no other broker's database. `main` runs as
# context-broker-broker on the platform's postgres-cluster; every further broker as
# context-broker-{name} on its own context-broker-{name}-db. Checked from both sides, since
# either policy alone opens the path.
package main

broker_of(cluster) := "context-broker-broker" if cluster == "postgres-cluster"

broker_of(cluster) := trim_suffix(cluster, "-db") if {
	startswith(cluster, "context-broker-")
	endswith(cluster, "-db")
}

is_broker(name) if startswith(name, "context-broker-")

deny contains msg if {
	input.kind == "NetworkPolicy"
	own := broker_of(input.spec.podSelector.matchLabels["cnpg.io/cluster"])
	some rule in input.spec.ingress
	some peer in rule.from
	name := peer.podSelector.matchLabels["app.kubernetes.io/name"]
	is_broker(name)
	name != own
	msg := sprintf("NetworkPolicy/%s: the database of %s admits the broker %s", [input.metadata.name, own, name])
}

# A peer naming no pod admits every pod of its namespaces, other brokers among them.
deny contains msg if {
	input.kind == "NetworkPolicy"
	own := broker_of(input.spec.podSelector.matchLabels["cnpg.io/cluster"])
	startswith(input.spec.podSelector.matchLabels["cnpg.io/cluster"], "context-broker-")
	some rule in input.spec.ingress
	some peer in object.get(rule, "from", [{}])
	count(object.get(object.get(peer, "podSelector", {}), "matchLabels", {})) == 0
	msg := sprintf("NetworkPolicy/%s: the database of %s admits a peer that names no pod", [input.metadata.name, own])
}

deny contains msg if {
	input.kind == "NetworkPolicy"
	name := input.spec.podSelector.matchLabels["app.kubernetes.io/name"]
	is_broker(name)
	some rule in input.spec.egress
	some peer in rule.to
	cluster := peer.podSelector.matchLabels["cnpg.io/cluster"]
	owner := broker_of(cluster)
	owner != name
	msg := sprintf("NetworkPolicy/%s: the broker %s reaches the database of %s", [input.metadata.name, name, owner])
}

# A broker port answers the gateway and the Portal alone inside the mesh (T-4875): the broker
# believes the x-jc-* subject a caller sends, so any other identity on its Server could speak for
# anyone. A broker's MeshTLSAuthentication names those two ServiceAccounts and its own (a call to
# itself through its Service, which carries no subject), nothing else.
broker_callers := {"context-gateway", "portal"}

broker_self(ref) if {
	ref.name == trim_suffix(input.metadata.name, "-api-clients")
	object.get(ref, "namespace", input.metadata.namespace) == input.metadata.namespace
}

deny contains msg if {
	input.kind == "MeshTLSAuthentication"
	startswith(input.metadata.name, "context-broker")
	some ref in input.spec.identityRefs
	not ref.kind == "ServiceAccount"
	msg := sprintf("MeshTLSAuthentication/%s: a broker admits ServiceAccounts alone, not %s", [input.metadata.name, ref.kind])
}

deny contains msg if {
	input.kind == "MeshTLSAuthentication"
	startswith(input.metadata.name, "context-broker")
	some ref in input.spec.identityRefs
	not broker_callers[ref.name]
	not broker_self(ref)
	msg := sprintf("MeshTLSAuthentication/%s: the broker admits %s/%s, outside the gateway and the Portal", [input.metadata.name, object.get(ref, "namespace", ""), ref.name])
}

# A caller's ServiceAccount counts only in its component's own namespace (T-5702): a
# ServiceAccount named portal in an App namespace is not the Portal. The broker shares one
# namespace with both on a single-namespace install; otherwise each sits in
# {instance}-{component}, as jc.namespace names them.
broker_instance := trim_suffix(input.metadata.namespace, "-context-broker")

caller_namespace_ok(ref) if object.get(ref, "namespace", input.metadata.namespace) == input.metadata.namespace

caller_namespace_ok(ref) if ref.namespace == sprintf("%s-%s", [broker_instance, ref.name])

deny contains msg if {
	input.kind == "MeshTLSAuthentication"
	startswith(input.metadata.name, "context-broker")
	some ref in input.spec.identityRefs
	broker_callers[ref.name]
	not caller_namespace_ok(ref)
	msg := sprintf("MeshTLSAuthentication/%s: the broker admits %s only from its own namespace %s, not %s", [input.metadata.name, ref.name, sprintf("%s-%s", [broker_instance, ref.name]), object.get(ref, "namespace", "")])
}

deny contains msg if {
	input.kind == "MeshTLSAuthentication"
	startswith(input.metadata.name, "context-broker")
	count(object.get(input.spec, "identities", [])) > 0
	msg := sprintf("MeshTLSAuthentication/%s: a broker names its callers by ServiceAccount, not by identity pattern", [input.metadata.name])
}
