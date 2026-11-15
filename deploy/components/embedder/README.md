# embedder

The embedding model (`multilingual-e5-small`, int8 ONNX) as one service several deployments
share: monorepo `crates/embedder`, binary `jc-embedder`, image `ghcr.io/marek-mraz-jc/jc-embedder`.
Today each assistant loads the model in its own process (about 300 MB each); with this component
one pod serves them all.

The component is opt-in. An environment turns it on by listing `embedder` in `components:` of
its `global.yaml.gotmpl`, after `jc`, its one caller (`componentDependencies`). Only `addons`
lists it, as a render surface with an example image pin
(`environments/addons/embedder.yaml.gotmpl`).

```bash
helmfile apply -i --selector component=embedder
```

## 1. What it deploys

One release of `charts/workload`, `embedder-server`:

- **Deployment `embedder`**, one replica, image pinned by digest in `images.yaml`. It starts
  `jc-embedder --config /etc/jc-embedder/embedder.toml` (ConfigMap `embedder`): listener 8089,
  `POST /v1/embeddings` and `GET /healthz`. The image carries the model files and ONNX
  Runtime, each checked against its SHA-256 when the image is built and again before the model
  loads.
- **Service `embedder`** on 8089. No volume: the service stores nothing.

## 2. Callers

Each calling deployment is one entry of `embedder.server.deployments`:

```yaml
embedder:
  server:
    deployments:
      - name: dev                      # its name in metrics and logs
        issuer: https://jc.dev.example/id/realms/jc
        jwksUrl: https://jc.dev.example/id/realms/jc/protocol/openid-connect/certs
        clients: [jc-assistant]        # the client ids (azp) it calls with
        maxTextsPerMinute: 1200        # its quota
```

A caller signs in with a client-credentials token its own kaisatsu signed for audience
`embedder` (scope `embedder`), the check jc's door makes (monorepo `crates/shared/jc-door`). The
token's issuer picks the deployment; a token of another issuer, another audience, an unlisted
client or past its expiry is refused 401. Quota, the in-flight limit and the metrics count per
deployment, never per organization. The render refuses an empty list.

The calling deployment's kaisatsu needs a client (`jc-assistant`) with an optional client scope
`embedder` whose audience mapper adds `embedder` to `aud`; the assistant reads
`JC_ASSISTANT_EMBEDDER_URL` (`http://embedder.<namespace>.svc.cluster.local:8089`),
`JC_ASSISTANT_EMBEDDER_TOKEN_URL`, `JC_ASSISTANT_EMBEDDER_CLIENT_ID` and
`JC_ASSISTANT_EMBEDDER_CLIENT_SECRET_FILE`.

## 3. Network

`networkpolicies.yaml`: ingress on 8089 from jc's pods only (`jc-server` in jc's namespace),
with jc's egress half (`jc-egress-to-embedder`); egress to DNS, the cluster ingress on 443 (an
in-cluster kaisatsu's JWKS at its public issuer) and public addresses on 443 (a deployment
elsewhere). The Linkerd halves on 4143 are in `networkpolicies-linkerd.yaml`, and meshed, the
Linkerd Server on 8089 admits jc's identity alone (`meshAuthorization`). A deployment in another
cluster has no way in yet: it needs an edge route first.

## 4. Resources

Measured 2026-11-09 on the release binary (monorepo `crates/embedder/tests/load.rs`), host load
about 31 on 16 cores: the model is ready 1.2 s after start; 436 MiB resident idle, 513 MiB peak
while two deployments sent 96 passages of 1.1 KB each at once; 12.2 texts/s with 2 ONNX
threads (12.8 with 1, 10.4 with 4: the loaded host gave more threads nothing); a question asked
during that indexing took 66 ms on average, 87 ms at most. Requests 512 Mi (the peak), limits
768 Mi and 2 CPUs (the thread count). Measure again on the first stage apply, on a quiet node.

One replica: the model runs one text at a time (the int8 export's vectors depend on the batch
otherwise), and one queue puts every deployment's question before any passage.

## 5. Before an environment lists `embedder`

1. Pin the first digest `image-embedder.yml` signed on monorepo `main` in `images.yaml`.
2. Name the calling deployments in `embedder.server.deployments`.
3. Add the `embedder` client scope to each caller's kaisatsu realm and point its assistant at the
   service (section 2).
4. No ServiceMonitor scrapes it yet: the chart sets no `metrics_addr`, so the per-deployment
   counters (`embedder_texts_total`, `embedder_refusals_total`, `embedder_request_seconds`) wait
   for a scrape rule.
