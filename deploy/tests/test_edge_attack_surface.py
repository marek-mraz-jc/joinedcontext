"""The edge attack surface: what a request from the internet may set, reach and cost.

Five attack vectors, played against the edge configuration rather than against a cluster:

* **T-1672 (GW12, AG-38)** a header the edge should own arrives from outside;
* **T-1673 (OPS-31)** a route without the authentication its sibling has;
* **T-1674 (OPS-27)** TLS, HSTS and the security headers on every host;
* **T-1675 (GW26)** request smuggling and oversized requests;
* **T-1676 (OPS-31)** the admin surfaces are reachable from the internet.

The live half of each vector — actually sending the header, the 1 GB body, the probe for
the Admin API — is `joinedcontext-conformance/tests/security/test_edge_hardening.py`,
which runs against a throwaway environment and never against dev.

Why these walk `components/*/apisix-{routes,plugins}.yaml` and not only a render: a render
carries the components that one environment lists, and every edge test before this one read
the `local` render alone. CKAN is in `dev` and Grafana in `addons`, so neither has ever been
walked by an edge test — and `ckan-redirect` shipped with no header sanitisation and none of
the response headers its sibling `portal-redirect` carries. A route is checked here on the
day it is written, whether or not the one rendered environment switches its component on.
"""

import glob
import re
from pathlib import Path

import pytest
import yaml
from conftest import load_yaml_all

PROJECT_ROOT = Path(__file__).resolve().parent.parent

# Every header the platform trusts on its internal hop: a tenancy or authorization claim, or
# a proxy hint APISIX sets itself. A client that can set one of these chooses its own tenant
# or its own identity. `X-Forwarded-For` is deliberately absent — nginx maintains it and the
# rate limit keys on it (T-0026).
TRUSTED_HEADERS = (
    "NGSILD-Tenant",
    "X-Userinfo",
    "X-Access-Token",
    "X-Allowed-Scope-Ids",
    "X-Endpoint-Slug",
    "X-Consumer-Identity",
    "X-Forwarded-User",
    "X-Forwarded-Host",
    "X-Forwarded-Proto",
    "X-Forwarded-Port",
    "X-Forwarded-Prefix",
    "X-Forwarded-Server",
    "X-Real-IP",
    # The broker shows every version of an attribute when the gateway sets it (ADR-N-054 2.4).
    "JC-Read-Versions",
)

# Whole header families the platform's own services write on the internal hop, cleared by
# prefix because a list of names goes stale the day a service adds one (T-5566): the gateway's
# caller identity to the broker (`x-jc-subject`, `x-jc-groups`, `x-jc-client`, ...), the
# provenance it stamps (`jc-provenance-*`) and Linkerd's own (`l5d-client-id`, which the
# gateway's MQTT decider reads). The Lua compares the lower-cased, hyphenated name.
TRUSTED_PREFIXES = ("x-jc-", "jc-provenance-", "l5d-")
# The one name of those families a browser writes by design: the assistant widget's visitor
# token (API/05 §1.2.1, platform `chat::VISITOR_HEADER`), a bearer the chat route checks itself
# and no service reads as identity. Clearing it answers every widget question 401.
BROWSER_HEADERS = {"x-jc-visitor"}

# How a caller with no token is authenticated on a route. The class is what the reviewed
# allow-list below records, and every route carries the plugins its class names.
EDGE_LOGIN = "edge-login"  # openid-connect, unauth_action: auth — nothing anonymous is proxied
EDGE_SESSION = "edge-session"  # openid-connect, unauth_action: pass — the session becomes a bearer
UPSTREAM = "upstream"  # anonymous is proxied; the upstream is the one that authenticates
TERMINATES = "terminates"  # the edge answers by itself and never dials the upstream
UPSTREAM_DECIDES = "upstream"  # the edge leaves Cache-Control to the upstream, which always sets it

