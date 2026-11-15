"""The server WASM Apps on jc-prod and jc-dev (jc-app-hosts slice 2, ADR-N-044, AP-143..AP-158).

jc's Portal places each wasm App on a shard of wasm-host, writes the shard's placement ConfigMap and
store-key Secret (reconciler/wasm_shards.rs) and hands an App host's /api/* to the shard. What is
coupled across files: the shards and their pins, jc's settings and token, the one Role jc's token
reaches, both halves of every NetworkPolicy (service port and the Linkerd proxy's 4143), and
apps-db under dev's CloudNativePG operator.

The shard does not call jc back yet: jc trusts X-Forwarded-* from the whole pod network, so only
APISIX may reach its listener (test_jc_component.py), and jc-wasm-host passes a component's own
forwarding headers on (platform crates/wasm-host host.rs). Until the host strips them, a shard has
no gateway and no Portal URL and no rule lets it reach jc.
"""

import pytest

ENVS = ("jc-prod", "jc-dev")
SHARDS = (0, 1)
JC = {"app.kubernetes.io/name": "jc-server"}
SHARD = {"app.kubernetes.io/name": "wasm-host-shard"}
APPS_DB = {"cnpg.io/cluster": "apps-db"}
STORE = {"app.kubernetes.io/component": "store"}


@pytest.fixture(scope="module", params=ENVS)
def env(request, rendered):
    return request.param, rendered(request.param)


def one(docs, kind, name):
    found = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert len(found) == 1, f"{kind} {name}: {len(found)}"
    return found[0]


def env_of(container):
    return {e["name"]: e.get("value") for e in container.get("env", [])}


def jc_container(docs):
    return one(docs, "Deployment", "jc")["spec"]["template"]["spec"]["containers"][0]


def reach(docs, subject, direction, peer, port, namespace):
    """Whether a policy selecting `subject` opens `direction` to or from `peer` in `namespace` on `port`."""
    key = "from" if direction == "ingress" else "to"
    for policy in docs:
        if policy.get("kind") != "NetworkPolicy":
            continue
        labels = policy["spec"].get("podSelector", {}).get("matchLabels") or {}
        if not labels or not subject.items() <= labels.items():
            continue
        for rule in policy["spec"].get(direction) or []:
            if port not in [p.get("port") for p in rule.get("ports", [])]:
                continue
            for p in rule.get(key, []):
                matched = p.get("podSelector", {}).get("matchLabels") or {}
                ns = (p.get("namespaceSelector", {}).get("matchLabels") or {}).get("kubernetes.io/metadata.name")
                if peer.items() <= matched.items() and ns == namespace:
                    return True
    return False


@pytest.mark.parametrize("shard", SHARDS)
def test_each_shard_runs_the_pinned_host_non_root_and_bounded(env, shard):
    _, docs = env
    pod = one(docs, "Deployment", f"jc-wasm-host-{shard}")["spec"]["template"]["spec"]
    (container,) = pod["containers"]
    assert container["image"].startswith("ghcr.io/marek-mraz-jc/joinedcontext-platform:") and "@sha256:" in container["image"]
    assert container["command"] == ["/usr/local/bin/jc-wasm-host"]
    assert pod["automountServiceAccountToken"] is False
    assert pod["securityContext"]["runAsNonRoot"] is True
    security = container["securityContext"]
    assert security["readOnlyRootFilesystem"] is True and security.get("privileged") is not True
    assert security["allowPrivilegeEscalation"] is False
    assert container["resources"]["limits"]["memory"] and container["resources"]["requests"]["memory"]
    assert not any("hostPath" in v for v in pod.get("volumes", []))


@pytest.mark.parametrize("shard", SHARDS)
def test_each_shard_reads_its_database_and_bucket_and_calls_no_jc_listener(env, shard):
    name, docs = env
    container = one(docs, "Deployment", f"jc-wasm-host-{shard}")["spec"]["template"]["spec"]["containers"][0]
    settings = env_of(container)
    assert "JC_GATEWAY_URL" not in settings and "JC_WASM_PORTAL_URL" not in settings
    assert settings["JC_WASM_DB_URL"] == f"postgresql://wasm_host_{shard}@apps-db-rw.{name}.svc.cluster.local:5432/apps?sslmode=require"
    assert settings["JC_WASM_S3_ENDPOINT"] == f"http://artifact-store.{name}.svc.cluster.local:9000"
    assert settings["JC_WASM_S3_BUCKET"] == "apps"


