# Portal contract fixtures

788 streams the Portal (crates/portal at 3ef832e) hands its runner for the seed of joinedcontext-deployment cee6ebcb9, rendered by tools/portal-dump (the Portal's StreamDeployer::converge, pipeline identity and validation on) and written by scripts/portal-contract/regen.py. resources.yaml is that deployment's components/pipeline-runner/streams/resources.yaml; proto/ holds its components/pipeline-runner/streams/gtfs-realtime.proto (Apache-2.0, google/transit), which the runner mounts at /proto.

The Portal renders no stream for these (its own refusal, no runner call):

- praha/golemio-garages: Error("bento.yaml: invalid pipeline.processors `${DS_GOLEMIO_MR_PARKIT_TOKEN}`: a step names no runner variable but JC_GATEWAY_URL, JC_GATEWAY_HOST, JC_TOKEN_URL, JC_ORG_DOMAIN, JC_CLIENT_ID, JC_CLIENT_SECRET, the JC_SPACE names and the envVar of its own pipeline's secretRefs: the rest of the runner's environment is other projects' credentials (PL-16)")
