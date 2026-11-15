"""One embedder for jc-prod and jc-dev (owner 2026-11-10 16:3xZ, jc-2gib).

jc-prod runs components/embedder; both jc instances call it instead of loading e5-small and ONNX
Runtime in their own process (~0.45 GiB each). Each signs in with its own kaisatsu's token (the
embedder's [[deployments]]: issuer, keys, client, quota); the network admits the jc pods of the
listed namespaces only, on 8089 and, meshed, on Linkerd's 4143, and Linkerd's meshAuthorization the
jc ServiceAccount of each. An environment that lists no callers keeps jc's own namespace alone.
"""

import tomllib

import pytest

CALLERS = ["jc-prod", "jc-dev"]
JC = {"app.kubernetes.io/name": "jc-server", "app.kubernetes.io/instance": "jc-server"}
EMBEDDER = {"app.kubernetes.io/name": "embedder-server", "app.kubernetes.io/instance": "embedder-server"}
URL = "http://embedder.jc-prod.svc.cluster.local:8089"


def one(docs, kind, name):
    found = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert len(found) == 1, f"{kind}/{name}: {len(found)}"
    return found[0]


def admitted(docs, port):
    """(namespaces, pod labels) of every peer an embedder ingress policy admits on `port`."""
    peers = []
    for d in docs:
        if d.get("kind") != "NetworkPolicy" or d["spec"].get("podSelector", {}).get("matchLabels") != EMBEDDER:
            continue
        for rule in d["spec"].get("ingress") or []:
            if any(p.get("port") == port for p in rule.get("ports", [])):
                for peer in rule.get("from", []):
                    ns = peer.get("namespaceSelector", {})
                    names = ns.get("matchLabels", {}).get("kubernetes.io/metadata.name")
                    names = [names] if names else [v for e in ns.get("matchExpressions", []) for v in e["values"]]
                    peers.append((sorted(names), peer.get("podSelector", {}).get("matchLabels")))
    return peers


def jc_toml(docs):
    for d in docs:
        if d.get("kind") == "ConfigMap" and d["metadata"]["name"].startswith("jc"):
            for value in (d.get("data") or {}).values():
                if "[mounts" in value:
                    return tomllib.loads(value)
    raise AssertionError("no jc.toml rendered")


@pytest.mark.parametrize("port", [8089, 4143])
def test_jc_prods_embedder_admits_the_jc_pods_of_both_instances_only(rendered, port):
    assert admitted(rendered("jc-prod"), port) == [(sorted(CALLERS), JC)]


def test_linkerd_admits_the_jc_identity_of_both_instances(rendered):
    auth = one(rendered("jc-prod"), "MeshTLSAuthentication", "embedder-api-clients")
    refs = auth["spec"]["identityRefs"]
    assert sorted((r["namespace"], r["name"]) for r in refs) == sorted((ns, "jc") for ns in CALLERS)


def test_each_instance_signs_in_with_its_own_realm(rendered):
    toml = tomllib.loads(one(rendered("jc-prod"), "ConfigMap", "embedder")["data"]["embedder.toml"])
    deployments = {d["name"]: d for d in toml["deployments"]}
    assert sorted(deployments) == sorted(CALLERS)
    issuers = [d["issuer"] for d in deployments.values()]
    assert len(set(issuers)) == len(issuers), "each instance its own issuer"
    for d in deployments.values():
        assert d["clients"] == ["jc-assistant"]
        assert d["jwks_url"] == d["issuer"] + "/protocol/openid-connect/certs"
        assert d["jwks_url"].startswith("https://")
        assert d["max_texts_per_minute"] > 0


@pytest.mark.parametrize("env", CALLERS)
def test_jc_calls_the_shared_embedder(rendered, env):
    assert jc_toml(rendered(env))["assistant"]["embedder_url"] == URL


def test_jc_dev_has_the_egress_half_to_jc_prods_embedder(rendered):
    policy = one(rendered("jc-dev"), "NetworkPolicy", "jc-egress-to-shared-embedder")
    assert policy["spec"]["podSelector"]["matchLabels"] == JC
    (rule,) = policy["spec"]["egress"]
    assert rule["to"] == [
        {"namespaceSelector": {"matchLabels": {"kubernetes.io/metadata.name": "jc-prod"}}, "podSelector": {"matchLabels": EMBEDDER}}
    ]
    assert sorted(p["port"] for p in rule["ports"]) == [4143, 8089]
    assert not [d for d in rendered("jc-dev") if d["metadata"]["name"] == "embedder"], "jc-dev runs no embedder of its own"


def test_an_environment_naming_no_callers_admits_its_own_jc_alone(rendered):
    assert admitted(rendered("addons"), 8089) == [(["addons"], JC)]
    assert "embedder_url" not in jc_toml(rendered("addons")).get("assistant", {})
