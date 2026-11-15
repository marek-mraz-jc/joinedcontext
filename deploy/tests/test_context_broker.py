"""The context broker (Antares) as dev renders it: what is specific to it (T-1668, R1, CC-04).

The shared suites see it as one more workload (image pins, the NetworkPolicy lint, kubeconform).
These pin what only it guarantees: it runs as nobody with nothing writable but /tmp, answers its
probes, fits the node, is reachable from the gateway and the Portal only, reaches its database and
nothing else, and is addressed by the gateway and the Portal at the port its Service serves."""

import base64
import shutil

import pytest

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")


@pytest.fixture(scope="module")
def dev(rendered):
    return rendered("dev")


def one(docs, kind, name):
    return next(d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name)


def pod_of(docs):
    return one(docs, "Deployment", "context-broker")["spec"]["template"]["spec"]


def env_of(container):
    return {e["name"]: e for e in container.get("env", [])}


def quantity(text):
    """A Kubernetes memory quantity in bytes (Ki, Mi, Gi)."""
    units = {"Ki": 1 << 10, "Mi": 1 << 20, "Gi": 1 << 30}
    for suffix, factor in units.items():
        if text.endswith(suffix):
            return int(text[: -len(suffix)]) * factor
    return int(text)


@requires_helmfile
def test_the_broker_image_is_pinned_by_digest(dev):
    (container,) = pod_of(dev)["containers"]
    image = container["image"]
    assert "@sha256:" in image and len(image.split("@sha256:")[1]) == 64, image


@requires_helmfile
def test_the_broker_runs_as_nobody_with_a_read_only_root_and_no_token(dev):
    pod = pod_of(dev)
    (container,) = pod["containers"]
    assert pod["securityContext"]["runAsNonRoot"] is True
    assert pod["securityContext"]["runAsUser"] != 0
    assert pod["securityContext"]["seccompProfile"]["type"] == "RuntimeDefault"
    assert pod["automountServiceAccountToken"] is False
    context = container["securityContext"]
    assert context["readOnlyRootFilesystem"] is True
    assert context["allowPrivilegeEscalation"] is False
    assert context["capabilities"]["drop"] == ["ALL"]
    # /tmp and its own admin credential, read only (T-3642).
    mounts = {m["mountPath"]: m.get("readOnly", False) for m in container.get("volumeMounts", [])}
    assert mounts == {"/tmp": False, "/var/run/secrets/antares/admin": True}


@requires_helmfile
def test_the_broker_answers_its_probes_on_its_own_port(dev):
    (container,) = pod_of(dev)["containers"]
    port = container["ports"][0]["containerPort"]
    assert container["livenessProbe"]["httpGet"] == {"path": "/q/health", "port": port}
    assert container["readinessProbe"]["httpGet"] == {"path": "/q/ready", "port": port}


@requires_helmfile
def test_liveness_waits_an_hour_for_a_migration_on_boot(dev):
    """The broker migrates before it listens (T-4852): without a startup probe, liveness kills a
    pod 55 s into a long migration and every restart begins it again."""
    (container,) = pod_of(dev)["containers"]
    startup = container["startupProbe"]
    assert startup["httpGet"] == container["livenessProbe"]["httpGet"]
    assert startup["periodSeconds"] * startup["failureThreshold"] >= 3600


@requires_helmfile
@pytest.mark.parametrize("env", ["dev", "production"])
def test_the_old_broker_stops_before_the_new_one_migrates(rendered, env):
    """T-5311: under a rolling update the old pod kept writing to attr_instances, so migration
    0011's ACCESS EXCLUSIVE lock timed out on every retry, and two in-process matchers shared one
    database for the rollout. Recreate stops the old pod first."""
    deployment = one(rendered(env), "Deployment", "context-broker")
    assert deployment["spec"]["strategy"] == {"type": "Recreate"}
    assert deployment["spec"]["replicas"] == 1


