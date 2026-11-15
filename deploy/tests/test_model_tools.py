"""Model Tools as dev renders it: what is specific to it (T-1667, DM-10, Architecture/11).

The LinkML generators and the Smart Data Models import, statelessly. What only this component
guarantees: it runs as nobody with nothing writable but /tmp, answers its probe, fits the node,
only the Portal reaches it, and the Portal addresses the port its Service serves. Its one way out
is HTTPS to the public internet for an import, never an address inside the cluster."""

import shutil

import pytest

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")

PRIVATE_RANGES = {"10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16", "127.0.0.0/8", "100.64.0.0/10"}


@pytest.fixture(scope="module")
def dev(rendered):
    return rendered("dev")


def one(docs: list[dict], kind: str, name: str) -> dict:
    found = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert len(found) == 1, f"expected exactly one {kind}/{name}, got {len(found)}"
    return found[0]


def pod_of(docs):
    return one(docs, "Deployment", "model-tools")["spec"]["template"]["spec"]


def env_of(container):
    return {e["name"]: e.get("value") for e in container.get("env", [])}


@requires_helmfile
def test_the_image_is_pinned_by_digest(dev):
    (container,) = pod_of(dev)["containers"]
    image = container["image"]
    assert "@sha256:" in image and len(image.split("@sha256:")[1]) == 64, image


@requires_helmfile
def test_it_runs_as_nobody_with_a_read_only_root_and_no_token(dev):
    pod = pod_of(dev)
    (container,) = pod["containers"]
    assert pod["securityContext"]["runAsNonRoot"] is True
    assert pod["securityContext"]["runAsUser"] != 0
    assert pod["automountServiceAccountToken"] is False
    context = container["securityContext"]
    assert context["readOnlyRootFilesystem"] is True
    assert context["allowPrivilegeEscalation"] is False
    assert context["capabilities"]["drop"] == ["ALL"]
    assert [m["mountPath"] for m in container.get("volumeMounts", [])] == ["/tmp"]
    assert env_of(container)["PYSTOW_HOME"].startswith("/tmp/"), "its cache must live where it may write"


@requires_helmfile
def test_it_holds_no_secret(dev):
    (container,) = pod_of(dev)["containers"]
    assert all("valueFrom" not in e for e in container.get("env", []))
    assert "envFrom" not in container


@requires_helmfile
def test_it_answers_its_probes_on_its_own_port(dev):
    (container,) = pod_of(dev)["containers"]
    port = container["ports"][0]["containerPort"]
    assert container["livenessProbe"]["httpGet"]["port"] == port
    assert container["readinessProbe"]["httpGet"]["port"] == port
    assert str(port) == env_of(container)["MODEL_TOOLS_PORT"]


@requires_helmfile
def test_it_has_limits_that_fit_the_16_gb_node(dev):
    (container,) = pod_of(dev)["containers"]
    limits = container["resources"]["limits"]
    assert limits["memory"].endswith(("Mi", "Gi"))
    gib = int(limits["memory"][:-2]) / (1024 if limits["memory"].endswith("Mi") else 1)
    assert gib <= 2, limits


@requires_helmfile
def test_the_portal_addresses_the_port_the_service_serves(dev):
    (port,) = one(dev, "Service", "model-tools")["spec"]["ports"]
    (container,) = pod_of(dev)["containers"]
    assert port["targetPort"] == container["ports"][0]["containerPort"]
    portal = one(dev, "Deployment", "portal")["spec"]["template"]["spec"]["containers"][0]
    assert env_of(portal)["JC_PORTAL_MODEL_TOOLS_URL"] == f"http://model-tools.dev.svc.cluster.local:{port['port']}"


@requires_helmfile
def test_only_the_portal_reaches_it(dev):
    policy = one(dev, "NetworkPolicy", "model-tools")["spec"]
    (inbound,) = policy["ingress"]
    assert [f["podSelector"]["matchLabels"] for f in inbound["from"]] == [{"app.kubernetes.io/name": "portal-portal"}]
    assert [p["port"] for p in inbound["ports"]] == [8080]
    assert one(dev, "NetworkPolicy", "default-deny-model-tools")["spec"]["podSelector"] == {}


@requires_helmfile
def test_its_only_ways_out_are_dns_and_https(dev):
    policy = one(dev, "NetworkPolicy", "model-tools")["spec"]
    ports = sorted({p["port"] for rule in policy["egress"] for p in rule["ports"]})
    assert ports == [53, 443]


@requires_helmfile
def test_its_https_way_out_never_reaches_an_address_inside_the_cluster(dev):
    """DM-10 (T-2559): the import fetches from the public internet; a name the person typed must not
    steer it at the Kubernetes API, a node, a webhook, another pod or a metadata service."""
    policy = one(dev, "NetworkPolicy", "model-tools")["spec"]
    blocks = [to["ipBlock"] for rule in policy["egress"] for to in rule["to"] if "ipBlock" in to]
    assert blocks, "no HTTPS rule at all"
    for block in blocks:
        assert PRIVATE_RANGES <= set(block.get("except", [])), block