# The reviewed allow-list of T-1673: every route of every component, what authenticates a
# caller on it, and why that is allowed to be what it is. A route added to any component is
# red here until somebody writes its line, which is the point of the list.
#
# `cacheable` is the surface that serves its own static assets and may be cached by the
# browser; every other route answers `no-store` — except the gateway's, where the upstream
# decides (`UPSTREAM_DECIDES`): it says `private` on every answer itself, `no-store` or, on the
# schema artifacts, `no-cache` with an ETag so a client revalidates (T-2261, T-2262, EP-51). `framing` is the X-Frame-Options a browser
# gets: SAMEORIGIN for a surface that frames its own pages, DENY everywhere else, and none
# (`FRAMED_BY_UPSTREAM`) where the upstream names the sites that may frame it in its own
# `frame-ancestors`, which X-Frame-Options cannot express.
FRAMED_BY_UPSTREAM = None
ROUTE_CLASSES = {
    "portal-ui": {
        "reach": EDGE_LOGIN, "cacheable": True, "framing": "SAMEORIGIN",
        "why": "the Portal is the management application; a visitor logs in at the edge first",
    },
    "portal-public": {
        "reach": EDGE_SESSION, "cacheable": True, "framing": "SAMEORIGIN",
        "why": "the public open-data catalogue page (EP-81): a visitor reads it without an account, a signed-in person with the session; every read it makes is portal-api's",
    },
    "portal-public-assets": {
        "reach": EDGE_SESSION, "cacheable": True, "framing": "SAMEORIGIN",
        "why": "the Portal's static bundle, the same files for everyone and no data, which the public catalogue page loads before anyone signs in (EP-81)",
    },
    "portal-public-view": {
        "reach": EDGE_SESSION, "cacheable": True, "framing": "SAMEORIGIN",
        "why": "a data view published as a public link (T-3108): the page holds no data, every read it makes goes to the public Endpoint, where the gateway enforces what it shows",
    },
    "portal-public-form": {
        "reach": UPSTREAM, "cacheable": True, "framing": FRAMED_BY_UPSTREAM,
        "why": "a form view published as a public form (T-3103): the page holds no data, its one write is an anonymous create through the public Endpoint, which the gateway decides and rate-limits; the Portal names the sites that may frame it in the page's own frame-ancestors, from the Endpoint's embedOrigins (EP-101), and the page is anonymous for everyone, so no edge session is read (EP-102, T-3266)",
    },
    "portal-public-docs": {
        "reach": EDGE_SESSION, "cacheable": True, "framing": "SAMEORIGIN",
        "why": "a public Endpoint's API documentation (T-3265): the page holds no data, it reads the OpenAPI document the gateway generates under the caller's rights",
    },
    "portal-public-connect": {
        "reach": EDGE_SESSION, "cacheable": True, "framing": "SAMEORIGIN",
        "why": "a public named MCP server's setup page (T-4169): the same bundle for everyone, holding no data; it asks the server's own address for its tools without credentials",
    },
    "portal-home": {
        "reach": EDGE_SESSION, "cacheable": True, "framing": "SAMEORIGIN",
        "why": "the front door `/` (T-5627, EP-81): the Portal sends a visitor without a session to the public catalogue with `no-store` and serves a signed-in person the home page; no data is served on the path itself",
    },
    "portal-public-glossary": {
        "reach": EDGE_SESSION, "cacheable": True, "framing": "SAMEORIGIN",
        "why": "the glossary (T-3236, T-5293): the same words for everyone, holding no data; every word's hint links here, the catalogue's included",
    },
    "portal-public-accessibility": {
        "reach": EDGE_SESSION, "cacheable": True, "framing": "SAMEORIGIN",
        "why": "the accessibility statement (T-5108, Decree 78/2020): a public body links it from every page, a visitor included; the same text for everyone, holding no data",
    },
    "portal-api": {
        "reach": EDGE_SESSION, "cacheable": False, "framing": "DENY",
        "why": "CLIs and service accounts present a bearer the Portal verifies (OPS-33)",
    },
    "portal-well-known": {
        "reach": UPSTREAM, "cacheable": False, "framing": "DENY",
        "why": "RFC 9728 protected-resource metadata is public by the RFC; it names no secret",
    },
    "portal-connector": {
        "reach": UPSTREAM, "cacheable": False, "framing": "DENY",
        "why": "a personal connector's MCP server (API/14 §3): an MCP client holds only a bearer, so the Portal answers 401 with WWW-Authenticate itself, verifies the token's audience and connector scope, the owner and the client on every call, and limits per connector and person",
    },
    "assistant-chat": {
        "reach": UPSTREAM, "cacheable": False, "framing": "DENY",
        "why": "the knowledge assistant's public deployments answer anyone on their allowed origins; jc-assistant checks the Origin, limits per deployment and client, and caps the budget (API/05, AG-100, AG-101)",
    },
    "assistant-widget": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": FRAMED_BY_UPSTREAM,
        "why": "the widget page a public deployment's own sites frame (AG-114): no data and no token in it; jc-assistant sets frame-ancestors to the deployment's allowedOrigins, no-store on the page and a short max-age on its script and style",
    },
    "assistant-unknown": {
        "reach": TERMINATES, "cacheable": False, "framing": "DENY",
        "why": "any other path on the assistant host (T-4200): the edge answers a 404 page in words that names no deployment, and never dials jc-assistant",
    },
    "portal-live-notify": {
        "reach": UPSTREAM, "cacheable": False, "framing": "DENY",
        "why": "the gateway delivers the Portal's own live-update notifications; the broker holds no token, the path's key is an HMAC only the Portal makes, and a notification only makes a view read again (API/01 §32)",
    },
    "portal-metrics": {
        "reach": TERMINATES, "cacheable": False, "framing": "DENY",
        "why": "the scrape path answers 404 at the edge; the ServiceMonitor reaches the pod (OPS-16)",
    },
    "portal-redirect": {
        "reach": TERMINATES, "cacheable": False, "framing": "DENY",
        "why": "the apex answers 302 to the Portal host and never dials an upstream",
    },
    "context-space": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "the gateway is the enforcement point: anonymous sees what the space's Policy grants",
    },
    "context-space-read": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "context-space's reads with its chain, counted apart so the Organization sets reads and writes apart (ADR-N-035)",
    },
    "context-space-portal-read": {
        "reach": EDGE_SESSION, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "context-space-portal's reads with its chain, counted apart so the Organization sets reads and writes apart (ADR-N-035)",
    },
    "catalog-feed": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "the DCAT-AP feeds a national portal harvests, the organization's and each publisher's (EP-84, EP-127): the gateway reads no token on them and lists public Endpoints only",
    },
    "context-endpoint": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "an Endpoint publishes what its Policy grants, to anonymous callers by design (EP-01)",
    },
    "context-endpoint-ws": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "context-endpoint's WebSockets (T-4873): the same chain, 50 open per client address",
    },
    "context-endpoint-bulk": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "context-endpoint's bulk exports (T-4717): the same chain, 10 at once per client",
    },
    "context-mcp-hub": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "the MCP hub (EP-87): the gateway verifies the bearer and decides each call as the named Endpoint's own",
    },
    "context-mcp-hub-portal": {
        "reach": EDGE_SESSION, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "the MCP hub on the Portal's origin, where the Endpoints view shows it; the edge session becomes the bearer and the gateway still decides",
    },
    "context-mcp-named": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "the public named MCP servers for a caller without a bearer (T-4141): the hub's chain, 60 calls a minute per client IP; the gateway decides each call as the member Endpoint's own",
    },
    "context-mcp-named-portal": {
        "reach": EDGE_SESSION, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "the same on the Portal's origin, so the anonymous limit is not one hostname away",
    },
    "context-space-portal": {
        "reach": EDGE_SESSION, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "the space surface on the Portal's origin, where the UI reads it from; the edge session becomes the bearer and the gateway's PEP still decides",
    },
    "context-endpoint-portal": {
        "reach": EDGE_SESSION, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "the same surface on the Portal's origin, with the edge session turned into a bearer",
    },
    "context-endpoint-portal-ws": {
        "reach": EDGE_SESSION, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "context-endpoint-portal's WebSockets (T-4873): the same chain, 50 open per client address",
    },
    "context-endpoint-portal-bulk": {
        "reach": EDGE_SESSION, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "context-endpoint-portal's bulk exports (T-4717): the same chain, 10 at once per client",
    },
    "gitea-forge": {
        "reach": UPSTREAM, "cacheable": False, "framing": "SAMEORIGIN",
        "why": "the forge authenticates with its own session and Git credentials",
    },
    "gitea-registry": {
        "reach": UPSTREAM, "cacheable": False, "framing": "DENY",
        "why": "a node pulls a fullstack App's image with the read:package token the registry checks itself (AP-108); no page is served",
    },
    "gitea-registry-token": {
        "reach": UPSTREAM, "cacheable": False, "framing": "DENY",
        "why": "containerd's OAuth POST to the token realm, which the forge answers 404 so the node falls back to GET (T-2665)",
    },
    "keycloak": {
        "reach": UPSTREAM, "cacheable": False, "framing": "SAMEORIGIN",
        "why": "the realm is the identity provider; its login pages must be reachable unauthenticated",
    },
    "keycloak-discovery": {
        "reach": UPSTREAM, "cacheable": False, "framing": "SAMEORIGIN",
        "why": "the realm's public discovery documents, read before any login; the edge drops registration_endpoint (T-5611)",
    },
    "grafana": {
        "reach": UPSTREAM, "cacheable": True, "framing": "SAMEORIGIN",
        "why": "Grafana signs everyone in through the realm; anonymous access and the login form are off",
    },
    "argocd": {
        "reach": UPSTREAM, "cacheable": False, "framing": "DENY",
        "why": "Argo CD verifies every token itself; login is the realm's PKCE flow, anonymous access and the local admin are off (ADR-N-056 §2.8)",
    },
    "ckan": {
        "reach": UPSTREAM, "cacheable": True, "framing": "SAMEORIGIN",
        "why": "an open-data catalogue is public reading; writing is CKAN's own session",
    },
    "decidim": {
        "reach": UPSTREAM, "cacheable": True, "framing": "SAMEORIGIN",
        "why": "participation is public reading; Decidim signs people in through the realm itself, and the edge refuses its system console, local password reset, invitations and sign-up (ADR-N-065 §3, T-6316)",
    },
    "jc": {
        "reach": UPSTREAM, "cacheable": True, "framing": "SAMEORIGIN",
        "why": "jc in one binary on jc.{domain}: its Portal, identity (kaisatsu), catalogue and dashboards authenticate every caller themselves and serve their own bundles; the edge refuses the internal mounts /streams, /broker and /git with 404",
    },
    "jc-identity": {
        "reach": UPSTREAM, "cacheable": True, "framing": "SAMEORIGIN",
        "why": "kaisatsu, jc's identity provider, under /id: discovery, sign-in and token endpoints are public by OIDC and authenticate themselves; the route reaches a starting pod so the Portal of that pod can read the realm",
    },
    "ckan-webassets": {
        "reach": UPSTREAM, "cacheable": True, "framing": "SAMEORIGIN",
        "why": "CKAN's fingerprinted stylesheets and scripts, the same files for everyone, kept a year by the browser (T-4201)",
    },
    "ckan-action-cache": {
        "reach": UPSTREAM, "cacheable": True, "framing": "SAMEORIGIN",
        "why": "CKAN's anonymous action API reads kept 60 s at the edge; a cookie or a token bypasses it (T-4717)",
    },
    "ckan-map": {
        "reach": UPSTREAM, "cacheable": True, "framing": FRAMED_BY_UPSTREAM,
        "why": "a dataset's map any site may frame (T-4639): the page holds no data, no form and no session; the visitor's browser reads the Endpoint's public OGC API Features anonymously, and CKAN's page names frame-ancestors * itself while the edge strips the session cookie",
    },
    "basemap": {
        "reach": UPSTREAM, "cacheable": True, "framing": "DENY",
        "why": "the background map's tiles, style, fonts and sprites on tiles.{domain} (T-4638): public map data, read-only nginx answering GET/HEAD/preflight, the same files for everyone, its own larger bucket",
    },
    "status-page": {
        "reach": UPSTREAM, "cacheable": True, "framing": "DENY",
        "why": "the public status page on status.{domain} (T-5102): surface states, 90-day uptime and incident notes, the same page for everyone, GET/HEAD only, no form, no cookie, no host or error body",
    },
    "security-txt": {
        "reach": TERMINATES, "cacheable": False, "framing": "DENY",
        "why": "RFC 9116 security.txt is public by the RFC; the edge answers it and names only the reporting contact (T-1721)",
    },
    "api-context-space": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "context-space on the public data host api.{domain} (T-4630, ADR-N-052 §2.1): the same chain, the gateway enforces the space's Policy",
    },
    "api-context-space-read": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "context-space-read on the data host (T-4630): its reads counted apart, as on the apex",
    },
    "api-catalog-feed": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "catalog-feed on the data host (T-4630): the DCAT-AP feeds, public Endpoints only",
    },
    "api-context-endpoint": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "context-endpoint on the data host (T-4630): an Endpoint publishes what its Policy grants",
    },
    "api-context-endpoint-ws": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "api-context-endpoint's WebSockets (T-4873): the same chain, 50 open per client address",
    },
    "api-context-endpoint-bulk": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "api-context-endpoint's bulk exports (T-4717): the same chain, 10 at once per client",
    },
    "api-context-endpoint-mcp": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "api-context-endpoint's MCP (T-6185): the same chain unbuffered for SSE, never in the edge cache",
    },
    "api-context-endpoint-schema": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "api-context-endpoint's schema artifacts (T-6507): the same chain, off proxy_cache so a read with its ETag reaches the gateway's 304",
    },
    "api-context-endpoint-history": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "api-context-endpoint's temporal history (T-6185): the bulk route's 10 at once, anonymous answers kept at the edge",
    },
    "api-mcp-hub": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "context-mcp-hub on the data host (T-4630): the gateway decides each call as the named Endpoint's own",
    },
    "api-mcp-named": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "context-mcp-named on the data host (T-4630): anonymous callers, 60 calls a minute per client IP",
    },
    "api-oauth": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "the gateway's stateless anonymous OAuth server (T-4630, platform T-4631, ADR-N-052 §2.2): nothing stored, 30 a minute per client IP, redirects only to Google's relay",
    },
    "api-oauth-metadata": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "the RFC 8414 and RFC 9728 documents of the anonymous OAuth server (T-4630): public by the RFCs",
    },
    "api-moved-endpoint": {
        "reach": TERMINATES, "cacheable": False, "framing": "DENY",
        "why": "an old address of a public data surface, anonymous: 308 to the same path on api.{domain} (T-4630)",
    },
    "api-moved-portal-endpoint": {
        "reach": TERMINATES, "cacheable": False, "framing": "DENY",
        "why": "an old address of a public data surface, anonymous: 308 to the same path on api.{domain} (T-4630)",
    },
    "api-moved-mcp": {
        "reach": TERMINATES, "cacheable": False, "framing": "DENY",
        "why": "an old address of a public data surface, anonymous: 308 to the same path on api.{domain} (T-4630)",
    },
    "api-moved-portal-mcp": {
        "reach": TERMINATES, "cacheable": False, "framing": "DENY",
        "why": "an old address of a public data surface, anonymous: 308 to the same path on api.{domain} (T-4630)",
    },
    "api-moved-mcp-named": {
        "reach": TERMINATES, "cacheable": False, "framing": "DENY",
        "why": "an old address of a public data surface, anonymous: 308 to the same path on api.{domain} (T-4630)",
    },
    "api-moved-portal-mcp-named": {
        "reach": TERMINATES, "cacheable": False, "framing": "DENY",
        "why": "an old address of a public data surface, anonymous: 308 to the same path on api.{domain} (T-4630)",
    },
    "api-moved-cs": {
        "reach": TERMINATES, "cacheable": False, "framing": "DENY",
        "why": "an old address of a public data surface, anonymous: 308 to the same path on api.{domain} (T-4630)",
    },
    "api-moved-portal-cs": {
        "reach": TERMINATES, "cacheable": False, "framing": "DENY",
        "why": "an old address of a public data surface, anonymous: 308 to the same path on api.{domain} (T-4630)",
    },
    "api-moved-catalog": {
        "reach": TERMINATES, "cacheable": False, "framing": "DENY",
        "why": "an old address of a public data surface, anonymous: 308 to the same path on api.{domain} (T-4630)",
    },
    "ckan-old-host": {
        "reach": TERMINATES, "cacheable": False, "framing": "DENY",
        "why": "data.{domain}, the catalogue's old host, answers 301 to opendata.{domain} with the same path and never dials an upstream (T-4679)",
    },
    "ckan-redirect": {
        "reach": TERMINATES, "cacheable": False, "framing": "DENY",
        "why": "/ckan on the apex answers 302 to the catalogue host and never dials an upstream",
    },
    "basemap": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "the background map's tiles, style, fonts and sprites (T-4638): public map data under the OpenStreetMap licence, read-only, and nginx keeps them a day",
    },
    "status-page": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": "DENY",
        "why": "the public status page (T-5102): states of the public surfaces, public by design; the page sets public, max-age=30 itself",
    },
    "ai": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": FRAMED_BY_UPSTREAM,
        "why": "the standalone assistant (T-4694): static pages with no data, no token and no cookie; a launcher on a city's site frames /{publicId}, app.js shows it only inside the deployment's origins and the chat answers those alone",
    },
    "viz": {
        "reach": UPSTREAM, "cacheable": UPSTREAM_DECIDES, "framing": FRAMED_BY_UPSTREAM,
        "why": "the chart renderer (T-4641, ADR-N-053): static pages with no data, no token and no cookie; nginx sets each page's frame-ancestors and Cache-Control, any site may frame the host page and the frame that runs a spec connects nowhere",
    },
}

