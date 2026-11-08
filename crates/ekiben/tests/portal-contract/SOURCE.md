# Portal contract fixtures

788 streams the Portal (crates/portal at 3ef832e) hands its runner for the seed of joinedcontext-deployment cee6ebcb9, rendered by tools/portal-dump (the Portal's StreamDeployer::converge, pipeline identity and validation on) and written by scripts/portal-contract/regen.py. resources.yaml is that deployment's components/pipeline-runner/streams/resources.yaml; proto/ holds its components/pipeline-runner/streams/gtfs-realtime.proto (Apache-2.0, google/transit), which the runner mounts at /proto.
