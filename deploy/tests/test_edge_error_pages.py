"""The edge's own error answers and the maintenance switch (T-5119, API-08, R20).

When no route matches, a plugin fails, an upstream is down or slow, or the deployer has put a
host in maintenance, nginx answers by itself, and before T-5119 that answer was an `openresty`
page or APISIX's `{"error_msg": …}`. Now a browser gets a page in its language and every other
client RFC 9457 Problem Details, with nothing of the deployment in either.

The first tests read the committed configuration; the last ones run the pinned APISIX image with
the edge's http and server blocks and the `edge-not-found` global rule, because what a caller is
answered is only proved by an answer. They skip where there is no docker and need no cluster.
"""

import json
import re
import textwrap
from pathlib import Path

import pytest
import yaml
from test_apisix_429_contract import (
    VALUES,
    answer,
    http_end_snippet,
    pinned_apisix,
    server_snippet,
)

pytestmark = pytest.mark.xdist_group("docker-apisix-429")

ROOT = Path(__file__).resolve().parent.parent
STANDALONE = ROOT / "components/apisix/charts/configuration/templates/_standalone.tpl"
ENVIRONMENT = ROOT / "components/apisix/default-environment.yaml.gotmpl"
CODES = (404, 500, 502, 503, 504)
CATALOGUE = {"resource-not-found", "internal-error", "upstream-unavailable", "service-unavailable", "delivery-timeout"}
PAGE = {"Accept": "text/html,application/xhtml+xml;q=0.9,*/*;q=0.8"}
# Words that would tell a visitor what runs behind the edge (R20); the catalogue slug
# `upstream-unavailable` names a kind of failure, not a service.
INTERNALS = re.compile(r"openresty|nginx|apisix|upstream(?!-unavailable)|kubernetes|svc\.cluster|127\.0\.0\.1|error_msg", re.IGNORECASE)


def locations(snippet: str) -> dict[str, str]:
    return dict(re.findall(r"location (@jc_\w+) \{\n(.*?)\n\}", snippet, re.DOTALL))


def not_found_rule() -> dict:
    lines = STANDALONE.read_text().splitlines()
    start = next(i for i, line in enumerate(lines) if line.strip() == "- id: edge-not-found")
    block = []
    for line in lines[start:]:
        if block and line.strip().startswith("{{"):
            break
        block.append(line)
    return yaml.safe_load(textwrap.dedent("\n".join(block)))[0]


def test_every_edge_error_has_a_page_and_problem_details():
    snippet = server_snippet()
    found = locations(snippet)
    for code in CODES:
        assert f"error_page {code} = @jc_{code}_$jc_page_format;" in snippet
        html, problem = found[f"@jc_{code}_html"], found[f"@jc_{code}_json"]
        assert "default_type text/html;" in html and "default-src 'none'" in html
        assert "default_type application/problem+json;" in problem
        for body in (html, problem):
            assert 'X-Content-Type-Options "nosniff"' in body and "no-store" in body
        for raw in re.findall(r"return (\d+) '(.*?)';", problem):
            status, details = int(raw[0]), json.loads(raw[1])
            assert status == code == details["status"], details
            # A type of the platform's catalogue (jc-core ProblemType, API/00 §4), so a client
            # that switches on the slug reads an edge error like a service's.
            assert details["type"].rsplit("/", 1)[1] in CATALOGUE, details
            assert details["type"].startswith("https://joinedcontext.com/errors/") and details["title"]
            assert not INTERNALS.search(raw[1]), raw[1]


def test_the_pages_exist_in_slovak_and_english_and_name_nothing_internal():
    http = http_end_snippet()
    for kind in ("not_found", "error", "unavailable", "maintenance"):
        block = re.search(rf"map \$jc_refusal \$jc_page_{kind} \{{\n(.*?)\n\}}", http, re.DOTALL).group(1)
        assert '<html lang="sk">' in block and '<html lang="en">' in block, kind
        assert "<script" not in block and "http-equiv" not in block, kind
        assert not INTERNALS.search(block), kind


def test_maintenance_is_off_by_default_and_retries_after_a_bounded_window():
    defaults = ENVIRONMENT.read_text()
    assert re.search(r"maintenance:\n\s+hosts: \[\]\n\s+retryAfterSeconds: 900\n", defaults)
    template = VALUES.read_text()
    assert "retryAfterSeconds must be 1..86400 seconds" in template
    assert "is not a subdomain name" in template


