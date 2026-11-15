# Assistant

`jc-assistant` from the platform image (docs Architecture/22 §1, ADR-N-040, T-3052, T-3055): the
knowledge assistant. Once a minute it reads the organization's `KnowledgeSource` and
`AssistantDeployment` manifests, queues every source its schedule names, crawls the public site
or the catalogue, and stores and embeds the passages in its own database. Its chat route,
`https://assistant.{domain}/api/v1/d/{publicId}/chat` (docs API/05), answers the public
deployments. One Deployment and a ClusterIP Service on 8080; the kubelet probes `/healthz` and
`/readyz` on the same port.

Deploy the component with:
```bash
helmfile apply -i --selector component=assistant
```

## Turning it on

No environment lists `assistant` yet. The platform image pinned in `images.yaml` is the
gateway's, which predates the binary; pin a digest whose build contains
`/usr/local/bin/jc-assistant` with the chat (platform T-3055 and later), and the agent proxy's
digest with the assistant caller (same build), then add `assistant` after `functions` in the
environment's component list. It needs `gitea`, `agent-runner`, `context-gateway` and
`keycloak` listed, and says so if one is missing.

## What it wires

- **Edge**: `assistant-chat` on `assistant.{domain}`, no login, 30 requests a minute per address
  at the edge; the service limits each deployment and client again and checks the Origin.
- **Keycloak**: the `jc-assistant` client, service account only, audience `helsinki-agent-proxy`;
  federated (PF-47): it proves itself with the worker pod's projected ServiceAccount token and
  holds no secret. The proxy serves its token for model calls named for a deployment, counted
  against that deployment's day.

- **PostgreSQL**: the `assistant` database and role (`databases.yaml`), the password in
  `db-assistant`, the `vector` extension created by the role job. The worker runs its own
  migrations as that role, so the forced row-level security of every tenant table holds.
- **Forge**: git-sync and, in layout 2, the checkouts sidecar, both with the read-only
  `gitea-token-gateway`; the forge bootstrap mints that token into this namespace and
  restarts `jc-assistant` when it mints it again.
- **Network**: ingress from APISIX on 8080; egress to PostgreSQL, the forge, the agent proxy,
  the gateway (connectors' MCP surfaces), Keycloak, CoreDNS, TCP 80/443 on every public address
  and the ingress controller (the platform's own CKAN is the cluster's public host, T-3054).
  Every private range is excepted, so a source URL never reaches the cluster.
- **Embeddings**: the model and ONNX Runtime are in the image; one ONNX thread, as many as the
  CPU limit (T-3053).

## Sizing

One crawl at a time; a PDF and its text sit in memory while it is extracted, so the limit is
1Gi and the request what the idle loop uses.