def test_jcs_portal_is_pointed_at_the_database_the_shards_the_bucket_and_the_store(env):
    name, docs = env
    container = jc_container(docs)
    settings = env_of(container)
    assert settings["JC_PORTAL_APPS_DB_URL"] == (
        f"postgresql://jc_apps_admin:$(APPS_DB_PASSWORD)@apps-db-rw.{name}.svc.cluster.local:5432/apps?sslmode=require"
    )
    assert settings["JC_PORTAL_WASM_SHARDS"] == str(len(SHARDS))
    assert settings["JC_PORTAL_WASM_HOST_NAMESPACE"] == one(docs, "Deployment", "jc-wasm-host-0")["metadata"]["namespace"]
    assert settings["JC_PORTAL_APPS_BUCKET"] == "apps"
    assert settings["JC_PORTAL_ARTIFACT_STORE_ENDPOINT"] == f"http://artifact-store.{name}.svc.cluster.local:9000"
    assert settings["JC_PORTAL_ARTIFACT_STORE_BUCKET"] == "jc-artifacts"
    # Every credential is a secretKeyRef, never a value; $(VAR) expands only from an earlier entry.
    refs = {e["name"]: e["valueFrom"]["secretKeyRef"] for e in container["env"] if "valueFrom" in e}
    assert refs["APPS_DB_PASSWORD"] == {"name": "db-apps-admin", "key": "password"}
    assert refs["JC_PORTAL_ARTIFACT_STORE_ACCESS_KEY"] == {"name": "artifact-store-root", "key": "ACCESS_KEY_ID"}
    assert refs["JC_PORTAL_ARTIFACT_STORE_SECRET_KEY"] == {"name": "artifact-store-root", "key": "ACCESS_SECRET_KEY"}
    names = [e["name"] for e in container["env"]]
    assert names.index("APPS_DB_PASSWORD") < names.index("JC_PORTAL_APPS_DB_URL")


def test_jcs_token_reaches_each_shards_placement_and_store_key_by_name_alone(env):
    name, docs = env
    assert one(docs, "Deployment", "jc")["spec"]["template"]["spec"]["automountServiceAccountToken"] is True
    role = one(docs, "Role", "portal-wasm-shards")
    rules = {tuple(r["resources"]): (r.get("resourceNames"), r["verbs"]) for r in role["rules"]}
    assert rules == {
        ("configmaps",): ([f"jc-wasm-host-{s}-placements" for s in SHARDS], ["get", "patch"]),
        ("secrets",): ([f"apps-host-{s}-store" for s in SHARDS], ["get", "patch"]),
        # RBAC cannot name an object on create; jc's client refuses any other name (apps/kube.rs).
        ("configmaps", "secrets"): (None, ["create"]),
    }
    binding = one(docs, "RoleBinding", "portal-wasm-shards")
    assert binding["subjects"] == [{"kind": "ServiceAccount", "name": "jc", "namespace": name}]
    assert binding["roleRef"]["name"] == "portal-wasm-shards"
    # Nothing else in the render binds jc's ServiceAccount in jc's namespace or cluster-wide; beside
    # mqtt-brokers the brokers chart's portal-mqtt-brokers binds it in the brokers' own namespace
    # (tests/test_mqtt_brokers.py reads that Role).
    bound = [
        (b["kind"], b["metadata"].get("namespace"), b["metadata"]["name"])
        for b in docs if b.get("kind") in ("RoleBinding", "ClusterRoleBinding")
        if any(s.get("name") == "jc" and s.get("kind") == "ServiceAccount" for s in b.get("subjects") or [])
    ]
    bound = [b for b in bound if not (b[0] == "RoleBinding" and b[2] == "portal-mqtt-brokers" and b[1] != name)]
    assert [b[2] for b in bound] == ["portal-wasm-shards"]


