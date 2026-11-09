# Edge session for jc: the person's access token behind APISIX

Design only, no product code. Written 2026-11-09 by jc-edge-design from monorepo `jc-portal-1`
76ce961c, deployment `main` 11d32d09d and deployment branch `jc-ship`, and the rehearsal evidence
of jc-migrate (`/workspace/evidence/jc-migrate/edge-20261109/apisix-try.log`,
`/workspace/evidence/jc-migrate/smoke/charts-apps/2026-11-09T025813Z/dev/out/REPORT.md`).

Owner rules this design keeps:

- The edge is APISIX with the Kubernetes configuration, in the appliance too. jc gets no edge code.
- The appliance is one container (APISIX + jc + forge) under 2 GB. Postgres and NATS run beside it.
- Limits are per deployment, never per organization.
- Identity is kaisatsu (Keycloak-compatible) only, with audience-bound tokens and no static keys
  between services.

## 1. What fails, and why

The charts-apps smoke on the dev rehearsal has 8 failing panels (REPORT.md lines 20-33). They fail
for two causes, and only one of them is the edge.

| Panels | Endpoint | Audience in the seed | Error | Cause |
|---|---|---|---|---|
| bbsk#1, bbsk#2 | `qfhhh5no…` (bbsk KPI) | `public` (`components/context-gateway/seed/bbsk/bbsk-endpoint-kpi.yaml:19`) | daiya: `token exchange … refused: no token of the person for this Endpoint` | daiya |
| helsinki#1 | `kvhnixtb…` | `public` | same | daiya |
| helsinki#2, #3, pv-9705…#1, pv-e8c0…#1 | `jtjnhkaa…` (air quality) | `public` (`seed/helsinki/helsinki-endpoint-air-quality.yaml:13`) | same | daiya |
| helsinki#4 "City bikes, members" | members-only | not public | Portal: `this panel is read with your own access token, which this session does not carry` | no edge session |

The jc-migrate note of 03:01Z says all 8 panels read members-only Endpoints. The seed shows that
7 of them read public Endpoints.

**daiya cause.** The Portal judges these Endpoints public (`members_only`,
`crates/portal/src/api/grafana_dashboards.rs:566`). It hands daiya no person token, which is
correct. daiya's datasource has `oauthPassThru` on, and with that flag it always calls the
exchanger, even with an empty caller token (`crates/daiya/query/src/ngsild.rs:111-120`, the same
at `odata.rs:56`). `PersonTokenExchanger` (`crates/daiya/api/src/state.rs:455`) then refuses the
empty token. Grafana does something else with `oauthPassThru` and no user token: it sends the
query without `Authorization`, and the gateway answers a public Endpoint anonymously. So this is
a daiya parity bug. It needs no edge and no token (slice S1).

**Edge cause.** That leaves one panel, and with it every other "read as the person" path of the
Portal (section 2.3). In jc those paths have no edge session.

## 2. Today on dev

### 2.1 Edge login

1. A person opens `https://portal.{domain}/…`. Route `portal-ui` (`components/portal/apisix-routes.yaml:4`)
   runs plugin config `portal-ui` (`components/portal/apisix-plugins.yaml`):
   - `serverless-pre-function` clears `X-Access-Token`, `X-Userinfo` and the other forged
     headers, in both the hyphen and underscore spellings, plus every `x-jc-`, `jc-provenance-`
     and `l5d-` header (lines 34-57);
   - `openid-connect` (line 71) with client `edge`, secret `${EDGE_CLIENT_SECRET}`, discovery
     `https://idm.${DOMAIN}/realms/${REALM}/.well-known/openid-configuration`,
     `unauth_action: auth`, `use_pkce: true`, `use_jwks: false`, `set_access_token_header: true`,
     session cookie `__Host-jc_edge` (idle 3600 s, absolute 36000 s).
2. A person without a session goes through the code flow with Keycloak realm `${REALM}`, which is
   `instanceSlug`: `dev` on dev. The callback lands on `https://portal.{domain}/callback`, which
   the plugin owns.