@requires_helmfile
def test_the_broker_has_limits_that_fit_the_16_gb_node(dev):
    (container,) = pod_of(dev)["containers"]
    resources = container["resources"]
    assert quantity(resources["requests"]["memory"]) <= quantity(resources["limits"]["memory"])
    assert quantity(resources["limits"]["memory"]) <= 2 * (1 << 30), "one broker may not take an eighth of the node"


@requires_helmfile
def test_the_database_password_comes_from_the_secret_the_database_is_made_with(dev):
    (container,) = pod_of(dev)["containers"]
    env = env_of(container)
    ref = env["PGPASSWORD"]["valueFrom"]["secretKeyRef"]
    secret = one(dev, "Secret", ref["name"])
    assert ref["key"] in (secret.get("data") or secret.get("stringData") or {})
    database = one(dev, "Database", "antares")["spec"]
    url = env["ANTARES_DATABASE_URL"]["value"]
    assert url.startswith(f"postgresql://{database['owner']}:$(PGPASSWORD)@{database['cluster']['name']}-rw."), url
    assert url.endswith(f"/{database['name']}"), url
    literal = [name for name, var in env.items() if "PASSWORD" in name and "valueFrom" not in var]
    assert literal == [], "a password is a secretKeyRef, never a literal value"


@requires_helmfile
def test_receiver_info_is_sealed_with_a_generated_key_of_the_broker_alone(dev):
    """T-5838/T-5839: ANTARES_SECRET_KEY, 64 hex digits from a generated Secret only the broker's
    namespace holds, never a literal."""
    (container,) = pod_of(dev)["containers"]
    ref = env_of(container)["ANTARES_SECRET_KEY"]["valueFrom"]["secretKeyRef"]
    assert ref == {"name": "context-broker-secret-key", "key": "key"}
    secret = one(dev, "Secret", ref["name"])
    assert secret["metadata"]["namespace"] == one(dev, "Deployment", "context-broker")["metadata"]["namespace"]
    assert secret["metadata"]["annotations"]["helm.sh/resource-policy"] == "keep"
    key = base64.b64decode(secret["data"]["key"]).decode()
    assert len(key) == 64 and int(key, 16) >= 0, "the broker reads 64 hex digits as its 32-byte key"


@requires_helmfile
def test_row_level_security_is_required(dev):
    (container,) = pod_of(dev)["containers"]
    assert env_of(container)["ANTARES_REQUIRE_RLS"]["value"] == "1"


@requires_helmfile
def test_the_gateway_and_the_portal_address_the_port_the_service_serves(dev):
    service = one(dev, "Service", "context-broker")
    # The API port by name: the Service also carries the metrics port (T-5095), which serves no API.
    (port,) = [p for p in service["spec"]["ports"] if p["name"] == "http"]
    (container,) = pod_of(dev)["containers"]
    (api,) = [p for p in container["ports"] if p["name"] == "http"]
    assert port["targetPort"] == api["containerPort"]
    address = f"http://context-broker.dev.svc.cluster.local:{port['port']}"
    gateway = one(dev, "Deployment", "context-gateway")["spec"]["template"]["spec"]["containers"][0]
    portal = one(dev, "Deployment", "portal")["spec"]["template"]["spec"]["containers"][0]
    assert env_of(gateway)["JC_GATEWAY_BROKER_URL"]["value"] == address
    assert env_of(portal)["JC_PORTAL_BROKER_URL"]["value"] == address


@requires_helmfile
def test_only_the_gateway_and_the_portal_reach_the_broker(dev):
    policy = one(dev, "NetworkPolicy", "context-broker")["spec"]
    (container,) = pod_of(dev)["containers"]
    # With metrics on (dev since T-5154) a second rule admits Prometheus to the metrics port only.
    inbound, *scrape = policy["ingress"]
    for rule in scrape:
        assert [f["podSelector"]["matchLabels"]["app.kubernetes.io/name"] for f in rule["from"]] == ["prometheus"]
        assert [p["port"] for p in rule["ports"]] == [9464]
    assert len(scrape) <= 1
    peers = sorted(f["podSelector"]["matchLabels"]["app.kubernetes.io/name"] for f in inbound["from"])
    assert peers == ["context-gateway-gateway", "portal-portal"]
    assert [p["port"] for p in inbound["ports"]] == [container["ports"][0]["containerPort"]]