def test_an_unmatched_route_is_handed_to_the_error_page():
    function = not_found_rule()["plugins"]["serverless-pre-function"]["functions"][0]
    assert "ctx.matched_route == nil" in function and "ngx.exit(404)" in function


def rules(extra_routes: list[dict]) -> str:
    document = {
        "global_rules": [not_found_rule()],
        "upstreams": [{"id": "closed", "type": "roundrobin", "nodes": {"127.0.0.1:9": 1}}],
        "routes": [
            {"id": "down", "uri": "/down", "host": "portal.test", "upstream_id": "closed"},
            {
                "id": "broken",
                "uri": "/broken",
                "host": "portal.test",
                "upstream_id": "closed",
                "plugins": {"serverless-pre-function": {"phase": "access", "functions": ["return function() error('secret-internal-detail') end"]}},
            },
            # An answer the route itself writes (as an upstream would) is never replaced.
            {
                "id": "own-404",
                "uri": "/own",
                "host": "portal.test",
                "upstream_id": "closed",
                "plugins": {"fault-injection": {"abort": {"http_status": 404, "body": "the route's own answer"}}},
            },
            *extra_routes,
        ],
    }
    return yaml.safe_dump(document, allow_unicode=True) + "#END\n"


def test_the_pinned_image_answers_edge_errors_in_both_shapes(tmp_path):
    with pinned_apisix(tmp_path, rules([])) as base:
        portal = {"Host": "portal.test"}

        status, headers, body = answer(base + "/down", portal)
        assert status == 502 and headers["Content-Type"] == "application/problem+json", (headers, body)
        problem = json.loads(body)
        assert problem["status"] == 502 and problem["type"].endswith("/upstream-unavailable")
        assert headers["Retry-After"] == "30" and headers["X-Content-Type-Options"] == "nosniff"
        assert not INTERNALS.search(body), body

        status, headers, body = answer(base + "/down", {**portal, **PAGE, "Accept-Language": "sk-SK,sk;q=0.9"})
        assert status == 502 and headers["Content-Type"] == "text/html; charset=utf-8", headers
        assert '<html lang="sk">' in body and "Služba je dočasne nedostupná" in body
        assert "default-src 'none'" in headers["Content-Security-Policy"] and not INTERNALS.search(body)
        assert '<html lang="sk">' in answer(base + "/down", {**portal, **PAGE, "Accept-Language": "cs"})[2]
        assert '<html lang="en">' in answer(base + "/down", {**portal, **PAGE, "Accept-Language": "fi"})[2]

        status, headers, body = answer(base + "/no-such-page", portal)
        assert status == 404 and headers["Content-Type"] == "application/problem+json", body
        assert json.loads(body)["type"].endswith("/resource-not-found") and "error_msg" not in body
        status, headers, body = answer(base + "/no-such-page", {**portal, **PAGE, "Accept-Language": "en-GB"})
        assert status == 404 and "This page does not exist" in body and '<a href="/">' in body

        status, headers, body = answer(base + "/broken", portal)
        assert status == 500 and json.loads(body)["type"].endswith("/internal-error"), body
        assert "secret-internal-detail" not in body and not INTERNALS.search(body)

        status, headers, body = answer(base + "/own", portal)
        assert status == 404 and body == "the route's own answer", body


def test_the_pinned_image_holds_a_host_in_maintenance(tmp_path):
    with pinned_apisix(tmp_path, rules([]), maintenance={"portal.test": 600}) as base:
        status, headers, body = answer(base + "/down", {"Host": "portal.test"})
        assert status == 503 and headers["Content-Type"] == "application/problem+json", body
        problem = json.loads(body)
        assert problem["type"].endswith("/service-unavailable") and problem["status"] == 503
        assert "planned maintenance" in problem["detail"]
        assert headers["Retry-After"] == "600", headers

        status, headers, body = answer(base + "/no-such-page", {"Host": "portal.test", **PAGE, "Accept-Language": "sk"})
        assert status == 503 and "Prebieha plánovaná údržba" in body, body
        assert headers["Retry-After"] == "600" and headers["Content-Type"] == "text/html; charset=utf-8"

        # Another host stays open: it gets the ordinary answer (no route there), not maintenance.
        status, headers, body = answer(base + "/down", {"Host": "other.test"})
        assert status == 404 and json.loads(body)["type"].endswith("/resource-not-found"), body
