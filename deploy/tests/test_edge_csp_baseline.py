"""OPS-34: the upstream owns its Content Security Policy; the edge adds a baseline where none came.

The Portal sends its own policy and each App its per-App one (AP-12, AP-122). A
`Content-Security-Policy` the edge set in `response-rewrite` would replace them, and a second one
beside them would intersect and break the page. So every route carries a `header_filter`
`serverless-post-function` that sets the baseline only when the answer has no policy yet:
`frame-ancestors` as the route's `X-Frame-Options` says, `object-src 'none'`, `base-uri 'self'`.

The first tests read `components/*/apisix-plugins.yaml`. The last one runs the pinned APISIX image
with the Lua a component ships, because "only when there is none" is only proved by an answer:
it is skipped where there is no docker, and it needs no cluster.
"""

import copy
import socket
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path

import pytest
import yaml

from test_apisix_429_contract import pinned_image, server_snippet

pytestmark = pytest.mark.xdist_group("docker-apisix-csp")

PROJECT_ROOT = Path(__file__).resolve().parent.parent
PLUGIN_FILES = sorted(PROJECT_ROOT.glob("components/*/apisix-plugins.yaml"))
ANCESTORS = {"DENY": "'none'", "SAMEORIGIN": "'self'"}
# The routes whose upstream names in its own `frame-ancestors` the sites that may frame its page
# (AG-114, and a public form's page, EP-101); the edge sets no X-Frame-Options there, and an
# answer without a policy gets DENY's.
# The chart renderer (T-4641) too: any site may frame its host page; and the standalone
# assistant (T-4694), whose app.js shows a framed copy only inside its deployment's origins.
FRAMED_BY_UPSTREAM = {
    "assistant/assistant-widget", "portal/portal-public-form", "ckan/ckan-map", "viz/viz", "ai/ai", "jc/jc-apps",
}


def baseline(framing: str) -> str:
    return f"frame-ancestors {ANCESTORS[framing]}; object-src 'none'; base-uri 'self'"


def lua(framing: str) -> str:
    """The one function every route runs; only the `frame-ancestors` source differs."""
    return (
        "return function()\n"
        '  if ngx.header["Content-Security-Policy"] == nil then\n'
        '    ngx.header["Content-Security-Policy"] =\n'
        f'      "{baseline(framing)}"\n'
        "  end\n"
        "end"
    )


def plugin_configs() -> dict[str, dict]:
    found = {}
    for path in PLUGIN_FILES:
        for name, entry in (yaml.safe_load(path.read_text()) or {}).items():
            found[f"{path.parent.name}/{name}"] = entry["plugins"]
    assert found, "no component ships an apisix-plugins.yaml"
    return found


def csp_problems(route: str, plugins: dict) -> list[str]:
    """What is wrong with one route's CSP handling, each in words that say what to change."""
    problems = []
    headers = (plugins.get("response-rewrite") or {}).get("headers") or {}
    for section in ("set", "add"):
        entries = headers.get(section) or {}
        names = entries.keys() if isinstance(entries, dict) else [e.split(":", 1)[0] for e in entries]
        if any(name.strip().lower() == "content-security-policy" for name in names):
            problems.append(
                f"{route}: response-rewrite {section}s Content-Security-Policy, which replaces or "
                "narrows the policy the upstream sends (OPS-34); drop it, the upstream owns its CSP "
                "and serverless-post-function adds the baseline where none came"
            )
    framing = (headers.get("set") or {}).get("X-Frame-Options")
    if route in FRAMED_BY_UPSTREAM:
        if framing is not None:
            problems.append(f"{route}: X-Frame-Options {framing!r} overrides the frame-ancestors its upstream names")
            return problems
        framing = "DENY"
    if framing not in ANCESTORS:
        problems.append(f"{route}: X-Frame-Options is {framing!r}; the baseline follows DENY or SAMEORIGIN")
        return problems
    post = plugins.get("serverless-post-function")
    if not post:
        problems.append(
            f"{route}: no serverless-post-function, so an answer with no CSP leaves the edge with "
            f"none; add the header_filter baseline `{baseline(framing)}` (OPS-34)"
        )
    elif post.get("phase") != "header_filter" or post.get("functions") != [lua(framing)]:
        problems.append(
            f"{route}: serverless-post-function is not the OPS-34 baseline for {framing}; it must "
            f"run in header_filter and set `{baseline(framing)}` only when no CSP is present"
        )
    return problems


def test_every_route_adds_the_baseline_only_where_the_upstream_sent_none():
    """OPS-34: every route of every component, the addons and the edge-answered ones included."""
    problems = [p for route, plugins in plugin_configs().items() for p in csp_problems(route, plugins)]
    assert not problems, "\n".join(problems)