3. On every later request the plugin puts the person's access token in `X-Access-Token`. The
   other routes with the same login:
   - `portal-public` (line 168) and `portal-api` (line 344) use `unauth_action: pass`, so a CLI
     bearer passes untouched;
   - `apps-surface` (line 646) uses `__Host-jc_edge_apps` on the apex;
   - `context-space-portal` and `context-endpoint-portal` (`components/context-gateway/apisix-plugins.yaml:233`,
     `:464`) set `access_token_in_authorization_header: true`. On the Portal host the session
     becomes `Authorization: Bearer` for `/api/endpoint/*`, `/cs/*` and `/api/mcp*`
     (`components/context-gateway/apisix-routes.yaml:171`, `:224`, `:394`).
4. Secrets: `EDGE_CLIENT_SECRET` comes from Secret `keycloak-client-edge` and `OIDC_SESSION_SECRET`
   from `apisix-oidc-session` (`components/apisix/values/apisix/base-values.yaml.gotmpl:47-60`).
   The standalone template substitutes `${REALM}`, `${DOMAIN}` and the secret placeholders
   (`components/apisix/charts/configuration/templates/_standalone.tpl:21`).
5. Discovery, token and JWKS calls go to the public host `idm.{domain}`. CoreDNS points that host
   back at the cluster ingress (the hairpin).

### 2.2 The `edge` client (`components/apisix/keycloak-clients.yaml:62`)

| Setting | Value |
|---|---|
| Type | confidential, standard flow only, PKCE S256 |
| Not allowed | direct grants, service account, offline_access |
| Signature | RS256 for access and ID tokens, per client. lua-resty-openidc verifies RS/HS only, and the realm signs ES256 (T-0252) |
| Redirects | `portal.{domain}/*`, apex `/apps/*` |
| Logout | `backchannelLogout: true`: Keycloak posts every end of an edge session to the Portal (T-5568) |
| Audience mappers | `portal-api` (the Portal verifies `aud`, PF-46); `context-gateway` (custom); `helsinki-agent-proxy`; the seeded per-Endpoint audiences |

### 2.3 Readers of the token

- **Portal.** `JC_TRUST_EDGE_TOKEN: "true"` and
  `JC_OIDC_ISSUER: https://idm.{domain}/realms/{instanceSlug}`
  (`components/portal/values/portal/base-values.yaml.gotmpl:221-222`).
  - `Front::of` (`crates/portal/src/auth/session.rs:55`) picks Bearer, then Edge (`x-access-token`,
    line 36, counted only with trust on), then the Portal's own cookie.
  - The edge token is verified like a bearer: issuer, signature, expiry, `aud = portal-api`
    (`session.rs:377`).
  - `persons_token` (`crates/portal/src/agents/identity.rs:19`) returns that token for Edge and
    Bearer, and `None` for the Portal's own cookie session.
  - Callers: `grafana_dashboards.rs:609` (`person_for`), `kpi_check.rs:507`,
    `entity_comments.rs:154`, `knowledge.rs:1328`, `agent_runs.rs:667/1041/1620`,
    `catalogue_readable.rs:119`, `activity.rs:416`, `apps/roles.rs:41`, `apps/functions.rs:146`.
  - Back-channel logout accepts a logout token addressed to `edge` only when trust is on
    (`crates/portal/src/auth/oidc.rs:1076`).
- **Panel path.** `person_for` exchanges the person's token at the realm
  (`OidcClient::exchange_for_endpoint`, `crates/portal/src/auth/oidc.rs:364`: token exchange V2 as
  `portal-api`, scope `endpoint:{slug}`, audience the hub). The Portal hands the result to daiya in
  `x-daiya-person-token` (`crates/daiya/api/src/state.rs:409`).
  - `PersonTokenExchanger` forwards it only to the datasource of that slug and only when the token
    carries scope `endpoint:{slug}` (`state.rs:455-480`).
  - On dev the old Portal passes it to Grafana the same way.
