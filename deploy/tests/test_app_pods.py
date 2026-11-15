"""The ui-rust App pods jc's Portal proxies on their App hosts (components/app-pods).

Prod's reconciler ran hsl-transport as Deployment and Service `app-hsl-transport`; jc runs the same
pod from the image the monorepo's image-apps.yml signs. The pod is reached by jc alone and reaches
jc alone (both halves of each NetworkPolicy, Linkerd's proxy port beside the app port), on jc's
apps listener (8083: the gateway's GET/HEAD /api/endpoint/... alone), never on jc's public
listener, which stays the edge's (test_jc_component.py).
"""

import copy
import json
import re

import pytest

from conftest import load_yaml
from test_jc_component import check_only_the_edge_reaches_jc, planted

PIN = """
---
images:
  app-pods:
    hsl-transport:
      repository: 'ghcr.io/marek-mraz-jc/jc-app-hsl-transport'
      tag: 'main'
      digest: 'sha256:0000000000000000000000000000000000000000000000000000000000000000'
"""


def of_kind(docs, kind, name=None):
    return [d for d in docs if d.get("kind") == kind and name in (None, d["metadata"]["name"])]


def with_app_pods(tree):
    """jc-prod (which lists app-pods after jc) with an example pin."""
    path = tree / "deployment/environments/jc-prod/global.yaml.gotmpl"
    assert "\n  - jc\n" in path.read_text() and "\n  - app-pods\n" in path.read_text()
    (path.parent / "app-pods.yaml.gotmpl").write_text(PIN)


def without_app_pods(tree):
    """jc-prod with app-pods taken out of its components."""
    path = tree / "deployment/environments/jc-prod/global.yaml.gotmpl"
    text = path.read_text()
    assert "\n  - app-pods\n" in text
    path.write_text(text.replace("\n  - app-pods\n", "\n", 1))


RENDERED: list[list[dict]] = []


@pytest.fixture
def prod(rendered_variant):
    """jc-prod with app-pods, rendered once for the module (the fixture copies the tree)."""
    if not RENDERED:
        RENDERED.append(rendered_variant("jc-prod", with_app_pods))
    return RENDERED[0]


def policies(docs):
    """Every NetworkPolicy of the render by name."""
    return {p["metadata"]["name"]: p for p in of_kind(docs, "NetworkPolicy")}


def admits(policy, direction, labels, port):
    peer_key = "from" if direction == "ingress" else "to"
    for rule in policy["spec"].get(direction, []):
        ports = {p.get("port") for p in rule.get("ports", [])}
        for peer in rule.get(peer_key, []):
            match = (peer.get("podSelector") or {}).get("matchLabels", {})
            if match and all(labels.get(k) == v for k, v in match.items()) and port in ports:
                return True
    return False


JC = {"app.kubernetes.io/name": "jc-server", "app.kubernetes.io/instance": "jc-server"}
POD = {"app.kubernetes.io/name": "app-hsl-transport", "app.kubernetes.io/instance": "app-pods-hsl-transport"}


def test_the_pod_runs_as_prod_ran_it_pinned_nonroot_and_limited(prod):
    (deployment,) = of_kind(prod, "Deployment", "app-hsl-transport")
    assert deployment["metadata"]["namespace"] == "jc-prod"
    spec = deployment["spec"]["template"]["spec"]
    assert spec["securityContext"]["runAsNonRoot"] is True
    assert spec["securityContext"]["runAsUser"] == 65532
    assert spec["automountServiceAccountToken"] is False
    (container,) = spec["containers"]
    assert re.fullmatch(r"ghcr\.io/marek-mraz-jc/jc-app-hsl-transport:main@sha256:[0-9a-f]{64}", container["image"])
    assert container["securityContext"]["readOnlyRootFilesystem"] is True
    assert container["securityContext"]["capabilities"]["drop"] == ["ALL"]
    assert container["resources"]["limits"]["memory"] and container["resources"]["limits"]["cpu"]
    env = {e["name"]: e["value"] for e in container["env"]}
    assert env["JC_ENDPOINT_URL"] == (
        "http://jc.jc-prod.svc.cluster.local:8083/api/endpoint/sv4ibpmhwowb6gnzqy6vp2b3iy/"
    )
    assert env["JC_BASE_PATH"] == "/"
    config = json.loads(env["JC_APP_CONFIG"])
    assert config["portal"] == "https://portal.smartcity.joinedcontext.com/projects/helsinki"
    assert config["slug"] == "sv4ibpmhwowb6gnzqy6vp2b3iy"
    (service,) = of_kind(prod, "Service", "app-hsl-transport")
    assert [p["port"] for p in service["spec"]["ports"]] == [8080]


