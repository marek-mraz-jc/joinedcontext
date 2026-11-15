# Context Gateway

Rust Context Gateway Policy Enforcement Point (PEP) guarding the context broker.

## APISIX Route Mapping

APISIX standalone mode derives upstream, route, and plugin_config IDs from the same map key.
While architecture docs describe shared `pc-*` plugin configs, each route in `apisix-plugins.yaml`
defines its dedicated plugin configuration under the matching route ID (`context-space` and
`context-endpoint`).

Deploy the component with:
```bash
helmfile apply -i --selector component=context-gateway
```

## Configuration

`JC_GATEWAY_BROKER_URL` and `JC_GATEWAY_ORG_DOMAIN` (from `global.orgDomain`) are required;
the JWKS is fetched in-cluster over plain http, the issuer is the public realm URL tokens carry,
and `JC_GATEWAY_PUBLIC_URL` is the apex host. The image is public and pinned by digest.

## The endpoint table

The gateway projects its endpoint table from the configuration repository (CC-08): without
`JC_GATEWAY_REPO_DIR` the table is empty and every slug answers 404, which looks like a routing
bug and is not one. The repository is the forge's: a `git-sync` sidecar (image pinned by digest
in `images.yaml`, the read-only token the forge bootstrap mints as `gitea-token-gateway`) keeps
a shallow checkout under `/repo`, the gateway reads `/repo/current` and re-reads it within a
second of a change (T-0278). The same container runs once as an init container, so the gateway
never starts on an empty table. An Endpoint a steward merges in the Portal serves within one
sync period (10 s).

What the table holds on `dev` is what the forge bootstrap commits (`seed/helsinki`, and the
conformance project in `seed/banskabystrica`: one context space `ovzdusie`, one public endpoint,
one read-only policy for the anonymous caller, the writing account and the `bb-air-quality`
data model). Each folder's `index.yaml` maps a file to its repository path.

The model is what `schema/v1/json-schema` and `schema/v1/context.jsonld` answer from (DM-22,
EP-49): without it the endpoint has no models and every schema URL is a 404. Its LinkML source
and the four artifacts generated from it live in `seed/banskabystrica/`, committed beside the
manifest. Regenerate the two the gateway serves after editing the source:

```bash
python3 ../../../joinedcontext-platform/tools/model-tools/src/gen_json_schema.py \
  seed/banskabystrica/bb-air-quality.linkml.yaml -o seed/banskabystrica/bb-air-quality.v1.schema.json
python3 ../../../joinedcontext-platform/tools/model-tools/src/gen_context.py \
  seed/banskabystrica/bb-air-quality.linkml.yaml -o seed/banskabystrica/bb-air-quality.v1.context.jsonld
```

The anonymous grant names the attributes it reaches, so `reliability` and `refDevice` are
absent from the entities, from the JSON Schema and from the `@context`, and listed in
`redactedSlots` of `schema/index.json` (EP-47). That is the least-privilege moment of the
demo, and it is one list in `policy.yaml`.