- **Gateway.** It reads `Authorization` only (`auth/subject.rs admit_on`). The browser's direct
  reads of a closed Endpoint work on dev only because `context-endpoint-portal` turns the edge
  session into that header. In jc this is the gap `bins/jc/STATUS.md:374-376` names.

### 2.4 jc today

- **Edge config.** `components/jc` on `jc-ship` has one slug-mode route `jc` on `jc.{domain}`
  (`/*`), plus `jc-identity` (`/id/*` through the headless Service). The plugin config `jc` has no
  `openid-connect`. It clears `X-Access-Token` ("jc authenticates every caller itself"), and its
  `uri-blocker` refuses `/streams`, `/broker` and `/git` with 404. So even the committed jc edge
  can never supply a person token.
- **Realm.** jc's Portal signs in at the fixed realm `jc` (`bins/jc/src/setup.rs:18 PORTAL_REALM`).
  The constant is used at `siblings.rs:153`, `modules/gateway.rs:136`,
  `modules/kaisatsu.rs:106,108,238` and `setup.rs:423,454`. The shipped realm has `portal-api`,
  `portal-reconciler`, `context-gateway` and `account-console`, and no `edge`.
- **Rehearsal.** jc-migrate ran dev's rendered edge against the rehearsal and every
  `portal.{domain}` page answered 500. In slug mode kaisatsu sits under `/id`, so the discovery URL
  `idm.{domain}/realms/dev/.well-known/openid-configuration` reached the Portal SPA, HTML came back
  and the JSON decode failed (`apisix-try.log`, `openidc_discover(): could not decode JSON`).
- **Smoke suites.** They take one `--base-url` and slug paths (`/id/realms/jc/...`,
  `/dashboards/api/...`, `/git/api/v1/...` in
  `bins/jc/smoke/charts-apps/{session.py,suite.py}` on `jc-smoke-charts-apps`). They refuse any
  host that is not `localhost` or a loopback IP (`bins/jc/smoke/common.py:19`).
- **A finding that weakens T-5528.** KNOWLEDGE T-5528 says the Portal never holds a person's
  access token. But the Portal's own cookie session already carries the person's refresh token
  in its sealed `REFRESH_COOKIE` (`crates/portal/src/auth/session.rs:173-176`). It refreshes with
  it (`oidc.rs:390`) and drops the access token it gets back. The Portal therefore holds a
  credential stronger than an access token for every cookie session, just not server-side.

## 3. Options

### (a) Host mode with the edge's `openid-connect` against kaisatsu (as on dev)

jc runs in host mode as CUTOVER section 4 says:

- kaisatsu at `idm.{domain}` with prefix `""`;
- Portal at `portal.{domain}`;
- dataroom at `opendata.{domain}`;
- daiya at `{domain}/grafana`;
- shako at `{domain}/git`.

The deployment keeps the old components' route and plugin files unchanged and moves only their
upstreams to jc. kaisatsu then serves the `edge` client in the deployment's realm, and the
realm's name becomes a deployment setting.

| Aspect | Assessment |
|---|---|
| Same APISIX configuration | yes, file for file. Only upstream values differ, which CUTOVER already plans |
| Fixes | the members-only panel; every `persons_token` caller; the browser's direct gateway reads (the STATUS gap) through `context-endpoint-portal` |
| Security | the trust chain dev has run since ADR-N-019, unchanged: header sanitiser on every route, token verified like a bearer, `aud` bound, `__Host-` cookie, lifetime bound to the realm SSO times, back-channel logout. New surface: kaisatsu must sign the `edge` client RS256 while the realm stays ES256 (supported per client: `crates/kaisatsu/oidc/src/handlers.rs:1117-1119`, `server/tests/portal_contract.rs:1578`) |
| Issuer | stays `https://idm.{domain}/realms/<realm>`. CUTOVER needs that anyway: MCP connectors, CIMD clients, decidim, observability and jcctl configurations name it |
| Cost | one jc setting (the realm name, 7 call sites), one realm file (the `edge` client), deployment route upstreams, rehearsal and suite changes. About 5 worker slices |
| Appliance RAM | APISIX standalone with 2 workers, about 60-120 MiB. Sessions live in the cookie, so no shared-dict growth |
| Hairpin | discovery dials `idm.{domain}`. Kubernetes keeps CoreDNS. The appliance maps the name to 127.0.0.1 in its own `/etc/hosts` and serves 443 with the real certificate, so `ssl_verify: true` holds |

