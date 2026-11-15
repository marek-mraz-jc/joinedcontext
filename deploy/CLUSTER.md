# joinedcontext-deployment

[![ci](https://github.com/marek-mraz-jc/joinedcontext-deployment/actions/workflows/ci.yml/badge.svg)](https://github.com/marek-mraz-jc/joinedcontext-deployment/actions/workflows/ci.yml)
[![ci-full](https://github.com/marek-mraz-jc/joinedcontext-deployment/actions/workflows/ci-full.yml/badge.svg)](https://github.com/marek-mraz-jc/joinedcontext-deployment/actions/workflows/ci-full.yml)

The Helmfile deployment of the **joinedcontext platform**: one component per folder, one
environment per values file, and a rendered manifest that is gated before it ever reaches a
cluster. Forked from the CIVITAS/CORE v2 deployment repository (EUPL 1.2, see `LICENSE` and
`CONTRIBUTORS.md`) and stripped to the components the platform keeps.

## 1. What is in it

| Component | Role |
|---|---|
| `agent-runner` | the credential proxy of a builder run (`jc-agent-proxy`, AG-49…AG-52) |
| `apisix` | the edge gateway, standalone file mode: no etcd, no Admin API (ADR-N-007) |
| `artifact-store` | RustFS: the S3 API the reconciler writes rendered state to (PF-29, PF-30) |
| `audit-logging` | a Vector daemonset shipping the audit trail of the gateway, Keycloak and the forge |
| `basemap` | the background map from the cluster: a Protomaps PMTiles extract, its style, fonts and sprites on `tiles.{domain}` (T-4638) |
| `viz` | the chart renderer on `viz.{domain}`: a host page and a sandboxed Vega frame that draw an Endpoint's published visualizations for the catalogue and any site that embeds them (T-4641, ADR-N-053) |
| `ckan` | the open-data catalogue: the public face of a published Endpoint |
| `context-broker` | AntaresBroker, the NGSI-LD broker behind the gateway (ADR-N-008) |
| `context-gateway` | the enforcement point in front of every broker surface |
| `demo-feeds` | a NATS broker and live feed publisher, `dev` only (PL-50) |
| `functions` | `jc-functions`: the QuickJS runtime a generated application calls (SDK-21…SDK-23) |
| `gitea` | the in-cluster forge holding the configuration repositories (ADR-N-004) |
| `grafana` | the operational dashboards, provisioned from ConfigMaps |
| `keycloak` | identity: the realm, its clients and the import job |
| `model-tools` | the stateless LinkML generator image behind the editor |
| `monitoring` | Prometheus Operator scrape configuration and edge alerting (OPS-16, TS-22) |
| `networkpolicies` | default-deny per component and the explicit allow rules |
| `mqtt-brokers` | one RMQTT per `MqttBroker`, devices over TLS on 8883, the pipeline runner on 1883 (ADR-N-048) |
| `observability` | the OpenTelemetry Collector that fills the Portal's activity stream |
| `pipeline-runner` | Bento, in both execution modes of PL-04 |
| `portal` | the management application: API, embedded UI and in-process reconciler |
| `postgres` | CloudNativePG: the operator, the cluster and the databases |
| `prepare` | what has to exist before anything else: namespaces, annotations, the CA |
| `runtime-policies` | the Kyverno ClusterPolicies enforced at admission |
| `sandbox-reaper` | a CronJob deleting a sandbox namespace once it outlives its TTL |
| `secrets` | the generated platform credentials, one per purpose, replicated per namespace |

Each lives in `components/<name>/`: a `helmfile.yaml.gotmpl`, the values per environment, its
images pinned by digest, its NetworkPolicies, its APISIX routes and its Keycloak clients.
`just new-component` scaffolds a new one from `template-component/`.

Around them: `defaults/` (the deployment directory an installation starts from, including
`defaults/environment/global.yaml`), `policies/` (the conftest rules the rendered output is
gated by), `ci/policies/` (the Kyverno tests), `charts/` (the charts written here rather
than pulled), `tests/` and `scripts/`.

Removed from the legacy repository: `authz` (OPA), `config-adapters`, `etcd`, `frost`,
`geoserver`, `kafka`, `nifi`, `superset`, `valkey`, the Java/Robot system tests and the demo
seeding scripts.

## 2. How it fits

Nothing here decides what the platform does — it decides where it runs. The specification is
in the `docs` repository: `Deployment/` for the installation itself,
`Deployment/08-security-hardening.md` for the baseline every rendered object is held to,
`Deployment/10-edge-routing-apisix.md` for the route table, `Operations/` for running it, and
ADR-N-007 for why APISIX runs in standalone file mode.

## 3. Build

The artifact is a rendered manifest, so building is rendering. `_dev-assemble` copies
`defaults/deployment` and the example environments into `deployment/` (gitignored), which is
what an installation keeps in its own repository:

```bash
just _dev-assemble
helmfile -f deployment/helmfile.yaml -e local repos
scripts/render.sh local /tmp/rendered-local.yaml
```

## 4. Test

The fast checks, on the manifest just rendered — every image pinned by digest, and no
plaintext credential in anything that would reach a cluster:

```bash
python3 scripts/ci/check-image-digests.py /tmp/rendered-local.yaml
python3 scripts/check-secrets.py /tmp/rendered-local.yaml
```

`ci.yml` runs those for every environment and adds what needs a downloaded tool:
`kubeconform -strict`, `conftest test -p policies`, `gitleaks` over the rendered secrets and
the golden object list. `ci-full.yml` adds the Kyverno gate and the k3d deployment variants;
it is `workflow_dispatch` only while this repository's Actions minutes are metered.

## 5. Run locally

A local k3d cluster, from nothing:

```bash
just deploy-k3d                 # the cluster itself (dev-deployment/)
just deploy-operators local     # the shared operators, once per cluster
just deploy-instance local      # one instance (namespace = global.instanceSlug)
```

`just --list` prints every recipe. The `dev` cluster is a different thing: `just dev-apply`
syncs it and `just dev-smoke` verifies it, both against `.secrets/kubeconfig-dev.yaml`, and
both belong to whoever holds that cluster. `just dev-smoke` is the per-apply check (about 5 min);
the catalogue and source-link sweeps (about 20 min of reads) run hourly as `just dev-smoke-sweeps`,
and `just dev-smoke-full` runs both, as the release gate and `just prod-smoke` do (T-6148).

A pin reaches `dev` without waiting for the hourly batch (T-6110). A commit whose every file is a
`components/<c>/images.yaml` runs ci's short set (render and gate every environment, the pinned
signatures, the docs pin, the tests that read the moved images.yaml). Once that run is green,
`just dev-apply-pins` applies it at once: it renders `dev` at the commit `jc-release` records as
applied and at HEAD, syncs only the releases whose objects moved, waits for the workloads of
those releases alone (T-6155: no wait on CNPG or an unrelated pod) and runs
`just dev-smoke-component <c>` for each moved component (the Portal: the served commit is the
pin, the login reaches Keycloak, one Endpoint read; the gateway: one Endpoint read and an MCP
`tools/list`; each route also answers the edge security headers). Its last line gives the seconds
from the commit to `dev` running it smoked. `just dev-apply-pins '' true` prints the releases and
their `helmfile diff` only. A range with any other file in it (charts, values, seeds) takes the
full way: `just dev-apply` and `just dev-smoke`. A red rollout or smoke holds its releases in the
ConfigMap `jc-apply-red`: the next `just dev-apply-pins` of a held release is refused until a fix,
`just dev-apply` and a green `just dev-smoke` clear the hold. `just dev-smoke` and the sweeps
still run hourly, and the release gate (`just dev-smoke-full`, the standards) is unchanged.

## 6. Security

Report a vulnerability in the platform privately through the security advisories of the
repository it lives in ([platform](https://github.com/marek-mraz-jc/joinedcontext-platform/security/advisories/new),
[portal](https://github.com/marek-mraz-jc/joinedcontext-portal/security/advisories/new)), and
one in this deployment to the maintainers directly. Please do not open a public issue for one.

No credential is in this repository. A platform secret is generated at install time into the
cluster and replicated per namespace by `components/secrets`; anything an installation brings
with it is named by a `secretRef` in its own values. `ci.yml` renders every environment and
fails on a plaintext one, which is why the render is the gate rather than a review.

## 7. Working here

A change to a component is a change to what runs: render it, read the diff of the rendered
manifest, and keep the commit inside one component so a revert is one thing. An image moves by
digest, never by tag. Cite the requirement ids in the commit message, and add the acceptance
check to `tests/` rather than to a runbook step somebody has to remember.