# Plugin configs no route of the base names: the Portal copies each onto the routes it composes
# (ADR-N-037), so they carry the headers and the limit of the class those routes get.
TEMPLATE_CLASSES = {
    "api-moved": {
        "reach": TERMINATES, "cacheable": False, "framing": "DENY",
        "why": "the shared plugin config of the api-moved-* routes: the 308 from the apex and the Portal host to the data host for an anonymous caller (T-4630, ADR-N-052 §2.1.3): answers in rewrite, never dials the gateway",
    },
    "apps-surface": {
        "reach": EDGE_LOGIN, "cacheable": True, "framing": "SAMEORIGIN",
        "why": "every App's own host, whose pages ask for a login before they show anything (AP-133)",
    },
    "jc-apps": {
        "reach": UPSTREAM, "cacheable": True, "framing": FRAMED_BY_UPSTREAM,
        "why": "the plugin config of the jc-apps route that defaults/environment/apisix-routes.yaml.gotmpl adds in jc host mode: every App host {name}.apps.{domain} to jc in host mode: jc's Portal serves App {name} alone there, signs its visitor in with the App's own client and hands the App's own endpoint slugs to the gateway (bins/jc edge-session-design S4a); each App page names the Portal in its own frame-ancestors (AP-122), so the edge sets no X-Frame-Options",
    },
}

