"""Attack vector: a write without the CSRF token, or from another origin (T-1679, PF-46).

The CSRF half of the vector is played where the gate lives, in the Portal:
`joinedcontext-portal/tests/edge_auth_csrf_tests.rs` sends every mutating route with the
session cookie and no token, with a wrong token, with a token from another cookie name and
with a byte that differs, and proves the gate sits in front of every mutating API route and of
no read. Nothing of that is repeated here.

This file is the other half — the origin — and it lives in the deployment repository because
that is where the answer is decided. Two rules together:

* the Portal's own routes carry no `cors` plugin at all, so a page on another origin gets no
  `Access-Control-Allow-Origin` and the browser never lets it read the response, whatever the
  CSRF gate would have said;
* the endpoint surface, which is meant to be read by browser apps, names the origins it
  allows by a regular expression and allows no credentials with them.

A regular expression is where this goes wrong quietly: `^https://.+\\.platform.example.org$`
reads as "a subdomain of ours" and means "a subdomain of anything that looks like ours,
because `.` matches every character" — `https://evil.platform-example.org` is a host somebody
else can register, and it matched. The escaping is now done by the ConfigMap template
(`${DOMAIN_RE}`), and `test_a_look_alike_domain_is_not_our_domain` is what keeps it done.
"""

import re

import pytest
import yaml

#: The domain the `local` environment renders with.
DOMAIN = "joinedcontext.test"

#: Every route the Portal answers. A `cors` plugin on any of them would publish the Portal's
#: API to a page on another origin.
PORTAL_CONFIGS = (
    "portal-ui",
    "portal-public",
    "portal-api",
    "portal-well-known",
    "portal-live-notify",
    "portal-metrics",
    "apps-surface",
    "portal-redirect",
)


@pytest.fixture(scope="module")
def plugin_configs(rendered):
    cm = next(
        d for d in rendered("local")
        if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "apisix-standalone-base"
    )
    parsed = yaml.safe_load(cm["data"]["apisix.yaml"])
    return {pc["id"]: pc.get("plugins", {}) for pc in parsed["plugin_configs"]}


@pytest.fixture(scope="module")
def cors_configs(plugin_configs):
    """Every route that answers CORS at all, by route id."""
    return {name: plugins["cors"] for name, plugins in plugin_configs.items() if "cors" in plugins}


def test_the_portal_answers_no_cross_origin_request_at_all(plugin_configs, cors_configs):
    """The strongest form of the defence: there is nothing to get wrong.

    A cross-origin `fetch` to the Portal is sent by the browser (that is what the CSRF gate is
    for) but its response is unreadable without an `Access-Control-Allow-Origin`, and the Portal
    sends none. A preflighted request — anything with `x-csrf-token` on it — never leaves the
    browser at all.
    """
    for config_id in PORTAL_CONFIGS:
        assert config_id in plugin_configs, f"{config_id} is not a route any more"
        assert config_id not in cors_configs, (
            f"{config_id} gained a cors plugin: the Portal's API is now readable cross-origin"
        )


def test_no_route_anywhere_reflects_an_origin_together_with_credentials(cors_configs):
    """`allow_credential: true` beside a pattern is the combination that loses the session:
    the browser then sends the cookie and hands the response to the page that asked."""
    for name, cors in cors_configs.items():
        assert cors["allow_credential"] is False, name
        assert cors.get("allow_origins", "*") != "*" or "allow_origins_by_regex" in cors, name
        assert "*" not in cors.get("allow_origins", ""), f"{name}: a wildcard origin"


def test_a_look_alike_domain_is_not_our_domain(cors_configs):
    """The regression this file exists for.

    Every pattern is compiled and fired at the hosts an attacker can actually register. With
    the domain's dots escaped they all miss; with a bare `.` the first two match.
    """
    ours = [f"https://apps.{DOMAIN}", f"https://a.b.{DOMAIN}"]
    theirs = [
        f"https://evil.{DOMAIN.replace('.', '-')}",   # joinedcontext-test, one registration away
        f"https://evil.{DOMAIN.replace('.', 'x')}",   # joinedcontextxtest
        f"https://{DOMAIN}",                          # the apex is not a subdomain of itself
        f"http://apps.{DOMAIN}",                      # plaintext
        f"https://apps.{DOMAIN}.evil.example",        # our name as a prefix of theirs
        f"https://apps.{DOMAIN}:8443",                # a port is not part of an allowed origin
        f"https://evil.example/https://apps.{DOMAIN}",
        f"https://evil.example@apps.{DOMAIN}",        # userinfo in front of our host
    ]

    assert cors_configs, "no route answers CORS: this test would pass by having nothing to check"
    for name, cors in cors_configs.items():
        patterns = [re.compile(p) for p in cors["allow_origins_by_regex"]]
        for origin in ours:
            assert any(p.match(origin) for p in patterns), f"{name} refuses our own {origin}"
        for origin in theirs:
            assert not any(p.match(origin) for p in patterns), f"{name} admits {origin}"


def test_a_browser_app_may_only_read_and_may_only_send_what_it_needs(cors_configs):
    """The endpoint surface is a read surface across origins. A write from another origin is not
    part of the deal, and neither is a header that carries a second credential."""
    for name, cors in cors_configs.items():
        methods = {m.strip() for m in cors["allow_methods"].split(",")}
        assert methods <= {"GET", "HEAD", "OPTIONS"}, f"{name} allows {methods - {'GET', 'HEAD', 'OPTIONS'}}"
        headers = {h.strip().lower() for h in cors["allow_headers"].split(",")}
        assert headers <= {"authorization", "content-type"}, f"{name} allows {headers}"
        assert "x-csrf-token" not in headers, (
            f"{name} would let another origin preflight a Portal mutation"
        )


def test_another_origin_reads_only_the_count_and_paging_headers(cors_configs):
    """T-4333: a catalogue page shows how many records an NGSI-LD query matched. T-5414: a catalogue
    chart follows a cut temporal answer to the end of its window (Link rel="intervalafter",
    Content-Range). These are the headers the Endpoint surface exposes across origins; no other
    header reaches another page."""
    exposed = {name: cors.get("expose_headers") for name, cors in cors_configs.items() if cors.get("expose_headers")}
    assert set(exposed) == {"context-endpoint", "context-endpoint-portal"}, exposed
    assert set(exposed.values()) == {"NGSILD-Results-Count,Content-Range,Link"}, exposed