@requires_helmfile
def test_no_broker_ingress_admits_a_peer_from_any_namespace(dev):
    """The broker reads a request without x-jc-* as the cluster's own, and any pod can carry any
    label: a peer must be named in its own namespace, never with `namespaceSelector: {}`. Every
    policy selecting the broker counts, the Linkerd one (4143 reaches every port) included."""
    broker = "context-broker-broker"
    selecting = [
        d for d in dev
        if d.get("kind") == "NetworkPolicy"
        and d["spec"].get("podSelector", {}).get("matchLabels", {}).get("app.kubernetes.io/name") == broker
    ]
    assert len(selecting) >= 1
    for policy in selecting:
        for rule in policy["spec"].get("ingress", []):
            for peer in rule.get("from", []):
                assert peer.get("namespaceSelector") != {}, (policy["metadata"]["name"], peer)
                assert "namespaceSelector" in peer or "podSelector" in peer, (policy["metadata"]["name"], peer)


@requires_helmfile
def test_the_broker_reaches_dns_its_database_and_public_subscribers_only(dev):
    """Since the switch the broker delivers itself (plan 5.5, T-3653), so it no longer dials the
    gateway (T-3925); its one route out of the cluster is the public range of T-3882."""
    policy = one(dev, "NetworkPolicy", "context-broker")["spec"]
    targets = sorted(
        (
            next(iter(rule["to"][0].get("podSelector", {}).get("matchLabels", {}).values()), "ipBlock"),
            tuple(sorted({p["port"] for p in rule["ports"]})),
        )
        for rule in policy["egress"]
    )
    assert targets == [
        ("ipBlock", (80, 443)),
        ("kube-dns", (53,)),
        ("postgres-cluster", (5432,)),
    ]
    assert one(dev, "NetworkPolicy", "default-deny-context-broker")["spec"]["podSelector"] == {}


@requires_helmfile
def test_the_mcp_surface_admits_the_portal_and_apex_origins_alone(dev):
    """Antares MC8 (T-3814): rmcp refuses every request whose Origin is not listed, and the gateway
    forwards the browser's Origin; the list names the Portal's and the Apps' public origins on 443
    and nothing else (the gateway's Host is pinned by test_context_broker_vanilla)."""
    (container,) = pod_of(dev)["containers"]
    origins = env_of(container)["ANTARES_MCP_ALLOWED_ORIGINS"]["value"].split(",")
    portal = env_of(one(dev, "Deployment", "portal")["spec"]["template"]["spec"]["containers"][0])
    public = [portal[k]["value"] for k in ("JC_PORTAL_PUBLIC_URL", "JC_PORTAL_APPS_URL")]
    assert sorted(origins) == sorted(f"{u}:443" for u in public), (origins, public)
    assert all(o.startswith("https://") and "*" not in o for o in origins), origins


@requires_helmfile
@pytest.mark.parametrize("env", ["dev", "production", "smartcity"])
def test_a_broker_mcp_write_is_decided_on_the_portal_page(rendered, env):
    """T-5830: with ANTARES_MCP_CONFIRM_URL unset the AI client's own accept confirmed a broker
    MCP write (form mode). Set, the broker asks by URL and writes only on the decision the
    person recorded on the Portal page (/mcp-confirmations/$id, Portal T-5829)."""
    docs = rendered(env)
    (container,) = pod_of(docs)["containers"]
    portal = env_of(one(docs, "Deployment", "portal")["spec"]["template"]["spec"]["containers"][0])
    page = env_of(container)["ANTARES_MCP_CONFIRM_URL"]["value"]
    assert page == portal["JC_PORTAL_PUBLIC_URL"]["value"] + "/mcp-confirmations/{id}", page
    assert page.startswith("https://") and page.count("{id}") == 1, page
