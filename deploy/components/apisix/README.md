# APISIX

Edge API gateway in **standalone file mode** (docs ADR-N-007, Deployment/10): no etcd,
no Admin API. Routes, upstreams and plugin configs are rendered by the `configuration`
chart into the ConfigMap `apisix-standalone-base` (`apisix.yaml`, ending with `#END`). The
Portal's reconciler adds every published App's routes and client secret to that base and writes
the Secret `apisix-standalone-config`, which APISIX mounts and reloads on change (ADR-N-030,
AP-112). The chart seeds that Secret with the base until the Portal first writes it and marks it
`joinedcontext.com/composed-by: portal`. The shared secrets are injected as environment
variables and referenced as `${{VAR}}` inside the rule file.

Parts: `configuration` (base ConfigMap, the seeded Secret, the Portal's `edge-file-composer`
Role, Linkerd `Server`), then `apisix` (upstream Helm chart, `deployment.mode: standalone`,
its rule-file volume patched from a ConfigMap to the Secret in `component.yaml`).

Contributing routes: add `apisix-routes.yaml` and `apisix-plugins.yaml` to a component.

```bash
helmfile apply -i --selector component=apisix
```

## Plugin chain

Every route runs the same ordered chain (docs Deployment/10 §4). APISIX orders plugins by
their own priority, not by the order they appear in `apisix-plugins.yaml`:

1. `proxy-buffering` — off on the context-gateway surfaces, so SSE and large GeoJSON/CSV
   exports stream instead of filling an nginx buffer for five minutes.
2. `request-id` — UUIDv4, echoed to the client.
3. `serverless-pre-function` — drops every header a client could use to forge tenancy or
   authorization (`NGSILD-Tenant`, `X-Userinfo`, `X-Access-Token`, `X-Allowed-Scope-Ids`,
   `X-Endpoint-Slug`, `X-Consumer-Identity`, `JC-Read-Versions`), every header of the families
   the platform's services write on the internal hop (`x-jc-*`, `jc-provenance-*`, `l5d-*`,
   matched by prefix over all headers, T-5566) and the proxy hints APISIX sets itself
   (`X-Forwarded-Host/Proto/Port/Prefix/Server`, `X-Real-IP`). `X-Forwarded-For` stays:
   nginx maintains it, and it is the client-IP audit trail and the UI rate-limit key.
4. `cors` — the endpoint surface answers browsers from `https://*.<domain>` only.
5. `proxy-rewrite` — puts the trusted `X-Forwarded-Proto`/`X-Forwarded-Port` back.
6. `limit-count` / `limit-conn` — the traffic classes below.
7. `openid-connect` — the edge login on the Portal routes and the apps surface (below).
8. `response-rewrite` — HSTS, `nosniff`, `Referrer-Policy`, `X-Frame-Options`
   (`SAMEORIGIN` for the UIs, `DENY` for the APIs) and `no-store` on everything
   authenticated.
9. `serverless-post-function` — a baseline `Content-Security-Policy` (`frame-ancestors` as
   `X-Frame-Options` says, `object-src 'none'`, `base-uri 'self'`) on an answer that carries
   none (OPS-34). The Portal and each App send their own policy, which the edge never
   replaces; it runs in `header_filter` after `response-rewrite`, so APISIX's own refusals
   get it too.

## Edge login

The `openid-connect` plugin in session mode is the one login front for everything a person
opens in a browser (docs ADR-N-019, AP-26…AP-29): `portal-ui` and `portal-api` on
`portal.{domain}`, `apps-surface` (`/apps/*`) on the apex, and every `app-{name}` route jcctl
renders. One confidential Keycloak client, `edge` (`keycloak-clients.yaml` here), whose
secret reaches the gateway as `${{EDGE_CLIENT_SECRET}}` from the Secret `keycloak-client-edge`
the way the session key does from `apisix-oidc-session`; both names are declared in
`extraEnvVars` and `apisix.nginx.envs`. The plugin sets `X-Userinfo` and `X-Access-Token` for
the upstream after the pre-function has cleared them from the client request. A route with
`unauth_action: auth` sends a visitor without a session to Keycloak; `portal-api` uses `pass`,
so a bearer caller reaches the Portal untouched and the Portal verifies the token itself.

Two facts of T-0252 still shape the configuration. The realm signs ES256 and lua-resty-openidc
verifies RS/HS only, so the `edge` client is signed RS256 by per-client override (the realm
keeps an active RS256 key at a lower priority; see the Keycloak component), and the plugin
runs without `use_jwks`, `public_key` or `introspection_endpoint`: with any of them it would
verify a presented bearer, refuse every ES256 token with 401 before `unauth_action` is
consulted, and no CLI could call the Portal. The Context Gateway surfaces carry no plugin and
verify bearer tokens themselves (docs Deployment/10 §4, OPS-33).

The plugin dials the public issuer host (`idm.{domain}`) for discovery, tokens, the JWKS and
userinfo: the same hairpin through the ingress controller the Portal makes, so the pod takes
443 out of the mesh's outbound redirect and the NetworkPolicy allows 443/8443 out.

