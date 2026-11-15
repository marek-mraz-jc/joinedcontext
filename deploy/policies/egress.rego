# Workloads that fetch an address somebody else chose (a DataSource URL a person types and a
# check probes, a subscriber a subscription names) reach public addresses only (T-0752, MF-39).
# An egress range that leaves a private range reachable lets that address steer the workload at
# the Kubernetes API, a node, a webhook or a metadata service.
package main

# The ranges ADR-N-040's SSRF check treats as private, per address family: carrier-grade NAT
# (100.64.0.0/10) included, and loopback, unique-local and link-local for IPv6 (T-5702).
private_ranges := {"10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16", "127.0.0.0/8", "100.64.0.0/10"}

private_ranges_v6 := {"::1/128", "fc00::/7", "fe80::/10"}

private_for(cidr) := private_ranges_v6 if contains(cidr, ":")

private_for(cidr) := private_ranges if not contains(cidr, ":")

fetches_chosen_addresses(policy) if {
	policy.spec.podSelector.matchLabels["app.kubernetes.io/name"] in {"pipeline-runner-runner", "context-gateway-gateway", "context-broker-broker"}
}

# The catalogue harvest fetches the download URLs a national catalogue lists (T-4151).
fetches_chosen_addresses(policy) if {
	policy.spec.podSelector.matchLabels["app.kubernetes.io/name"] == "ckan"
	policy.spec.podSelector.matchLabels["app.kubernetes.io/component"] == "harvest"
}

deny contains msg if {
	input.kind == "NetworkPolicy"
	fetches_chosen_addresses(input)
	some rule in input.spec.egress
	some peer in rule.to
	excepted := {cidr | some cidr in object.get(peer.ipBlock, "except", [])}
	missing := private_for(peer.ipBlock.cidr) - excepted
	count(missing) > 0
	msg := sprintf("NetworkPolicy/%s: egress to %s leaves %v reachable", [input.metadata.name, peer.ipBlock.cidr, sort(missing)])
}

# An egress rule without peers reaches every address, private ranges included (T-5702).
deny contains msg if {
	input.kind == "NetworkPolicy"
	fetches_chosen_addresses(input)
	some rule in input.spec.egress
	count(object.get(rule, "to", [])) == 0
	msg := sprintf("NetworkPolicy/%s: an egress rule without `to` reaches every address, private ranges included", [input.metadata.name])
}
