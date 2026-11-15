"""wasm-host, the shared host of server WASM Apps, as dev renders it (T-3361, ADR-N-044 §2.5-2.7,
AP-143, AP-145, AP-147, AP-157, AP-158).

What is coupled across files: one Deployment per shard behind the Service the edge route names,
each reading its shard's placement and store key (the Portal's) and its database login (the
secret generator's) from files; NetworkPolicies that let only the edge in and only the gateway,
apps-db, the store, the token service and the Portal (a request's caller, email and ai, T-3622) out; the Portal's Role here, by name; the Portal's settings that point at
all of it; and the store's `apps` bucket with its 90-day rule for retired exports."""

import pytest

SHARDS = ("0", "1")


@pytest.fixture
def docs(rendered):
    return rendered("dev")


def one(docs, kind, name):
    found = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert len(found) == 1, f"{kind} {name}: {len(found)}"
    return found[0]


def env_of(container):
    return {e["name"]: e.get("value") for e in container.get("env", [])}


@pytest.mark.parametrize("shard", SHARDS)
def test_each_shard_runs_the_pinned_host_with_no_token_and_a_read_only_root(docs, shard):
    deployment = one(docs, "Deployment", f"jc-wasm-host-{shard}")
    pod = deployment["spec"]["template"]["spec"]
    (container,) = pod["containers"]
    assert "@sha256:" in container["image"]
    assert container["command"] == ["/usr/local/bin/jc-wasm-host"]
    assert pod["automountServiceAccountToken"] is False
    assert container["securityContext"]["readOnlyRootFilesystem"] is True
    assert pod["securityContext"]["runAsNonRoot"] is True
    assert container["readinessProbe"]["httpGet"]["path"] == "/healthz"
    service = one(docs, "Service", f"jc-wasm-host-{shard}")
    # 8080 the edge's, 9090 /metrics and /jobs for the Portal alone (T-3857).
    assert [(p["name"], p["port"]) for p in service["spec"]["ports"]] == [("http", 8080), ("admin", 9090)]
    assert [p["containerPort"] for p in container["ports"]] == [8080, 9090]


# What one compiled component costs a shard, measured by the 10 000-App load test (T-3345,
# ADR-N-044 §8), and what the shard needs beside its cache: compiles in flight and instances.
MIB_PER_COMPONENT = 1.25
HEADROOM_MIB = 256


def mib(quantity):
    units = {"Ki": 1 / 1024, "Mi": 1, "Gi": 1024}
    for unit, factor in units.items():
        if quantity.endswith(unit):
            return float(quantity[: -len(unit)]) * factor
    raise AssertionError(f"a memory limit in Ki, Mi or Gi: {quantity}")


@pytest.mark.parametrize("shard", SHARDS)
def test_each_shard_keeps_no_more_compiled_components_than_its_memory_holds(docs, shard):
    (container,) = one(docs, "Deployment", f"jc-wasm-host-{shard}")["spec"]["template"]["spec"]["containers"]
    cached = int(env_of(container)["JC_WASM_CACHED_COMPONENTS"])
    limit = mib(container["resources"]["limits"]["memory"])
    assert cached * MIB_PER_COMPONENT + HEADROOM_MIB <= limit, (
        f"{cached} compiled components need about {cached * MIB_PER_COMPONENT + HEADROOM_MIB:.0f} MiB; "
        f"the shard may use {limit:.0f} MiB and would be OOM-killed (T-3345)"
    )