**Realm choice.**

- (a1) An `edge` client in realm `jc` would change the issuer of every migrated deployment
  (`realms/dev` becomes `realms/jc`) and break every external client: refused.
- (a2) Recommended: the realm name is a deployment setting (`[identity] realm` in `jc.toml`,
  default `jc`). The cutover imports the Keycloak realm `dev`, whose `edge` client and secret come
  with the export. A fresh install, the appliance included, gets the `edge` client from jc's
  first-start realm files.

### (b) The Portal keeps the person's tokens server-side

At sign-in the Portal stores the access and refresh tokens per session (Postgres, encrypted with a
KEK) and uses them in `persons_token` for cookie sessions.

| Aspect | Assessment |
|---|---|
| Same APISIX configuration | no. jc's edge stays without a login and diverges from Kubernetes, where the edge does the login |
| Fixes | the panels and the Portal-side readers only. The browser's direct `/api/endpoint/*` reads stay anonymous (the gateway reads `Authorization` alone) unless every such read is proxied through the Portal: a second data path |
| Security cost | reverses T-5528. The Portal database plus its KEK becomes a vault of every live person's tokens, so one Portal compromise or one backup leak hands out live sessions. It also needs refresh scheduling, revocation on back-channel logout, and multi-replica consistency. A new table, a new KEK and a new key-rotation story |
| Cost | the largest of the three: about 1500 lines of Portal code and a migration. It still leaves two login fronts (edge on Kubernetes, Portal cookie on jc) to keep equal |
| Verdict | rejected |

### (c) Simpler paths found

- **(c1) The daiya parity fix.** `oauthPassThru` with no caller token sends no `Authorization`, as
  Grafana does. This clears 7 of the 8 panels with no edge and no token. It is needed under every
  option. Taken (S1).
- **(c2) The Portal mints the access token from the refresh cookie it already holds.** On
  `Front::Portal` it runs one refresh grant per request that needs the token, keeps the token in
  memory for that request only, and stores nothing new. Costs:
  - one realm call per panel read;
  - concurrent refreshes race when refresh-token rotation is on (`invalid_grant`), and a rotated
    refresh token has to be written back through Set-Cookie on a data read;
  - the browser's direct gateway reads stay anonymous;
  - jc's edge still diverges from Kubernetes.

  The supervisor's 03:10Z decision also excludes a token path inside jc. Rejected for deployed
  environments. It stays as the documented option for a bare `jc` with no edge, which neither
  Kubernetes nor the appliance is.