def jc_env(docs):
    (jc,) = of_kind(docs, "Deployment", "jc")
    return {e["name"]: e.get("value") for c in jc["spec"]["template"]["spec"]["containers"] for e in c.get("env", [])}


def jc_toml_text(docs):
    return next(c["data"]["jc.toml"] for c in of_kind(docs, "ConfigMap") if "jc.toml" in (c.get("data") or {}))


def service_ports(docs, name):
    (service,) = of_kind(docs, "Service", name)
    return {p["port"]: p.get("name") for p in service["spec"]["ports"]}


# The host the pods' reads used to take on jc's public listener, before the apps listener.
OLD_SERVICE_HOST = "jc.jc-prod.svc.cluster.local"


def test_jc_proxies_the_app_host_and_serves_the_reads_on_its_apps_listener(prod):
    env = jc_env(prod)
    assert env["JC_PORTAL_APP_PODS"] == (
        "hsl-transport=http://app-hsl-transport.jc-prod.svc.cluster.local:8080"
    )
    assert env["JCBIN_APPS_LISTEN"] == "0.0.0.0:8083"
    assert service_ports(prod, "jc")[8083] == "apps"
    # No mount for the pods on the public listener: its host is the edge's alone.
    assert OLD_SERVICE_HOST not in jc_toml_text(prod)


def test_without_app_pods_jc_neither_proxies_nor_opens_the_apps_listener(rendered_variant):
    docs = rendered_variant("jc-prod", without_app_pods)
    assert not of_kind(docs, "Deployment", "app-hsl-transport")
    env = jc_env(docs)
    assert "JC_PORTAL_APP_PODS" not in env and "JCBIN_APPS_LISTEN" not in env
    assert 8083 not in service_ports(docs, "jc")
    assert not of_kind(docs, "Server", "jc-apps")
    assert OLD_SERVICE_HOST not in jc_toml_text(docs)


def test_jc_and_the_pod_reach_each_other_alone_both_halves_both_ports(prod):
    found = policies(prod)
    by_target = {}
    for policy in found.values():
        selector = policy["spec"]["podSelector"].get("matchLabels", {})
        by_target.setdefault(selector.get("app.kubernetes.io/name"), []).append(policy)

    def any_admits(target, direction, peer, port):
        return any(admits(p, direction, peer, port) for p in by_target.get(target, []))

    for port in (8080, 4143):
        # jc -> pod: jc's egress, the pod's ingress
        assert any_admits("jc-server", "egress", POD, port), port
        assert any_admits("app-hsl-transport", "ingress", JC, port), port
    for port in (8083, 4143):
        # pod -> jc (the apps listener): the pod's egress, jc's ingress
        assert any_admits("app-hsl-transport", "egress", JC, port), port
        assert any_admits("jc-server", "ingress", POD, port), port
    # Never jc's public listener, in either half.
    assert not any_admits("app-hsl-transport", "egress", JC, 8080)
    assert not any_admits("jc-server", "ingress", POD, 8080)
    # The pod takes nothing from the edge or any other pod, and calls nothing but jc and DNS.
    for policy in by_target["app-hsl-transport"]:
        for rule in policy["spec"].get("ingress", []):
            for peer in rule.get("from", []):
                assert (peer.get("podSelector") or {}).get("matchLabels") == JC, policy["metadata"]["name"]
        for rule in policy["spec"].get("egress", []):
            for peer in rule.get("to", []):
                labels = (peer.get("podSelector") or {}).get("matchLabels")
                namespace = (peer.get("namespaceSelector") or {}).get("matchLabels", {})
                assert labels == JC or labels == {"k8s-app": "kube-dns"} or (
                    labels is None and namespace == {"kubernetes.io/metadata.name": "linkerd"}
                ), policy["metadata"]["name"]
                assert "ipBlock" not in peer, policy["metadata"]["name"]


