# jc

joinedcontext in one binary (monorepo `bins/jc`, image `ghcr.io/marek-mraz-jc/jc`): kaisatsu
(identity), dataroom (catalogue), daiya (dashboards), ekiben (streams), Antares (broker), shako
(forge) and the Portal in one process, `jc --role all`, on one host `jc.{domain}`.

The component is opt-in. An environment turns it on by listing `jc` in `components:` of its
`global.yaml.gotmpl`. Only `addons` lists it, as a render surface with an example image pin
(`environments/addons/jc.yaml.gotmpl`); dev, smartcity and stage do not.

Deploy the component with:

```bash
helmfile apply -i --selector component=jc
```

## 1. What it deploys

One release of `charts/workload`, `jc-server`:

- **Deployment `jc`**, one replica, image pinned by digest in `images.yaml`. It starts
  `jc --config /etc/jc/jc.toml` (ConfigMap `jc`): slug mode, the Portal at `/`, kaisatsu at
  `/id`, dataroom at `/opendata`, daiya at `/dashboards`, ekiben at `/streams`, Antares at
  `/broker`, shako at `/git`. Public listener 8080, admin listener 9091 (`/metrics` behind the
  bearer of Secret `jc-admin-token`, `/healthz` open).
- **Init container `seed-state`** copies Secret `jc-setup` into the state directory
  `/var/lib/jc/state` (0700, files 0600, a memory `emptyDir`). jc reads its first-start secrets
  there (monorepo `bins/jc/src/setup.rs`) and never generates them in the pod, so a restart and
  a rollout keep the same KEK, client secrets and tokens.
- **Init container `kaisatsu-migrate`** runs `kaisatsu migrate` as `kaisatsu_migrate`, which owns
  `jc_kaisatsu` and grants `kaisatsu_app` what it needs. jc then connects as `kaisatsu_app`,
  neither superuser nor BYPASSRLS (kaisatsu refuses to boot otherwise).
- **PersistentVolumeClaim `jc-files`** (5 GiB, ReadWriteOnce) at `/var/lib/jc-data`: ekiben's
  files and shako's repositories.
- **Service `jc`** on 8080.

Postgres comes from CloudNativePG through `databases.yaml`: `jc_portal` (role `jc_portal`),
`jc_antares` (role `jc_antares`, PostGIS) and `jc_kaisatsu` (role `kaisatsu_migrate`, plus
`kaisatsu_app` as an additional role). Pools: the Portal's compiled 5, `antaresPool` (8) and
`kaisatsuPool` (4) plus the migration login; `tests/test_postgres_connection_budget.py` counts
all three.

Secrets (`secrets.yaml`, generated once by the secrets component and kept, never in Git):
`jc-setup` (11 keys, one per file of the state directory), `jc-admin-token` and one
`db-jc-*` login per database role.

## 2. Edge

One route `jc` (`apisix-routes.yaml`) on `jc.{domain}`. jc authenticates every caller itself, so
the edge proxies anonymous requests with the platform's header sanitiser, a 600-per-minute
bucket and the security headers. It refuses `/streams`, `/broker` and `/git` with 404: ekiben
takes one shared token, the in-binary broker runs no views or Policy engine, and shako is the
Portal's forge. The Portal reaches all three inside the pod.

A second route `jc-identity` sends `/id/*` (kaisatsu) to the headless Service `jc-pods`, which
lists a pod before it is Ready. At start jc serves kaisatsu, then builds the Portal, which reads
the realm's discovery at its public issuer `https://jc.{domain}/id/realms/jc`. Through the
Service `jc` that read would wait for the readiness only the built Portal gives, and jc stops on
the failed read: a crash loop on every start. The pod's egress to the cluster ingress on 443
carries that read.

## 3. Network

`networkpolicies.yaml`: ingress from APISIX on 8080 only; egress to DNS, `postgres-cluster` on
5432, the cluster ingress on 443 (the Portal reads kaisatsu's discovery at its public issuer
URL) and public addresses on 443. Each peer has its other half (`postgres-from-jc`,
`apisix-egress-to-jc`) and its Linkerd half on 4143 (`networkpolicies-linkerd.yaml`).

## 4. Resources

Measured 2026-11-08 with `docker stats` on the image (`jc --role all`, the smoke test's config,
memory stores, jemalloc settings of the pod): 94 MiB once every module answered (2 s after
start), 99 MiB after 2100 requests over seven module routes, 22 MiB resident 20 s later. Not
measured: the Postgres stores of the Portal, kaisatsu and Antares, and real data volumes. The
requests (256 Mi production, 192 Mi development) cover the measured peak with Postgres pools;
the limits (1 Gi, 768 Mi) leave room for Antares' 200 MiB memory budget. Measure again on the
first stage apply.

## 5. Before an environment lists `jc`

1. Pin the first digest `image.yml` signed on monorepo `main` in `images.yaml`
   (`scripts/verify-pinned-images.py` accepts only that workflow on `main`).
2. The package `ghcr.io/marek-mraz-jc/jc` is private while the monorepo is: make it public or
   give the namespace an image pull secret.
3. Free Postgres connections: stage and smartcity sit at exactly 55. jc replaces portal,
   context-broker, keycloak, ckan, grafana and gitea, whose pools it frees when they leave the
   environment's list.
4. dataroom, daiya, shako and ekiben keep their state in memory or on the claim: a restart
   loses the catalogue, the dashboards and ekiben's in-memory topics.
5. Name the edge's pod network in `jc.server.trustedProxies` (k3s: `["10.42.0.0/16"]`, as
   apisix's `trustedProxyCidr`). jc reads the caller's address from `X-Forwarded-For` only from a
   peer inside it, so kaisatsu's sign-in lock and daiya's public rate limit count per caller. The
   render refuses an empty list and any `/0` (`tests/test_jc_component.py`).