- **(c3) Keep slug mode and only point discovery at `jc.{domain}/id/realms/<realm>`.** This breaks
  CUTOVER's unchanged issuer, and slug mode shares one origin for every module: the Portal cookie
  on `/` reaches kaisatsu, dataroom and daiya (supervisor notes 2026-11-08, "use host mode for
  Portal + kaisatsu"). Rejected.

Only (a) keeps "same APISIX configuration as Kubernetes".

## 4. Recommendation

1. **S1 now:** the daiya parity fix (c1). It closes 7 of the 8 rehearsal panels on its own.
2. **Option (a2) for the rest.** Host mode per CUTOVER section 4. The deployment's realm name is a
   jc setting. kaisatsu serves the `edge` client (imported on cutover, written by first-start
   setup on a fresh install). The old components' APISIX files stay byte for byte, with
   upstreams moved to jc.
3. **Retire the slug-mode `jc.{domain}` route** for deployed environments. Slug mode stays for a
   bare local `jc` without an edge, where members-only reads as the person are documented as
   unsupported.
4. **No new token store, and no token path in jc.** T-5528 stands for the access token. The
   refresh-cookie finding (2.4) goes to the security review as a note, with no change.

One open question for the owner, out of scope here: dev's edge plugin configs say
"The Organization's spec.limits.edge.requestsPerMinute.* replaces the count below (ADR-N-035)",
which is a per-organization limit. The owner rule says per deployment. The cutover keeps the
counts as rendered.

## 5. Slices

Each slice is one worker and one branch. S2 to S4 depend on each other in this order. S1 is
independent. S5 needs S2 to S4. S6 comes with the appliance task.

### S1: daiya reads with no token when none was handed over (monorepo `crates/daiya/query`)

- **Change.** `ngsild.rs:111` and the `odata.rs:56` branch: when `oauth_pass_thru` is on and the
  caller token is empty, add no `Authorization` and do not call the exchanger. A non-empty token
  still goes through the exchanger, so `PersonTokenExchanger`'s `endpoint:{slug}` rule stays.
- **Done when.** The 7 public panels of section 1 answer frames in charts-apps on both rehearsals.
  "City bikes, members" still shows the Portal's "does not carry" line.
- **Tests.**
  - A panel over a public Endpoint renders with no person token, and the upstream request carries
    no `Authorization` header (stub asserts absence).
  - A members-only Endpoint read with no token reaches the gateway anonymously, and its 401 is the
    panel's error. No other token is ever attached.
  - A token without `endpoint:{slug}` is still refused (`crates/daiya/api/tests/person_token.rs`
    stays green).
  - Red proof: revert the empty-token guard and see the first test fail.
- **Security.** No widening: the gateway decides the anonymous read by the Endpoint's audience. A
  person token still reaches only its own Endpoint.

### S2: the realm name is a deployment setting (monorepo `bins/jc`)

- **Change.**
  - Add `[identity] realm` in `jc.toml` and `JCBIN_IDENTITY_REALM`. Default `jc`. Refuse a name
    outside `[A-Za-z0-9._-]` and the name `master`.
  - Replace `setup::PORTAL_REALM` with that value at the 7 call sites (`siblings.rs:153`,
    `modules/gateway.rs:136`, `modules/kaisatsu.rs:106,108,238`, `setup.rs:423,454`). The
    endpoint-scope linker follows it too.
  - One realm per deployment: Organizations stay inside it (multiorg).
- **Done when.** `jc` with `realm = "dev"` over an imported Keycloak `dev` export:
  - the Portal signs in at `/realms/dev`;
  - the gateway admits tokens with `iss = …/realms/dev`;
  - the linker links `endpoint:{slug}` scopes to `portal-api` in `dev`.
- **Tests.**
  - Config parse, with the bad names refused.
  - `portal_siblings` with the realm set.
  - **A token of realm A never admits in realm B.** kaisatsu holds `jc` and `dev`, and the deployment
    names `dev`. A valid realm-`jc` token is 401 at the Portal (bearer and `X-Access-Token`) and
    at the gateway. A token exchange at `dev` with a realm-`jc` subject token is refused
    (`invalid_grant` or `invalid_token`). Red proof: point the Portal's issuer check at the wrong
    realm once.
- **Security.** The issuer stays exact, never a prefix match. Keys stay per realm. Both are
  asserted in the realm test.

### S3: kaisatsu serves the `edge` client (monorepo `bins/jc/src/setup.rs`, `modules/kaisatsu.rs`)

- **Change.**
  - First-start setup gets two new secrets, `EdgeClientSecret` (32 bytes) and `EdgeSessionSecret`
    (32 bytes, for APISIX's `OIDC_SESSION_SECRET`), each one 0600 file, never logged.
  - When the state directory holds the edge client secret, realm file `50-jc-edge.json` is applied
    in the configured realm. It mirrors `components/apisix/keycloak-clients.yaml:62`:
    confidential; standard flow and PKCE S256 only; RS256 access and ID tokens; redirects
    `{portal}/*` and `{apex}/apps/*`; back-channel logout to the Portal; `fullScopeAllowed` as on
    dev; audience mappers `portal-api` and `context-gateway`; no direct grants, no service
    account, no `offline_access`.
  - The realm keeps an active RS256 key.
  - A realm imported on cutover that already holds `edge` keeps its secret: apply mode
    `no-delete`, and the file does not overwrite an existing `secret`.
- **Done when.** APISIX 3.17.0 (the digest dev pins) completes a code flow against jc's kaisatsu
  in host mode, and the Portal verifies the resulting `X-Access-Token`.
- **Tests.**
  - The realm file's shape (unit).
  - The ID token for `edge` is RS256 and verifies against the realm JWKS.
  - The access token carries `aud` `portal-api` and `context-gateway`, and not
    `account`-only.
  - No `edge` client is written when the secret file is absent (deny by default).
  - Back-channel logout of an edge session revokes the Portal subject.
  - Discovery at `idm.{host}/realms/<realm>/.well-known/openid-configuration` is JSON with the
    matching `issuer` in host mode.
- **Security.**
  - The client secret reaches APISIX by environment only, in Kubernetes from Secret
    `keycloak-client-edge`.
  - No static key between services: the edge is an OIDC client of the realm.
  - Token lifetime is bound to the realm SSO idle (3600 s) and max (36000 s) times.

### S4: components/jc in host mode behind the old routes (deployment, from `jc-ship`)

- **Change.**
  - `jc.toml` gets `mode = "host"` and the mounts of CUTOVER section 4, plus `[identity] realm`
    set to `{{ instanceSlug }}`.
  - Pod env: `JC_TRUST_EDGE_TOKEN: "true"`, and Secrets `keycloak-client-edge` and
    `apisix-oidc-session` copied into the state directory by `seed-state`.
  - Each component route file stays as it is. Its `upstream:` goes through one helper, roughly
    `jc.upstream (dict "component" "portal" "default" "<svc>")`. The helper answers
    `jc.<ns>.svc:8080` when the environment lists that component under `jc.replaces`; CUTOVER's
    steps fill that list.
  - Delete the slug route `jc` and its `/id/*` early-start route for environments in host mode.
    Keep an `idm.{domain}` route to `jc-pods` (headless) if the start-order reason of the
    `jc-identity` route still holds.
  - `uri-blocker` rows on every route whose upstream is jc:
    - `^/streams([/?]|$)` and `^/broker([/?]|$)` stay refused with 404;
    - `^/git([/?]|$)` is refused until step 4, then served by the re-pointed `gitea-forge` route
      (CUTOVER step 4).
  - Goldens: `JUST_NO_DOTENV=true just golden-update`, because routes are added and removed.
- **Done when.** The render for an environment with `jc.replaces: [keycloak, portal, grafana, ckan]`
  is the same apisix.yaml as `main` apart from upstream lines (proved by a test that diffs the
  route objects without `upstream`). kubeconform, conftest and `kyverno test` are green.
- **Tests.**
  - `tests/test_apisix_routes.py` and `test_apisix_edge.py` (both pin the plugin sets per route).
  - `test_edge_attack_surface.py`: a `ROUTE_CLASSES` row for every new route.
  - Every route to jc runs the forged-header pre-function, so `X-Access-Token` is cleared.
  - `test_networkpolicies.py` (APISIX to jc on 8080 and 4143 only).
  - `test_postgres_connection_budget.py`.
- **Security.**
  - jc accepts ingress from APISIX only (NetworkPolicy).
  - `/streams` and `/broker` stay closed.
  - No route opens that is closed today.
  - `JC_TRUST_EDGE_TOKEN` is set only where every route to jc clears the header.

### S5: the rehearsal behind the edge, and the suites on hosts (monorepo `deploy/rehearsal`, `bins/jc/smoke`)

- **Change.**
  - `deploy/rehearsal/up.sh` starts APISIX standalone (the pinned digest) from the S4 render with
    rehearsal values: `domain = rh-<env>.localhost`, plain http, discovery
    `http://idm.rh-<env>.localhost:<port>/realms/<realm>/…`, every upstream `127.0.0.1:<jc port>`.
    jc binds its public listener on 127.0.0.1 in host mode.
  - The suites replace `--base-url` with three arguments:
    - `--edge http://127.0.0.1:<edge port>`;
    - `--domain rh-<env>.localhost` (each module's requests go to the edge with
      `Host: <module>.<domain>`; Chromium resolves `*.localhost` itself);
    - `--internal-url http://127.0.0.1:<jc port>` for the paths the edge refuses (`/broker`,
      `/streams`, shako's `/git/api`). The edge stays closed for them, and no route opens for a
      test.
  - `loopback_only` keeps checking the edge and internal addresses.
  - The cookie jar is keyed by host, because the `__Host-` cookies of the Portal, idm and the apps
    surface are host-only.
  - `session.sign_in` follows the edge's code flow (portal, then idm login form, then
    `portal/callback`).
  - Paths drop the slug prefixes: `/id/realms/jc` becomes `idm` `/realms/<realm>`, `/dashboards`
    becomes apex `/grafana`, `/opendata` becomes the `opendata` host.
- **Done when.**
  - charts-apps on both rehearsals through the edge shows 0 panel failures, "City bikes, members"
    included.
  - The ui suite counts no worse than `2026-11-09T022124Z` (dev 401/68/456).
- **Tests.**
  - **A members-only panel renders through the edge.** "City bikes, members" returns frames with
    rows for a signed-in member, and a non-member gets the gateway refusal.
  - **A client's `X-Access-Token` is stripped and never trusted.**
    - Without an edge session, send through the edge a valid token of another person in
      `X-Access-Token` (and `X_Access_Token`). The Portal sees `Front::Portal`, so the call is
      401 or redirects to sign-in, and never runs as that person.
    - With a session, a client header carrying another person's token is replaced by the
      session's own token (assert the subject in `/api/v1/auth/me`).
    - A forged token (bad signature) sent straight to jc's loopback listener is 401.
  - **A token for realm A never admits in realm B**, through the edge. A realm-`jc` token as a
    bearer on `portal.{domain}/api/v1/…` and on `/api/endpoint/{slug}` is 401 when the deployment
    realm is `dev`.
  - Logout: `/logout` ends the edge session and the kaisatsu session, and the next page asks for a
    login.
- **Security.**
  - The rehearsal stays on loopback.
  - The suites never read a secret into a log.
  - The person password stays in memory, as today.

### S6 (with the appliance task): one container, same configuration

- **Change.**
  - The image build renders apisix.yaml from a pinned deployment commit with appliance values:
    every replaced component's upstream is `127.0.0.1:8080`, and the domain comes from the
    installation.
  - The entrypoint starts jc (public listener on 127.0.0.1), then APISIX.
  - APISIX reads `EDGE_CLIENT_SECRET` and `OIDC_SESSION_SECRET` from jc's state files (S3).
  - `/etc/hosts` maps `idm.{domain}` and `portal.{domain}` to 127.0.0.1 for the discovery hairpin.
    APISIX serves 443 with the installation's certificate.
  - `nginx_config.worker_processes: 2`.
- **Done when.**
  - S5's three edge tests pass against the container.
  - `docker stats` peak with every module answering is under 2 GB (APISIX + jc + shako), measured
    as `components/jc/README.md` section 4 does.
- **Security.**
  - jc is not reachable from outside the container except through APISIX.
  - The container runs as non-root, holds no `docker.sock`, and sets memory limits.

## Related

- `bins/jc/CUTOVER.md` sections 3-4 (steps and host-mode mounts).
- `bins/jc/STATUS.md`, "Gaps left (stage 8)" (the gateway reads `Authorization` alone).
- deployment `components/apisix/README.md` "Edge login" and `components/portal/apisix-plugins.yaml`.
- KNOWLEDGE T-5528, T-6549 (cookie size), T-0252 (RS256 edge client), T-5568 (back-channel logout).