@pytest.mark.parametrize("shard", SHARDS)
def test_only_jcs_identity_reaches_a_shards_job_records_through_the_mesh(env, shard):
    name, docs = env
    authentication = one(docs, "MeshTLSAuthentication", f"jc-wasm-host-{shard}-api-clients")
    assert authentication["spec"]["identityRefs"] == [{"kind": "ServiceAccount", "name": "jc", "namespace": name}]


@pytest.mark.parametrize("port", (8080, 4143))
def test_jc_reaches_the_shards_both_halves_and_never_the_other_way(env, port):
    """knowledge/deployment.md: a meshed peer needs caller egress and callee ingress on the service
    port and on 4143: jc to a shard (an App host's /api/*). Nothing lets a shard reach jc."""
    name, docs = env
    assert reach(docs, JC, "egress", SHARD, port, name)
    assert reach(docs, SHARD, "ingress", JC, port, name)
    assert not reach(docs, SHARD, "egress", JC, port, name)
    assert not reach(docs, JC, "ingress", SHARD, port, name)


def test_jc_reads_the_shards_job_records_on_their_admin_port(env):
    name, docs = env
    assert reach(docs, JC, "egress", SHARD, 9090, name)
    assert reach(docs, SHARD, "ingress", JC, 9090, name)


@pytest.mark.parametrize("peer, port", [(APPS_DB, 5432), (STORE, 9000)])
def test_jc_reaches_the_apps_database_and_the_store_both_halves(env, peer, port):
    name, docs = env
    assert reach(docs, JC, "egress", peer, port, name)
    assert reach(docs, peer, "ingress", JC, port, name)
    assert reach(docs, JC, "egress", peer, 4143, name)
    assert reach(docs, peer, "ingress", JC, 4143, name) or reach(docs, peer, "ingress", {}, 4143, name) or any(
        p["metadata"]["name"] == "artifact-store-allow-linkerd" for p in docs if p.get("kind") == "NetworkPolicy"
    )


def test_jc_reaches_the_api_server_at_the_environments_address_alone(env):
    _, docs = env
    policy = one(docs, "NetworkPolicy", "jc-server-kube-api")
    (rule,) = policy["spec"]["egress"]
    assert rule["to"] == [{"ipBlock": {"cidr": "10.255.0.101/32"}}]
    assert [p["port"] for p in rule["ports"]] == [6443]


def test_apps_db_is_run_by_devs_operator_both_halves(env):
    name, docs = env
    operator = {"app.kubernetes.io/name": "cloudnative-pg"}
    assert reach(docs, APPS_DB, "ingress", operator, 8000, "dev")
    assert reach(docs, APPS_DB, "ingress", operator, 5432, "dev")
    assert reach(docs, operator, "egress", APPS_DB, 8000, name)
    assert reach(docs, operator, "egress", APPS_DB, 5432, name)


def test_no_edge_route_reaches_a_shard_past_jcs_visibility_check(rendered):
    """A non-public App's /api/* reaches its shard only through jc, which checks the caller first
    (app_host.rs); an APISIX upstream naming a shard would skip that."""
    docs = rendered("jc-prod")
    texts = [v for d in docs if d.get("kind") in ("ConfigMap", "Secret")
             for v in (d.get("data") or d.get("stringData") or {}).values()
             if isinstance(v, str) and "routes:" in v and "#END" in v]
    assert texts, "no APISIX standalone file in the render"
    assert all("jc-wasm-host" not in t for t in texts)


def test_jc_without_wasm_host_holds_no_api_token(rendered):
    """addons runs jc with no wasm-host: no token, no API server egress, no Service-host mount."""
    docs = rendered("addons")
    deployment = [d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "jc"]
    if not deployment:
        pytest.skip("addons renders no jc")
    assert deployment[0]["spec"]["template"]["spec"]["automountServiceAccountToken"] is False
    assert not [d for d in docs if d.get("kind") == "NetworkPolicy" and d["metadata"]["name"] == "jc-server-kube-api"]
    assert "JC_PORTAL_APPS_DB_URL" not in env_of(deployment[0]["spec"]["template"]["spec"]["containers"][0])
