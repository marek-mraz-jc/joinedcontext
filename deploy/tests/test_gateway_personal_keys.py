"""The spaces' personal-data keys reach the gateway as a mounted file (DM-121, ADR-N-054 2.6, T-5336).

What is coupled across files: the Portal's reconciler writes the Secret `personal-data-keys` into
the gateway's namespace (`JC_PORTAL_GATEWAY_NAMESPACE`), with a Role there that names that Secret
and nothing else; the admission policy `{release}-portal-secrets` admits the name; the gateway
mounts it read-only, optional, without subPath (a rotated key arrives in place), and reads it at
`JC_GATEWAY_PERSONAL_KEYS`."""

import pytest


@pytest.fixture
def docs(rendered):
    return rendered("dev")


def one(docs, kind, name):
    found = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert len(found) == 1, f"{kind} {name}: {len(found)}"
    return found[0]


def env_of(container):
    return {e["name"]: e.get("value") for e in container.get("env", [])}


def gateway(docs):
    deployment = one(docs, "Deployment", "context-gateway")
    spec = deployment["spec"]["template"]["spec"]
    container = next(c for c in spec["containers"] if "JC_GATEWAY_BROKER_URL" in env_of(c))
    return deployment, spec, container


def test_the_gateway_mounts_the_keys_read_only_where_it_reads_them(docs):
    _, spec, container = gateway(docs)
    volume = next(v for v in spec["volumes"] if v["name"] == "personal-keys")
    assert volume["secret"]["secretName"] == "personal-data-keys"
    assert volume["secret"]["optional"] is True, "the gateway starts before the Portal's first sync"
    mount = next(m for m in container["volumeMounts"] if m["name"] == "personal-keys")
    assert mount["readOnly"] is True
    assert "subPath" not in mount, "a subPath mount never sees a rotated key"
    assert env_of(container)["JC_GATEWAY_PERSONAL_KEYS"] == mount["mountPath"]
    others = [c["name"] for c in spec["containers"] + spec.get("initContainers", [])
              if c is not container and any(m["name"] == "personal-keys" for m in c.get("volumeMounts", []))]
    assert others == [], "only the gateway reads the keys"


def test_the_portal_writes_the_keys_into_the_gateways_namespace_and_nothing_else_there(docs):
    deployment, _, _ = gateway(docs)
    namespace = deployment["metadata"]["namespace"]
    portal = one(docs, "Deployment", "portal")
    env = {}
    for container in portal["spec"]["template"]["spec"]["containers"]:
        env.update(env_of(container))
    assert env["JC_PORTAL_GATEWAY_NAMESPACE"] == namespace

    role = one(docs, "Role", "portal-personal-data-keys")
    assert role["metadata"]["namespace"] == namespace
    rules = {(tuple(r["resources"]), tuple(r["verbs"])): r.get("resourceNames") for r in role["rules"]}
    assert rules == {
        (("secrets",), ("get", "patch")): ["personal-data-keys"],
        (("secrets",), ("create",)): None,
    }
    binding = one(docs, "RoleBinding", "portal-personal-data-keys")
    assert binding["metadata"]["namespace"] == namespace
    assert binding["subjects"] == [
        {"kind": "ServiceAccount", "name": "portal", "namespace": portal["metadata"]["namespace"]}
    ]