def mesh_clients(docs, namespace, labels, port):
    """The identities Linkerd admits to `port` of the pods with `labels`: the one Server there must
    deny by default and carry no HTTPRoute; each AuthorizationPolicy on it names its
    MeshTLSAuthentications. None when the port is not held that way."""
    in_ns = lambda kind: [d for d in docs if d.get("kind") == kind and d["metadata"].get("namespace") == namespace]
    servers = [
        s for s in in_ns("Server")
        if s["spec"].get("port") == port
        and all(labels.get(k) == v for k, v in s["spec"]["podSelector"].get("matchLabels", {}).items())
    ]
    if len(servers) != 1 or servers[0]["spec"].get("accessPolicy") != "deny":
        return None
    name = servers[0]["metadata"]["name"]
    if any(any(r.get("name") == name for r in d["spec"].get("parentRefs") or []) for d in in_ns("HTTPRoute")):
        return None
    authentications = {a["metadata"]["name"]: a["spec"].get("identityRefs") or [] for a in in_ns("MeshTLSAuthentication")}
    policies = [p for p in in_ns("AuthorizationPolicy") if p["spec"]["targetRef"].get("name") == name]
    if not policies:
        return None
    clients = []
    for policy in policies:
        refs = policy["spec"].get("requiredAuthenticationRefs") or []
        if not refs or any(r.get("kind") != "MeshTLSAuthentication" for r in refs):
            return None
        for ref in refs:
            clients += authentications.get(ref["name"], [])
    return clients


JC_SA = {"kind": "ServiceAccount", "name": "jc", "namespace": "jc-prod"}
POD_SA = {"kind": "ServiceAccount", "name": "app-hsl-transport", "namespace": "jc-prod"}


def test_the_pods_mesh_server_admits_jc_alone(prod):
    (deployment,) = of_kind(prod, "Deployment", "app-hsl-transport")
    labels = deployment["spec"]["template"]["metadata"]["labels"]
    assert mesh_clients(prod, "jc-prod", labels, 8080) == [JC_SA]


def test_jcs_apps_listener_admits_the_app_pods_identity_alone(prod):
    """8083 has a Server of its own (a port with none falls back to the namespace default, which
    admits every meshed identity) naming the pod's ServiceAccount alone; the pod carries that
    identity: its ServiceAccount, meshed, no API token."""
    (jc,) = of_kind(prod, "Deployment", "jc")
    jc_labels = jc["spec"]["template"]["metadata"]["labels"]
    assert mesh_clients(prod, "jc-prod", jc_labels, 8083) == [POD_SA]
    (pod,) = of_kind(prod, "Deployment", "app-hsl-transport")
    spec = pod["spec"]["template"]
    assert spec["spec"]["serviceAccountName"] == "app-hsl-transport"
    assert spec["metadata"].get("annotations", {}).get("linkerd.io/inject") != "disabled"
    assert pod["metadata"]["namespace"] != "jc-prod-mqtt"

    # Planted: a second identity, an open policy, or no Server at all is caught.
    def edited(kind, change):
        out = []
        for d in prod:
            if d.get("kind") == kind and d["metadata"]["name"].startswith("jc-apps"):
                d = copy.deepcopy(d)
                change(d["spec"])
            out.append(d)
        return out

    wider = edited("MeshTLSAuthentication", lambda s: s["identityRefs"].append({"kind": "Namespace", "name": "jc-prod-mqtt"}))
    assert mesh_clients(wider, "jc-prod", jc_labels, 8083) != [POD_SA]
    assert mesh_clients(edited("AuthorizationPolicy", lambda s: s.update(requiredAuthenticationRefs=[])), "jc-prod", jc_labels, 8083) is None
    assert mesh_clients(edited("Server", lambda s: s.update(accessPolicy="all-authenticated")), "jc-prod", jc_labels, 8083) is None
    assert mesh_clients([d for d in prod if d["metadata"].get("name") != "jc-apps"], "jc-prod", jc_labels, 8083) is None


def test_with_app_pods_only_the_edge_reaches_jcs_public_listener(prod):
    """The invariant of test_jc_component.py on jc-prod with app-pods listed, and a planted rule
    admitting the App pods to the public listener is caught."""
    check_only_the_edge_reaches_jc(prod, "jc-prod+app-pods")
    for ports in ([{"protocol": "TCP", "port": 8080}], [{"protocol": "TCP", "port": "http"}]):
        rule = planted("jc-prod", {"matchLabels": {"app.kubernetes.io/name": "jc-server"}}, ports)
        rule["spec"]["ingress"][0]["from"] = [{"podSelector": {"matchLabels": POD}}]
        with pytest.raises(AssertionError, match="admits pods that are not APISIX"):
            check_only_the_edge_reaches_jc([*prod, rule], "planted-app-pods")