The apex answers `/` (and any path no apex route serves) with a 302 to the Portal host
(`portal-redirect`, the lowest-priority apex route); `/apps/*`, `/git/*`, `/cs/*` and
`/api/endpoint/*` stay on the apex.

| Class | Route | Budget | Key |
|---|---|---|---|
| 1 standard web | `portal-ui`, `apps-surface`, `keycloak` | 300/min, 1200/min | client IP |
| 2 authenticated API | `portal-api`, `context-space` | 1200/min | bearer token |
| 3 high-throughput streams | `context-endpoint` | 5000/min | bearer token + IP |
| 4 bulk exports | `context-endpoint` | 10 concurrent | bearer token + IP |

The token string is the only client identity APISIX has, so a bucket is per token rather
than per subject and resets when a client renews. Keying by `sub` needs an APISIX consumer or a claim-extracting pre-function.
`policy: local` counts per gateway pod; with more than one replica the effective budget is
the class value times the replica count, and a shared counter needs Redis.

## Edge TLS

`components/apisix/charts/configuration/templates/edge-tls.yaml` renders one cert-manager
`Certificate` (`apisix-edge` → Secret `apisix-edge-tls`) whose SAN list is the apex domain plus
every routed subdomain, derived from the same route map the rule file is built from. The
Ingress carries **no** `cert-manager.io/cluster-issuer` annotation on purpose: the
ingress-shim would issue a second certificate for the same Secret, and two ACME orders for
one name is how you meet a rate limit. The Secret is `apisix-edge-tls` rather than the
`apisix-tls` the shim used to fill, because two Certificates may not share one Secret: an
instance upgraded from the annotation blocks its own apply until the old Certificate is
deleted, while a new Secret name lets the same run replace both. The leftover `apisix-tls`
Secret is unreferenced afterwards and can be deleted at any time.

The listener policy (BSI TR-02102, TLS 1.2/1.3, forward-secret AEAD suites only) follows the
ingress class. Traefik gets a `TLSOption` referenced as
`<namespace>-bsi-tr-02102@kubernetescrd`; ingress-nginx gets the `ssl-ciphers` and
`ssl-prefer-server-ciphers` annotations — its protocol floor is a controller-wide setting
(`ssl-protocols`, TLS 1.2+1.3 by default), not something one Ingress can pin.

### The App wildcard (ADR-N-037 §6)

`global.ingress.appsWildcard.enabled` puts every App host on one certificate
`*.apps.<domain>` (`apisix-apps-wildcard` → Secret `apisix-apps-wildcard-tls`), issued by DNS-01
through the namespaced Issuer `letsencrypt-dns01` and Hetzner's webhook (the `dns01` part, in
cert-manager's namespace). The edge Ingress gets a `*.apps.<domain>` rule and TLS entry, and the
Portal stops making a certificate and Ingress per App once the wildcard is `Ready`. Per-App
certificates made before stay until the wildcard is proven, then one change removes them.

The chart grants the webhook get/list/watch on every Secret of the cluster; `component.yaml`
empties that grant, and the Issuer reads the token from a mounted file (`tokenFilePath`).

Before turning it on, once per installation (Deployment/01 §2 has the full recipe):

1. A Hetzner project that holds the zone `apps.<domain>` and nothing else, since a Hetzner
   token is scoped to a project, not to a zone. In it, the zone with `*` `A` to the ingress
   address.
2. A Read & Write token of that project, as the Secret `hetzner-dns` (key `token`) in
   cert-manager's namespace. Supply it the way `components/secrets/README.md` §2 supplies any
   operator secret, never in values or Git.
3. `NS` records for `apps.<domain>` at the parent zone's registrar, naming the zone's Hetzner
   name servers. Step 1's `*` record has to exist first, or App hosts stop resolving.

Check it with `openssl s_client -connect <ingress>:443 -servername anything.apps.<domain>`: the
subject is `CN = *.apps.<domain>`, for a host no App was ever published on too.

## Error pages and maintenance (T-5119)

When the edge answers by itself (no route for the address 404, a plugin failure 500, an
upstream that is down or slow 502/503/504, a rate limit 429), a browser gets a page in its
language (Slovak for `sk` and `cs`, else English) and every other client RFC 9457 Problem
Details with a type from the platform's catalogue (API/00 §4). A 502/503/504 carries
`Retry-After: 30`. An answer an upstream wrote passes through unchanged. The pages and the
handlers live in the `httpEnd` and `httpSrv` snippets of `values/apisix/base-values.yaml.gotmpl`.

Planned window, for example the broker migration on dev:

1. In the environment's values, list the subdomains whose service is off:
   ```yaml
   apisix:
     apisix:
       maintenance:
         hosts: [context]        # subdomain names, for example portal, context, data
         retryAfterSeconds: 900  # 1..86400, what Retry-After tells clients
   ```
2. Apply apisix (`JUST_NO_DOTENV=true just sync-component <env> apisix`). Every request to those hosts gets a
   503 with `Retry-After`, a maintenance page or Problem Details, and never reaches the upstream.
3. After the window set `hosts: []` and apply again.

`tests/test_edge_error_pages.py` runs the pinned image with both snippets and checks each answer.