@pytest.mark.parametrize("shard", SHARDS)
def test_each_shard_reads_its_own_placement_key_and_login_from_files_never_variables(docs, shard):
    pod = one(docs, "Deployment", f"jc-wasm-host-{shard}")["spec"]["template"]["spec"]
    (container,) = pod["containers"]
    env = env_of(container)
    assert env["JC_WASM_SHARD"] == shard
    assert env["JC_WASM_DB_URL"].startswith(f"postgresql://wasm_host_{shard}@apps-db-rw.")
    assert env["JC_WASM_DB_URL"].endswith("/apps?sslmode=require")
    assert ":" not in env["JC_WASM_DB_URL"].split("://", 1)[1].split("@", 1)[0], "no password in the URL"
    assert all("valueFrom" not in e for e in container["env"]), "no credential as a variable"
    volumes = {v["name"]: v for v in pod["volumes"]}
    assert volumes["placement"]["configMap"]["name"] == f"jc-wasm-host-{shard}-placements"
    assert volumes["store"]["secret"]["secretName"] == f"apps-host-{shard}-store"
    assert volumes["db"]["secret"]["secretName"] == f"apps-host-{shard}-db"
    assert volumes["db"]["secret"]["items"] == [{"key": "password", "path": "password"}]
    mounts = {m["name"]: m["mountPath"] for m in container["volumeMounts"]}
    assert env["JC_WASM_PLACEMENTS"].startswith(mounts["placement"] + "/")
    assert env["JC_WASM_S3_KEY_FILE"].startswith(mounts["store"] + "/")
    assert env["JC_WASM_DB_PASSWORD_FILE"] == mounts["db"] + "/password"
    # T-6195: file links on the Apps' hosts, never the store's in-cluster address.
    assert env["JC_WASM_APPS_HOST"].startswith("apps.") and "svc.cluster.local" not in env["JC_WASM_APPS_HOST"]
    assert "JC_WASM_S3_PUBLIC_ENDPOINT" not in env


def test_the_edge_and_the_portal_alone_reach_a_shard_and_a_shard_reaches_the_gateway_the_database_the_store_its_token_service_and_the_portal(docs):
    policy = one(docs, "NetworkPolicy", "wasm-host-shard")
    sources = [
        (peer["podSelector"]["matchLabels"]["app.kubernetes.io/name"], [p["port"] for p in rule["ports"]])
        for rule in policy["spec"]["ingress"]
        for peer in rule["from"]
    ]
    assert sources == [("apisix", [8080]), ("portal-portal", [9090])], (
        "the edge for an App's API, the Portal for /jobs on the admin port (T-3372, T-3857), nobody else")
    reached = []
    for rule in policy["spec"]["egress"]:
        ports = sorted(p["port"] for p in rule["ports"])
        for peer in rule["to"]:
            labels = peer.get("podSelector", {}).get("matchLabels", {})
            reached.append((tuple(sorted(labels.items())), tuple(ports)))
    assert sorted(reached) == sorted([
        ((("app.kubernetes.io/name", "context-gateway-gateway"),), (8080,)),
        ((("cnpg.io/cluster", "apps-db"),), (5432,)),
        ((("app.kubernetes.io/component", "store"),), (9000,)),
        ((("app.kubernetes.io/name", "wasm-host-tokens"),), (4180,)),
        ((("app.kubernetes.io/name", "portal-portal"),), (8080,)),
        ((("k8s-app", "kube-dns"),), (53, 53)),
    ])
    for rule in policy["spec"]["egress"]:
        assert all("ipBlock" not in peer for peer in rule["to"]), "no address outside the cluster"


def test_the_peers_admit_the_shards_and_apisix_may_call_them(docs):
    shard = {"app.kubernetes.io/name": "wasm-host-shard"}

    def admits(policy):
        return any(peer.get("podSelector", {}).get("matchLabels") == shard for rule in policy["spec"].get("ingress", []) for peer in rule["from"])

    for name in ("artifact-store", "context-gateway", "portal"):
        assert admits(one(docs, "NetworkPolicy", name)), name
    apisix = one(docs, "NetworkPolicy", "apisix")
    assert any(
        peer.get("podSelector", {}).get("matchLabels") == shard and [p["port"] for p in rule["ports"]] == [8080]
        for rule in apisix["spec"]["egress"] for peer in rule["to"]
    )


def test_the_portal_may_write_only_each_shards_placement_and_key_here(docs):
    role = one(docs, "Role", "portal-wasm-shards")
    named = {tuple(r["resources"]): (r.get("resourceNames"), r["verbs"]) for r in role["rules"]}
    assert named[("configmaps",)] == (["jc-wasm-host-0-placements", "jc-wasm-host-1-placements"], ["get", "patch"])
    assert named[("secrets",)] == (["apps-host-0-store", "apps-host-1-store"], ["get", "patch"])
    assert named[("configmaps", "secrets")] == (None, ["create"])
    assert all("delete" not in r["verbs"] and "list" not in r["verbs"] for r in role["rules"])
    binding = one(docs, "RoleBinding", "portal-wasm-shards")
    assert binding["subjects"] == [{"kind": "ServiceAccount", "name": "portal", "namespace": binding["subjects"][0]["namespace"]}]


