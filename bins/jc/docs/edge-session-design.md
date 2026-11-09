# Edge session for jc: the person's access token behind APISIX

Design only, no product code. First written 2026-11-09 by jc-edge-design (79b36856). Revised the
same day by jc-edge-design-2 after the supervisor's critic
(`/workspace/supervisor/workers/queue/done/29-jc-edge-design.critic.md`, points 2-8 and 11).

Sources, re-checked for this revision:

- monorepo `jc-realm-name` 1858742e (S2 built on `jc-portal-1` 76ce961c);
- deployment `main` db7b9a38c and branch `jc-ship` 6e0555e1d (the first design cited
  11d32d09d; every deployment cite below was re-read at db7b9a38c);
- platform `main` eaa84afc for `jcctl` (`crates/jcctl/src/apisix.rs`);
- measurements of this revision under `/workspace/evidence/jc-edge-design-2/` (section 6).

Owner rules this design keeps:

- The edge is APISIX with the Kubernetes configuration, in the appliance too. jc gets no edge code.
- The appliance is one container (APISIX + jc + forge) under 2 GB. Postgres and NATS run beside it.
- Limits are per deployment, never per organization.
- Identity is kaisatsu (Keycloak-compatible) only. Tokens are audience-bound, no static keys
  between services, a client-sent `X-Access-Token` is stripped, and no token crosses realms.

## 1. What fails, and why

The charts-apps smoke on the dev rehearsal has 8 failing panels
(`/workspace/evidence/jc-migrate/smoke/charts-apps/2026-11-09T025813Z/dev/out/REPORT.md` lines
20-33). They fail for two causes, and only one of them is the edge.

| Panels | Endpoint | Audience in the seed | Error | Cause |
|---|---|---|---|---|
| bbsk#1, bbsk#2 | `qfhhh5no…` (bbsk KPI) | `public` (`components/context-gateway/seed/bbsk/bbsk-endpoint-kpi.yaml:19`) | daiya: `token exchange … refused: no token of the person for this Endpoint` | daiya |
| helsinki#1 | `kvhnixtb…` | `public` | same | daiya |
| helsinki#2, #3, pv-9705…#1, pv-e8c0…#1 | `jtjnhkaa…` (air quality) | `public` (`seed/helsinki/helsinki-endpoint-air-quality.yaml:13`) | same | daiya |
| helsinki#4 "City bikes, members" | members-only | not public | Portal: `this panel is read with your own access token, which this session does not carry` | no edge session |

**daiya cause.** With `oauthPassThru` on, daiya called the exchanger even with an empty caller
token (`crates/daiya/query/src/ngsild.rs:111-120`). Grafana sends such a query without
`Authorization`, and the gateway answers a public Endpoint anonymously. S1 fixes this parity bug
with no edge; the supervisor released it as 31-jc-daiya-public.

**Edge cause.** That leaves one panel, and with it every Portal path that reads as the person
(section 2.3). In jc those paths have no edge session.

## 2. Today on dev

### 2.1 Edge login

1. A person opens `https://portal.{domain}/…`. Route `portal-ui`
   (`components/portal/apisix-routes.yaml:4`) runs plugin config `portal-ui`
   (`components/portal/apisix-plugins.yaml:26`):
   - `serverless-pre-function` clears `X-Access-Token`, `X-Userinfo` and the other forged
     headers, in both the hyphen and underscore spellings, plus every `x-jc-`, `jc-provenance-`
     and `l5d-` header (lines 34-57);
   - `openid-connect` (line 71) with client `edge`, secret `${EDGE_CLIENT_SECRET}`, discovery
     `https://idm.${DOMAIN}/realms/${REALM}/.well-known/openid-configuration`,
     `unauth_action: auth`, `use_pkce: true`, `use_jwks: false`, `set_access_token_header: true`,
     session cookie `__Host-jc_edge` (idle and rolling 3600 s, absolute 36000 s). No
     `renew_access_token_on_expiry`, `access_token_expires_leeway` or `refresh_session_interval`
     is set, so lua-resty-openidc renews on expiry, per request.
2. The code flow runs against realm `${REALM}`, which is `instanceSlug` (`dev` on dev). The
   callback is `https://portal.{domain}/callback`, owned by the plugin.
3. On every later request the plugin puts the person's access token in `X-Access-Token`. Other
   routes with the same login: `portal-public` (plugins line 168) and `portal-api` (line 344)
   with `unauth_action: pass`; `apps-surface` (line 646, `__Host-jc_edge_apps`); and
   `context-space-portal` / `context-endpoint-portal`, which turn the session into
   `Authorization: Bearer` on the Portal host
   (`components/context-gateway/apisix-routes.yaml:61`, `:244`).
   The APISIX plugin maps `unauth_action: pass` to lua-resty-openidc's `deny` and lets the request
   through without a token when that fails (`apisix/plugins/openid-connect.lua` in the pinned
   image, lines 852-873).
4. Secrets: `EDGE_CLIENT_SECRET` from Secret `keycloak-client-edge` and `OIDC_SESSION_SECRET` from
   `apisix-oidc-session` (`components/apisix/values/apisix/base-values.yaml.gotmpl:47-60`). The
   standalone template substitutes `${REALM}`, `${DOMAIN}` and the secret placeholders
   (`components/apisix/charts/configuration/templates/_standalone.tpl:21`).
5. Discovery, token, JWKS and userinfo calls go to the public host `idm.{domain}`, which CoreDNS
   points back at the ingress (the hairpin). APISIX's nginx takes 16 KiB request headers
   (`base-values.yaml.gotmpl:176-180`), and the userinfo hairpin needs that (section 6.2).
