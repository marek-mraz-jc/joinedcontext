# Cluster import: the rehearsal kit against jc-dev and jc-prod on the dev cluster

The owner's order of 2026-11-09 16:1xZ is in tasks/GOAL-jc-dev.md, section "Two jc instances on the dev cluster". The deployer applies it; jc-migrate wrote this file. The rehearsal kit does each step below against a docker PostgreSQL and a host `jc`. On the cluster, the same commands run from the deployer's shell. They reach the instance through `kubectl port-forward`, which ends on the pod's own loopback address, the only address the import may write to. Nothing in this sequence writes to dev's current stack or to prod.

## 1. What each instance gets

| | jc-dev | jc-prod |
|---|---|---|
| export | dev's databases, read only (deployer) | `/workspace/evidence/jc-rehearsal/export/prod-<UTC>` (prod-deployer, read only) |
| realm | dev's live realm (dev already runs deployment main) | prod's live realm plus main's realm lines (step 3) |
| people | allow-list only (`REHEARSAL_PEOPLE`, 0700), no credential; personas keep e-mail | none |
| temporal | the last 24 h (import cut-off, then the hourly `retain`); KeyPerformanceIndicator whole (`WHOLE_HISTORY`, supervisor 20:1xZ) | the same; the bundle must hold the KPI history |
| secrets | every one fresh | every one fresh; no prod credential (supervisor) |

## 2. Inputs on the deployer's host

1. The export dir, settled. `import` refuses a bundle with a data file newer than `manifest.json`.
2. A deployment clone. For jc-prod, also `rehearsal.py render --deployment <clone> --env smartcity --out <dir>` of `release` and of `main`, each giving a `keycloak-config.values.yaml`.
3. The jc release binary of the instance's image (for `kaisatsu check` and `daiya`), or `docker run --entrypoint` of the image.
4. The age identity of the instance (`REHEARSAL_AGE_KEY_FILE`), if its projects carry SOPS files. For jc-prod on dev: dev's identity, as release's files name dev's recipient.

## 3. Realm files (offline, before jc starts)

```sh
python3 -I rehearsal.py realms --export <export> --out <work>/state/realms \
  --kaisatsu-bin <jc dir>/kaisatsu --secrets-dir /run/jc/realm-secrets \
  [--people <0700 dir> --demo-users <deployment>/components/keycloak/demo-users.yaml]   # jc-dev only
  [--lines-from <release render>/keycloak-config.values.yaml \
   --lines-to   <main render>/keycloak-config.values.yaml]                               # jc-prod
```