def test_the_portal_is_pointed_at_the_database_the_shards_the_bucket_and_this_namespace(docs):
    portal = one(docs, "Deployment", "portal")
    container = next(c for c in portal["spec"]["template"]["spec"]["containers"] if "JC_PORTAL_APPS_DB_URL" in env_of(c))
    env = env_of(container)
    assert env["JC_PORTAL_APPS_DB_URL"].startswith("postgresql://jc_apps_admin:$(APPS_DB_PASSWORD)@apps-db-rw.")
    assert env["JC_PORTAL_WASM_SHARDS"] == "2"
    assert env["JC_PORTAL_APPS_BUCKET"] == "apps"
    assert env["JC_PORTAL_WASM_HOST_NAMESPACE"] == one(docs, "Deployment", "jc-wasm-host-0")["metadata"]["namespace"]
    password = next(e for e in container["env"] if e["name"] == "APPS_DB_PASSWORD")
    assert password["valueFrom"]["secretKeyRef"] == {"name": "db-apps-admin", "key": "password"}
    names = [e["name"] for e in container["env"]]
    assert names.index("APPS_DB_PASSWORD") < names.index("JC_PORTAL_APPS_DB_URL"), "$(VAR) expands only from earlier"


def test_the_store_keeps_an_apps_bucket_whose_retired_exports_expire_after_ninety_days(docs):
    job = next(d for d in docs if d.get("kind") == "Job" and "bucket" in d["metadata"]["name"])
    script = job["spec"]["template"]["spec"]["containers"][0]["command"][-1]
    assert 'apps="apps"' in script
    assert '"Prefix":"apps/retired/"' in script and '"Days":90' in script


@pytest.mark.parametrize("shard", SHARDS)
def test_each_shard_asks_the_portal_in_the_cluster_for_a_caller_and_the_services(docs, shard):
    """T-3622, API/06 §5: the Portal's Service on its own port, never the public host."""
    container = one(docs, "Deployment", f"jc-wasm-host-{shard}")["spec"]["template"]["spec"]["containers"][0]
    portal = env_of(container)["JC_WASM_PORTAL_URL"]
    assert portal.startswith("http://portal.") and portal.endswith(".svc.cluster.local:8080"), portal


def linkerd(docs, kind):
    return [d for d in docs if d.get("kind") == kind and d.get("apiVersion", "").startswith("policy.linkerd.io/")]


@pytest.mark.parametrize("shard", SHARDS)
def test_only_the_portal_s_identity_reaches_a_shards_job_records_through_the_mesh(docs, shard):
    """T-3857: the proxy's 4143 admits the edge too; the Server on 9090 refuses every identity
    but the Portal's, so a meshed edge never reads /jobs or /metrics."""
    deployment = one(docs, "Deployment", f"jc-wasm-host-{shard}")
    assert deployment["spec"]["template"]["metadata"]["annotations"]["linkerd.io/inject"] == "enabled"
    (server,) = [s for s in linkerd(docs, "Server") if s["metadata"]["name"] == f"jc-wasm-host-{shard}-api"]
    assert (server["spec"]["port"], server["spec"]["accessPolicy"]) == (9090, "deny")
    (policy,) = [p for p in linkerd(docs, "AuthorizationPolicy") if p["spec"]["targetRef"]["name"] == server["metadata"]["name"]]
    (required,) = policy["spec"]["requiredAuthenticationRefs"]
    (authentication,) = [m for m in linkerd(docs, "MeshTLSAuthentication") if m["metadata"]["name"] == required["name"]]
    portal = one(docs, "Deployment", "portal")
    assert authentication["spec"]["identityRefs"] == [
        {"kind": "ServiceAccount", "name": "portal", "namespace": portal["metadata"]["namespace"]}
    ]