# The workloads that answer inside the cluster only. None of them may appear as the upstream
# of a route, and none of them may be published by a Service the node forwards (T-1676).
INTERNAL_WORKLOADS = (
    "context-broker",
    "pipeline-runner",
    "artifact-store",
    "model-tools",
    "observability-collector",
)


def _component_documents(name: str) -> dict[str, tuple[str, dict]]:
    """Every `components/*/apisix-<name>.yaml` entry, by id, with the file it came from.

    The files are Go templates, but every directive of theirs sits inside a scalar, so YAML
    parses them as written and a test reads what a component ships rather than what one
    environment happens to render.
    """
    found: dict[str, tuple[str, dict]] = {}
    for path in sorted(glob.glob(str(PROJECT_ROOT / "components/*" / f"apisix-{name}.yaml"))):
        relative = str(Path(path).relative_to(PROJECT_ROOT))
        for document in load_yaml_all(Path(path).read_text()):
            for entry_id, body in (document or {}).items():
                assert entry_id not in found, f"{entry_id} is defined twice: {found[entry_id][0]} and {relative}"
                found[entry_id] = (relative, body)
    assert found, "no component ships an apisix-%s.yaml; the walk found nothing to check" % name
    return found


@pytest.fixture(scope="module")
def component_routes() -> dict[str, tuple[str, dict]]:
    return _component_documents("routes")


@pytest.fixture(scope="module")
def component_plugins() -> dict[str, tuple[str, dict]]:
    return _component_documents("plugins")


def sanitiser(plugins: dict) -> str:
    """The Lua the route clears request headers with, as one string."""
    pre = plugins.get("serverless-pre-function")
    assert pre, "the route has no serverless-pre-function"
    assert pre["phase"] == "rewrite", f"the sanitiser runs in {pre['phase']}, after the proxy decision"
    return "\n".join(pre["functions"])


@pytest.fixture(scope="module")
def apisix_config(rendered):
    """The config.yaml APISIX boots with, from the rendered ConfigMap."""
    cm = next(
        d for d in rendered("local")
        if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "apisix"
    )
    return yaml.safe_load(cm["data"]["config.yaml"])


# --- T-1672: a header the edge should own arrives from outside (GW12, AG-38) ---------------


def test_every_route_of_every_component_clears_every_header_the_platform_trusts(component_plugins):
    """GW12, AG-38, OPS-32 — a client that can set one of these chooses its own tenant or identity.

    Walks the component sources, so a route in an addon nobody renders is checked too.
    """
    for route_id, (source, config) in component_plugins.items():
        lua = sanitiser(config["plugins"])
        missing = [header for header in TRUSTED_HEADERS if f'"{header}"' not in lua]
        assert not missing, f"{route_id} ({source}) does not clear {missing}"
        assert "ngx.req.clear_header" in lua, f"{route_id} ({source}) names the headers and clears none"


# The clearing loop itself, inside whatever else a route's pre-function does: the endpoint
# routes guard the egress path in the same function (R46), so the Lua as a whole differs and
# this part of it may not.
CLEARING_LOOP = re.compile(
    r"local forged = \{.*?for _, header in ipairs\(forged\) do.*?\n\s*end",
    re.DOTALL,
)


def clearing_loop(lua: str) -> str:
    """The clearing loop of a route's pre-function, indentation-independent."""
    found = CLEARING_LOOP.search(lua)
    assert found, f"no clearing loop in:\n{lua}"
    return "\n".join(line.strip() for line in found.group(0).splitlines())


def test_the_clearing_loop_is_the_same_lua_on_every_route(component_plugins):
    """One route drifting from the others is how a header gets through, and drift is invisible
    in review: the Lua is copied into every component file by hand. `ckan-redirect` had no
    pre-function at all until T-1672, and no test saw it because CKAN is in `dev` while every
    edge test reads the `local` render."""
    by_lua: dict[str, list[str]] = {}
    for route_id, (source, config) in component_plugins.items():
        by_lua.setdefault(clearing_loop(sanitiser(config["plugins"])), []).append(f"{route_id} ({source})")
    assert len(by_lua) == 1, "the clearing loop differs between routes:\n" + "\n\n".join(
        "--- " + ", ".join(routes) + "\n" + lua for lua, routes in by_lua.items()
    )


PREFIX_LOOP = re.compile(
    r"for name in pairs\(ngx\.req\.get_headers\(0\)\) do.*?\n\s*end\n\s*end\n\s*end",
    re.DOTALL,
)


def test_every_route_clears_the_platform_identity_prefixes(component_plugins):
    """GW12, AG-38, GW25 (T-5566): an `x-jc-*`, `jc-provenance-*` or `l5d-*` header from the
    internet never reaches a service; until now only the gateway stripped them, so one handler
    that forgot to would have trusted a forged caller. `get_headers(0)` reads every header: the
    default stops at 100, and the 101st would pass uncleared."""
    loops: dict[str, list[str]] = {}
    for route_id, (source, config) in component_plugins.items():
        lua = sanitiser(config["plugins"])
        found = PREFIX_LOOP.search(lua)
        assert found, f"{route_id} ({source}) does not clear the {TRUSTED_PREFIXES} header families"
        loop = "\n".join(line.strip() for line in found.group(0).splitlines())
        for prefix in TRUSTED_PREFIXES:
            assert f'"{prefix}"' in loop, f"{route_id} ({source}) does not clear {prefix}*"
        assert 'name:lower():gsub("_", "-")' in loop, f"{route_id} ({source}) misses X_JC_Subject"
        assert "ngx.req.clear_header(name)" in loop, f"{route_id} ({source}) names the prefixes and clears none"
        exempt = set(re.findall(r'key ~= "([^"]+)"', loop))
        assert exempt <= BROWSER_HEADERS, f"{route_id} ({source}) lets {exempt - BROWSER_HEADERS} through"
        loops.setdefault(loop, []).append(f"{route_id} ({source})")
    assert len(loops) == 1, "the prefix loop differs between routes:\n" + "\n\n".join(
        "--- " + ", ".join(routes) + "\n" + lua for lua, routes in loops.items()
    )