def test_a_route_that_overrides_the_upstreams_policy_is_refused_with_what_to_change():
    """OPS-34: the forbidden configuration, planted in a copy of a real route, is named."""
    plugins = copy.deepcopy(plugin_configs()["portal/portal-ui"])
    plugins["response-rewrite"]["headers"]["set"]["Content-Security-Policy"] = "default-src 'self'"
    del plugins["serverless-post-function"]
    problems = csp_problems("portal/portal-ui", plugins)
    assert any("replaces or narrows the policy the upstream sends" in p for p in problems), problems
    assert any("add the header_filter baseline" in p for p in problems), problems

    unframed = copy.deepcopy(plugin_configs()["portal/portal-api"])
    unframed["serverless-post-function"]["functions"] = [lua("SAMEORIGIN")]
    assert csp_problems("portal/portal-api", unframed), "a DENY route framed by 'self' passed"


def test_the_refusal_the_server_block_writes_carries_the_deny_baseline():
    """OPS-34: the 429 page is built after the plugin chain, so it sets the baseline itself."""
    expected = f'add_header Content-Security-Policy "{baseline("DENY")}" always;'
    assert expected in server_snippet()


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def answer(url: str) -> tuple[int, list[str]]:
    """The status and every Content-Security-Policy header of the answer, in order."""
    try:
        with urllib.request.urlopen(url, timeout=5) as response:
            return response.status, response.headers.get_all("Content-Security-Policy") or []
    except urllib.error.HTTPError as refused:
        return refused.code, refused.headers.get_all("Content-Security-Policy") or []


def route(route_id: str, uri: str, plugins: dict, upstream: str | None = None) -> dict:
    entry = {"id": route_id, "uri": uri, "plugins": plugins}
    if upstream:
        entry["upstream"] = {"type": "roundrobin", "nodes": {upstream: 1}}
    return entry


@pytest.mark.skipif(not __import__("shutil").which("docker"), reason="docker not installed")
def test_the_pinned_image_keeps_the_upstreams_policy_and_adds_the_baseline_only_where_none_came(
    tmp_path,
):
    """OPS-34, played on `apache/apisix` as pinned, with the Lua `portal-api` and `portal-ui` ship."""
    configs = plugin_configs()
    deny = configs["portal/portal-api"]["serverless-post-function"]
    same_origin = configs["portal/portal-ui"]["serverless-post-function"]
    # The upstream is APISIX itself: two inner routes answer with and without a policy of their own.
    upstream = "127.0.0.1:9080"
    inner = "return function() {csp}ngx.say('inner'); ngx.exit(200) end"
    routes = [
        route("with", "/inner/with", {"serverless-pre-function": {"phase": "rewrite", "functions": [
            inner.format(csp="ngx.header['Content-Security-Policy'] = \"default-src 'self'\"; ")]}}),
        route("without", "/inner/without", {"serverless-pre-function": {"phase": "rewrite", "functions": [
            inner.format(csp="")]}}),
        route("edge", "/edge/*", {
            "proxy-rewrite": {"regex_uri": ["^/edge/(.*)", "/inner/$1"]},
            "serverless-post-function": deny,
        }, upstream),
        # What the Portal composes for an App: `apps-surface` plus its own frame-ancestors (AP-122).
        route("app", "/app/*", {
            "proxy-rewrite": {"regex_uri": ["^/app/(.*)", "/inner/$1"]},
            "response-rewrite": {"headers": {
                "add": ["Content-Security-Policy: frame-ancestors portal.example.org"],
                "remove": ["X-Frame-Options"],
            }},
            "serverless-post-function": same_origin,
        }, upstream),
        # A refusal APISIX writes itself, before any upstream is dialled.
        route("refused", "/refused", {"key-auth": {}, "serverless-post-function": deny}, upstream),
    ]
    (tmp_path / "config.yaml").write_text(
        "deployment:\n  role: data_plane\n  role_data_plane:\n    config_provider: yaml\n"
    )
    (tmp_path / "apisix.yaml").write_text(yaml.safe_dump({"routes": routes}) + "#END\n")

    port = free_port()
    name = f"apisix-csp-baseline-{port}"
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
    try:
        base = f"http://127.0.0.1:{port}"
        deadline = time.time() + 60
        while time.time() < deadline:
            try:
                first = answer(f"{base}/edge/with")
                break
            except OSError:
                time.sleep(1)
        else:
            pytest.fail("APISIX did not start")

        assert first == (200, ["default-src 'self'"]), "the upstream's policy is kept, alone"
        assert answer(f"{base}/edge/without") == (200, [baseline("DENY")])
        assert answer(f"{base}/refused") == (401, [baseline("DENY")])
        assert answer(f"{base}/app/with") == (
            200, ["default-src 'self'", "frame-ancestors portal.example.org"]
        ), "an App's own policy and the Portal's frame-ancestors, and no baseline beside them"
        assert answer(f"{base}/app/without") == (200, ["frame-ancestors portal.example.org"])
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)
