# Stateful data in the production-shaped namespaces outlives its claim (T-6718, G-5): a claim
# there names the Retain class `local-path-retain` (cluster/hetzner extra-manifests). The claims
# created before that class keep their class (storageClassName cannot change on a claim); their
# volumes were set to Retain by hand on 2026-11-11 (evidence/T-6718/after.tsv) and are named here.
package main

retain_namespaces := {"jc-prod", "jc-prod-mqtt", "jc-dev", "prod-final-backup"}

retain_class := "local-path-retain"

retained_by_hand := {
	"jc-prod/PersistentVolumeClaim/jc-files",
	"jc-prod/PersistentVolumeClaim/basemap-tiles",
	"jc-prod/Cluster/postgres-cluster",
	"jc-dev/Cluster/postgres-cluster",
}

volume_owner := sprintf("%s/%s/%s", [object.get(input, ["metadata", "namespace"], ""), input.kind, input.metadata.name])

# Each claim an object makes, with the class it names ("" for the cluster's default).
claim_class contains object.get(input, ["spec", "storageClassName"], "") if input.kind == "PersistentVolumeClaim"

claim_class contains object.get(t, ["spec", "storageClassName"], "") if {
	input.kind == "StatefulSet"
	some t in object.get(input, ["spec", "volumeClaimTemplates"], [])
}

claim_class contains object.get(input, ["spec", "storage", "storageClass"], "") if input.kind == "Cluster"

claim_class contains object.get(input, ["spec", "walStorage", "storageClass"], "") if {
	input.kind == "Cluster"
	input.spec.walStorage
}

deny contains msg if {
	object.get(input, ["metadata", "namespace"], "") in retain_namespaces
	not volume_owner in retained_by_hand
	some class in claim_class
	class != retain_class
	msg := sprintf("%s: a claim names class %q; data here outlives its claim only in %q (T-6718)", [volume_owner, class, retain_class])
}
