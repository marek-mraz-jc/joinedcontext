# jc cutover plan: stage, then dev, then prod

This plan moves stage, dev and prod (`smartcity`) from the separate components to the one binary
`jc` (deployment component `components/jc`). Nobody runs it from this file. Each step is a
deployment branch, reviewed and applied by the deployer.

Written 2026-11-08 by jc-rest from the deployment renders of `origin/main` cee6ebcb9 and the
`jc-ship` branch of the deployment repo. The edge stays APISIX (owner rule): every step changes
APISIX routes and upstreams. It changes no edge product.

## 1. What jc replaces

| Old component | jc module | Pool it frees | What moves | Blocker before it can leave |
|---|---|---|---|---|
| `grafana` | daiya | none (Grafana keeps no role in `postgres-cluster`) | dashboards: already files in `components/grafana/charts/dashboards`, provisioned at boot | daiya keeps dashboards in memory unless jc-pg gives it Postgres; the boot provisioning reloads files, nothing else survives a restart |
| `ckan` | dataroom | `ckan` 8 | catalogue records (jc-migrate's export) | dataroom on Postgres (jc-pg); `JC_CKAN_URL` needs https (EP-67), true on stage, dev and prod |
| `keycloak` | kaisatsu | `keycloak` 4 | realm `jc` users, groups, clients, credentials (kaisatsu_import reads Keycloak realm JSON) | the issuer must not change: kaisatsu serves `https://idm.{domain}/realms/<realm>` (section 4); every session ends once |
| `gitea` | shako | `gitea` 3 | every repository (`git push --mirror`), the config repository first; tokens issued again | jc-shako-odata deletes shako's Gitea API, so the old Portal cannot use shako: gitea leaves with the old Portal (step 4). App builds have no shako-native runner yet: `gitea-runner` has nothing to serve and builds answer "builds unavailable" |
| `portal` | Portal | `portal` 5 | nothing (the Portal is stateless; its preferences tier is the `portal` database, kept) | none beyond kaisatsu and shako being in jc |
| `context-broker` | Antares | `antares` 13 | none if jc's Antares uses the same database | jc builds Antares without `views` and `jc-policy`: `context-gateway` reads Endpoints through `/q/views`, which jc does not serve. Step 5 waits for that build |

These stay: `context-gateway` (no jc module, re-pointed to jc's broker in step 5), `assistant`,
`decidim`, `pipeline-runner`, `mqtt-brokers`, `wasm-host`, `apps-db`, `observability` and the
platform components.

## 2. Postgres connection budget

The rule is `tests/test_postgres_connection_budget.py`:

    ceil(sum x 1.2) + superuser_reserved <= max_connections
    sum <= 0.8 x max_connections

The figures below come from the renders, through that test's `budget()` function.

| Env | max_connections | reserved | pools now | sum | need |
|---|---|---|---|---|---|
| stage | 61 | 3 | antares 13, assistant 10, ckan 8, decidim 5, gitea 3, keycloak 4, portal 5 | 48 | 61 (full) |
| dev | 55 | 3 | antares 13, assistant 10, ckan 8, gitea 3, keycloak 4, portal 5 | 43 | 55 (full) |
| smartcity (prod) | 61 | 3 | as stage | 48 | 61 (full) |

jc's pools (deployment `components/jc`):

| Role | Ceiling |
|---|---|
| `jc_portal` | 5, compiled |
| `jc_antares` | 8 (`antaresPool`) |
| `kaisatsu_app` | 4 (`kaisatsuPool`) + 1 change listener |
| `kaisatsu_migrate` | 1 (init container) |

Total 19. dataroom, daiya and shako add the pool maxima jc-pg writes into STATUS, called D, Y
and S below. jc-pg (branch `jc-pg` 5ffda4b5, `bins/jc/STATUS.md` "Postgres connection budget")
sets each to its `max_connections`, default 5; kaisatsu takes one more for its change listener, so
`kaisatsuPool` 4 holds 5 connections.

With D = Y = S = 5 and kaisatsu at 5 + 1 migrate, the sums and needs per step are:

| After step | stage / prod sum | need | dev sum | need |
|---|---|---|---|---|
| 1 | 53 | 67 | 48 | 61 |
| 2 | 50 | 63 | 45 | 57 |
| 3 | 52 | 66 | 47 | 60 |
| 4 | 54 | 68 | 49 | 62 |
| 5 | 49 | 62 | 44 | 56 |

Every step outgrows today's `max_connections` (61, 55). Raise it once, before step 1: 70 on stage
and prod (largest need 68, 80 % line 56 above the largest sum 54), 64 on dev (need 62, line 51.2
above 49). The memory check below holds for both. Rerun the budget test on each step's render.

No environment has room for jc beside everything it replaces. So:

- Each step adds a jc role and removes the old component in the same apply.
- The databases stay. The old component's database is kept untouched until the soak of that
  step ends.

Step 1 adds Y. Step 2 frees `ckan` 8 and adds D. Step 3 frees `keycloak` 4 and adds kaisatsu's
5 + 1. Step 4 frees `portal` 5 and `gitea` 3 and adds `jc_portal` 5 + S. Step 5 frees `antares` 13
and adds `jc_antares` 8. The table above holds the results.

- The `max_connections` raise (daiya on its memory store, Y = 0, would fit step 1 without it):
  - stage and prod: memory `128 + max x 19` MiB of 2560 Mi (`test_postgres_memory_budget.py`).
    max 70 needs 1458 MiB, 57 % of the limit.
  - dev: 2 Gi; max 64 needs 1344 MiB.
  - Each environment's `postgres.yaml.gotmpl` sets it. Run `test_postgres_memory_budget.py` and
    `test_postgres_connection_budget.py` on the render.
- Each step's branch runs both budget tests and adds the new roles to `CLIENTS`: `jc_portal`,
  `jc_antares` and `kaisatsu_migrate` are there already on deployment branch `jc-ship`.

## 3. Order and environments

stage first, then dev, then smartcity (prod). An environment takes the next step only after:

- its own `dev-smoke` (or the stage smoke) is green;
- the walker finds nothing new;
- the previous step has soaked for one day there.

Prod takes a step only after stage and dev both soaked it.

| Step | jc role set (`jc --role`) | Old component removed from `components:` | Old routes re-pointed to Service `jc` |
|---|---|---|---|
| 0 | none | none | none: preconditions only (section 6) |
| 1 | `daiya` | `grafana` | `/grafana*` on the apex |
| 2 | `daiya,dataroom` | `ckan` | `opendata.{domain}`, `data.{domain}` (the second catalogue host), `/ckan*` on the apex |
| 3 | `+kaisatsu` | `keycloak` | `idm.{domain}` |
| 4 | `+portal,shako` | `portal`, `gitea`, `gitea-runner` | `portal.{domain}`, the apex `/` redirect, `/git/*`, `/v2/*`, `/v2/token` |
| 5 | `all` | `context-broker` | none public (the edge refuses `/broker`); `context-gateway`'s broker URL becomes `http://jc:8080/broker` |

Before step 4, the old Portal keeps its `JC_OIDC_ISSUER`, `JC_CKAN_URL` and `JC_GRAFANA_URL`
pointed at the same public host names. Those now land on jc. No Portal setting changes until
step 4.

## 4. Edge and DNS

DNS does not change: every host name stays and keeps its HTTP-01 certificate. jc runs in host
mode with an explicit mount per old host and prefix. `[mounts.<module>]` takes one table or an
array `[[mounts.<module>]]` of `{host, path_prefix}` (`bins/jc/src/config.rs`). The first entry
is the module's own address (issuer, absolute URLs); each later entry adds a router entry, and the
module sees the request's own host in its `Mount`. The gateway and the assistant answer at whole
paths on every entry, so their prefixes must be `""` or one of their own paths
(`Module::root_paths`); every other module's later entries are stripped of their prefix as its
first is. jc refuses at start two equal entries, a wildcard host, and in host mode an entry
without a host.

The rows dev's APISIX routes serve (deployment db7b9a38c, `components/*/apisix-routes.yaml`;
App hosts and the `{domain}/apps/*` row come with the App-host slice):

| Module | Host | Paths | jc entries |
|---|---|---|---|
| kaisatsu | `idm.{domain}` | `/*` | `idm.{domain}` `""` |
| Portal | `portal.{domain}` | `/*` | `portal.{domain}` `""` |
| gateway | `api.{domain}` | `/cs/*`, `/api/endpoint/*`, `/catalog*`, `/api/mcp*`, `/oauth/*`, `/.well-known/oauth-*` | `api.{domain}` `""` (first entry) |
| gateway | `{domain}` | `/cs/*`, `/catalog*`, `/api/endpoint/*`, `/api/mcp*` | `/cs`, `/catalog`, `/catalog.jsonld`, `/catalog.ttl`, `/api/endpoint`, `/api/mcp` |
| gateway | `portal.{domain}` | `/cs/*`, `/api/mcp*`, `/api/endpoint/*` | `/cs`, `/api/mcp`, `/api/endpoint` |
| dataroom | `opendata.{domain}` | `/*` | `opendata.{domain}` `""` |
| daiya | `{domain}` | `/grafana*` | `{domain}` `/grafana` |
| shako | `{domain}` | `/git/*`, `/v2/*` | `{domain}` `/git`; `/v2` on every host (below) |
| none: APISIX `redirect` | `{domain}` `/*`, `{domain}` `/ckan*`, `data.{domain}` `/*` | | no entry: jc answers 404 there |

The router matches whole path segments (longest prefix first), not APISIX's suffix globs, so the
three `/catalog*` spellings the gateway serves are three entries. Differences S4b keeps in APISIX
or accepts:

- `/catalog*` on `{domain}` and `api.{domain}` also matches `/catalogue`, `/catalogs` and the like
  on dev; jc answers 404 on `{domain}` (on `api.{domain}` the gateway answers its own 404).
- `/api/mcp*` and `/grafana*` match `/api/mcpx` and `/grafanax` on dev; jc answers 404.
- shako's `/v2` answers on every host in jc, on `{domain}` only on dev.
- `portal.{domain}/.well-known/oauth-protected-resource*` stays with the Portal, as on dev
  (`portal-well-known`): the gateway has no entry there.

The table as `jc.toml` (`bins/jc/tests/fixtures/dev-mounts.toml` with `city.test` for the
domain; `tests/one_binary.rs::host_mode_answers_every_dev_row_from_its_module` sends one request
per row with `Host` as APISIX sends it):

```toml
mode = "host"
base_host = "{domain}"

[mounts.kaisatsu]
host = "idm.{domain}"
path_prefix = ""

[mounts.portal]
host = "portal.{domain}"
path_prefix = ""

[mounts.dataroom]
host = "opendata.{domain}"
path_prefix = ""

[mounts.daiya]
host = "{domain}"
path_prefix = "/grafana"

[mounts.shako]
host = "{domain}"
path_prefix = "/git"

[[mounts.gateway]]
host = "api.{domain}"
path_prefix = ""

# One entry per (host, path): /cs, /api/endpoint, /api/mcp, /catalog, /catalog.jsonld and
# /catalog.ttl on {domain}; /cs, /api/endpoint and /api/mcp on portal.{domain}.
[[mounts.gateway]]
host = "{domain}"
path_prefix = "/cs"

[[mounts.gateway]]
host = "portal.{domain}"
path_prefix = "/api/endpoint"
```

jc forwards `X-Forwarded-Host` only from a peer in `trusted_proxies`: S4b sets APISIX's
`pass_host: pass` and lists APISIX's addresses there, or a client could choose the module.

The issuer stays
`https://idm.{domain}/realms/<realm>`: tokens, clients' configured issuers and the gateway's
`admit_on` keep working without a change.

APISIX keeps every plugin of the old routes:

- the header sanitiser;
- the rate limits, including `ckan`'s cache keys;
- the CSP, HSTS and security headers.

Each step changes only the upstream to the Service `jc` port 8080. `components/jc/apisix-routes.yaml`
on deployment branch `jc-ship` serves `jc.{domain}` in slug mode. For the cutover, replace that
route with the host routes above.

- Its `uri-blocker` for `/git` goes in step 4, because the forge is public on `/git` today.
- `/streams` and `/broker` stay blocked.

The NetworkPolicies of `components/jc` need the egress halves of the old components' peers
before each step. Step 2 needs the outbound hosts dataroom harvests. Step 3 needs Keycloak's
CIMD egress on 443 to public addresses, which jc already has.

### 4a. Several Organizations on one deployment

`[[organizations]]` (`bins/jc/src/config.rs`, fixture `bins/jc/tests/fixtures/two-orgs.toml`)
puts each Organization on its own hosts: `name` (the JC Organization's `metadata.name`), `hosts =
{ portal, opendata, api, mcp }`, `catalogue_id` (a UUID) and an optional `portal_token_file`.

- Each host is the Organization's whole host: the Portal on `portal`, dataroom on `opendata`,
  the gateway on `api` and `mcp`, and the gateway's `/cs`, `/api/endpoint` and `/api/mcp` on
  `portal` too. jc adds these entries after each module's own address.
- jc refuses to start on a name, catalogue or host given twice (hosts compared without the
  port), and on a host any other mount also names, a mode default included.
- dataroom gets one catalogue per Organization, named after it on its `opendata` host, and in it
  the CKAN organisation of the same name. A `portal_token_file` binds a publisher of that
  catalogue alone (principal `portal-ckan:<catalogue id>`), with Read and Propose under that
  CKAN organisation only. With Organizations configured a loopback host names no catalogue.
- dataroom and the gateway read the host and scheme from jc's router (the forwarded host of a
  trusted proxy), never the raw `Host` header. On an Organization's `api`, `mcp` and `portal`
  hosts every gateway listing (`/api/endpoints`, the MCP hub, `/catalog.jsonld`, `/catalog.ttl`,
  `/cs`) holds that Organization's Endpoints alone, and another Organization's Endpoint, space
  or MCP server answers 404.
- With Organizations configured jc keeps a host registry (docs/API/15-addresses-v1.md §2,
  `jc_router::HostTable`): each Organization's hosts are bound to it; the base host, every other
  mounted host and the loopback names are shared, where a first path segment naming an
  Organization selects it (`/hel/opendata/...`). A host outside the registry answers 421, an
  unknown `_`-shaped segment or another Organization's name on a bound host the router's 404.
  On a bound host the prefixed form (`/hel/...`) answers in place for the API and 308 to the
  prefix-less path for a page. The router decides the Organization once (`jc_router::Organization`);
  the Portal pin, the gateway's listings and dataroom's catalogue read it.
- `[[hosts]]` binds a further host to an Organization and maps its root below it:
  `host = "data.example.com"`, `organization = "hel"`, `root = "/opendata"` serves the
  Organization's catalogue at `https://data.example.com/dataset/{name}`. A row names an
  `[[organizations]]` entry and a host no other row or Organization holds.
- dataroom writes every absolute id and link under the catalogue's canonical base
  (`{scheme}://{opendata host}`), whichever registered host read it, so a DCAT `@id` never moves.
- kaisatsu stays on one host and one issuer for every Organization.

## 5. Rollback per step

Every step is reversible while its old database is kept:

1. Put the old component back in `components:`.
2. Point its routes back at the old upstream.
3. Drop the jc role from `--role`.
4. Apply.

| Step | Rollback cost |
|---|---|
| 1 daiya | none: Grafana's dashboards are files, Grafana's state was never moved |
| 2 dataroom | catalogue edits made in jc are lost unless exported back (jc-migrate's export in reverse). Keep CKAN read-only during the soak to avoid that |
| 3 kaisatsu | people created and passwords changed in kaisatsu during the soak are lost. Sessions end again. Freeze people writes for the soak, or export the realm from kaisatsu and import it into Keycloak |
| 4 Portal + shako | commits made in shako are pushed back to Gitea with `git push --mirror` from shako before the rollback apply. Proposals open in jc's Portal are lost |
| 5 Antares | none if jc's Antares used the same `antares` database; otherwise the entities written meanwhile |

Never delete an old database, PVC or Secret inside a step. A separate branch removes them after
the prod soak, once the backup of section 6 exists.

## 6. Preconditions (step 0)

1. A signed `jc` digest from `image.yml` on monorepo `main`, pinned in `components/jc/images.yaml`.
2. `ghcr.io/marek-mraz-jc/jc` is pullable: either the package is public or namespace `jc` has an
   image pull secret.
3. jc-pg merged: dataroom, daiya and shako on Postgres (D, Y and S in section 2 assume its default
   of 5 each; recompute if a `max_connections` changes).
4. jc-migrate's exports tried on stage's data: CKAN catalogue, Keycloak realm with credentials,
   Gitea repositories.
5. `jc-identity` route: jc stops no longer on a failed discovery read (jc-rest item 1), so this
   route is optional in host mode. Keep it: the Portal is ready sooner.
6. jc's environment carries the Portal's two workload namespaces, as the old Portal's does
   (`components/portal/values/portal/base-values.yaml.gotmpl`):
   - `JC_PORTAL_PIPELINE_NAMESPACE`: `pipeline-runner.identities.pipelineNamespace`
     (prod `smartcity-pipeline-identities`);
   - `JC_PORTAL_APP_IDENTITY_NAMESPACE`: `wasm-host.identities.appNamespace`
     (prod `smartcity-app-identities`).

   Without them the Portal keeps every imported `pl-`/`appjob-` client, including one whose
   Pipeline or App is gone, and warns once at start (`workload_clients.rs` `derives`).

## 7. Backup that must exist before prod

Prod (`smartcity`) has only the nightly `pg_dumpall` CronJob `postgres-cluster-local-backup`.
It writes to a node volume: no off-site copy and no point-in-time recovery. A lost node loses
everything. Before prod's step 1:

- **Postgres.** `postgres.backups` turned on with the owner's S3 bucket and keys (Secret
  `postgres-backup-s3`). Then one green `drDrill` run on prod's archive (ConfigMap
  `postgres-cluster-dr-drill` shows the row counts and RTO/RPO).
  - This needs the owner's credentials, so it is blocked on `@user`.
- **Gitea repositories.** A dump of the `gitea` PVC (or `gitea dump`), age-encrypted to the drill
  key, off the node.
- **Keycloak realm.** An export with users and credentials, kept beside the dump.
- **CKAN.** The file store and the `ckan` database (part of `pg_dumpall`).
- **Restore proof.** One restore of all four into a throwaway environment on stage before prod
  applies step 1.

## Related

- `bins/jc/STATUS.md`: gaps of the one binary.
- deployment `components/jc/README.md`: the component and its edge.
- deployment `tests/test_postgres_connection_budget.py`, `tests/test_postgres_memory_budget.py`.