6. App hosts `{name}.apps.{domain}` are not in the helm render: the Portal's edge-file wave reads
   ConfigMap `apisix-standalone-base`, adds each published App's routes with its own client
   `app-{name}`, and writes Secret `apisix-standalone-config`
   (`crates/portal/src/reconciler/edge_file.rs:1-24`, started in `crates/portal/src/state.rs:625-655`
   only with an in-cluster Kubernetes client).

### 2.2 The `edge` client (`components/apisix/keycloak-clients.yaml:62-353`)

| Setting | Value |
|---|---|
| Type | confidential, standard flow only, PKCE S256 |
| Not allowed | direct grants, service account, offline_access |
| Signature | RS256 for access and ID tokens by client attribute (lines 89-90); the realm default is ES256 (`components/keycloak/values/config/base-values.yaml.gotmpl:119`). lua-resty-openidc verifies RS/HS only (T-0252) |
| Redirects | `portal.{domain}/*`, apex `/apps/*` |
| Logout | `backchannelLogout: true` (T-5568) |
| Audience mappers | 31: `portal-api`, `context-gateway`, `helsinki-agent-proxy` (line 127) and 28 seeded per-Endpoint slugs |
| Source of truth | the file is merged into `.Values.keycloakClients` and rendered by keycloak-config-cli's template (`components/keycloak/values/config/base-values.yaml.gotmpl:108-112` realm, `:702-703` clients, `:785` secret placeholder `$(CLIENT_SECRET_<ID>)`, `:93-105` the Secrets) |

The realm rotates refresh tokens: `revokeRefreshToken: true`, `refreshTokenMaxReuse: 0`
(`components/keycloak/values/config/base-values.yaml.gotmpl:121-122`). The access token lives
300 s in the base profile (`:123`) and 3600 s in `development` (`development-values.yaml.gotmpl:16`),
which dev and prod use.

### 2.3 Readers of the token

- **Portal.** `JC_TRUST_EDGE_TOKEN: "true"`, `JC_OIDC_ISSUER: https://idm.{domain}/realms/{instanceSlug}`
  (`components/portal/values/portal/base-values.yaml.gotmpl:221-222`). `Front::of`
  (`crates/portal/src/auth/session.rs:55`) picks Bearer, then Edge (`x-access-token`, line 36,
  only with trust on), then the Portal's own cookie. The edge token is verified like a bearer:
  issuer, signature, expiry, `aud = portal-api`. `persons_token`
  (`crates/portal/src/agents/identity.rs:19`) hands it to `grafana_dashboards.rs:609` and the
  other readers listed in the first design.
- **Panel path.** `OidcClient::exchange_for_endpoint` (`crates/portal/src/auth/oidc.rs:364`) turns
  the person's token into one for `endpoint:{slug}`; daiya forwards it only to that slug.
- **Gateway.** It reads `Authorization` only. The browser's direct reads of a closed Endpoint work
  on dev because `context-endpoint-portal` converts the edge session (`bins/jc/STATUS.md:374-376`).

### 2.4 jc today

- **Edge config.** `components/jc` on `jc-ship` has the slug-mode route `jc` on `jc.{domain}` and
  `jc-identity` (`/id/*` through the headless Service) (`components/jc/apisix-routes.yaml`). Its
  plugin config has no `openid-connect`, clears `X-Access-Token`, and its `uri-blocker` refuses
  `^/streams`, `^/broker`, `^/git` with 404 (`components/jc/apisix-plugins.yaml:54-59`).
- **Realm (S2, built).** `[identity] realm` decides the deployment's realm (jc-realm-name
  1858742e). An imported realm that defines `portal-api` is owned by the import, and jc writes
  none of its first-start files into it (`bins/jc/src/modules/kaisatsu.rs:153-185`, `:238-259`).
  The shipped realm `jc` has no `edge` client.
- **Host mode.** One host and prefix per module (`bins/jc/src/config.rs:163-166`
  `MountOverride`). `plan_mounts` adds extra router entries only for the gateway's root paths in
  slug mode (`bins/jc/src/app.rs:94-111`); in host mode the gateway gets `api.{domain}` alone
  (`bins/jc/src/module.rs:55-95`). The router itself takes several entries per module and refuses
  only an equal host and prefix (`crates/shared/jc-router/src/lib.rs:190-224`).
- **T-5528 note (unchanged).** The Portal's cookie session carries the person's refresh token in
  `REFRESH_COOKIE` (`crates/portal/src/auth/session.rs:173-176`). It goes to the security review
  as a note.

## 3. Options

- **(a) Host mode behind the edge's `openid-connect`, as on dev.** jc runs in host mode, the old
  components' route and plugin files stay, and only upstreams move to jc. kaisatsu serves the
  `edge` client in the deployment's realm. Only (a) keeps "same APISIX configuration as
  Kubernetes".
- **(b) The Portal stores tokens server-side.** Rejected: it reverses T-5528, adds a token vault,
  and the browser's direct gateway reads stay anonymous.
- **(c1) daiya parity.** Taken as S1.
- **(c2) The Portal mints access tokens from its refresh cookie.** Rejected for deployed
  environments. The critic noted (point 11) that c2's rotation race also hits (a). Section 6.1
  measures it: APISIX renews once per request, so parallel requests after expiry race on (a) too.
  S3b fixes that race in kaisatsu, so (a) stays the choice.
- **(c3) Slug mode with discovery under `/id`.** Rejected: it changes the issuer and shares one
  origin for every module.

## 4. Recommendation

1. S1 (released) and S2 (built) stand.
2. Option (a): host mode, with **several mounts per module** (S4a), because dev serves the gateway
   on three hosts and the Portal on two plus every App host (section 5, point 4).