- `--secrets-dir` is the path inside the pod where the realm secrets Secret is mounted (step 4). kaisatsu takes `$(file:...)` only under it.
- Outputs, beside `--out` (in `<work>/state`):
  - `realm-vars.txt`: every secret name the realm needs;
  - `realm-state-secrets.txt`: the clients jc signs in as (portal-api, portal-reconciler, context-gateway, jc-assistant, agent-proxy), with the state file each one's secret goes to;
  - an export with `jc-assistant` and no `agent-proxy` (prod's) gets jc's proxy client `agent-proxy` (introspection only: service account, no flow, no role, no redirect, fresh secret `KC_AGENT_PROXY_SECRET`) and one audience mapper `jc-proxy-audience` naming it on `jc-assistant`; without them every assistant model call answers 503 (introspection 401). An existing `agent-proxy` is kept, its secret untouched, and the step stops if it has a flow, a redirect, full scope, token exchange, another authenticator or a role;
  - `realm-pipeline-clients.txt`: each `<project>-pipelines` client and each Pipeline's own client `<project>-pl-<pipeline>`, with its secret name;
  - `realm-<name>-lines.json`: main's lines put in;
  - `realm-<name>-left-out.json`: what kaisatsu refuses.
- jc's own clients and each Pipeline's own client `<project>-pl-<pipeline>` (account attribute `<project>/pl-<pipeline>`) switch from federated-jwt to their secret, because jc signs in as them from one process: ekiben runs every stream in jc, so no stream has a Kubernetes ServiceAccount token. Name and audience mappers stay as prod had them (jc-pipeline-audience: for all 824 prod pipelines the mappers equal the Endpoints the pipeline names). Every other workload client keeps federated-jwt; jc-kaisatsu-audience (5f8adf60) checks it against the cluster's JWKS.
- The realm file can hold people (jc-dev): keep it in a 0600 Secret, never in a ConfigMap.

## 4. Secrets (one Kubernetes Secret per kind, generated, never copied)

1. Realm secrets: one key per line of `realm-vars.txt`, each `openssl rand -hex 32`, mounted at `--secrets-dir`, mode 0400.
2. jc's state client secrets: for each line `<state file> <NAME>` of `realm-state-secrets.txt`, the state file holds the same value as realm secret `<NAME>`. That is the one source for those clients (F5). Copy it on every start, before jc reads its state.
   - kaisatsu re-applies a realm whose secret files changed behind an unchanged document (kaisatsu import, 2026-11-09), so a rotated value reaches the stored client at the next start.
3. Pipeline clients: `--ekiben-client-secrets-dir` holds one file per line of `realm-pipeline-clients.txt`, `<project>-pipelines` and `<project>-pl-<pipeline>` alike, named by the client (a copy, not a link), with that client's realm secret. jc hands a `-pl-` file to ekiben as `JC_PIPELINE_SECRET_<CLIENT>` (capitals, `-` as `_`).
4. Store credentials: one Postgres role per module (kaisatsu, Portal, Antares, shako, daiya, dataroom, ekiben), each password in its own file (`[<m>.database] password_file`). jc refuses a password in a URL.

## 5. jc's configuration (as up.sh writes jc.toml)

- `[identity] realm`: `dev` or `smartcity` (from `realm-name.txt`).
- `JC_PORTAL_PIPELINE_IDENTITY=pipeline-secret` when `realm-pipeline-clients.txt` names a `-pl-` client (up.sh sets it): every stream signs in as its Pipeline's own client, whose token names only the Endpoints that pipeline writes. Without it every stream signs in as `<project>-pipelines`, and the gateway refuses each write to an Endpoint outside that client's audience with 401 `token is not bound to this resource`. A Pipeline created after the import has no own client: ekiben refuses its stream (400, `environment variable 'JC_PIPELINE_SECRET_...' was not defined`) and the Pipeline shows the refusal; it never falls back to the project's client.
- `trusted_proxies` and `trusted_proxy_identities` (cluster only; the rehearsal has no edge and sets neither):
  - `trusted_proxies`: the pod network APISIX runs in (`["10.42.0.0/16"]` on k3s; the chart's `jc.server.trustedProxies`, `JCBIN_TRUSTED_PROXIES`).
  - `trusted_proxy_identities`: APISIX's Linkerd identity, e.g. `apisix.<namespace>.serviceaccount.identity.linkerd.cluster.local` (`JCBIN_TRUSTED_PROXY_IDENTITIES`, jc a964cea1 and later). Without it any pod that reaches jc can set `X-Forwarded-*` (jc-wasm-deploy, test_jc_component).
  - Check live before the pin (deployer RUNBOOK-apply.md): the ConfigMap's `trusted_proxies`, and the APISIX pod IP inside that range.
- `[dataroom.aliases]`: `catalog = "zilina-catalog"` for jc-prod (`rehearsal.py aliases`; RENAMES.md).
- `[dataroom.hidden]`: `bins/jc/docs/catalogue-hidden.toml` appended (prod's publish-gate list).
- `[dataroom.publishers]`: `bins/jc/docs/dataroom-publishers.toml` appended (prod's publisher badges and words, the CKAN chart's `organizations`).
- `[dataroom.branding]`: `logo` and `favicon` `"logo.svg"`, `logo_svg` the deployment's `branding/logo.svg` (prod's global `branding.logo`, `favicon`, `logoSvg`).
- `[[dataroom.harvest]]`: `bins/jc/docs/harvest-sources.toml` appended, with `bins/jc/docs/od-datasets.json` copied beside jc.toml (the toml names it relatively). jc harvests hub-bbsk and nkod-banskabystrica at start, then on schedule.
- `JC_GATEWAY_HUB_KNOWLEDGE_SOURCES`: `rehearsal.py hub-sources --deployment <clone at the instance's ref>`.
- `JCBIN_EKIBEN_RESOURCES_FILE`: `components/pipeline-runner/streams/resources.yaml`.
- `gtfs-realtime.proto`: under the ekiben file root's `proto/`.
- `JCBIN_PORTAL_AGE_KEY_FILE`: the instance's age identity.
- Each CkanInstance `spec.url` is fenced by `import` to the instance's own dataroom. On the cluster it must name `jc-<instance>`'s own catalogue host, never dev's or prod's CKAN.
  - On the cluster, `rehearsal.py import --catalogue-url https://<instance catalogue host>` (deployer, T-6676 section I: jc-dev `https://jcdev-opendata.dev.joinedcontext.com`, jc-prod `https://jcprod-opendata.dev.joinedcontext.com`). It takes an https origin only.

## 6. The import (the window: jc on allow-all, reachable only through port-forward)

1. Start jc with `ANTARES_POLICY=allow-all` and `--listen 127.0.0.1:<port>`.
   - No Service selects the pod meanwhile: a `jc-import=true` label that the Service selector excludes, or the Service scaled out of the way.
   - The import's writes carry no subject; jc's default policy answers them 401.
2. From the deployer's shell: `kubectl -n <ns> port-forward pod/<jc pod> <local>:<port>`.
3. Run `python3 -I rehearsal.py import --export <export> --base http://localhost:<local> --state <state view> --report <work>/import-report.json --daiya-bin <jc dir>/daiya --catalogue-url <instance catalogue>`.
   - `<state view>` is a 0700 dir holding the files `import` reads: `shako-token` and `antares-admin-token`, read from the pod's state Secret.
   - The import writes shako repositories (current files, CkanInstances fenced, catalog datasets named apart), Antares current state and temporal instances newer than the later of `exportedAt - 24 h` and `now - 24 h`, and daiya dashboards.
   - It is idempotent: a rerun reports `unchanged`.
4. Restart jc on its default policy and its normal listener. Then run `rehearsal.py policy-check --base http://localhost:<local> --state <state view>`: an anonymous write must answer 401 and `/broker/q/health` must name `joinedcontext`. If not, scale jc to 0 and stop.
5. The wasm Apps' database (jc-prod only, before the publish in step 6). The final export (`agents/prod-deployer/backups/prod-20261109T1958Z-final`, prod's only copy: read it, never move or change it) holds `pg-apps.dump.age` and `globals-apps-db.sql.age`. They restore into jc-prod's `apps-db` once its Cluster is Ready, while jc is still in the window, so no publish runs against a half-restored database. The plaintext stays in a 0700 scratch dir that is deleted afterwards, never under evidence/ or in a repository.

   ```sh
   X=/workspace/agents/prod-deployer/backups/prod-20261109T1958Z-final
   W=$(mktemp -d); chmod 700 "$W"
   for f in pg-apps.dump globals-apps-db.sql; do
     age -d -i /workspace/.secrets/age-dev.key -o "$W/$f" "$X/$f.age"
     [ "$(sha256sum < "$W/$f" | cut -c1-64)" = "$(cat "$X/$f.sha256")" ] || { echo "$f: checksum"; exit 1; }
   done
   # Only the Apps' own roles and their grants to the managed logins. CloudNativePG and
   # apps-db's initdb own postgres, streaming_replica, cnpg_metrics_exporter, jc_set_config,
   # jc_apps_admin and wasm_host_<n>, whose passwords are this instance's generated ones.
   grep -E '^(CREATE ROLE app_[0-9a-f]{16}(_owner)?;|ALTER ROLE app_[0-9a-f]{16}(_owner)? WITH .*;|GRANT app_[0-9a-f]{16}(_owner)? TO (jc_apps_admin|wasm_host_[0-9]+) .*;)$' \
     "$W/globals-apps-db.sql" > "$W/roles.sql"
   [ "$(grep -c PASSWORD "$W/roles.sql")" = 0 ] || exit 1          # 80 lines, no password
   P=$(kubectl -n jc-prod get pod -l cnpg.io/cluster=apps-db,cnpg.io/instanceRole=primary -o name)
   kubectl -n jc-prod exec -i "$P" -c postgres -- psql -v ON_ERROR_STOP=1 -d apps < "$W/roles.sql"
   # The server's own pg_restore (16): a newer client writes SET transaction_timeout, which 16 refuses.
   # --clean drops what jc's Portal migrated into the empty database at its first start.
   kubectl -n jc-prod exec -i "$P" -c postgres -- \
     pg_restore --exit-on-error --single-transaction --clean --if-exists -d apps < "$W/pg-apps.dump"
   kubectl -n jc-prod exec "$P" -c postgres -- psql -Atd apps -c \
     "select name, shard from jc_apps.apps order by name"                # 10 rows, as below
   rm -rf "$W"
   ```

   - Expected (scratch restore of this export into PostgreSQL 16.15, jc-wasm-deploy 2026-11-10): 10 schemas `app_<16 hex>`, `jc_apps.apps` 10 rows, `jc_apps.migrations` 21, `public._sqlx_migrations` 2 (checksums equal to jc's `crates/portal/apps_db/migrations`, so jc accepts the database), and 887 rows in the App tables. Per App: air-weather-explorer 722 (hourly 720, stations 2), alerts-heatmap 26 (weekly_places), bike-rebalancing 1 (levels), bike-weather-demand 9 (station_models), data-quality-inspector 129 (quality_runs 10, run_types 119); district-compare, event-day-planner, kpi-forecast, news-topics and transit-reach hold their tables with no rows.
   - The wasm components need no copy: step 6's publish takes each from its bundle (`.jc/component.wasm`) into the store's `apps` bucket as `components/sha256-<hex>.wasm`. The export's 10 wasm bundles match `apps/builds.json`, and each component's digest is one of prod's own bucket objects (jc-wasm-deploy REPORT.md). After the publish every wasm App shows `status.shard`, and `https://<app>.apps.smartcity.joinedcontext.com/api/...` reaches its shard; the shard's own gateway and Portal calls wait on the wasm-host header fix (jc-wasm-deploy REPORT.md).
6. Apps (apps.sh, jc-apps-export): `apps.py publish` (prod builds into shako) and `apps.py grant`, same base. Never `REHEARSAL_APP_HOLDER_ID` on the cluster.
7. Keep the 24 h rule: `rehearsal.py retain --host <pg> --port <port> --password-file <antares pw> --every 3600` against the instance's Antares database. A CronJob running the same statement is enough: delete temporal instances older than 24 h, audit rows kept.

## 7. Proof

`bins/jc/smoke/run all` against the instance's public base, with the gate flags of `evidence/jc-dev/GATE-*.md`. Prod's temporal rows count only within the 24 h window of the export.

## Related

- REHEARSAL.md: the docker rehearsal the steps come from.
- EXPORT.md: the bundle format.
- tasks/GOAL-jc-dev.md: the gate and the two instances.