@pytest.mark.xdist_group("docker-apisix-sanitiser")
@pytest.mark.skipif(not __import__("shutil").which("docker"), reason="docker not installed")
@pytest.mark.parametrize("route_id", ["portal-api", "assistant-chat"])
def test_the_pinned_image_clears_forged_identity_headers_before_the_upstream(component_plugins, tmp_path, route_id):
    """GW12, AG-38 (T-5566), played on `apache/apisix` as pinned with the Lua the route ships:
    the upstream, APISIX itself, echoes the headers it was sent. The assistant's chat route
    keeps the widget's `X-JC-Visitor`, or every widget question answers 401."""
    import json
    import subprocess
    import time
    import urllib.request

    from test_apisix_429_contract import pinned_image
    from test_edge_csp_baseline import free_port

    pre = component_plugins[route_id][1]["plugins"]["serverless-pre-function"]
    echo = 'return function() ngx.say(require("cjson").encode(ngx.req.get_headers(0))); ngx.exit(200) end'
    routes = [
        {"id": "echo", "uri": "/echo", "plugins": {
            "serverless-pre-function": {"phase": "rewrite", "functions": [echo]}}},
        {"id": "edge", "uri": "/edge", "plugins": {
            "proxy-rewrite": {"uri": "/echo"}, "serverless-pre-function": pre},
         "upstream": {"type": "roundrobin", "nodes": {"127.0.0.1:9080": 1}}},
    ]
    (tmp_path / "config.yaml").write_text(
        "deployment:\n  role: data_plane\n  role_data_plane:\n    config_provider: yaml\n"
        "nginx_config:\n  http:\n    underscores_in_headers: \"on\"\n"
    )
    (tmp_path / "apisix.yaml").write_text(yaml.safe_dump({"routes": routes}) + "#END\n")
    port = free_port()
    name = f"apisix-sanitiser-{port}"
    subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)
    started = subprocess.run(
        ["docker", "run", "-d", "--name", name, "-p", f"127.0.0.1:{port}:9080",
         "-v", f"{tmp_path / 'config.yaml'}:/usr/local/apisix/conf/config.yaml:ro",
         "-v", f"{tmp_path / 'apisix.yaml'}:/usr/local/apisix/conf/apisix.yaml:ro",
         pinned_image()],
        capture_output=True, text=True, check=False,
    )
    if started.returncode != 0:
        pytest.skip(f"the pinned image does not run here: {started.stderr.strip()}")
    forged = {
        "X-JC-Subject": "admin", "x-jc-groups": "/platform-admins", "X_JC_Client": "portal",
        "JC-Provenance-Actor": "someone-else", "L5d-Client-Id": "mqtt-a.mqtt.serviceaccount.identity",
        "X-Userinfo": "e30",
    }
    kept = {"X-Jcx-Note": "kept", "X-JC-Visitor": "visitor-token", "Traceparent": "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"}
    request = urllib.request.Request(f"http://127.0.0.1:{port}/edge", headers={**forged, **kept})
    try:
        deadline = time.time() + 60
        while True:
            try:
                with urllib.request.urlopen(request, timeout=5) as response:
                    seen = json.loads(response.read())
                break
            except OSError:
                if time.time() > deadline:
                    pytest.fail("APISIX did not start")
                time.sleep(1)
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)
    arrived = {key.lower().replace("_", "-") for key in seen}
    leaked = sorted(h for h in forged if h.lower().replace("_", "-") in arrived)
    assert not leaked, f"the upstream was sent {leaked}"
    for header, value in kept.items():
        assert seen.get(header.lower()) == value, f"{header} was cleared too: {seen}"


# --- F-12 (T-6662): an Authorization that is no bearer is no credential at the edge -----------

GATEWAY_CHAINS = ["context-space", "context-space-portal", "context-endpoint", "context-endpoint-portal"]


def test_the_gateway_chains_clear_an_authorization_that_is_no_bearer(component_plugins, component_routes):
    """F-12: the gateway reads `Basic` as anonymous, so the edge must too. Each gateway chain
    clears a non-bearer Authorization before limit-count and the cache read it, and every route
    for callers without a bearer matches any Authorization that is no bearer."""
    for chain in GATEWAY_CHAINS:
        lua = sanitiser(component_plugins[chain][1]["plugins"])
        assert 'ngx.req.clear_header("Authorization")' in lua, chain
        assert "^[Bb]earer\\s+\\S" in lua, chain
    no_bearer = ["http_authorization", "!", "~*", "^bearer\\s+\\S"]
    anonymous = [
        route_id for route_id, (_, route) in component_routes.items()
        if any(var[0] == "http_authorization" for var in route.get("vars", []))
    ]
    assert {"context-mcp-named", "api-mcp-named"} <= set(anonymous)
    for route_id in anonymous:
        vars_ = component_routes[route_id][1]["vars"]
        assert no_bearer in vars_, f"{route_id} matches {vars_}"


