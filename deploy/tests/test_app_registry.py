"""What a node needs to run a fullstack App's image, and nothing more (T-2616, AP-107, AP-108).

The Portal pushes the image to the forge's container registry with its own token; a node pulls
it from https://{domain}/v2/ with a Secret holding a token that reads packages and nothing else.
The bootstrap Job mints that token and writes the Secret into the apps namespace, the edge routes
/v2/ to the forge beside /git/, APISIX may reach App pods, and the Portal is told all three.
"""

import base64
import json
import shutil

import pytest

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")


def one(docs, kind, name):
    return next(d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name)


def env_of(container):
    return {e["name"]: e.get("value") for e in container.get("env", [])}


@pytest.fixture(scope="module")
def local(rendered):
    return rendered("local")


@pytest.fixture(scope="module")
def script(local):
    return one(local, "Job", "gitea-bootstrap")["spec"]["template"]["spec"]["containers"][0]["args"][0]


REGISTRY = {
    "REGISTRY_SECRET": "app-registry",
    "REGISTRY_SCOPES": '["read:package"]',
    "REGISTRY_HOST": "joinedcontext.test",
    "REGISTRY_NAMESPACES": "apps",
}


@requires_helmfile
def test_the_pull_secret_is_a_dockerconfigjson_of_a_token_that_reads_packages(script, tmp_path):
    """AP-108: the Job mints `read:package` alone and writes it as the registry credential of
    the primary host, keeping the token and its scopes as the record; a second run finds the
    token still reading the organization's packages and keeps it, and no run prints it."""
    from test_forge_seed_converges import Forge

    forge = Forge(tmp_path, {})
    first = forge.run(script, **REGISTRY)
    assert first.returncode == 0, first.stderr
    written = forge.secrets["app-registry"]
    assert written["type"] == "kubernetes.io/dockerconfigjson"
    token = base64.b64decode(written["data"]["token"]).decode()
    assert json.loads(base64.b64decode(written["data"]["scopes"])) == ["read:package"]
    config = json.loads(base64.b64decode(written["data"][".dockerconfigjson"]))
    assert list(config["auths"]) == ["joinedcontext.test"]
    assert base64.b64decode(config["auths"]["joinedcontext.test"]["auth"]).decode() == f"forge-admin:{token}"
    assert any("/namespaces/apps/secrets/app-registry" in line for line in forge.log)
    assert any('"name":"jc-registry-pull","scopes":["read:package"]' in line for line in forge.log) or \
        "minted jc-registry-pull" in first.stdout
    assert token not in first.stdout + first.stderr

    second = forge.run(script, **REGISTRY)
    assert second.returncode == 0, second.stderr
    assert "the token in app-registry still authenticates" in second.stdout
    assert any(line.startswith("GET") and line.endswith("/api/v1/packages/joinedcontext") for line in forge.log)
    # The portal and gateway tokens stay plain Secrets: the pull shape is this one's alone.
    assert forge.secrets["gitea-token-portal"]["type"] == "Opaque"


@requires_helmfile
def test_without_app_pods_no_pull_token_is_minted(script, tmp_path):
    from test_forge_seed_converges import Forge

    forge = Forge(tmp_path, {})
    result = forge.run(script, **{**REGISTRY, "REGISTRY_NAMESPACES": ""})
    assert result.returncode == 0, result.stderr
    assert "app-registry" not in forge.secrets
    assert "jc-registry-pull" not in result.stdout


@requires_helmfile
def test_the_portal_is_told_the_registry_the_pull_secret_and_the_apisix_namespace(local):
    """AP-108: the three settings the reconciler composes an App pod from agree with what the
    rest of the render says: the Job's Secret name and apps namespace, the apex host the edge
    routes /v2/ on, and the namespace APISIX actually runs in."""
    portal = one(local, "Deployment", "portal")
    env = env_of(portal["spec"]["template"]["spec"]["containers"][0])
    job = env_of(one(local, "Job", "gitea-bootstrap")["spec"]["template"]["spec"]["containers"][0])
    apisix = one(local, "Deployment", "apisix")

    assert env["JC_PORTAL_APPS_PULL_SECRET_NAME"] == job["REGISTRY_SECRET"] == "app-registry"
    assert json.loads(job["REGISTRY_SCOPES"]) == ["read:package"]
    assert env["JC_PORTAL_APPS_NAMESPACE"] in job["REGISTRY_NAMESPACES"].split()
    assert env["JC_PORTAL_APPS_REGISTRY"] == job["REGISTRY_HOST"] == env["JC_PORTAL_APPS_URL"].removeprefix("https://")
    assert env["JC_PORTAL_APISIX_NAMESPACE"] == apisix["metadata"]["namespace"]