3. kaisatsu gets two fixes before an edge points at it:
   - S3a: a client that names a signature algorithm never gets a token signed with another one,
     and jc refuses to start when the edge client is not RS256;
   - S3b: a rotation grace window, so parallel renewals keep the session.
4. The `edge` client comes from the deployment's own realm render (S3c). keycloak-config-cli's
   input becomes jc's import file, and jc keeps no second copy of the client. Section 5, point 6
   gives the reasons.
5. The deployment re-points the old routes at jc (S4b). A diff test allows only the differences
   it lists by name.
6. No new token store and no token path in jc.

## 5. Answers to the critic

### Point 2: the signing-key fallback

- **Evidence.** `select_signing_key` (`crates/kaisatsu/oidc/src/handlers.rs:1213-1232`) takes the
  client's `access.token.signed.response.alg` / `id.token.signed.response.alg` or the realm
  default. With no ACTIVE key of that algorithm it signs with any ACTIVE key. Test
  `signing_key_falls_back_to_an_active_key` (`crates/kaisatsu/server/tests/oidc_errors.rs:248-266`)
  pins that for PS512.
- **When it bites.** Every realm gets ACTIVE ES256 and RS256 keys:
  - on creation, in both stores (`crates/kaisatsu/store-memory/src/lib.rs:81-99`,
    `store-pg/src/lib.rs:305-336`);
  - on every key read, in the memory store (`store-memory/src/lib.rs:1637`);
  - immediate rotation keeps one ACTIVE (`crates/kaisatsu/crypto/src/keys.rs:341-389`,
    `admin/src/operations.rs:396-422`).

  So an RS256 edge client gets RS256 today, and the first design's citations
  (`handlers.rs:1117`, `portal_contract.rs:1578`) were wrong. The fallback still bites in three
  cases: a PG realm whose RS256 row was retired by hand, an algorithm kaisatsu does not generate,
  and an imported `edge` client whose export lost the two attributes. In the last case the realm
  default (ES256 on dev) signs.
- **What the edge does with ES256 (measured).** It does not refuse the login, which T-0252
  assumed. lua-resty-openidc 1.8.0 logs `ignored id_token signature as algorithm 'ES256' is not
  supported` and signs the person in with the ID token's signature unchecked (`openidc.lua:1061-1078`;
  APISIX's `accept_unsupported_alg` defaults to `true`, `openid-connect.lua:397-401`). With
  `accept_unsupported_alg: false` the same login is a 500 ("token is signed using algorithm
  \"ES256\" which is not supported"), and the RS256 login still passes
  (`results/edge-es256-red.txt`). So a lost attribute silently downgrades verification.
- **Keycloak.** Keycloak 26.8.1 does not fall back. Asked for ES512 with no ES512 key, it created
  one and signed ES512 (`/workspace/evidence/jc-edge-design-2/results/alg-probe-keycloak.txt`). The
  fallback is a parity deviation.
