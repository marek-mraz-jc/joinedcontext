"""T-5594: the edge's session cookies stay at the edge (OPS-27).

`openid-connect` keeps a person's session in `__Host-jc_edge` (lua-resty-session splits a large
one into `__Host-jc_edge2`..) and hands the upstream the person's token. Before T-5594 the
Cookie header went along too, and the Portal hop on dev answered 431 to a steward's 8 KB session
beside the token. The `edge-not-found` global rule now drops the edge's and an App host's session
cookies after the route's rewrite phase, where `openid-connect` reads them, and before the
upstream. The run test uses the pinned image: a front route records in its rewrite phase what
`openid-connect` would see, and proxies to an echo route that reports what the upstream receives.
"""

import json

import pytest
import yaml
from test_apisix_429_contract import answer, pinned_apisix
from test_edge_error_pages import not_found_rule

pytestmark = pytest.mark.xdist_group("docker-apisix-429")

ECHO = (
    "return function(conf, ctx) "
    "require('apisix.core').response.exit(200, "
    "{cookie = ngx.var.http_cookie, seen = ngx.var.http_x_seen_cookie}) end"
)


def rules() -> str:
    document = {
        "global_rules": [not_found_rule()],
        "upstreams": [{"id": "echo", "type": "roundrobin", "pass_host": "rewrite",
                       "upstream_host": "echo.test", "nodes": {"127.0.0.1:9080": 1}}],
        "routes": [
            {"id": "front", "uri": "/*", "host": "portal.test", "upstream_id": "echo",
             # Rewrite phase, like openid-connect: the session cookie must still be there.
             "plugins": {"proxy-rewrite": {"headers": {"set": {"X-Seen-Cookie": "$http_cookie"}}}}},
            {"id": "echo", "uri": "/*", "host": "echo.test",
             # Rewrite phase on the upstream side: what arrived, before this hop's own rule ran.
             "plugins": {"serverless-pre-function": {"phase": "rewrite", "functions": [ECHO]}}},
        ],
    }
    return yaml.safe_dump(document) + "#END\n"


def test_the_rule_drops_only_the_edge_session_names():
    function = not_found_rule()["plugins"]["serverless-pre-function"]["functions"][0]
    assert 'find("^__Host%-jc_edge")' in function and 'find("^__Host%-jc_app_")' in function
    assert "ngx.exit(404)" in function


def test_the_upstream_never_receives_the_edge_session(tmp_path):
    with pinned_apisix(tmp_path, rules()) as base:
        portal = {"Host": "portal.test"}
        session = "__Host-jc_edge=" + "a" * 4000 + "; __Host-jc_edge2=" + "b" * 3900

        status, _, body = answer(base + "/projects/helsinki/home",
                                 {**portal, "Cookie": f"{session}; __Host-jc_csrf=c; __Host-jc_session=d"})
        echoed = json.loads(body)
        assert status == 200, body
        assert echoed["seen"].startswith("__Host-jc_edge=aaa") and "__Host-jc_edge2=bbb" in echoed["seen"]
        assert echoed["cookie"] == "__Host-jc_csrf=c; __Host-jc_session=d"

        # Only the session: no Cookie header at all reaches the upstream.
        status, _, body = answer(base + "/", {**portal, "Cookie": session})
        assert status == 200 and "cookie" not in json.loads(body), body

        cookies = "__Host-jc_edge_apps=1; __Host-jc_edge_app_x=2; __Host-jc_app_air_quality=3; lang=sk"
        status, _, body = answer(base + "/", {**portal, "Cookie": cookies})
        assert json.loads(body)["cookie"] == "lang=sk", body

        # A look-alike name is someone else's cookie and passes untouched, as does no cookie.
        status, _, body = answer(base + "/", {**portal, "Cookie": "x__Host-jc_edge=1;jc_edge=2"})
        assert json.loads(body)["cookie"] == "x__Host-jc_edge=1;jc_edge=2", body
        status, _, body = answer(base + "/", portal)
        assert status == 200 and json.loads(body) == {}, body

        # The rule's first job stands: an address no route serves is still the edge's 404.
        status, _, _ = answer(base + "/", {"Host": "nobody.test"})
        assert status == 404