@pytest.mark.xdist_group("docker-apisix-sanitiser")
@pytest.mark.skipif(not __import__("shutil").which("docker"), reason="docker not installed")
def test_a_basic_header_meets_the_anonymous_limit_on_the_pinned_image(component_plugins, component_routes, tmp_path):
    """F-12 played on `apache/apisix` as pinned: 61 MCP calls from one address, each with a fresh
    `Authorization: Basic`, take the anonymous route and its 60 per minute, and the upstream sees
    no Authorization; a bearer still takes the hub route and arrives as sent."""
    import json
    import subprocess
    import time
    import urllib.error
    import urllib.request

    from test_apisix_429_contract import pinned_image
    from test_edge_csp_baseline import free_port

    pre = component_plugins["context-endpoint"][1]["plugins"]["serverless-pre-function"]
    named = component_routes["context-mcp-named"][1]
    echo = 'return function() ngx.say(require("cjson").encode(ngx.req.get_headers(0))); ngx.exit(200) end'
    # One dict per route: yaml.safe_dump writes a shared one as an alias, which APISIX drops.
    def upstream() -> dict:
        return {"type": "roundrobin", "nodes": {"127.0.0.1:9080": 1}}

    routes = [
        {"id": "echo", "uri": "/echo", "plugins": {
            "serverless-pre-function": {"phase": "rewrite", "functions": [echo]}}},
        {"id": "named", "uri": named["uri"], "priority": named["priority"], "vars": named["vars"],
         "plugins": {"serverless-pre-function": pre, "limit-count": named["plugins"]["limit-count"],
                     "proxy-rewrite": {"uri": "/echo"}},
         "upstream": upstream()},
        {"id": "hub", "uri": "/api/mcp*", "priority": 20,
         "plugins": {"serverless-pre-function": pre, "proxy-rewrite": {"uri": "/echo"}},
         "upstream": upstream()},
    ]
    (tmp_path / "config.yaml").write_text(
        "deployment:\n  role: data_plane\n  role_data_plane:\n    config_provider: yaml\n"
    )
    (tmp_path / "apisix.yaml").write_text(yaml.safe_dump({"routes": routes}) + "#END\n")
    port = free_port()
    name = f"apisix-basic-{port}"
    subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)
    started = subprocess.run(
        ["docker", "run", "-d", "--name", name, "-p", f"127.0.0.1:{port}:9080",
         "-v", f"{tmp_path / 'config.yaml'}:/usr/local/apisix/conf/config.yaml:ro",
         "-v", f"{tmp_path / 'apisix.yaml'}:/usr/local/apisix/conf/apisix.yaml:ro",
         pinned_image()],
        capture_output=True, text=True, check=False,
    )
    if started.returncode != 0:
        pytest.skip(f"the pinned image does not run here: {started.stderr.strip()}")

    def call(authorization: str) -> tuple[int, dict]:
        request = urllib.request.Request(
            f"http://127.0.0.1:{port}/api/mcp/p/s", method="POST", data=b"{}",
            headers={"Authorization": authorization, "Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(request, timeout=5) as response:
                return response.status, json.loads(response.read())
        except urllib.error.HTTPError as refused:
            return refused.code, {}

    try:
        deadline = time.time() + 60
        while True:
            try:
                status, seen = call("Bearer a.b.c")
                break
            except OSError:
                if time.time() > deadline:
                    pytest.fail("APISIX did not start")
                time.sleep(1)
        assert status == 200 and seen.get("authorization") == "Bearer a.b.c", (status, seen)
        statuses = []
        for n in range(61):
            status, seen = call(f"Basic {n:04d}")
            statuses.append(status)
            if status == 200:
                assert "authorization" not in seen, seen
        assert statuses[:60] == [200] * 60, statuses
        assert statuses[60] == 429, statuses
        assert call("Bearer a.b.c")[0] == 200, "a bearer keeps its own route and limit"
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)


def test_the_sanitiser_clears_the_underscore_spelling_of_every_header(component_plugins, apisix_config):
    """`X_Userinfo` is a header of its own on the wire, and `underscores_in_headers on` lets it
    travel. A WSGI or CGI upstream then folds it onto the same name as `X-Userinfo`, so
    clearing only the hyphenated spelling leaves the attack open on the catalogue. The Lua
    clears both spellings of every name; this asserts the reason it has to."""
    assert apisix_config["nginx_config"]["http"]["underscores_in_headers"] == "on", (
        "underscores_in_headers is no longer on: the second clear_header may go, but only "
        "together with the comment that explains it"
    )
    for route_id, (source, config) in component_plugins.items():
        lua = sanitiser(config["plugins"])
        assert 'gsub("%-", "_")' in lua, (
            f"{route_id} ({source}) clears the hyphenated spelling only; X_Userinfo travels"
        )


def test_the_client_ip_header_survives_the_sanitiser(component_plugins):
    """It is the audit trail and the rate-limit key, and nginx maintains it itself (T-0929)."""
    for route_id, (source, config) in component_plugins.items():
        assert '"X-Forwarded-For"' not in sanitiser(config["plugins"]), f"{route_id} ({source})"


# --- T-1673: a route without the authentication its sibling has (OPS-31) -------------------


def test_every_component_route_is_named_in_the_reviewed_allow_list(component_routes):
    """The list of what answers without a token is reviewed, not inferred. A new route is red
    here until somebody writes down how a caller on it is authenticated and why."""
    assert set(component_routes) == set(ROUTE_CLASSES), {
        "not classified": sorted(set(component_routes) - set(ROUTE_CLASSES)),
        "classified but gone": sorted(set(ROUTE_CLASSES) - set(component_routes)),
    }


def test_every_route_carries_the_authentication_its_class_names(component_routes, component_plugins):
    """The allow-list is a claim about the route table; this is the claim checked against it."""
    for route_id, expected in ROUTE_CLASSES.items():
        # A route shares another's plugin config when it names one (`pluginConfig`, T-2665).
        source, config = component_plugins[component_routes[route_id][1].get("pluginConfig", route_id)]
        plugins = config["plugins"]
        oidc = plugins.get("openid-connect")
        reach = expected["reach"]
        where = f"{route_id} ({source}) is classified {reach}"

        if reach in (EDGE_LOGIN, EDGE_SESSION):
            assert oidc, f"{where} but carries no openid-connect plugin"
            assert oidc["bearer_only"] is False, f"{where}: session mode, not bearer-only"
            wanted = "auth" if reach == EDGE_LOGIN else "pass"
            assert oidc["unauth_action"] == wanted, f"{where}: unauth_action is {oidc['unauth_action']}"
        else:
            assert not oidc, f"{where} but carries the edge login; reclassify it or remove the plugin"

        if reach == TERMINATES:
            answered = {"redirect", "fault-injection"} & set(plugins)
            assert answered, f"{where} but answers nothing itself; it would proxy anonymously"

        assert expected["why"].strip(), f"{route_id} is classified without a reason"


def test_a_route_that_reaches_its_upstream_anonymously_is_rate_limited(component_routes, component_plugins):
    """An unauthenticated route with no bucket is a free amplifier for whoever finds it. The
    routes that terminate at the edge are bounded too — answering 302 still costs a worker."""
    for route_id, (source, config) in component_plugins.items():
        if {**ROUTE_CLASSES, **TEMPLATE_CLASSES}[route_id]["reach"] == EDGE_LOGIN:
            continue  # the login front answers a redirect to the realm before anything else
        limit = config["plugins"].get("limit-count")
        assert limit, f"{route_id} ({source}) passes anonymous traffic with no limit-count"
        assert limit["rejected_code"] == 429, f"{route_id} ({source})"
        assert limit["count"] > 0 and limit["time_window"] > 0, f"{route_id} ({source})"


# --- T-1674: TLS, HSTS and the security headers on every host (OPS-27) ---------------------


def response_headers(config: dict) -> dict:
    rewrite = config["plugins"].get("response-rewrite")
    assert rewrite, "the route sets no response headers"
    return rewrite["headers"]["set"]


def test_every_route_of_every_component_answers_with_the_transport_headers(component_plugins):
    """OPS-27, OPS-34, OPS-36 — HSTS, no content sniffing and a referrer policy on every answer of every host,
    the two addons and the routes the edge answers by itself included."""
    for route_id, (source, config) in component_plugins.items():
        headers = response_headers(config)
        assert headers["Strict-Transport-Security"] == "max-age=31536000; includeSubDomains; preload", (
            f"{route_id} ({source})"
        )
        assert headers["X-Content-Type-Options"] == "nosniff", f"{route_id} ({source})"
        assert headers["Referrer-Policy"] in ("no-referrer", "strict-origin-when-cross-origin"), (
            f"{route_id} ({source}) sends an unreviewed referrer policy: {headers['Referrer-Policy']}"
        )


def test_framing_and_caching_follow_the_class_the_allow_list_gives_the_route(component_plugins):
    """A surface that frames its own pages says SAMEORIGIN; everything else says DENY. A
    surface that serves its own static assets may be cached; everything else is `no-store`,
    so an answer that depended on a token never sits in a shared cache."""
    for route_id, (source, config) in component_plugins.items():
        expected = {**ROUTE_CLASSES, **TEMPLATE_CLASSES}[route_id]
        headers = response_headers(config)
        assert headers.get("X-Frame-Options") == expected["framing"], f"{route_id} ({source})"
        if expected["cacheable"] == UPSTREAM_DECIDES:
            assert "Cache-Control" not in headers, (
                f"{route_id} ({source}) overrides the Cache-Control its upstream sets for itself"
            )
        elif expected["cacheable"]:
            assert "Cache-Control" not in headers, (
                f"{route_id} ({source}) is classified cacheable and sets Cache-Control anyway"
            )
        else:
            assert headers["Cache-Control"] == "no-store, no-cache, must-revalidate", (
                f"{route_id} ({source})"
            )


def test_the_edge_does_not_name_its_own_version(apisix_config):
    """A `Server: APISIX/3.13.0` banner is the first thing a scanner matches against a CVE
    list. The header stays — nginx has no switch that removes it — and the number goes."""
    assert apisix_config["apisix"]["enable_server_tokens"] is False


def test_http_answers_a_redirect_and_never_the_page(rendered):
    """OPS-27 — on the ingress-nginx branch the redirect is said on the Ingress rather than
    left to the controller's `ssl-redirect` default, which is a ConfigMap key an operator may
    turn off for every Ingress at once. The Traefik branch redirects on the `web` entrypoint,
    which the Hetzner kit owns; the live conformance check is what holds that one."""
    ingress = next(d for d in rendered("production") if d.get("kind") == "Ingress")
    annotations = ingress["metadata"]["annotations"]
    assert annotations["nginx.ingress.kubernetes.io/force-ssl-redirect"] == "true"
    assert ingress["spec"]["tls"], "a redirect to https with no certificate is a redirect to nothing"


# --- T-1675: request smuggling and oversized requests at the edge (GW26) -------------------


def nginx_http(apisix_config: dict) -> dict:
    return apisix_config["nginx_config"]["http"]


def test_the_edge_bounds_how_much_header_it_will_read(apisix_config):
    """GW26 — 64 KB of headers must be refused, not buffered. nginx answers 494 once a request
    outgrows `large_client_header_buffers`, and the number has to be a bounded one: without
    the directive the worker keeps reading into the default 8 KB and fails on the first line
    instead, which is a different error to read in a log at three in the morning."""
    snippet = apisix_config["nginx_config"]["http_end_configuration_snippet"]
    buffers = re.search(r"^\s*large_client_header_buffers\s+(\d+)\s+(\d+)([kKmM]);", snippet, re.MULTILINE)
    assert buffers, f"the http block sets no large_client_header_buffers:\n{snippet}"
    count, size, unit = int(buffers.group(1)), int(buffers.group(2)), buffers.group(3).lower()
    ceiling = count * size * (1024 if unit == "k" else 1024 * 1024)
    assert 0 < ceiling <= 256 * 1024, f"a {ceiling} byte header ceiling is not a ceiling"

    one = re.search(r"^\s*client_header_buffer_size\s+(\d+)([kKmM]);", snippet, re.MULTILINE)
    assert one, "the first header buffer has no size, so every request allocates the large ones"


def test_the_edge_bounds_how_slowly_a_request_may_arrive(apisix_config):
    """GW26 — a slow body holds a worker and its connection for as long as the client trickles.
    Each of these is a ceiling on one read, not on the whole request, so the values are
    generous; what matters is that none of them is absent or zero, because nginx reads `0` as
    no timeout at all and the connection would then be held until the client let go."""
    http = nginx_http(apisix_config)
    for directive in ("client_header_timeout", "client_body_timeout", "send_timeout", "keepalive_timeout"):
        value = http.get(directive)
        assert value, f"{directive} is not set; a slow client holds the worker indefinitely"
        seconds = int(re.fullmatch(r"(\d+)s?", str(value)).group(1))
        assert 0 < seconds <= 300, f"{directive} is {value}"


def server_block_ceiling(apisix_config) -> int:
    """The server block's `client_max_body_size`, in bytes."""
    server = apisix_config["nginx_config"].get("http_server_configuration_snippet", "")
    found = re.search(r"^\s*client_max_body_size\s+(\d+)([kKmM]);", server, re.MULTILINE)
    assert found, f"the proxy server block sets no client_max_body_size:\n{server!r}"
    unit = found.group(2).lower()
    ceiling = int(found.group(1)) * (1024 if unit == "k" else 1024 * 1024)
    return ceiling


def test_the_body_ceiling_is_a_number_in_the_server_block(apisix_config):
    """GW26 — the 1 GB body. `client_max_body_size 0` in the http block is APISIX's own and
    reads as "no limit" for a body with a Content-Length, while nginx's chunked filter
    compares against the same zero without a guard and refuses every streamed body (T-2263).
    The edge's own number therefore lives in the server block, and it is a number."""
    ceiling = server_block_ceiling(apisix_config)
    assert 0 < ceiling <= 64 * 1024 * 1024, f"a {ceiling} byte body ceiling is not a ceiling"


def test_the_served_body_limit_starts_at_the_server_blocks_ceiling(apisix_config, rendered):
    """T-2892, ADR-N-035: the `edge-body` rule the Portal rewrites to the Organization's value
    renders the catalog default, which is the server block's own number, so a base served before
    the Portal composes anything admits exactly what the server block does."""
    ceiling = server_block_ceiling(apisix_config)
    cm = next(
        d for d in rendered("local")
        if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "apisix-standalone-base"
    )
    rules = {r["id"]: r for r in yaml.safe_load(cm["data"]["apisix.yaml"]).get("global_rules", [])}
    # `edge-headers` (T-5107) is the header floor; tests/test_apisix_edge.py holds its body.
    # `edge-not-found` (T-5119) hands an unmatched address to the server block's error page;
    # tests/test_edge_error_pages.py holds its body.
    assert set(rules) == {"edge-body", "edge-headers", "edge-not-found"}, rules
    assert rules["edge-body"]["plugins"] == {"client-control": {"max_body_size": ceiling}}
    assert ceiling == 16 * 1024 * 1024, "the catalog default of spec.limits.edge.maxRequestBodyMegabytes"


# --- T-1676: the admin surfaces are reachable from the internet (OPS-31) -------------------


def test_the_gateway_has_no_admin_api_to_probe_for(apisix_config, rendered):
    """OPS-31 — standalone file mode (ADR-N-007): the routes come from a ConfigMap, so the
    Admin API has nothing to do and is not built in. No port for it, no key to guess."""
    assert apisix_config["apisix"]["enable_admin"] is False
    assert apisix_config["deployment"]["role"] == "data_plane"

    service = next(
        d for d in rendered("local")
        if d.get("kind") == "Service" and d["metadata"]["name"] == "apisix-gateway"
    )
    published = {port["port"] for port in service["spec"]["ports"]}
    assert published <= {80, 443}, f"the gateway Service publishes {sorted(published)}"
    assert 9180 not in published and 9090 not in published


@pytest.mark.parametrize("environment", ["local", "dev", "production"])
def test_no_workload_is_published_past_the_edge(rendered, environment):
    """OPS-31 — a NodePort or a LoadBalancer is a second front door with no route table, no
    rate limit and no header sanitisation in front of it. The edge is the only way in, so
    every Service is a ClusterIP and the only Ingress is the gateway's."""
    documents = rendered(environment)
    published = [
        f"{d['metadata']['name']}: {d['spec']['type']}"
        for d in documents
        if d.get("kind") == "Service" and d["spec"].get("type") not in (None, "ClusterIP")
    ]
    assert not published, f"{environment} publishes {published}"

    ingresses = {d["metadata"]["name"] for d in documents if d.get("kind") == "Ingress"}
    assert ingresses <= {"apisix"}, f"{environment} has an Ingress beside the edge: {sorted(ingresses)}"


def test_no_route_reaches_a_workload_that_answers_inside_the_cluster(component_routes):
    """OPS-31 — the broker, the pipeline runner, the artifact store, the model tools and the
    collector are reached by our own services and by nothing on the internet. The gateway is
    what publishes context data, and it is the only one of them with a route."""
    for route_id, (source, route) in component_routes.items():
        upstream = route["upstream"].split(".")[0]
        assert upstream not in INTERNAL_WORKLOADS, (
            f"{route_id} ({source}) publishes {upstream}, which answers inside the cluster only"
        )


def test_the_identity_administrator_has_no_default_credential(rendered):
    """OPS-31 — the Keycloak admin console is on the identity host by design: it is the realm's
    own login. What must not be there is the password that comes with every tutorial."""
    keycloak = next(
        d for d in rendered("local")
        if d.get("kind") == "StatefulSet" and "keycloak" in d["metadata"]["name"]
    )
    env = {
        e["name"]: e
        for container in keycloak["spec"]["template"]["spec"]["containers"]
        for e in container.get("env", [])
    }
    password = env["KC_BOOTSTRAP_ADMIN_PASSWORD"]
    assert "value" not in password, "the bootstrap admin password is written into the manifest"
    assert password["valueFrom"]["secretKeyRef"]["name"], "the password comes from a Secret"


def test_the_dashboards_admit_nobody_anonymously():
    """OPS-31 — Grafana is published on the apex when the addon is on, so its own front door is
    the control. Anonymous access off, the local login form off, and the local administrator's
    password from a Secret rather than the `admin` the chart ships with."""
    values = (PROJECT_ROOT / "components/grafana/values/grafana/base-values.yaml.gotmpl").read_text()
    # Read as text rather than as YAML: the file is a Go template whose `token_url` line holds
    # an unquoted `{{ ... }}`, which no YAML parser accepts.
    for section, setting, wanted in (
        ("auth.anonymous", "enabled", "false"),
        ("auth", "disable_login_form", "true"),
        ("users", "allow_sign_up", "false"),
    ):
        found = re.search(
            rf"^  {re.escape(section)}:\n(?:    .*\n)*?    {setting}:\s*(\S+)\s*$",
            values,
            re.MULTILINE,
        )
        assert found, f"grafana.ini names no {section}.{setting}"
        assert found.group(1) == wanted, f"{section}.{setting} is {found.group(1)}"
    assert re.search(r"^admin:\n\s+existingSecret:\s+\S+", values, re.MULTILINE), (
        "the local administrator's password is not taken from a Secret"
    )


# --- T-0939 (EP-20, SP-22): who may open a connection to the edge's data plane --------------

APISIX_POD = {"app.kubernetes.io/name": "apisix", "app.kubernetes.io/instance": "apisix-apisix"}
# 9080 is the plaintext entry point, 4143 is where meshed traffic lands on the same pod.
DATA_PLANE_PORTS = (9080, 4143)
INGRESS_CONTROLLERS = (
    ("traefik", {"app.kubernetes.io/name": "traefik"}),
    ("kube-system", {"app.kubernetes.io/name": "traefik"}),
    ("ingress-nginx", {"app.kubernetes.io/name": "ingress-nginx"}),
)


def selects(selector: dict | None, labels: dict) -> bool:
    """A Kubernetes label selector; an empty one selects everything."""
    selector = selector or {}
    if any(labels.get(k) != v for k, v in (selector.get("matchLabels") or {}).items()):
        return False
    for e in selector.get("matchExpressions") or []:
        value, present = labels.get(e["key"]), e["key"] in labels
        held = {
            "In": present and value in e.get("values", []),
            "NotIn": not present or value not in e.get("values", []),
            "Exists": present,
            "DoesNotExist": not present,
        }[e["operator"]]
        if not held:
            return False
    return True


def admitted(policy: dict, port: int, namespace: str, labels: dict) -> bool:
    """Whether one ingress rule of `policy` lets a pod (`namespace`, `labels`) open `port`.
    An ipBlock counts as admitting anyone: every pod address is inside some CIDR."""
    own = policy["metadata"].get("namespace")
    for rule in policy["spec"].get("ingress") or []:
        ports = rule.get("ports")
        if ports and all(p.get("port") != port for p in ports):
            continue
        peers = rule.get("from")
        if not peers:
            return True
        for peer in peers:
            if "ipBlock" in peer:
                return True
            if "namespaceSelector" in peer:
                in_namespace = selects(peer["namespaceSelector"], {"kubernetes.io/metadata.name": namespace})
            else:
                in_namespace = namespace == own
            if in_namespace and selects(peer.get("podSelector"), labels):
                return True
    return False


def edge_ingress(docs: list[dict]) -> list[dict]:
    """Every ingress policy in APISIX's namespace that selects the APISIX pod."""
    policies = [d for d in docs if d.get("kind") == "NetworkPolicy"]
    (namespace,) = {p["metadata"].get("namespace") for p in policies if p["metadata"]["name"] == "apisix"}
    return [
        p for p in policies
        if p["metadata"].get("namespace") == namespace
        and "Ingress" in p["spec"].get("policyTypes", [])
        and selects(p["spec"].get("podSelector"), APISIX_POD)
    ]


@pytest.mark.parametrize("environment", ["local", "dev", "production"])
def test_only_the_ingress_controller_may_open_the_edge_data_plane(rendered, environment):
    """EP-20, SP-22 (T-0939): APISIX takes X-Real-IP from the whole pod network, so a pod that
    reaches 9080 or 4143 can name itself any caller and spend that caller's rate limit. Our own
    workloads, an unlabelled pod, and a pod merely labelled like Traefik elsewhere are refused."""
    policies = edge_ingress(rendered(environment))
    assert policies, "no ingress policy selects the APISIX pod"
    namespace = policies[0]["metadata"]["namespace"]
    strangers = [
        (namespace, {}),
        (namespace, {"app.kubernetes.io/name": "portal-portal"}),
        (namespace, {"app.kubernetes.io/name": "context-gateway-gateway"}),
        ("default", {}),
        ("default", {"app.kubernetes.io/name": "traefik"}),
        (namespace, {"app.kubernetes.io/name": "traefik"}),
    ]
    for port in DATA_PLANE_PORTS:
        for peer_namespace, labels in strangers:
            let_in = [p["metadata"]["name"] for p in policies if admitted(p, port, peer_namespace, labels)]
            assert not let_in, f"{environment}: {let_in} admit {peer_namespace}/{labels} on {port}"


@pytest.mark.parametrize("environment", ["local", "dev", "production"])
def test_every_supported_ingress_controller_still_reaches_the_edge(rendered, environment):
    """EP-20: the narrowing keeps the front door open for the controller the environment runs,
    unmeshed on 9080 and meshed on 4143, whether Traefik or ingress-nginx."""
    policies = edge_ingress(rendered(environment))
    for port in DATA_PLANE_PORTS:
        for controller_namespace, labels in INGRESS_CONTROLLERS:
            assert any(admitted(p, port, controller_namespace, labels) for p in policies), (
                f"{environment}: {controller_namespace}/{labels} cannot reach APISIX on {port}"
            )