- **Decision.**
  - kaisatsu signs a pinned algorithm with that algorithm or refuses. It generates a missing
    ES256, RS256 or EdDSA key, as Keycloak does. For any other algorithm (PS512) it answers 400
    `invalid_client` at the token endpoint ("client asks for PS512, which this realm cannot
    sign"), never a token signed otherwise. Test `oidc_errors.rs:248` flips to assert the
    refusal.
  - jc refuses to start (`StartError`, process exit 1) when the configured realm holds a client
    `edge` whose ID and access token algorithms are not both RS256, or when the realm has no ACTIVE
    RS256 key after `ensure_realm_keys`. The message names the client, the attribute and T-0252.
  - jc does not check a realm without an `edge` client: a bare jc has no edge.
  - The deployment sets `accept_unsupported_alg: false` on every `openid-connect` plugin config,
    on main for every environment (S3c). Kubernetes and the appliance then stay identical, and the
    edge refuses an unverifiable ID token instead of ignoring its signature.

### Point 3: refresh-token reuse revokes the session (measured)

- **Evidence.**
  - kaisatsu always rotates. A rotated-away handle revokes the family and deletes the SSO session
    (`handlers.rs:1961-1974`); a lost compare-and-swap answers `invalid_grant`
    (`handlers.rs:1436-1456`).
  - The realm flags `revokeRefreshToken` and `refreshTokenMaxReuse` are imported
    (`crates/kaisatsu/core/src/model.rs:20-21`) but read nowhere.
  - lua-resty-openidc 1.8.0 renews on expiry inside each request, with no lock across requests or
    workers (`resty/openidc.lua:1403-1494` in the pinned image).
- **Measurement (section 6.1).** APISIX 3.17.0 by digest, the `portal-ui` and `portal-api` plugin
  configs of dev, kaisatsu in jc 1858742e. One login, a 60 s access token, then 10 parallel
  requests after expiry:

  | IdP | route | requests with a token | the person afterwards | runs |
  |---|---|---|---|---|
  | kaisatsu | `portal-ui` (auth) | 1 of 10, the other 9 answer 302 to the IdP | logged out: the IdP asks for the password | 3 of 3 |
  | kaisatsu | `portal-api` (pass) | 1 of 10, the other 9 pass without a token (the Portal would answer 401) | logged out | 3 of 3 |
  | kaisatsu, sequential control (one request renews, then 10 parallel) | `portal-api` | 10 of 10 | session kept | 3 of 3 |
  | Keycloak 26.8.1 (dev's digest), same realm | `portal-ui` | 1 of 10 (9 x `REFRESH_TOKEN_ERROR` "Session doesn't have required client") | silent re-login, IdP session kept | 2 of 2 |
  | Keycloak 26.8.1 | `portal-api` | 1 of 10, the other 9 pass without a token | silent re-login, IdP session kept | 2 of 2 |
  | kaisatsu + grace stand-in (30 s) | `portal-ui` | 10 of 10, one token | session kept | 3 of 3 |
  | kaisatsu + grace stand-in (30 s) | `portal-api` | 10 of 10, one token | session kept | 3 of 3 |

  The race exists on dev today (critic point 11 holds): a Portal page that fires parallel API
  calls at the hourly renewal loses all but one of them. With kaisatsu, the person is also logged
  out.
- **Options weighed.**
  - APISIX settings cannot fix it. `renew_access_token_on_expiry: false` sends every expired
    request through the code flow: XHRs then get a cross-origin 302, and a `pass` route gets no
    token. `access_token_expires_leeway` moves the race earlier and keeps it. `refresh_session_interval`
    adds silent re-logins on top. A shared lock would mean edge code, which the owner rule forbids.
  - Honouring `refreshTokenMaxReuse` (Keycloak semantics) keeps the person signed in but still
    loses 9 requests, as the Keycloak rows show.
  - **Chosen: a rotation grace window in kaisatsu (S3b).** It answers every parallel renewal with
    the same successor, and the only measured runs with 10 of 10 requests used it.

### Point 4: hosts and prefixes per module

What dev's APISIX routes serve, per module jc replaces (route ids and lines in
`/workspace/evidence/jc-edge-design-2/dev-routes-main.txt`, deployment db7b9a38c):

| Module | Host | Paths | Routes |
|---|---|---|---|
| kaisatsu | `idm.{d}` | `/*` | `keycloak`, `keycloak-discovery` (`components/keycloak/apisix-routes.yaml:2,16`) |
| Portal | `portal.{d}` | `/*` | `portal-*` (`components/portal/apisix-routes.yaml:4-189`) |
| Portal | `{d}` | `/apps/*` (308 to the App host, `crates/jcctl/src/apisix.rs:10`) | `apps-surface` (plugins `:601`) |
| Portal (static Apps) | `{name}.apps.{d}`, plus each App's own proven hostnames | `/*` | composed by the Portal (`edge_file.rs`), Ingress `app-{name}` (`charts/namespaces/templates/edge-policy.yaml:4-21`) |
| gateway | `api.{d}` | `/cs/*`, `/api/endpoint/*`, `/catalog*`, `/api/mcp*`, `/oauth/*`, `/.well-known/oauth-*` | `api-*` (`components/context-gateway/apisix-routes.yaml:268-759`) |
| gateway | `{d}` | `/cs/*`, `/catalog*`, `/api/endpoint/*`, `/api/mcp*` | `context-space`, `catalog-feed`, `context-endpoint*`, `context-mcp-*`, `api-moved-*` (`:2-182`, `:615-703`) |
| gateway | `portal.{d}` | `/cs/*`, `/api/mcp*`, `/api/endpoint/*` | `context-*-portal*` (`:61-82`, `:164`, `:216`, `:244`, `:386`, `:799`) |
| dataroom | `opendata.{d}` | `/*` | `ckan`, `ckan-webassets`, `ckan-map`, `ckan-action-cache` (`components/ckan/apisix-routes.yaml:4-100`) |
| daiya | `{d}` | `/grafana*` | `grafana` (`components/grafana/apisix-routes.yaml:5`) |
| shako | `{d}` | `/git/*`, `/v2/*` | `gitea-forge`, `gitea-registry*` (`components/gitea/apisix-routes.yaml:2-41`) |
| none (APISIX answers) | `{d}` `/*`, `{d}` `/ckan*`, `data.{d}` `/*` | | `portal-redirect`, `ckan-redirect`, `ckan-old-host`: the `redirect` plugin, no upstream call (`components/portal/apisix-plugins.yaml:702-742`, `components/ckan/apisix-plugins.yaml:156-271`) |

jc today can express one row per module. So the gateway rows on `{d}` and `portal.{d}` and the
App hosts have no mount, and a Portal mount of `portal.{d}` with prefix `""` would take the
gateway's `/api/endpoint/*` on that host. The router already orders the longest prefix first,
so several entries per module are enough. That is slice S4a. App hosts need a wildcard host
entry (`*.apps.{d}`) that the router does not have, and the Portal's edge-file wave needs a
file target in the appliance. Both go into S4a and S6.

### Point 5: realm `dev` on a first start (checked)

`bins/jc/tests/gateway_endpoints.rs::a_deployment_on_realm_dev_admits_its_realm_and_no_other`
passes on 1858742e (`cargo test -p jc --locked --test gateway_endpoints a_deployment_on_realm_dev`,
1 passed in 15.4 s, `/workspace/evidence/jc-edge-design-2/results/point5-test.log`):

- the realm-owner file is `import` (`:1050`);
- `portal-api` keeps the imported redirect (`:1059`, "jc overwrote the imported portal-api");
- `dev` has no `admin` user (`:1061-1065`);
- after a restart on a fresh in-memory store without the twin files, which is a first start,
  `/id/realms/jc/.well-known/openid-configuration` is 404 and `realm-owner-jc` does not exist
  (`:1309-1326`).

The harness of section 6 repeats it with `--role kaisatsu`: `realms/jc` is 404 and the state
directory holds only `kaisatsu-kek`. With the Portal off, jc decides no realm owner
(`kaisatsu.rs:245-250`).

One risk remains. If the cutover's first start lacks the import dir while `realm = "dev"`, jc owns
`dev` and writes a first admin into it. S4b's `seed-state` therefore refuses to start jc when
`[identity] realm` is not `jc` and the import file is missing.

### S3 items: `aud`/`azp` through lua-resty-openidc, and userinfo with RS256 (measured)

The code flow through the pinned APISIX against jc's kaisatsu completes
(`results/cookie-size.jsonl`, `results/forged.json`):

- lua-resty-openidc accepts the RS256 ID token: issuer equal to discovery, `aud` = `edge`, `azp`
  `edge` (`openidc.lua:141-230`);
- the upstream receives `X-Access-Token`: RS256, `iss` `http://idm.edge.test:9080/realms/dev`, `aud`
  `[portal-api, context-gateway, helsinki-agent-proxy]`, `azp` `edge`;
- the userinfo endpoint accepts the RS256 access token: `X-Userinfo` names the person.

Red: the same import without the two algorithm attributes makes kaisatsu sign ES256, and the
login still succeeds, with lua-resty-openidc skipping the ID token signature
(`results/edge-es256-red.txt`, point 2). With `accept_unsupported_alg: false` it is a 500 at the
callback. S3a makes jc refuse to start in that case, and S3c sets the flag.

### Point 6: the edge client's source of truth

Removing keycloak-config-cli must not drop `components/apisix/keycloak-clients.yaml`. Decision:
**jc applies the realm the deployment already renders** (S3c).

- The deployment renders the instance realm (`config.<instanceSlug>.inline`,
  `components/keycloak/values/config/base-values.yaml.gotmpl:108-1249`) a second time, as
  `jc-realm.json` in a ConfigMap of `components/jc`. One change in the render: each
  `$(CLIENT_SECRET_<ID>)` becomes `$(file:/var/lib/jc/import-secrets/client-secret-<id>)`, and
  the `keycloak-client-<id>` Secrets mount there.
- The `master` realm (`:1250`) is not rendered for jc, because jc refuses `master`.
- jc mounts that file as `--kaisatsu-import-dir`. The realm defines `portal-api`, so jc's owner
  rule makes it `import`, and jc writes nothing of its own into it.
- So every `components/*/keycloak-clients.yaml` keeps working: `edge` with its 31 mappers,
  `helsinki-agent-proxy` (line 127), the per-Endpoint audiences, `apisix-gateway`, and the
  Portal's client.
- Apply mode is `no-delete` with the checksum fast path (`crates/kaisatsu/import/src/lib.rs:889-925`),
  as keycloak-config-cli ran on dev.
- jc's own first-start realm gets no `edge` client. The appliance takes the same render (S6), so
  the edge client has one definition. A bare jc has no edge.

### Point 7: S4's diff test

S4b's test lists every allowed difference by name (section 7, S4b, Tests). Anything else is red.

### Point 8: `/etc/hosts` and lua-resty-openidc (measured)

The pinned APISIX patches `ngx.socket.tcp` to resolve through its own DNS client
(`apisix/patch.lua:60-73`). That client reads the hosts file. Test on a docker network
(`harness/dns/run.sh`, `results/dns-hosts.txt`):

| Run | Setup | Result |
|---|---|---|
| red | no hosts entry, no DNS name for `idm.edge.test` | 500, `openidc_discover(): ... failed to parse domain: failed to query the DNS server` |
| green | `--add-host idm.edge.test:<IdP stub>` | 302 to the stub's authorization endpoint |
| decoy | Docker DNS alias `idm.edge.test` on a decoy stub, no hosts entry | 302 to the decoy |
| order | the decoy alias and `--add-host` to the real stub | 302 to the real stub: the hosts file wins over DNS |

So the appliance's `/etc/hosts` line for `idm.{domain}` holds, even where the container's DNS
resolves the public name. APISIX reads the file at start, so the entrypoint writes it first.

## 6. Measurements

Harness: `/workspace/evidence/jc-edge-design-2/harness/` (README there). Pieces:

- jc 1858742e, debug build, `--role kaisatsu`, host mode, realm `dev` imported from a Keycloak-style
  export (`gen.py`): dev's `edge` client with three of its audiences and a full-path groups mapper;
- APISIX `apache/apisix:3.17.0-ubuntu@sha256:27fdde75…` on `--network host` with dev's 16 KiB header
  snippet;
- routes generated from dev's `portal-ui` and `portal-api` plugin configs (`make_apisix_yaml.py`);
- an echo upstream that reports what the edge forwarded, never the token itself;
- a browser-like driver (`drive.py`): one cookie jar per host, applied in arrival order.

Forced by plain http on loopback: http URLs, `ssl_verify: false`, a cookie named `jc_edge`
without `Secure`. None of these touches renewal, chunking or header stripping.

### 6.1 Parallel renewal

Results in the point 3 table. Raw lines: `results/renewal-kaisatsu.jsonl`,
`renewal-kaisatsu-sequential.jsonl`, `renewal-kaisatsu-grace.jsonl`, `renewal-keycloak.jsonl`;
the grace stand-in is `grace_shim.py`.

### 6.2 Cookie and header size (T-6549)

Random 18-character group paths, full path, in the access token, the ID token and userinfo:

| groups | Cookie header to APISIX | chunks (bytes) | X-Userinfo to upstream | Keycloak cookie |
|---|---|---|---|---|
| 0 | 2477 | 2469 | 268 | 2974 |
| 25 | 3861 | 3853 | 1312 | |
| 60 | 5469 | 4088 + 1362 | 2776 | |
| 120 | 8169 | 4088 + 4062 | 5280 | 8712 |
| 250 | 13993 | 4088 + 4087 + 4087 + 1690 | 10688 | 14530 |

- lua-resty-session splits the cookie into chunks of at most 4096 bytes. No Set-Cookie passes the
  browser's 4096-byte limit, so T-6549's failure cannot happen at the edge.
- The limit that does bind is the request header size. With nginx's default 8 KiB buffers
  (`results/header-buffers-8k.txt`):
  - 60 and 120 random-path groups pass;
  - at 250 groups the userinfo hairpin answers 400 "Request Header Or Cookie Too Large", and the
    browser's next page is refused 400;
  - a first run with 48-character paths failed the hairpin already at 120 groups.

  With dev's 16 KiB snippet (`base-values.yaml.gotmpl:176-180`), 250 groups pass. The Cookie
  header reaches 16 KiB at about 290 groups.
- S4b and S6 therefore keep that snippet, and S5 tests 250 groups.

### 6.3 Forged headers

Through the `portal-api` route (`results/forged.json`):

- with no session, `X-Access-Token: forged…` and `X-Userinfo` arrive at the upstream as nothing,
  and so do `X_Access_Token` and `X_Userinfo`;
- with a session, the upstream receives the session's own token and userinfo instead of the
  forged values.

## 7. Slices

S1 is released and S2 is built. S3a, S3b and S4a are independent monorepo slices. S3c and S4b are
deployment slices: S3c needs S3a (the start check reads the rendered `edge`), and S4b needs S4a.
S5 needs S3b, S3c, S4a and S4b. S6 comes with the appliance task.

### S1: daiya reads with no token (released as 31-jc-daiya-public)

Unchanged from the first design.

### S2: the realm name is a deployment setting (built, jc-realm-name 1858742e)

Done. See `/workspace/evidence/jc-realm-name/REPORT.md` and point 5.

### S3a: one signature algorithm per client, and jc's edge check (monorepo `crates/kaisatsu/oidc`, `bins/jc`)

- **Change.**
  - `select_signing_key` (`handlers.rs:1213`): a client attribute naming an algorithm gets a key
    of that algorithm or a refusal. A missing ES256, RS256 or EdDSA key is generated through
    `ensure_realm_keys`'s path. Any other algorithm gets 400 `invalid_client`. Only the realm
    default may still fall back.
  - jc, after the realm apply: if the configured realm holds a client `edge`, its
    `id.token.signed.response.alg` and `access.token.signed.response.alg` must both be `RS256`,
    and the realm must hold an ACTIVE RS256 key. Otherwise jc stops with `StartError` naming the
    client and the attribute.
- **Done when.** A jc whose imported `edge` lacks the attributes refuses to start. The S3 code flow
  of section 5 passes with them.
- **Tests.**
  - `oidc_errors.rs::signing_key_falls_back_to_an_active_key` becomes
    `a_pinned_algorithm_without_a_key_is_refused` (PS512 gives 400) and
    `a_pinned_es256_rs256_eddsa_key_is_generated` (EdDSA on a fresh realm gives an EdDSA token).
  - `bins/jc/tests`: an import with `edge` but no algorithm attributes: jc exits non-zero and the
    log names `edge`. With the attributes it starts. With the PG store and the RS256 row set
    RETIRED by SQL it exits non-zero.
  - Red proof: revert the start check alone and see the import test start jc.
- **Security.** A client never receives a token in an algorithm it did not ask for. The refusal is
  a 400 that names no key.
- **Measures.** None at run time: one store read at start.

### S3b: refresh rotation grace (monorepo `crates/kaisatsu/oidc`, `core`, both stores)

- **Change.**
  - Rotation becomes deterministic. The successor of a presented handle is
    `{family}.{base64url(HMAC-SHA256(k, presented handle))}`, with `k` derived from the KEK by HKDF
    (info `kaisatsu refresh rotation v1`). Only hashes stay stored. The family gains `rotated_at`.
  - In `handle_refresh_token_grant` (`handlers.rs:1963`), take the handle the family rotated away
    last. If it is presented less than `G` seconds after `rotated_at`, by the same authenticated
    confidential client, and the family is not DPoP-bound, kaisatsu answers with a fresh access
    token and the same successor. It does not rotate again and does not revoke.
  - A compare-and-swap loser (`handlers.rs:1452`) takes the same path after re-reading the family.
  - Every other rotated-away handle still revokes the family and the SSO session, as today.
  - `G` comes from client attribute `refresh.token.rotation.grace.seconds`. Default 0, which is
    today's behaviour. The cap is 60. The deployment's `edge` client sets 30.
  - Keycloak parity: `revokeRefreshToken: false` turns rotation off, as in Keycloak. Dev sets it
    `true`.
- **Done when.** The section 6.1 harness, run against kaisatsu itself with no stand-in, shows
  10 of 10 requests with a token and "session kept", on both routes, 3 runs each.
- **Tests.**
  - kaisatsu, memory and PG stores: 10 concurrent refresh grants with one handle all get 200 and
    one refresh token. One family remains and it is not revoked.
  - The previous handle at `G + 1` s revokes the family and the SSO session.
  - A handle two rotations old revokes it inside `G` too.
  - A public client, a DPoP-bound family, another client's secret, or `G = 0` all get today's
    revocation.
  - `bins/jc/tests`: an APISIX-free parallel test against `/token` with the edge client.
  - The harness run named above goes into the task as evidence.
  - Red proof: set `G` to 0 and see the concurrent test lose 9.
- **Security.**
  - Replaying a stolen previous handle works only within `G`, only with the confidential client's
    secret (which only APISIX holds), and it yields the successor the person already holds.
    Reuse detection outside `G` is unchanged.
  - No raw handle is stored.
- **Measures.** The parallel harness above. One HMAC per renewal.

### S3c: jc applies the deployment's rendered realm (deployment `components/keycloak`, `components/jc`, from `jc-ship`)

- **Change.**
  - A named template renders `config.<instanceSlug>.inline` from
    `components/keycloak/values/config/base-values.yaml.gotmpl` once more as `jc-realm.json` in
    ConfigMap `jc-realm`. `$(CLIENT_SECRET_<ID>)` becomes `$(file:/var/lib/jc/import-secrets/client-secret-<id>)`,
    and the user-password placeholders map the same way. `master` stays out.
  - `components/jc` mounts the ConfigMap as `--kaisatsu-import-dir` and the `keycloak-client-*`
    Secrets as `--kaisatsu-import-secrets-dir`. `[identity] realm` is `{{ instanceSlug }}`.
  - The `edge` client gets `refresh.token.rotation.grace.seconds: "30"`
    (`components/apisix/keycloak-clients.yaml:86-90`).
  - Every `openid-connect` plugin config gets `accept_unsupported_alg: false`
    (`components/portal/apisix-plugins.yaml:71,168,344,646`,
    `components/context-gateway/apisix-plugins.yaml:233,464`). This lands on main for every
    environment, so jc's edge and Kubernetes' stay the same file (point 2).
  - keycloak-config-cli's Job stays while Keycloak runs. It goes with Keycloak once `jc.replaces`
    lists `keycloak`.
- **Done when.**
  - jc starts on the dev render with no import refusal.
  - kaisatsu holds every client of `.Values.keycloakClients`. `edge` has its 31 audience mappers,
    `helsinki-agent-proxy` among them.
  - The S3a start check passes.
- **Tests.**
  - `tests/test_jc_realm.py`: the jc render equals keycloak-config-cli's input except the listed
    placeholder rewrites and the absent `master`; the mapper set of `edge` is asserted by name.
  - A pytest that runs the jc image (`--entrypoint jc`, `--role kaisatsu`) on the rendered file
    and checks every client through the admin read, then refuses a render with an edge client
    lacking RS256 (S3a).
  - `test_keycloak_clients.py` and `scripts/check-secrets.py` updated for the new mount.
  - `test_apisix_edge.py`: every `openid-connect` in the render has `accept_unsupported_alg: false`.
    The harness proof is `results/edge-es256-red.txt` (ES256 is a 500 with the flag, a login
    without it).
- **Security.** Secrets stay in Kubernetes Secrets, read as 0600 files; no literal secret in the
  ConfigMap (gitleaks-rendered). No realm other than the instance realm reaches jc.
- **Measures.** Start-up apply time of the dev realm (31 + N clients), logged once.

### S4a: several mounts per module, and App hosts (monorepo `bins/jc`, `crates/shared/jc-router`)

- **Change.**
  - `[mounts.<module>]` takes either one table (as today) or an array `[[mounts.<module>]]` of
    `{host, path_prefix}`. The first entry is the module's own address: issuer, absolute URLs. Each
    later entry adds a router entry with the request's own host in `Mount`, as `also` does for
    slug-mode root paths today (`app.rs:94-111`).
  - The gateway's entries keep the root-path behaviour (`MountPlan::at_root`).
  - The router accepts one wildcard host label, `*.apps.{d}`, matched after exact hosts.
  - Refused at start: two equal entries, a wildcard anywhere but the first label, a wildcard on any
    module but the Portal.
  - The CUTOVER section 4 mount table gains the rows of section 5, point 4.
- **Done when.** jc in host mode answers each row of the point 4 table from the right module, with
  `Host` set as APISIX sends it. That includes `portal.{d}/api/endpoint/{slug}` (gateway, not
  Portal) and `x.apps.{d}/` (Portal static host).
- **Tests.**
  - config parse: single table, array, the refusals;
  - `plan_mounts` dry run with the full dev table;
  - a host-mode e2e: one request per row, with module and status asserted. A request on
    `portal.{d}/api/endpoint/x` never reaches the Portal;
  - an unlisted host is 404 (no default host fallback).
- **Security.** No path is answered on a host dev does not route it on. The wildcard is limited to
  `*.apps.{d}` and to the Portal.
- **Measures.** Router lookup stays a linear scan over about 30 entries: one benchmark line.

### S4b: components/jc behind the old routes (deployment, from `jc-ship`)

- **Change.**
  - `jc.toml`: `mode = "host"`, the S4a mount array, and `[identity] realm`.
  - Pod env `JC_TRUST_EDGE_TOKEN: "true"`.
  - Every replaced component's route file stays. Its `upstream:` goes through one helper,
    `jc.upstream (dict "component" "portal" "default" "<svc>")`, which answers
    `jc.<ns>.svc:8080` when the environment lists the component under `jc.replaces`.
  - Delete the slug routes `jc` and `jc-identity` for environments in host mode.
  - `uri-blocker` on every route whose upstream is jc: `^/streams([/?]|$)` and `^/broker([/?]|$)`
    (404). `^/git([/?]|$)` only until CUTOVER step 4.
  - `seed-state` refuses to start jc when `[identity] realm` is not `jc` and the realm file is
    missing (point 5 risk).
  - The Portal's edge-file wave needs jc's ServiceAccount to read `apisix-standalone-base` and
    write `apisix-standalone-config` in the APISIX namespace, the RBAC the Portal has today.
  - Goldens: `JUST_NO_DOTENV=true just golden-update`, because routes are removed.
- **Done when.** The render for an environment with `jc.replaces: [keycloak, portal, grafana, ckan,
  context-gateway, gitea]` passes `tests/test_jc_edge_diff.py`; kubeconform, conftest and
  `kyverno test .ci/policies` are green.
- **Tests.**
  - `tests/test_jc_edge_diff.py` (point 7). It renders the APISIX file for `dev` on main and for
    the same environment with `jc.replaces`, and compares route objects by `id`. Allowed
    differences, each listed in the test by name:
    - `upstream` / `upstream_id`, only on routes of a replaced component, only to
      `jc.<ns>.svc.cluster.local:8080`;
    - the `uri-blocker` plugin with exactly the rows above, only on those routes, and only where
      the route had no `uri-blocker`;
    - removed route ids: exactly `jc` and `jc-identity` (from `jc-ship`);
    - added route ids: none.

    Any other changed key, plugin, host, uri, priority or vars is red, with the route id and the
    JSON path.
  - `test_apisix_routes.py`, `test_apisix_edge.py`, `test_edge_attack_surface.py` (`ROUTE_CLASSES`),
    `test_networkpolicies.py` (APISIX to jc on 8080 and 4143 only),
    `test_postgres_connection_budget.py`.
  - Red proof: add a stray `proxy-rewrite` header to one re-pointed route and see the diff test
    name it.
- **Security.**
  - jc accepts ingress from APISIX only.
  - `/streams` and `/broker` stay closed.
  - No route opens that is closed today (the diff test).
  - `JC_TRUST_EDGE_TOKEN` is set only where every route to jc clears the header. The diff test
    keeps each route's pre-function.
- **Measures.** `kubectl top pod jc` before and after the cutover step, beside the replaced pods.

### S5: the rehearsal behind the edge, and the suites on hosts (monorepo `deploy/rehearsal`, `bins/jc/smoke`)

- **Change.**
  - `deploy/rehearsal/up.sh` starts APISIX standalone (pinned digest) from the S4b render with
    rehearsal values. The domain is `rh-<env>.localhost`, plain http, every upstream
    `127.0.0.1:<jc port>`, and dev's header snippet stays. `--add-host`/`/etc/hosts` maps `idm.`
    and `portal.` to 127.0.0.1, which section 5, point 8 proves works.
  - The suites take `--edge`, `--domain` and `--internal-url` instead of `--base-url`, keep a
    cookie jar per host, and follow the edge's code flow, as `harness/drive.py` does.
  - Paths lose the slug prefixes.
- **Done when.**
  - charts-apps through the edge shows 0 panel failures on both rehearsals, "City bikes,
    members" included.
  - The ui suite counts no worse than `2026-11-09T022124Z`.
- **Tests.**
  - **A members-only panel renders through the edge.** A member gets frames; a non-member gets
    the gateway's refusal.
  - **Parallel renewal.** Access token lifespan 60 s on the rehearsal realm. After expiry, the
    browser fires 10 parallel Portal API calls. All 10 answer as the person, the next page needs
    no password, and a second expiry cycle does the same. Needs S3b; without it this test is the
    red proof (1 of 10, logged out, section 6.1).
  - **Cookie size.** A member of 250 groups signs in. Every Set-Cookie is at most 4096 bytes,
    the panel renders, and X-Userinfo reaches the Portal. A member of 300 groups is answered with
    the edge's 400 page, not a Portal error.
  - **Forged headers stripped.** With no session, `X-Access-Token` / `X_Access_Token` carrying
    another person's valid token and a forged `X-Userinfo` reach the Portal as nothing: 401 or the
    sign-in redirect, never that person. With a session, `/api/v1/auth/me` names the session's
    subject, not the forged one. A forged token sent to jc's loopback listener is 401.
  - **Realm A never admits in realm B**, through the edge (as the first design).
  - **Logout** ends the edge and the kaisatsu session.
- **Security.** Loopback only; secrets never in logs; the person password stays in memory.
- **Measures.** Wall time of the parallel-renewal test and the cookie sizes per group count, in
  the task.

### S6 (with the appliance task): one container, same configuration

- **Change.**
  - The image build renders `apisix.yaml` and `jc-realm.json` (S3c) from one pinned deployment
    commit with appliance values. Every replaced upstream is `127.0.0.1:8080`, and the domain
    comes from the installation.
  - The entrypoint:
    - generates, once, the edge client secret, the session secret and the other client secrets as
      0600 files in the state volume, where both the import and APISIX read them;
    - writes `/etc/hosts` lines for `idm.{domain}` and `portal.{domain}` to 127.0.0.1;
    - starts jc on 127.0.0.1, then APISIX with `worker_processes: 2` and dev's header snippet.
  - The Portal's edge-file wave writes the composed file to APISIX's config path instead of the
    Kubernetes Secret when no in-cluster client exists. Today it is off there
    (`crates/portal/src/state.rs:625-655`), so the appliance serves no App host until this lands.
- **Done when.**
  - S5's edge tests pass against the container.
  - `docker stats` peak with every module answering stays under 2 GB (APISIX + jc + shako),
    measured as `components/jc/README.md` section 4 does.
- **Tests.**
  - The point 8 red/green run inside the container image: without the hosts line discovery fails,
    with it the code flow completes.
  - Start order: APISIX started before the hosts line is written fails discovery. The entrypoint
    test asserts the order.
- **Security.** jc is reachable only through APISIX. The container runs non-root, without
  `docker.sock`, with memory limits.
- **Measures.** The memory peak above, and APISIX RSS with 2 workers.

## Related

- `bins/jc/CUTOVER.md` sections 3-4 (steps; the mount table S4a extends).
- `bins/jc/STATUS.md`, "Gaps left (stage 8)" and "The deployment's realm".
- `/workspace/evidence/jc-realm-name/REPORT.md` (S2).
- `/workspace/evidence/jc-edge-design-2/` (harness, results, this file as DESIGN.md).
- deployment `components/apisix/README.md` "Edge login", `components/portal/apisix-plugins.yaml`,
  `components/apisix/keycloak-clients.yaml`.
- KNOWLEDGE T-5528, T-6549 (cookie size), T-0252 (RS256 edge client), T-5568 (back-channel logout).
