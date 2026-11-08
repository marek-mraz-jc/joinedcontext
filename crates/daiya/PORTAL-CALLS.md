# Grafana calls the Portal makes, and what daiya answers

Inventory of 2026-11-08 (branch `jc-catalog-dash-portal`). Every call below is replayed by
`api/tests/portal_contract.rs` against `daiya_api::api_router`, with the headers and bodies the
Portal sends, after the steps of the deployment's `provision.sh`.

Paths: `portal/` is `crates/portal/src/`; `provision.sh` is
`joinedcontext-deployment/components/grafana/charts/dashboards/provision.sh`.

## 1. Configuration (T-6509)

| Setting | Where | Meaning for daiya |
|---|---|---|
| `JC_PORTAL_GRAFANA_URL` | `portal/config.rs:580` | The daiya mount as the Portal reaches it; the Portal calls `{url}/api{path}` (`portal/grafana.rs:262`). |
| `JC_PORTAL_GRAFANA_PUBLIC_URL` | `portal/config.rs:581` | The daiya mount as a browser reaches it; the Portal links `{public}/d/{uid}?orgId={org}` (`portal/api/grafana.rs:214`). |
| `JC_PORTAL_GRAFANA_USER` / `_PASSWORD` | `portal/config.rs:582-583` | Grafana's local administrator. daiya: `DaiyaState::with_server_admin(user, password)`, in `daiya serve` the `serverAdmin { user, passwordFile }` block. |

All four or none (`portal/config.rs:576`).

## 2. Calls (`portal/grafana.rs` `Client`)

Every call: `Authorization: Basic base64(user:password)` (`portal/grafana.rs:263`), redirects
refused, 10 s timeout, answers read up to 1 MiB; `X-Grafana-Org-Id: <n>` on organization calls.

| # | Call | Caller | Body | What the Portal parses |
|---|---|---|---|---|
| G1 | `GET /api/orgs/name/{project}` | `portal/grafana.rs:271` `organization` | | 404 = no organization; else `id` (i64) |
| G2 | `GET /api/datasources` + org | `portal/grafana.rs:292` `datasources` | | `[{type, url, uid}]`; keeps `type == ngsild-grafana-datasource`, slug after `/api/endpoint/` |
| G3 | `POST /api/datasources` + org | `portal/grafana.rs:315` `create_public_datasource` | `{uid, name, type, access: proxy, url: {gateway}/api/endpoint/{slug}, jsonData: {pageSize}}` | 2xx |
| G4 | `POST /api/dashboards/db` + org | `portal/grafana.rs:339` `save` | `{dashboard: dashboard_of(..) (schemaVersion 39), folderUid: project-{project}, overwrite: true, message}` | 2xx |
| G5 | `GET /api/dashboards/uid/{uid}` + org | `portal/grafana.rs:355` `holds` | | 404 = gone, 2xx = held |
| G6 | `DELETE /api/dashboards/uid/{uid}` + org | `portal/grafana.rs:373` `remove` | | 404 = already gone, else 2xx |
| G7 | browser `GET /d/{uid}?orgId={n}` | link from `portal/api/grafana.rs:214` | | an HTML page for the person |

The sweep (`portal/grafana.rs:561`, every 300 s on the leader) uses G1, G5, then G2-G4 to put back
what a restart lost.

## 3. What `provision.sh` does first (the Portal assumes it)

`POST /api/orgs {name}` (409 then `GET /api/orgs/name/{name}`), `POST /api/datasources` of each
closed Endpoint (409 then `PUT /api/datasources/uid/{uid}`), `POST /api/folders
{uid: project-{project}}` (409/412 fine), `POST /api/dashboards/db` of the project dashboard. All as
the same local administrator, `X-Grafana-Org-Id` from the `POST /api/orgs` answer.

## 4. Fixed on this branch

- **Server administrator** (`model/src/auth.rs` `ServerAdminAuthorizer`, `api/src/state.rs`
  `with_server_admin`): before, the Portal's Basic credential matched no binding, and a project
  organization made by `POST /api/orgs` at runtime had no binding or member, so G1 to G6 and every
  `provision.sh` write were refused. The credential is compared by SHA-256 in constant time, makes
  a member of every organization in the directory, and allows every action there. A subject from
  the host resolver or the tokens file naming `SERVER_ADMIN_ACTOR` is anonymous (tokens file
  refuses it at start). Password at least 16 characters, read from a file in `daiya serve`.
- **Stable organization ids** (`api/src/compat.rs` `org_id_for_name`): the directory of names is
  memory only, and `POST /api/orgs` drew a random id, so on a Postgres store everything saved before
  a restart sat under an id nobody could reach. The id is now derived from installation and name.
- **The dashboard page** (`api/src/page.rs`): G7 answered 404. It now renders each panel's
  server-side table (`panel_view`) with the caller's rights; 404 for a missing organization,
  dashboard or right; no script, `Content-Security-Policy: default-src 'none'`.

Red proofs (each guard reverted once, the contract test failed): the server-admin allow, the
resolver guard, the credential compare, the derived org id, the page's read check, the page's
title escaping.

## 5. Kaisatsu token instead of the password

daiya verifies no JWT itself: identity comes from the host (`SubjectResolver`) or hashed API
credentials. For the Portal to call daiya with a kaisatsu audience-bound token:

1. The Portal (`portal/grafana.rs:263`) asks kaisatsu for a client-credentials token with audience
   the daiya mount and sends `Authorization: Bearer` instead of `basic_auth` (Portal change).
2. The host (`jc`) passes daiya a check that verifies that token in-process (kaisatsu signing key,
   `aud` = daiya, `azp` = the Portal's client, not expired) through
   `DaiyaState::with_server_admin_check(|headers| ..)`; daiya then treats the caller as the server
   administrator, exactly as the Basic credential.

Until both halves land, the password path stays: gap G-1 below.

## 6. Gaps left

- **G-1** The Portal holds Grafana's admin password (`JC_PORTAL_GRAFANA_PASSWORD`); see §5.
- **G-2** The organization directory (names, numbers) is memory only. With the derived ids a
  re-run of `provision.sh` after a restart finds the stored rows again, but numbers follow creation
  order: `provision.sh`'s `org_mapping` check holds only when it creates the organizations in the
  same order each start.
- **G-3** Nothing in `jc` runs `provision.sh`'s steps: the first-start setup must create each
  project's organization and folder `project-{project}` (and the closed Endpoint datasources), or
  the Portal page says the project has no organization.
- **G-4** The page is server-rendered tables, not Grafana's charts; NGSI-LD panels of closed
  Endpoints need the caller's token forwarded (`DenyTokenExchanger` refuses until the host supplies
  an exchanger, STATUS R3).
- **G-5** A person reaches the page only through the host's `SubjectResolver` (kaisatsu session);
  without it a browser is anonymous and gets 404, not a sign-in redirect.
