"""An unknown path on the assistant host is a page in words, not APISIX's JSON (T-4200).

The first tests read the committed route and its plugins; the last runs the pinned APISIX image
with them beside a chat route, because what a browser is answered is only proved by an answer.
It is skipped where there is no docker, and it needs no cluster.
"""

from pathlib import Path

import yaml

from test_apisix_429_contract import answer, pinned_apisix

COMPONENT = Path(__file__).resolve().parent.parent / "components/assistant"


def read(name: str) -> dict:
    # The one template expression is an upstream's namespace; the test reads everything else.
    lines = (COMPONENT / name).read_text().splitlines()
    return yaml.safe_load("\n".join(line.split("{{")[0] for line in lines))


def test_the_catch_all_is_below_every_other_route_of_the_host():
    routes = read("apisix-routes.yaml")
    unknown = routes["assistant-unknown"]
    assert unknown["uri"] == "/*" and unknown["subDomain"] == "assistant"
    others = [r for name, r in routes.items() if name != "assistant-unknown" and r.get("subDomain") == "assistant"]
    assert others and all(r["priority"] > unknown["priority"] for r in others)


def test_the_page_is_words_and_names_no_deployment():
    plugins = read("apisix-plugins.yaml")["assistant-unknown"]["plugins"]
    abort = plugins["fault-injection"]["abort"]
    assert abort["http_status"] == 404
    assert "Táto stránka neexistuje." in abort["body"] and 'lang="sk"' in abort["body"]
    assert "error_msg" not in abort["body"] and "{" not in abort["body"]
    assert plugins["response-rewrite"]["headers"]["set"]["Content-Type"].startswith("text/html")


def test_the_pinned_image_answers_the_page_and_leaves_the_chat_alone(tmp_path):
    plugins = read("apisix-plugins.yaml")["assistant-unknown"]["plugins"]
    rules = {
        "upstreams": [{"id": "closed", "type": "roundrobin", "nodes": {"127.0.0.1:9": 1}}],
        "routes": [
            {"id": "chat", "uri": "/api/v1/d/*", "priority": 10, "upstream_id": "closed"},
            {"id": "unknown", "uri": "/*", "priority": 0, "upstream_id": "closed", "plugins": plugins},
        ],
    }
    with pinned_apisix(tmp_path, yaml.safe_dump(rules, allow_unicode=True) + "#END\n") as base:
        status, headers, body = answer(base + "/no-such-page")
        assert status == 404, body
        assert headers["Content-Type"].startswith("text/html"), headers
        assert "Táto stránka neexistuje." in body and "error_msg" not in body
        assert headers["X-Content-Type-Options"] == "nosniff", headers
        # OPS-34: the edge adds the baseline header, the page narrows itself to nothing.
        assert "frame-ancestors 'none'" in headers["Content-Security-Policy"], headers
        assert """<meta http-equiv="Content-Security-Policy" content="default-src 'none'">""" in body
        # The chat's path still goes to its upstream, closed here: a 502, not the page.
        status, _, body = answer(base + "/api/v1/d/x/chat")
        assert status == 502 and "Táto stránka" not in body