@requires_helmfile
def test_the_edge_routes_v2_to_the_forge_without_the_git_prefix_strip(local):
    """AP-108: a node pulls from /v2/ at the apex's root, which the forge serves at its own root;
    stripping /git/ there, as the forge route does, would send every pull to a 404."""
    import yaml

    raw = one(local, "ConfigMap", "apisix-standalone-base")["data"]["apisix.yaml"]
    parsed = yaml.safe_load(raw)
    route = next(r for r in parsed["routes"] if r["id"] == "gitea-registry")
    upstream = next(u for u in parsed["upstreams"] if u["id"] == "gitea-registry")
    plugins = next(p for p in parsed["plugin_configs"] if p["id"] == "gitea-registry")["plugins"]
    assert route["uri"] == "/v2/*"
    # Pull only at the edge: the Portal pushes in-cluster (AP-107), so PUT/POST/PATCH/DELETE on
    # the public /v2/ would only ever be a push nobody meant.
    assert route["methods"] == ["GET", "HEAD"]
    assert list(upstream["nodes"])[0].startswith("gitea-http.") and list(upstream["nodes"])[0].endswith(":3000")
    assert "regex_uri" not in plugins.get("proxy-rewrite", {})
    assert "X-Access-Token" in plugins["serverless-pre-function"]["functions"][0]
    assert plugins["limit-count"]["rejected_code"] == 429


@requires_helmfile
def test_apisix_reaches_app_pods_on_their_one_port_and_nothing_else_there(local):
    """AP-26, AP-108: the edge's egress admits the pods the reconciler labels as Apps, in the
    apps namespace, on 8080 only; an App pod's own policy admits APISIX from its namespace."""
    policy = one(local, "NetworkPolicy", "apisix")
    portal_ns = one(local, "Deployment", "portal")["metadata"]["namespace"]
    rules = [
        rule for rule in policy["spec"]["egress"]
        if any(peer.get("podSelector", {}).get("matchLabels", {}).get("joinedcontext.com/app") == "true"
               for peer in rule.get("to", []))
    ]
    assert len(rules) == 1, rules
    (peer,) = rules[0]["to"]
    assert peer["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"] == portal_ns
    assert rules[0]["ports"] == [{"protocol": "TCP", "port": 8080}]


@requires_helmfile
def test_apisix_reaches_meshed_app_pods_on_the_inbound_proxy(local):
    """AP-108: an App pod is meshed, so the edge's traffic lands on its Linkerd inbound port;
    the 8080 rule alone would allow a connection the proxy never makes."""
    policy = one(local, "NetworkPolicy", "apisix-linkerd-control-plane")
    portal_ns = one(local, "Deployment", "portal")["metadata"]["namespace"]
    rules = [
        rule for rule in policy["spec"]["egress"]
        if any(peer.get("podSelector", {}).get("matchLabels", {}).get("joinedcontext.com/app") == "true"
               and peer["namespaceSelector"]["matchLabels"]["kubernetes.io/metadata.name"] == portal_ns
               for peer in rule.get("to", []))
    ]
    assert len(rules) == 1, rules
    assert rules[0]["ports"] == [{"protocol": "TCP", "port": 4143}]


@requires_helmfile
def test_containerd_oauth_post_to_the_token_realm_reaches_the_forge_and_nothing_else_does(local):
    """T-2665, AP-108: containerd with a pull Secret POSTs to /v2/token first and falls back to
    GET only on a 404, which the forge answers. Anywhere else the POST met the portal's catch-all
    and its HTML redirect, and every fullstack pull failed. POST is admitted on that one path only."""
    import yaml

    raw = one(local, "ConfigMap", "apisix-standalone-base")["data"]["apisix.yaml"]
    parsed = yaml.safe_load(raw)
    token = next(r for r in parsed["routes"] if r["id"] == "gitea-registry-token")
    registry = next(r for r in parsed["routes"] if r["id"] == "gitea-registry")
    upstream = next(u for u in parsed["upstreams"] if u["id"] == "gitea-registry-token")
    assert token["uri"] == "/v2/token" and token["methods"] == ["POST"]
    assert token["host"] == registry["host"] and token["priority"] > registry["priority"]
    assert list(upstream["nodes"])[0].startswith("gitea-http.")
    # The same header clearing and rate limit as every pull.
    assert token["plugin_config_id"] == "gitea-registry"
