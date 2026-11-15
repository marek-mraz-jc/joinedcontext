"""T-3993: the MQTT edge brokers (ADR-N-048, MF-54, PL-74, PL-76, PL-77).

The chart is rendered on its own with three brokers, two in one project and one of the same
name in another, because no environment declares a broker: the Portal's reconciler stamps them
(T-3994). The environment render then checks the wiring around them: the gateway's internal
listener, the runner's namespace variable and the NetworkPolicies.
"""

import ipaddress
import json
import re
import shutil
import socket
import ssl
import struct
import subprocess
import threading
import time
import tomllib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import ClassVar

import pytest
import yaml

PROJECT_ROOT = Path(__file__).resolve().parent.parent
CHART = PROJECT_ROOT / "components/mqtt-brokers/charts/brokers"
TEMPLATE = "broker-template"
STORAGE_PLUGINS = {"rmqtt-retainer", "rmqtt-session-storage", "rmqtt-message-storage"}

BASE = {
    "image": {"repository": "rmqtt/rmqtt", "tag": "0.24.0", "digest": "sha256:" + "0" * 64},
    "domain": "dev.example.org",
    "authUrl": "http://context-gateway.jc-context-gateway.svc.cluster.local:8082/internal/mqtt/auth",
    "runner": {"namespace": "jc-pipeline-runner", "serviceAccount": "pipeline-runner"},
    "serviceMesh": {"enabled": True},
    "gateway": {"namespace": "jc-context-gateway", "podLabels": {"app.kubernetes.io/name": "context-gateway-gateway"}},
    "brokers": [
        {"project": "helsinki", "name": "cameras"},
        {"project": "helsinki", "name": "buses", "limits": {"maxMessageBytes": 1024, "messagesPerSecond": 5, "maxConnections": 3}},
        {"project": "praha", "name": "cameras"},
    ],
}


def helm(values: dict, tmp_path: Path) -> subprocess.CompletedProcess:
    if shutil.which("helm") is None:
        pytest.skip("helm not installed")
    path = tmp_path / "values.yaml"
    path.write_text(yaml.safe_dump(values))
    return subprocess.run(
        ["helm", "template", "t", str(CHART), "-n", "jc-mqtt", "-f", str(path)],
        capture_output=True, text=True,
    )


@pytest.fixture(scope="module")
def docs(tmp_path_factory) -> list[dict]:
    result = helm(BASE, tmp_path_factory.mktemp("mqtt"))
    assert result.returncode == 0, result.stderr
    return [d for d in yaml.safe_load_all(result.stdout) if d]


def of(docs, kind):
    """The objects of one kind by name, the template the Portal stamps from left out."""
    return {d["metadata"]["name"]: d for d in docs if d["kind"] == kind and d["metadata"]["name"] not in (TEMPLATE, "mqtt-broker-pods")}


def config(docs, release):
    data = of(docs, "ConfigMap")[release]["data"]
    return {key: tomllib.loads(text) for key, text in data.items()}


RELEASES = ["mqtt-helsinki-cameras", "mqtt-helsinki-buses", "mqtt-praha-cameras"]
# The smallest packet cap (T-6043, the Portal's CONNECT_ALLOWANCE): a device's CONNECT carries a
# Keycloak token of about 1.2 KB as its password, and RMQTT's one max_packet_size caps it too.
CONNECT_ALLOWANCE = 4096


def test_a_token_connect_fits_every_broker_whatever_its_message_cap(docs):
    """T-6043: the hsl-bridge broker (maxMessageBytes 1024) refused its device's 1186-byte CONNECT
    ("Max size exceeded"); every listener's cap is maxMessageBytes, never under 4096."""
    wanted = {"mqtt-helsinki-cameras": 65536, "mqtt-helsinki-buses": 4096, "mqtt-praha-cameras": 65536}
    for release in RELEASES:
        for name, listener in config(docs, release)["rmqtt.toml"]["listener"]["tcp"].items():
            assert listener["max_packet_size"] == f"{wanted[release]}B", (release, name)


def test_each_broker_is_one_release_with_its_own_identity(docs):
    for kind in ("ServiceAccount", "ConfigMap", "Service", "Deployment", "Certificate", "IngressRouteTCP"):
        assert sorted(of(docs, kind)) == sorted(RELEASES), kind
    for release, deployment in of(docs, "Deployment").items():
        assert deployment["spec"]["template"]["spec"]["serviceAccountName"] == release


def test_the_last_acl_rule_denies_everything_and_nothing_is_stored(docs):
    for release in RELEASES:
        files = config(docs, release)
        assert files["rmqtt-acl.toml"]["rules"] == [["deny", "all"]]
        main = files["rmqtt.toml"]
        started = set(main["plugins"]["default_startups"])
        assert started == {"rmqtt-acl", "rmqtt-auth-http"}
        assert not started & STORAGE_PLUGINS
        # Started by the binary unless disabled: an HTTP API with no token.
        assert main["plugins"]["disabled_default_startups"] == ["rmqtt-http-api"]
        assert set(files) == {"rmqtt.toml", "rmqtt-auth-http.toml", "rmqtt-acl.toml"}


def test_only_the_two_plain_listeners_are_open(docs):
    main = config(docs, "mqtt-helsinki-buses")["rmqtt.toml"]
    listeners = main["listener"]
    for kind in ("tls", "ws", "wss", "quic"):
        assert listeners[kind]["external"]["enable"] is False, kind
    tcp = listeners["tcp"]
    assert tcp["internal"]["addr"] == "0.0.0.0:1883" and tcp["internal"]["proxy_protocol"] is False
    assert tcp["external"]["addr"] == "0.0.0.0:1884" and tcp["external"]["proxy_protocol"] is True
    for listener in tcp.values():
        assert listener["allow_anonymous"] is False
        assert listener["max_qos_allowed"] == 1
        assert listener["max_session_expiry_interval"] == "0s"
        # maxMessageBytes 1024, raised to the CONNECT allowance (T-6043).
        assert listener["max_packet_size"] == "4096B"
        assert listener["max_connections"] == 3
    assert main["rpc"]["server_addr"].startswith("127.0.0.1:")
    assert main["log"]["to"] == "console"


def test_every_connect_asks_the_gateway_and_an_error_refuses(docs):
    auth = config(docs, "mqtt-praha-cameras")["rmqtt-auth-http.toml"]
    assert auth["deny_if_error"] is True
    assert auth["disconnect_if_expiry"] is True
    request = auth["http_auth_req"]
    assert request["url"] == BASE["authUrl"]
    assert request["headers"] == {"content-type": "application/json"}
    assert request["params"] == {"clientid": "%c", "username": "%u", "password": "%P", "ipaddr": "%a"}
    # T-5560: every publish topic is decided by the gateway too, and a refusal disconnects.
    acl = auth["http_acl_req"]
    assert acl["url"] == BASE["authUrl"].removesuffix("/auth") + "/acl"
    assert acl["params"] == {"access": "%A", "clientid": "%c", "username": "%u", "topic": "%t"}
    assert auth["disconnect_if_pub_rejected"] is True


def test_hosts_differ_across_projects_and_tls_is_one_host_each(docs):
    hosts = {}
    for release, certificate in of(docs, "Certificate").items():
        assert certificate["spec"]["issuerRef"]["kind"] == "ClusterIssuer"
        (host,) = certificate["spec"]["dnsNames"]
        assert "*" not in host
        hosts[release] = host
    assert hosts == {
        "mqtt-helsinki-cameras": "cameras.helsinki.mqtt.dev.example.org",
        "mqtt-helsinki-buses": "buses.helsinki.mqtt.dev.example.org",
        "mqtt-praha-cameras": "cameras.praha.mqtt.dev.example.org",
    }
    for release, route in of(docs, "IngressRouteTCP").items():
        spec = route["spec"]
        assert spec["entryPoints"] == ["mqtts"]
        (rule,) = spec["routes"]
        assert rule["match"] == f"HostSNI(`{hosts[release]}`)"
        assert rule["services"] == [{"name": release, "port": 1884, "proxyProtocol": {"version": 2}}]
        assert spec["tls"] == {"secretName": f"{release}-tls", "options": {"name": "mqtt-bsi-tr-02102"}}


def test_no_state_on_disk_and_no_token_mounted(docs):
    for deployment in of(docs, "Deployment").values():
        pod = deployment["spec"]["template"]["spec"]
        assert pod["automountServiceAccountToken"] is False
        assert [v["name"] for v in pod["volumes"]] == ["config"]
        assert "configMap" in pod["volumes"][0]
        (container,) = pod["containers"]
        assert container["securityContext"]["readOnlyRootFilesystem"] is True
        assert "@sha256:" in container["image"]
    assert not [d for d in docs if d["kind"] == "PersistentVolumeClaim"]


@pytest.mark.parametrize("extra, live", [({}, "nc"), ({"portal": {"namespace": "jc-portal", "serviceAccount": "portal"}}, "wget")],
                         ids=["no http api", "http api"])
def test_a_hung_broker_leaves_the_service_and_is_restarted(tmp_path, extra, live):
    """G-16, T-6727: both probes run inside the container on loopback, since the Linkerd Servers
    refuse the kubelet as an unmeshed peer; liveness gives up after readiness."""
    result = helm({**BASE, **extra}, tmp_path)
    assert result.returncode == 0, result.stderr
    for name, deployment in of([d for d in yaml.safe_load_all(result.stdout) if d], "Deployment").items():
        (container,) = deployment["spec"]["template"]["spec"]["containers"]
        ready, alive = container["readinessProbe"], container["livenessProbe"]
        assert ready["exec"]["command"] == ["nc", "-z", "-w", "2", "127.0.0.1", "1883"], name
        assert alive["exec"]["command"][0] == live, name
        assert all("127.0.0.1" in " ".join(p["exec"]["command"]) for p in (ready, alive)), name
        assert alive["periodSeconds"] * alive["failureThreshold"] > ready["periodSeconds"] * ready["failureThreshold"], name


def test_each_listener_admits_one_mesh_identity(docs):
    servers = of(docs, "Server")
    authentications = of(docs, "MeshTLSAuthentication")
    for release in RELEASES:
        for listener, port, identity in (
            ("internal", 1883, {"kind": "ServiceAccount", "name": "pipeline-runner", "namespace": "jc-pipeline-runner"}),
            ("external", 1884, {"kind": "ServiceAccount", "name": "traefik", "namespace": "traefik"}),
        ):
            server = servers[f"{release}-{listener}"]["spec"]
            assert server["port"] == port and server["accessPolicy"] == "deny"
            assert authentications[f"{release}-{listener}"]["spec"]["identityRefs"] == [identity]


def test_the_internal_listener_admits_the_portals_capture_subscriber_when_named(tmp_path):
    """ADR-N-048 §2.10, T-4880: with the Portal named, 1883 admits its identity beside the
    runner's; 1884 still admits Traefik alone."""
    rendered = [d for d in yaml.safe_load_all(helm({**BASE, **PORTAL}, tmp_path).stdout) if d]
    authentications = of(rendered, "MeshTLSAuthentication")
    for release in RELEASES:
        assert authentications[f"{release}-internal"]["spec"]["identityRefs"] == [
            {"kind": "ServiceAccount", "name": "pipeline-runner", "namespace": "jc-pipeline-runner"},
            {"kind": "ServiceAccount", "name": "portal", "namespace": "jc-portal"},
        ]
        assert authentications[f"{release}-external"]["spec"]["identityRefs"] == [
            {"kind": "ServiceAccount", "name": "traefik", "namespace": "traefik"}]


@pytest.mark.parametrize("brokers, message", [
    ([{"project": "helsinki", "name": "cameras"}] * 2, "two brokers answer on cameras.helsinki.mqtt.dev.example.org"),
    ([{"project": "p" * 40, "name": "n" * 30}], "longer than a DNS label"),
])
def test_a_collision_or_an_unbounded_name_refuses_to_render(tmp_path, brokers, message):
    result = helm({**BASE, "brokers": brokers}, tmp_path)
    assert result.returncode != 0
    assert message in result.stderr


def test_no_broker_renders_only_the_template(tmp_path):
    """The template and the TLSOption the routes the Portal stamps from it name (T-5562)."""
    result = helm({**BASE, "brokers": []}, tmp_path)
    assert result.returncode == 0, result.stderr
    docs = [d for d in yaml.safe_load_all(result.stdout) if d]
    # The TLSOption the stamped routes name and the gateway's 8082 Server render with no broker.
    assert sorted((d["kind"], d["metadata"]["name"]) for d in docs) == [
        ("AuthorizationPolicy", "context-gateway-internal"), ("ConfigMap", TEMPLATE),
        ("MeshTLSAuthentication", "context-gateway-internal"), ("Server", "context-gateway-internal"),
        ("Service", "mqtt-broker-pods"), ("TLSOption", "mqtt-bsi-tr-02102")]


def test_no_traefik_renders_no_tls_option(tmp_path):
    """T-4709: TLSOption is a Traefik kind; on another ingress controller (the k3d variants run
    nginx) helm refused the whole release over it, so it renders only where the edge's does."""
    result = helm({**BASE, "ingress": {"className": "nginx"}}, tmp_path)
    assert result.returncode == 0, result.stderr
    kinds = {d["kind"] for d in yaml.safe_load_all(result.stdout) if d}
    assert "TLSOption" not in kinds and "ConfigMap" in kinds


def test_the_decider_port_admits_broker_identities_only(docs, tmp_path):
    """GW25, T-5566: the gateway decides a CONNECT on the caller's `l5d-client-id`, so 8082
    answers a meshed pod of the brokers' namespace and nobody else, whatever the NetworkPolicy
    lets through; with no mesh there is no Server to name."""
    gateway = {d["kind"]: d for d in docs if d["metadata"]["name"] == "context-gateway-internal"}
    assert set(gateway) == {"Server", "MeshTLSAuthentication", "AuthorizationPolicy"}
    for doc in gateway.values():
        assert doc["metadata"]["namespace"] == "jc-context-gateway"
    server = gateway["Server"]["spec"]
    assert server == {
        "podSelector": {"matchLabels": {"app.kubernetes.io/name": "context-gateway-gateway"}},
        "port": 8082, "proxyProtocol": "HTTP/1", "accessPolicy": "deny",
    }
    assert gateway["MeshTLSAuthentication"]["spec"]["identityRefs"] == [{"kind": "Namespace", "name": "jc-mqtt"}]
    policy = gateway["AuthorizationPolicy"]["spec"]
    assert policy["targetRef"] == {"group": "policy.linkerd.io", "kind": "Server", "name": "context-gateway-internal"}
    assert policy["requiredAuthenticationRefs"] == [
        {"group": "policy.linkerd.io", "kind": "MeshTLSAuthentication", "name": "context-gateway-internal"}]
    unmeshed = [d for d in yaml.safe_load_all(helm({**BASE, "serviceMesh": {"enabled": False}}, tmp_path).stdout) if d]
    assert not [d for d in unmeshed if d["metadata"]["name"] == "context-gateway-internal"]


def fill(template: str, release: str, project: str, name: str, limits: dict) -> list[dict]:
    """The template filled as the Portal fills it (src/reconciler/mqtt_brokers.rs), hash aside."""
    for sentinel, value in (
        ("__RELEASE__", release), ("__PROJECT__", project), ("__NAME__", name),
        ("__MAX_BYTES__", max(limits["maxMessageBytes"], CONNECT_ALLOWANCE)), ("__PER_SECOND__", limits["messagesPerSecond"]),
        ("__CONNECTIONS__", limits["maxConnections"]), ("__CONFIG_HASH__", "h"),
    ):
        template = template.replace(sentinel, str(value))
    return [d for d in yaml.safe_load_all(template) if d]


def comparable(docs: list[dict]) -> list[dict]:
    for d in docs:
        d["metadata"]["labels"]["app.kubernetes.io/managed-by"] = "-"
        if d["kind"] == "Deployment":
            d["spec"]["template"]["metadata"]["labels"]["app.kubernetes.io/managed-by"] = "-"
            d["spec"]["template"]["metadata"]["annotations"]["checksum/config"] = "-"
    return sorted(docs, key=lambda d: (d["kind"], d["metadata"]["name"]))


def test_the_template_filled_is_the_broker_the_chart_renders(docs):
    """T-3994: one description of a broker; what the Portal stamps is what an entry here runs."""
    (template,) = [d for d in docs if d["kind"] == "ConfigMap" and d["metadata"]["name"] == TEMPLATE]
    assert template["metadata"]["labels"]["app.kubernetes.io/managed-by"] == "Helm"
    stamped = fill(template["data"]["objects.yaml"], "mqtt-helsinki-buses", "helsinki", "buses", BASE["brokers"][1]["limits"])
    for d in stamped:
        assert d["metadata"]["labels"]["app.kubernetes.io/managed-by"] == "joinedcontext-portal"
        assert d["metadata"]["labels"]["app.kubernetes.io/part-of"] == "mqtt-brokers"
        assert d["metadata"]["labels"]["app.kubernetes.io/instance"] == "mqtt-helsinki-buses"
        assert d["metadata"]["name"].startswith("mqtt-")
    rendered = [d for d in docs if d["metadata"].get("labels", {}).get("app.kubernetes.io/instance") == "mqtt-helsinki-buses"]
    assert comparable(stamped) == comparable(rendered)
    kinds = {(d["apiVersion"], d["kind"]) for d in stamped}
    # The kinds the Portal's client allows a broker (src/apps/kube.rs MQTT_KINDS), and no other.
    assert kinds == {
        ("v1", "ServiceAccount"), ("v1", "ConfigMap"), ("v1", "Service"), ("apps/v1", "Deployment"),
        ("cert-manager.io/v1", "Certificate"), ("traefik.io/v1alpha1", "IngressRouteTCP"),
        ("policy.linkerd.io/v1beta3", "Server"), ("policy.linkerd.io/v1alpha1", "MeshTLSAuthentication"),
        ("policy.linkerd.io/v1alpha1", "AuthorizationPolicy"),
    }


def test_every_template_key_reads_the_same_as_yaml_1_1_and_1_2(docs):
    """T-5856 (dev 2026-10-28): Helm reads the chart as YAML 1.1, the Portal reads the template
    keys as YAML 1.2 (serde_norway). `defaultMode: 0444` is the number 292 to the first and the
    string "0444" to the second, so the API server refused the stamped Deployment and the
    dedicated port's route behind it was never applied. A plain scalar the two read apart
    (a leading-zero number, yes/no/on/off) has no place in a key the Portal stamps."""
    (template,) = [d for d in docs if d["kind"] == "ConfigMap" and d["metadata"]["name"] == TEMPLATE]
    apart = re.compile(r"0[0-9]+|[0-9_]*_[0-9_]*|(?i:y|n|yes|no|on|off)")

    def plain(node):
        if isinstance(node, yaml.ScalarNode):
            if node.style is None and apart.fullmatch(node.value):
                yield node.value
        elif isinstance(node, yaml.SequenceNode):
            for item in node.value:
                yield from plain(item)
        elif isinstance(node, yaml.MappingNode):
            for key, value in node.value:
                yield from plain(key)
                yield from plain(value)

    assert set(template["data"]) >= {"objects.yaml", "objects-port.yaml", "objects-certificate.yaml"}
    for key, text in template["data"].items():
        found = [value for node in yaml.compose_all(text) for value in plain(node)]
        assert not found, f"{key}: {found}"


def test_the_hostname_template_serves_the_city_name_on_8883_with_its_own_certificate(docs):
    """T-5874 (ADR-N-048 §2.11): a custom hostname is the platform door with the city's name, one
    HTTP-01 certificate for that name alone, and the route serves the Secret the Portal names."""
    (template,) = [d for d in docs if d["kind"] == "ConfigMap" and d["metadata"]["name"] == TEMPLATE]
    text = template["data"]["objects-hostname.yaml"]
    for sentinel in ("__HOSTNAME__", "__TLS_SECRET__"):
        assert sentinel in text
    text = text.replace("__HOSTNAME__", "mqtt.city.example").replace("__TLS_SECRET__", "city-mqtt-cert")
    stamped = fill(text, "mqtt-helsinki-buses", "helsinki", "buses", BASE["brokers"][1]["limits"])
    (certificate,) = of(stamped, "Certificate").values()
    assert certificate["spec"]["dnsNames"] == ["mqtt.city.example"]
    assert certificate["spec"]["secretName"] == "mqtt-helsinki-buses-tls"
    (route,) = of(stamped, "IngressRouteTCP").values()
    assert [r["match"] for r in route["spec"]["routes"]] == ["HostSNI(`mqtt.city.example`)"]
    assert route["spec"]["tls"]["secretName"] == "city-mqtt-cert"
    assert route["spec"]["tls"]["options"]["name"] == "mqtt-bsi-tr-02102"
    assert "mqtt.helsinki" not in yaml.safe_dump_all(stamped)
    # The same objects as the platform door but for the name and the Secret.
    platform = fill(template["data"]["objects.yaml"], "mqtt-helsinki-buses", "helsinki", "buses", BASE["brokers"][1]["limits"])
    assert {(d["kind"], d["metadata"]["name"]) for d in stamped} == {(d["kind"], d["metadata"]["name"]) for d in platform}



def test_the_api_server_refuses_a_portal_deployment_that_is_no_broker(docs, tmp_path):
    """T-6719, G-1: RBAC cannot narrow a Deployment's spec, so a native admission policy holds the
    Portal's Deployments here to the template: mqtt-… names, the chart's pinned RMQTT image in
    every container, nothing privileged, no host namespace or path; Deny, failing closed."""
    assert not [d for d in docs if d["kind"].startswith("ValidatingAdmissionPolicy")]
    result = helm({**BASE, "portal": {"namespace": "jc-portal", "serviceAccount": "portal"}}, tmp_path)
    assert result.returncode == 0, result.stderr
    rendered = [d for d in yaml.safe_load_all(result.stdout) if d]
    (policy,) = [d for d in rendered if d["kind"] == "ValidatingAdmissionPolicy"]
    (binding,) = [d for d in rendered if d["kind"] == "ValidatingAdmissionPolicyBinding"]
    spec = policy["spec"]
    assert spec["failurePolicy"] == "Fail" and binding["spec"]["validationActions"] == ["Deny"]
    assert binding["spec"]["policyName"] == policy["metadata"]["name"] == "jc-mqtt-portal-mqtt-brokers"
    assert spec["matchConstraints"]["namespaceSelector"]["matchLabels"] == {"kubernetes.io/metadata.name": "jc-mqtt"}
    assert spec["matchConstraints"]["resourceRules"] == [
        {"apiGroups": ["apps"], "apiVersions": ["v1"], "resources": ["deployments"], "operations": ["CREATE", "UPDATE"]}]
    assert spec["matchConditions"][0]["expression"] == 'request.userInfo.username == "system:serviceaccount:jc-portal:portal"'
    rules = " ".join(v["expression"] for v in spec["validations"] + spec["variables"])
    (broker,) = [d for d in of(rendered, "Deployment").values() if d["metadata"]["name"] == "mqtt-helsinki-cameras"]
    image = broker["spec"]["template"]["spec"]["containers"][0]["image"]
    assert f'c.image == "{image}"' in rules, "the image the template stamps is the one admitted"
    for clause in ('startsWith("mqtt-")', "privileged", "allowPrivilegeEscalation", "capabilities.add",
                   "hostNetwork", "hostPID", "hostIPC", "hostPath", "initContainers"):
        assert clause in rules, clause

def test_the_portal_gets_its_role_only_when_named(docs, tmp_path):
    assert not [d for d in docs if d["kind"] in ("Role", "RoleBinding")]
    result = helm({**BASE, "portal": {"namespace": "jc-portal", "serviceAccount": "portal"}}, tmp_path)
    assert result.returncode == 0, result.stderr
    rendered = [d for d in yaml.safe_load_all(result.stdout) if d]
    (role,) = [d for d in rendered if d["kind"] == "Role"]
    (binding,) = [d for d in rendered if d["kind"] == "RoleBinding"]
    assert binding["subjects"] == [{"kind": "ServiceAccount", "name": "portal", "namespace": "jc-portal"}]
    assert binding["roleRef"]["name"] == role["metadata"]["name"]
    granted = {(group, resource) for rule in role["rules"] for group in rule["apiGroups"] for resource in rule["resources"]}
    assert granted == {
        ("", "serviceaccounts"), ("", "configmaps"), ("", "services"), ("apps", "deployments"),
        ("cert-manager.io", "certificates"), ("traefik.io", "ingressroutetcps"),
        ("traefik.io", "middlewaretcps"),
        ("policy.linkerd.io", "servers"), ("policy.linkerd.io", "httproutes"),
        ("policy.linkerd.io", "meshtlsauthentications"), ("policy.linkerd.io", "authorizationpolicies"),
    }
    for rule in role["rules"]:
        assert "*" not in rule["verbs"] and "escalate" not in rule["verbs"] and "bind" not in rule["verbs"]
    # PF-129, T-5870: the reconciler applies, lists and retires a broker's allowedSources middleware.
    traefik = next(rule for rule in role["rules"] if "middlewaretcps" in rule["resources"])
    assert set(traefik["verbs"]) == {"get", "list", "create", "patch", "delete"}


PORTAL = {"portal": {"namespace": "jc-portal", "serviceAccount": "portal"}}


def test_the_portal_alone_reads_two_routes_of_the_http_api_under_the_mesh(tmp_path):
    """ADR-N-048 §2.8, T-4043: 6060 admits the Portal's identity on two GET routes; the API's
    publish, subscribe and kick routes match nothing, and the API holds no token."""
    result = helm({**BASE, **PORTAL}, tmp_path)
    assert result.returncode == 0, result.stderr
    rendered = [d for d in yaml.safe_load_all(result.stdout) if d]
    for release in RELEASES:
        files = config(rendered, release)
        assert files["rmqtt.toml"]["plugins"]["disabled_default_startups"] == []
        api = files["rmqtt-http-api.toml"]
        assert api["http_laddr"] == "0.0.0.0:6060" and "http_bearer_token" not in api
        server = of(rendered, "Server")[f"{release}-api"]["spec"]
        assert (server["port"], server["proxyProtocol"], server["accessPolicy"]) == (6060, "HTTP/1", "deny")
        route = of(rendered, "HTTPRoute")[f"{release}-api"]["spec"]
        assert route["parentRefs"] == [{"group": "policy.linkerd.io", "kind": "Server", "name": f"{release}-api"}]
        matches = [m for rule in route["rules"] for m in rule["matches"]]
        assert {(m["method"], m["path"]["type"], m["path"]["value"]) for m in matches} == {
            ("GET", "Exact", "/api/v1/metrics/history/sum"), ("GET", "Exact", "/api/v1/clients")}
        assert of(rendered, "MeshTLSAuthentication")[f"{release}-api"]["spec"]["identityRefs"] == [
            {"kind": "ServiceAccount", "name": "portal", "namespace": "jc-portal"}]
        policy = of(rendered, "AuthorizationPolicy")[f"{release}-api"]["spec"]
        assert policy["targetRef"] == {"group": "policy.linkerd.io", "kind": "HTTPRoute", "name": f"{release}-api"}
        ports = {p["port"] for p in of(rendered, "Service")[release]["spec"]["ports"]}
        assert ports == {1883, 1884, 6060}
    # The Portal stamps the same objects from the template.
    template = next(d for d in rendered if d["kind"] == "ConfigMap" and d["metadata"]["name"] == TEMPLATE)
    stamped = [d for d in yaml.safe_load_all(template["data"]["objects.yaml"]) if d]
    assert {d["kind"] for d in stamped if d["metadata"]["name"] == "__RELEASE__-api"} == {
        "Server", "HTTPRoute", "MeshTLSAuthentication", "AuthorizationPolicy"}


@pytest.mark.parametrize("values", [
    {**PORTAL, "serviceMesh": {"enabled": False}},
    {},
], ids=["no mesh", "no portal"])
def test_without_the_mesh_or_the_portal_the_http_api_stays_off(tmp_path, values):
    result = helm({**BASE, **values}, tmp_path)
    assert result.returncode == 0, result.stderr
    rendered = [d for d in yaml.safe_load_all(result.stdout) if d]
    for release in RELEASES:
        files = config(rendered, release)
        assert files["rmqtt.toml"]["plugins"]["disabled_default_startups"] == ["rmqtt-http-api"]
        assert "rmqtt-http-api.toml" not in files
        assert {p["port"] for p in of(rendered, "Service")[release]["spec"]["ports"]} == {1883, 1884}
    assert not [d for d in rendered if d["kind"] == "HTTPRoute"]


# The environment around the brokers.

@pytest.fixture(scope="module")
def local(rendered):
    return rendered("local")


def one(docs, kind, name):
    found = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert len(found) == 1, f"expected exactly one {kind}/{name}, got {len(found)}"
    return found[0]


def env_of(deployment):
    container = deployment["spec"]["template"]["spec"]["containers"][0]
    return {e["name"]: e.get("value") for e in container.get("env", [])}


def test_the_gateway_serves_its_internal_listener_on_8082(local):
    gateway = one(local, "Deployment", "context-gateway")
    assert env_of(gateway)["JC_GATEWAY_INTERNAL_BIND"] == "0.0.0.0:8082"
    ports = {p["name"]: p["port"] for p in one(local, "Service", "context-gateway")["spec"]["ports"]}
    assert ports["internal"] == 8082


def test_the_runner_knows_the_brokers_namespace(local):
    runner = one(local, "Deployment", "pipeline-runner")
    namespace = env_of(runner)["JC_MQTT_NAMESPACE"]
    assert namespace
    policy = one(local, "NetworkPolicy", "mqtt-brokers")
    assert policy["metadata"]["namespace"] == namespace


def test_only_brokers_reach_8082_and_only_the_runner_and_traefik_reach_a_broker(local):
    brokers = one(local, "NetworkPolicy", "mqtt-brokers")["spec"]
    by_port = {rule["ports"][0]["port"]: rule["from"] for rule in brokers["ingress"]}
    assert set(by_port) == {1883, 1884, 6060}
    assert by_port[6060][0]["podSelector"]["matchLabels"] == {"app.kubernetes.io/name": "portal-portal"}
    egress = one(local, "NetworkPolicy", "portal-to-mqtt-brokers")["spec"]
    assert [p["port"] for rule in egress["egress"] for p in rule["ports"]] == [6060, 1883]
    assert [peer["podSelector"]["matchLabels"] for peer in by_port[1883]] == [
        {"app.kubernetes.io/name": "pipeline-runner-runner"}, {"app.kubernetes.io/name": "portal-portal"}]
    assert by_port[1884][0]["podSelector"]["matchLabels"] == {"app.kubernetes.io/name": "traefik"}
    (traefik,) = [rule for rule in brokers["ingress"] if rule["ports"][0]["port"] == 1884]
    assert [p["port"] for p in traefik["ports"]] == [1884, 8884]
    internal = one(local, "NetworkPolicy", "gateway-from-mqtt-brokers")["spec"]
    (rule,) = internal["ingress"]
    assert rule["ports"] == [{"protocol": "TCP", "port": 8082}]
    assert rule["from"][0]["podSelector"]["matchLabels"] == {"app.kubernetes.io/name": "mqtt-broker"}
    # No other policy opens 8082 on the gateway.
    for doc in local:
        if doc.get("kind") != "NetworkPolicy" or doc["metadata"]["name"] == "gateway-from-mqtt-brokers":
            continue
        for rule in doc["spec"].get("ingress") or []:
            assert 8082 not in [p.get("port") for p in rule.get("ports") or []], doc["metadata"]["name"]


def test_the_portal_knows_the_brokers_namespace_and_holds_its_role_there(local):
    portal = one(local, "Deployment", "portal")
    namespace = env_of(portal)["JC_PORTAL_MQTT_NAMESPACE"]
    assert namespace == one(local, "NetworkPolicy", "mqtt-brokers")["metadata"]["namespace"]
    binding = one(local, "RoleBinding", "portal-mqtt-brokers")
    assert binding["metadata"]["namespace"] == namespace
    (subject,) = binding["subjects"]
    assert subject["namespace"] == portal["metadata"]["namespace"]
    assert subject["name"] == portal["spec"]["template"]["spec"]["serviceAccountName"]


def test_the_gateway_admits_the_tokenless_runner_from_its_pods_only(local):
    """T-4004: the gateway resolves the runner's headless Service, which lists every runner pod
    and nothing else; the public listener stays reachable from Traefik alone (the test above)."""
    host = env_of(one(local, "Deployment", "context-gateway"))["JC_GATEWAY_MQTT_RUNNER_HOST"]
    name, namespace = host.split(".")[:2]
    pods = one(local, "Service", name)
    assert pods["metadata"]["namespace"] == namespace
    assert pods["spec"]["clusterIP"] == "None" and pods["spec"]["publishNotReadyAddresses"] is True
    runner = one(local, "Deployment", "pipeline-runner")
    assert pods["spec"]["selector"] == runner["spec"]["selector"]["matchLabels"]
    assert runner["metadata"]["namespace"] == namespace
    # Only the runner asks for one.
    headless = [d["metadata"]["name"] for d in local if d.get("kind") == "Service" and d["spec"].get("clusterIP") == "None"
                and d["metadata"]["name"].startswith("pipeline-runner")]
    assert headless == [name]


def test_the_gateway_admits_the_portals_capture_subscriber_from_its_pods_only(local):
    """ADR-N-048 §2.10, T-4880: the Portal's headless Service lists the Portal's pods alone."""
    host = env_of(one(local, "Deployment", "context-gateway"))["JC_GATEWAY_MQTT_PORTAL_HOST"]
    name, namespace = host.split(".")[:2]
    pods = one(local, "Service", name)
    assert pods["metadata"]["namespace"] == namespace
    assert pods["spec"]["clusterIP"] == "None" and pods["spec"]["publishNotReadyAddresses"] is True
    portal = one(local, "Deployment", "portal")
    assert pods["spec"]["selector"] == portal["spec"]["selector"]["matchLabels"]
    assert portal["metadata"]["namespace"] == namespace


def test_the_gateway_admits_the_runner_from_the_brokers_own_pods(local):
    """T-5893: under the mesh a broker reports its internal listener's clients at its own address,
    so the gateway resolves the brokers' headless Service, which lists every broker pod and nothing
    else; only 1883 forwards without a PROXY header, and its Server admits the runner and the Portal."""
    host = env_of(one(local, "Deployment", "context-gateway"))["JC_GATEWAY_MQTT_BROKER_HOST"]
    name, namespace = host.split(".")[:2]
    pods = one(local, "Service", name)
    assert pods["metadata"]["namespace"] == namespace == one(local, "NetworkPolicy", "mqtt-brokers")["metadata"]["namespace"]
    assert pods["spec"]["clusterIP"] == "None" and pods["spec"]["publishNotReadyAddresses"] is True
    assert pods["spec"]["selector"] == {"app.kubernetes.io/name": "mqtt-broker", "app.kubernetes.io/part-of": "mqtt-brokers"}


def test_the_mqtts_route_uses_the_bsi_tls_option(docs):
    """T-5562: every broker's route on 8883, rendered and in the template the Portal stamps, ends
    TLS with the brokers' TLSOption, whose spec is the HTTP edge's BSI TR-02102-2 one."""
    routes = [d for d in docs if d["kind"] == "IngressRouteTCP"]
    template = of(docs, "ConfigMap") | {TEMPLATE: next(d for d in docs if d["metadata"]["name"] == TEMPLATE)}
    stamped = [d for d in yaml.safe_load_all(template[TEMPLATE]["data"]["objects.yaml"]) if d and d["kind"] == "IngressRouteTCP"]
    assert len(routes) == len(RELEASES) and len(stamped) == 1
    for route in routes + stamped:
        assert route["spec"]["tls"]["options"] == {"name": "mqtt-bsi-tr-02102"}, route["metadata"]["name"]
    option = [d for d in docs if d["kind"] == "TLSOption"]
    assert [(o["metadata"]["name"], o["metadata"]["namespace"]) for o in option] == [("mqtt-bsi-tr-02102", "jc-mqtt")]
    edge = (PROJECT_ROOT / "components/apisix/charts/configuration/templates/edge-tls.yaml").read_text()
    edge = edge[edge.index("kind: TLSOption"):]
    edge = yaml.safe_load(edge[edge.index("spec:"):edge.index("{{- end")])
    assert option[0]["spec"] == edge["spec"]


def test_no_publish_is_cached_and_a_refused_one_disconnects(docs):
    """T-5562: the gateway sends no X-Cache, so the broker asks for every publish; the plugin
    turns a refused one into a disconnect, which is how the gateway's messagesPerSecond holds."""
    for release in RELEASES:
        auth = config(docs, release)["rmqtt-auth-http.toml"]
        assert auth["disconnect_if_pub_rejected"] is True
        assert auth["http_acl_req"]["url"].endswith("/internal/mqtt/acl")


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


class Decider(BaseHTTPRequestHandler):
    """The gateway's two internal routes as a stub: every CONNECT allowed, a watcher's with a
    subscription to the cameras, a device's with none, and every publish allowed until `deny`
    is set, as when the repository drops the device's role, or, with `rate`, until the device
    publishes more than `rate` messages within one second, as the gateway's Rates refuse it.
    Like the gateway, it sends no X-Cache (T-5562)."""

    deny = threading.Event()
    rate: ClassVar[int | None] = None
    connects: ClassVar[list[str]] = []
    addresses: ClassVar[list[str]] = []
    usernames: ClassVar[list[str]] = []
    publishes: ClassVar[list[dict]] = []
    times: ClassVar[list[float]] = []

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["content-length"])))
        if self.path.endswith("/acl"):
            Decider.publishes.append(body)
            now = time.monotonic()
            Decider.times.append(now)
            flood = Decider.rate is not None and sum(now - t < 1 for t in Decider.times) > Decider.rate
            allow = body["access"] == "2" and not Decider.deny.is_set() and not flood
            answer = {"result": "allow" if allow else "deny"}
        else:
            Decider.connects.append(body["clientid"])
            Decider.addresses.append(body["ipaddr"])
            Decider.usernames.append(body["username"])
            watch = [{"permission": "allow", "action": "subscribe", "topic": "cameras/#", "qos": [0, 1]}]
            answer = {"result": "allow", "superuser": False, "expire_at": int(time.time()) + 3600,
                      "acl": watch if body["username"] == "watcher" else []}
        data = json.dumps(answer).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


def image() -> str:
    pinned = yaml.safe_load((PROJECT_ROOT / "components/mqtt-brokers/images.yaml").read_text())["mqtt-brokers"]["brokers"]
    return f"{pinned['repository']}@{pinned['digest']}"


class Rmqtt:
    """The pinned RMQTT with the rendered configuration of one broker on two free local ports,
    asking a Decider stub; mosquitto clients on its internal listener."""

    def __init__(self, tmp_path: Path, limits: dict | None = None, rate: int | None = None):
        self.decider = ThreadingHTTPServer(("127.0.0.1", 0), Decider)
        threading.Thread(target=self.decider.serve_forever, daemon=True).start()
        Decider.deny.clear()
        Decider.rate = rate
        for log in (Decider.connects, Decider.addresses, Decider.publishes, Decider.times):
            log.clear()
        self.internal, external = free_port(), free_port()
        broker = {"project": "helsinki", "name": "cameras"} | ({"limits": limits} if limits else {})
        values = {**BASE, "brokers": [broker],
                  "authUrl": f"http://127.0.0.1:{self.decider.server_port}/internal/mqtt/auth"}
        result = helm(values, tmp_path)
        assert result.returncode == 0, result.stderr
        data = of([d for d in yaml.safe_load_all(result.stdout) if d], "ConfigMap")["mqtt-helsinki-cameras"]["data"]
        (tmp_path / "plugins").mkdir()
        (tmp_path / "rmqtt.toml").write_text(
            data["rmqtt.toml"].replace("0.0.0.0:1883", f"127.0.0.1:{self.internal}")
            .replace("0.0.0.0:1884", f"127.0.0.1:{external}"))
        for name in ("rmqtt-auth-http.toml", "rmqtt-acl.toml"):
            (tmp_path / "plugins" / name).write_text(data[name])
        tmp_path.chmod(0o755)
        self.name = f"t5562-rmqtt-{self.internal}"
        self.clients: list[subprocess.Popen] = []
        self.received: list[float] = []
        started = subprocess.run(
            ["docker", "run", "-d", "--rm", "--name", self.name, "--network", "host", "--read-only",
             "-v", f"{tmp_path}:/etc/rmqtt:ro", image(), "-f", "/etc/rmqtt/rmqtt.toml"],
            capture_output=True, text=True, check=False)
        if started.returncode != 0:
            self.close()
            pytest.fail(started.stderr)

    def __enter__(self):
        deadline = time.monotonic() + 60
        while socket.socket().connect_ex(("127.0.0.1", self.internal)) != 0:
            assert time.monotonic() < deadline, "RMQTT did not listen"
            time.sleep(0.2)
        watcher = self.client("mosquitto_sub", "watcher", "-t", "cameras/#", stdout=subprocess.PIPE, text=True)
        threading.Thread(target=lambda: [self.received.append(time.monotonic()) for _ in watcher.stdout],
                         daemon=True).start()
        time.sleep(1)
        self.device = self.client("mosquitto_pub", "helsinki-cam-1", "-t", "cameras/helsinki-cam-1/frame", "-l",
                                  stdin=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        return self

    def client(self, tool: str, name: str, *args: str, **popen) -> subprocess.Popen:
        process = subprocess.Popen([tool, "-h", "127.0.0.1", "-p", str(self.internal), "-q", "1",
                                    "-i", name, "-u", name, "-P", "a-token", *args], **popen)
        self.clients.append(process)
        return process

    def send(self, seconds: float, every: float):
        """A frame every `every` s for `seconds`; mosquitto_pub reconnects when it is cut."""
        start = time.monotonic()
        while time.monotonic() - start < seconds:
            try:
                self.device.stdin.write("frame\n")
                self.device.stdin.flush()
            except BrokenPipeError:
                return
            time.sleep(every)

    def close(self):
        for client in self.clients:
            client.kill()
        subprocess.run(["docker", "rm", "-f", self.name], capture_output=True, check=False)
        self.decider.shutdown()

    def __exit__(self, *exc):
        self.close()


needs_rmqtt = pytest.mark.skipif(shutil.which("docker") is None or shutil.which("mosquitto_pub") is None,
                                 reason="no container runtime or no mosquitto clients, so RMQTT cannot run")


@pytest.mark.xdist_group("docker-rmqtt")
@needs_rmqtt
def test_a_publish_is_checked_or_the_client_kicked_after_revocation(tmp_path):
    """T-5560, T-5562: the pinned RMQTT with the rendered configuration asks the decider for
    every publish (no X-Cache), and once the decider denies, delivers nothing more of the device
    and disconnects it at each refused publish."""
    with Rmqtt(tmp_path) as broker:
        broker.send(3, 0.2)
        assert len(broker.received) >= 10, "the allowed device's frames did not arrive"
        asked = {(p["access"], p["clientid"], p["topic"]) for p in Decider.publishes}
        assert asked == {("2", "helsinki-cam-1", "cameras/helsinki-cam-1/frame")}, asked
        assert len(Decider.publishes) >= len(broker.received), "a publish passed without a question"
        assert Decider.connects.count("helsinki-cam-1") == 1
        revoked = time.monotonic()
        Decider.deny.set()
        broker.send(10, 0.2)
        last = max(broker.received) - revoked
        assert last < 1, f"the broker delivered the device's frames {last:.1f} s after its role ended"
        assert Decider.connects.count("helsinki-cam-1") > 1, "the refused device was never disconnected"


@pytest.mark.xdist_group("docker-rmqtt")
@needs_rmqtt
def test_a_flooding_publisher_is_throttled(tmp_path):
    """T-5562: a device sending 10x its broker's messagesPerSecond is refused by the decider's
    rate, disconnected by the pinned RMQTT at the first refusal, and the subscriber gets about
    the rate, not the flood."""
    rate = 5
    with Rmqtt(tmp_path, limits={"messagesPerSecond": rate}, rate=rate) as broker:
        seconds = 4
        broker.send(seconds, 1 / (10 * rate))
        time.sleep(1)
        assert Decider.connects.count("helsinki-cam-1") > 1, "the flooding device was never disconnected"
        delivered = len(broker.received)
        assert 0 < delivered <= (seconds + 1) * rate, f"{delivered} frames of a flood at {10 * rate}/s"


@pytest.mark.xdist_group("docker-rmqtt")
@needs_rmqtt
def test_a_device_with_a_1200_byte_token_signs_in_on_a_1024_byte_broker(tmp_path):
    """T-6043: the pinned RMQTT with maxMessageBytes 1024 takes a CONNECT whose password is a
    1200-byte token and delivers its publish, and still drops a packet over the 4096 cap."""
    with Rmqtt(tmp_path, limits={"maxMessageBytes": 1024}) as broker:
        def publish(name: str, message: str) -> subprocess.CompletedProcess:
            return subprocess.run(
                ["mosquitto_pub", "-h", "127.0.0.1", "-p", str(broker.internal), "-q", "1", "-i", name,
                 "-u", name, "-P", "t" * 1200, "-t", f"cameras/{name}/frame", "-m", message],
                capture_output=True, text=True, timeout=20, check=False)
        before = len(broker.received)
        signed = publish("helsinki-cam-2", "frame")
        assert signed.returncode == 0, signed.stderr
        assert "helsinki-cam-2" in Decider.connects
        deadline = time.monotonic() + 5
        while len(broker.received) == before and time.monotonic() < deadline:
            time.sleep(0.1)
        assert len(broker.received) == before + 1, "the token device's frame did not arrive"
        oversized = publish("helsinki-cam-3", "x" * (CONNECT_ALLOWANCE + 100))
        time.sleep(1)
        assert len(broker.received) == before + 1, "a packet over the cap was delivered"
        assert oversized.returncode != 0 or "helsinki-cam-3" not in Decider.connects


def port_docs(tmp_path, port: int = 18830) -> list[dict]:
    brokers = [{**BASE["brokers"][0], "port": port}, *BASE["brokers"][1:]]
    result = helm({**BASE, "brokers": brokers}, tmp_path)
    assert result.returncode == 0, result.stderr
    return [d for d in yaml.safe_load_all(result.stdout) if d]


def test_a_dedicated_port_passes_tls_through_to_the_brokers_own_tls_listener(tmp_path):
    """T-5856: a broker with a port gets its route on the entrypoint `mqtt-<port>`, any SNI
    or none, TLS passed through with PROXY v2 to RMQTT's TLS listener 8884, which serves the
    broker host's certificate; Traefik would answer a client without SNI with its default one.
    A broker without a port keeps only its 8883 route, and no Service leaves ClusterIP."""
    docs = port_docs(tmp_path)
    routes = {d["metadata"]["name"]: d["spec"] for d in docs if d["kind"] == "IngressRouteTCP"}
    assert sorted(routes) == sorted([*RELEASES[1:], "mqtt-helsinki-cameras-port"])
    port = routes["mqtt-helsinki-cameras-port"]
    assert port["entryPoints"] == ["mqtt-18830"]
    (route,) = port["routes"]
    assert route["match"] == "HostSNI(`*`)"
    assert route["services"] == [{"name": "mqtt-helsinki-cameras", "port": 8884, "proxyProtocol": {"version": 2}}]
    assert port["tls"] == {"passthrough": True}
    for name, spec in routes.items():
        assert spec["entryPoints"] == (["mqtt-18830"] if name.endswith("-port") else ["mqtts"]), name
    for d in docs:
        if d["kind"] == "Service":
            assert d["spec"].get("type", "ClusterIP") == "ClusterIP", d["metadata"]["name"]
            ports = [p["port"] for p in d["spec"]["ports"]]
            assert (8884 in ports) == (d["metadata"]["name"] == "mqtt-helsinki-cameras"), d["metadata"]["name"]

    tls = config(docs, "mqtt-helsinki-cameras")["rmqtt.toml"]["listener"]["tls"]["external"]
    assert tls["enable"] is True and tls["addr"] == "0.0.0.0:8884" and tls["proxy_protocol"] is True
    assert (tls["cert"], tls["key"]) == ("/etc/rmqtt-tls/tls.crt", "/etc/rmqtt-tls/tls.key")
    assert tls["allow_anonymous"] is False and tls["max_qos_allowed"] == 1 and tls["cross_certificate"] is False
    for release in RELEASES[1:]:
        assert config(docs, release)["rmqtt.toml"]["listener"]["tls"]["external"]["enable"] is False

    deployment = of(docs, "Deployment")["mqtt-helsinki-cameras"]["spec"]["template"]
    pod = deployment["spec"]
    secret = next(v for v in pod["volumes"] if v["name"] == "tls")["secret"]
    assert secret["secretName"] == "mqtt-helsinki-cameras-tls"
    assert [i["key"] for i in secret["items"]] == ["tls.crt", "tls.key"]
    assert {"name": "tls", "mountPath": "/etc/rmqtt-tls", "readOnly": True} in pod["containers"][0]["volumeMounts"]
    annotations = deployment["metadata"]["annotations"]
    assert annotations["config.linkerd.io/opaque-ports"] == "1883,8884"
    assert annotations["joinedcontext.com/tls-revision"] == ""
    other = of(docs, "Deployment")["mqtt-helsinki-buses"]["spec"]["template"]
    assert "joinedcontext.com/tls-revision" not in other["metadata"]["annotations"]
    assert [v["name"] for v in other["spec"]["volumes"]] == ["config"]

    # 8884 admits the ingress controller alone, as 1884 does on a broker without a port.
    server = of(docs, "Server")["mqtt-helsinki-cameras-tls"]["spec"]
    assert server["port"] == 8884 and server["accessPolicy"] == "deny"
    assert of(docs, "MeshTLSAuthentication")["mqtt-helsinki-cameras-tls"]["spec"]["identityRefs"] == [
        {"kind": "ServiceAccount", "name": "traefik", "namespace": "traefik"}]
    assert "mqtt-helsinki-buses-tls" not in of(docs, "Server")


def outside_routes(docs, release) -> list[str]:
    """The Traefik entrypoints that reach one broker from the internet."""
    return sorted(
        entry for d in docs if d["kind"] == "IngressRouteTCP"
        and d["metadata"]["labels"]["app.kubernetes.io/instance"] == release
        for entry in d["spec"]["entryPoints"])


def test_a_broker_has_one_outside_door(tmp_path):
    """T-5871 (owner: "from one mqtt broker only the safe port will go out"): a broker with a
    port is reached on that port alone, with no 8883 route by hostname and no plain listener
    1884 behind one; a broker without a port keeps 8883 alone."""
    docs = port_docs(tmp_path, 18835)
    assert outside_routes(docs, "mqtt-helsinki-cameras") == ["mqtt-18835"]
    for release in RELEASES[1:]:
        assert outside_routes(docs, release) == ["mqtts"], release
    listeners = config(docs, "mqtt-helsinki-cameras")["rmqtt.toml"]["listener"]
    assert listeners["tcp"]["external"] == {"enable": False}
    assert listeners["tcp"]["internal"]["addr"] == "0.0.0.0:1883"
    assert {p["port"] for p in of(docs, "Service")["mqtt-helsinki-cameras"]["spec"]["ports"]} == {1883, 8884}
    (container,) = of(docs, "Deployment")["mqtt-helsinki-cameras"]["spec"]["template"]["spec"]["containers"]
    assert 1884 not in {p["containerPort"] for p in container["ports"]}
    assert "mqtt-helsinki-cameras-external" not in of(docs, "Server")
    assert {s["spec"]["port"] for n, s in of(docs, "Server").items() if n.startswith("mqtt-helsinki-cameras-")} == {1883, 8884}
    assert of(docs, "Server")["mqtt-helsinki-buses-external"]["spec"]["port"] == 1884


def test_the_port_template_filled_is_the_broker_with_a_port_the_chart_renders(tmp_path):
    """The Portal stamps `objects-port.yaml` for a broker with a port: filled, it is what an entry
    with that port renders, and its Certificate revision lands in the pod annotation that rolls
    the broker on a renewal."""
    docs = port_docs(tmp_path)
    (template,) = [d for d in docs if d["kind"] == "ConfigMap" and d["metadata"]["name"] == TEMPLATE]
    text = template["data"]["objects-port.yaml"].replace("__PORT__", "18830")
    limits = {"maxMessageBytes": 65536, "messagesPerSecond": 10, "maxConnections": 1000}
    stamped = fill(text.replace("__TLS_REVISION__", ""), "mqtt-helsinki-cameras", "helsinki", "cameras", limits)
    rendered = [d for d in docs if d["metadata"].get("labels", {}).get("app.kubernetes.io/instance") == "mqtt-helsinki-cameras"]
    assert comparable(stamped) == comparable(rendered)
    renewed = fill(text.replace("__TLS_REVISION__", "3"), "mqtt-helsinki-cameras", "helsinki", "cameras", limits)
    (deployment,) = [d for d in renewed if d["kind"] == "Deployment"]
    assert deployment["spec"]["template"]["metadata"]["annotations"]["joinedcontext.com/tls-revision"] == "3"
    assert "8884" not in template["data"]["objects.yaml"] and "__PORT__" not in template["data"]["objects.yaml"]


@pytest.mark.parametrize(("ports", "message"), [
    ([18845], "outside the served range 18830-18839"),
    ([1883], "outside the served range 18830-18839"),
    ([18831, 18831], "two brokers hold port 18831"),
])
def test_a_port_outside_the_served_pool_or_held_twice_refuses_to_render(tmp_path, ports, message):
    brokers = [{**b, "port": p} for b, p in zip(BASE["brokers"], ports)]
    result = helm({**BASE, "brokers": brokers}, tmp_path)
    assert result.returncode != 0 and message in result.stderr, result.stderr


def proxy_v2(source: str, port: int) -> bytes:
    """The PROXY v2 header Traefik writes before the client's bytes (TCP over IPv4)."""
    addresses = ipaddress.IPv4Address(source).packed + ipaddress.IPv4Address("10.0.0.1").packed
    return b"\r\n\r\n\x00\r\nQUIT\n" + bytes([0x21, 0x11]) + struct.pack("!H", 12) + addresses + struct.pack("!HH", port, 8884)


# The TLSOption's suites (templates/tls-option.yaml) in OpenSSL names, and the TLS 1.2 ChaCha20
# suites rustls adds with no knob to drop them: AEAD with ECDHE like the rest (decided in T-5856).
TLS12_ALLOWED = {"ECDHE-ECDSA-AES128-GCM-SHA256", "ECDHE-ECDSA-AES256-GCM-SHA384", "ECDHE-RSA-AES128-GCM-SHA256",
                 "ECDHE-RSA-AES256-GCM-SHA384", "ECDHE-ECDSA-CHACHA20-POLY1305", "ECDHE-RSA-CHACHA20-POLY1305"}


@pytest.mark.xdist_group("docker-rmqtt")
@pytest.mark.skipif(shutil.which("docker") is None or shutil.which("openssl") is None, reason="no container runtime")
def test_the_dedicated_tls_listener_serves_the_broker_certificate_bsi_suites_and_the_client_address(tmp_path):
    """T-5856 option (a): the rendered 8884 listener of the pinned RMQTT, reached as Traefik's
    passthrough reaches it (PROXY v2, then TLS, no SNI), serves the mounted certificate, refuses
    TLS 1.0/1.1 and every suite outside TLS12_ALLOWED, and hands the gateway the address of the
    PROXY header on CONNECT."""
    decider = ThreadingHTTPServer(("127.0.0.1", 0), Decider)
    threading.Thread(target=decider.serve_forever, daemon=True).start()
    Decider.rate = None
    Decider.deny.clear()
    for log in (Decider.connects, Decider.addresses):
        log.clear()
    values = {**BASE, "brokers": [{"project": "helsinki", "name": "cameras", "port": 18830}],
              "authUrl": f"http://127.0.0.1:{decider.server_port}/internal/mqtt/auth"}
    result = helm(values, tmp_path)
    assert result.returncode == 0, result.stderr
    data = of([d for d in yaml.safe_load_all(result.stdout) if d], "ConfigMap")["mqtt-helsinki-cameras"]["data"]
    internal, external, tls = free_port(), free_port(), free_port()
    conf, certs = tmp_path / "conf", tmp_path / "tls"
    (conf / "plugins").mkdir(parents=True)
    certs.mkdir()
    (conf / "rmqtt.toml").write_text(
        data["rmqtt.toml"].replace("0.0.0.0:1883", f"127.0.0.1:{internal}")
        .replace("0.0.0.0:1884", f"127.0.0.1:{external}").replace("0.0.0.0:8884", f"127.0.0.1:{tls}"))
    for name in ("rmqtt-auth-http.toml", "rmqtt-acl.toml"):
        (conf / "plugins" / name).write_text(data[name])
    subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                    "-subj", "/CN=cameras.helsinki.mqtt.dev.example.org",
                    "-keyout", str(certs / "tls.key"), "-out", str(certs / "tls.crt")], capture_output=True, check=True)
    for path in (tmp_path, conf, conf / "plugins", certs):
        path.chmod(0o755)
    for path in (*certs.iterdir(), *conf.rglob("*.toml")):
        path.chmod(0o644)
    name = f"t5856-rmqtt-{tls}"
    started = subprocess.run(
        ["docker", "run", "-d", "--rm", "--name", name, "--network", "host", "--read-only",
         "-v", f"{conf}:/etc/rmqtt:ro", "-v", f"{certs}:/etc/rmqtt-tls:ro", image(), "-f", "/etc/rmqtt/rmqtt.toml"],
        capture_output=True, text=True, check=False)
    try:
        assert started.returncode == 0, started.stderr
        deadline = time.monotonic() + 60
        while socket.socket().connect_ex(("127.0.0.1", tls)) != 0:
            assert time.monotonic() < deadline, "RMQTT did not listen on its TLS port"
            time.sleep(0.2)

        def handshake(version=None, ciphers=None) -> ssl.SSLSocket:
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
            context.check_hostname = False
            context.verify_mode = ssl.CERT_NONE
            if version:
                context.minimum_version = context.maximum_version = version
            context.set_ciphers((ciphers or "ALL") + ":@SECLEVEL=0")
            raw = socket.create_connection(("127.0.0.1", tls), timeout=5)
            raw.sendall(proxy_v2("203.0.113.7", 40000))
            return context.wrap_socket(raw)  # no server_hostname: no SNI, as a bare device

        with handshake() as session:
            served = ssl.DER_cert_to_PEM_cert(session.getpeercert(binary_form=True))
            assert served.strip() == (certs / "tls.crt").read_text().strip()
            # MQTT 3.1.1 CONNECT, client id and username "cam-1", password "a-token".
            payload = b"\x00\x04MQTT\x04\xc2\x00\x3c" + b"".join(
                struct.pack("!H", len(v)) + v for v in (b"cam-1", b"cam-1", b"a-token"))
            session.sendall(b"\x10" + bytes([len(payload)]) + payload)
            assert session.recv(4) == b"\x20\x02\x00\x00"
        assert Decider.addresses == ["203.0.113.7"]

        for old in (ssl.TLSVersion.TLSv1, ssl.TLSVersion.TLSv1_1):
            with pytest.raises((ssl.SSLError, ConnectionError)):
                handshake(old).close()
        offered = [c["name"] for c in ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT).get_ciphers() if c["protocol"] == "TLSv1.2"]
        offered += ["ECDHE-RSA-AES128-SHA", "ECDHE-RSA-AES256-SHA384", "AES128-GCM-SHA256", "DHE-RSA-AES128-GCM-SHA256"]
        accepted = set()
        for cipher in offered:
            try:
                with handshake(ssl.TLSVersion.TLSv1_2, cipher) as session:
                    accepted.add(session.cipher()[0])
            except (ssl.SSLError, ConnectionError):
                pass
        assert accepted <= TLS12_ALLOWED, accepted - TLS12_ALLOWED
        assert {"ECDHE-RSA-AES128-GCM-SHA256", "ECDHE-RSA-AES256-GCM-SHA384"} <= accepted
        with handshake(ssl.TLSVersion.TLSv1_3) as session:
            assert session.version() == "TLSv1.3"
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)
        decider.shutdown()


CA = {"name": "helsinki-device-ca", "key": "ca.crt"}
CERT_RELEASE = "mqtt-helsinki-cameras-cert"
CERT_HOST = "cert.cameras.helsinki.mqtt.dev.example.org"


def cert_docs(tmp_path) -> list[dict]:
    brokers = [{**BASE["brokers"][0], "clientCa": CA}, *BASE["brokers"][1:]]
    result = helm({**BASE, "brokers": brokers}, tmp_path)
    assert result.returncode == 0, result.stderr
    return [d for d in yaml.safe_load_all(result.stdout) if d]


def test_a_certificate_door_is_a_release_of_its_own_that_asks_for_the_project_ca(tmp_path):
    """T-5875 (ADR-N-048 §2.12, option (a) of T-4861): a broker whose devices may use client
    certificates gets a second release `<release>-cert`, with its own ServiceAccount and so its
    own mesh identity, which is how the gateway tells a certificate device from a token one. It
    serves devices only: TLS in RMQTT on 8884 behind a passthrough route on 8883 by its own SNI,
    a client certificate the project CA signed required, the subject DN as the username, and no
    internal listener, no HTTP API. The broker's own release keeps its token door unchanged."""
    docs = cert_docs(tmp_path)
    assert of(docs, "ServiceAccount")[CERT_RELEASE]["metadata"]["name"] == CERT_RELEASE
    pod = of(docs, "Deployment")[CERT_RELEASE]["spec"]["template"]
    assert pod["spec"]["serviceAccountName"] == CERT_RELEASE
    assert pod["metadata"]["annotations"]["config.linkerd.io/opaque-ports"] == "8884"
    assert pod["metadata"]["annotations"]["joinedcontext.com/tls-revision"] == ""
    (container,) = pod["spec"]["containers"]
    assert [p["containerPort"] for p in container["ports"]] == [8884]
    assert {"name": "client-ca", "mountPath": "/etc/rmqtt-ca", "readOnly": True} in container["volumeMounts"]
    ca = next(v for v in pod["spec"]["volumes"] if v["name"] == "client-ca")["secret"]
    assert ca == {"secretName": "helsinki-device-ca", "defaultMode": 0o444, "items": [{"key": "ca.crt", "path": "ca.crt"}]}
    assert next(v for v in pod["spec"]["volumes"] if v["name"] == "tls")["secret"]["secretName"] == f"{CERT_RELEASE}-tls"

    toml = config(docs, CERT_RELEASE)
    listeners = toml["rmqtt.toml"]["listener"]
    assert listeners["tcp"] == {"internal": {"enable": False}, "external": {"enable": False}}
    tls = listeners["tls"]["external"]
    assert tls["enable"] is True and tls["addr"] == "0.0.0.0:8884" and tls["proxy_protocol"] is True
    assert tls["cross_certificate"] is True and tls["client_ca_certs"] == "/etc/rmqtt-ca/ca.crt"
    assert tls["cert_subject_dn_as_username"] is True and "cert_cn_as_username" not in tls
    assert tls["allow_anonymous"] is False
    assert "rmqtt-http-api.toml" not in toml
    assert toml["rmqtt.toml"]["plugins"]["disabled_default_startups"] == ["rmqtt-http-api"]
    assert toml["rmqtt-auth-http.toml"]["http_auth_req"]["params"]["password"] == "%P"
    assert [p["port"] for p in of(docs, "Service")[CERT_RELEASE]["spec"]["ports"]] == [8884]

    route = of(docs, "IngressRouteTCP")[CERT_RELEASE]["spec"]
    assert route["entryPoints"] == ["mqtts"] and route["tls"] == {"passthrough": True}
    assert route["routes"] == [{"match": f"HostSNI(`{CERT_HOST}`)",
                                "services": [{"name": CERT_RELEASE, "port": 8884, "proxyProtocol": {"version": 2}}]}]
    assert of(docs, "Certificate")[CERT_RELEASE]["spec"]["dnsNames"] == [CERT_HOST]
    servers = {n: s["spec"]["port"] for n, s in of(docs, "Server").items() if n.startswith(CERT_RELEASE)}
    assert servers == {f"{CERT_RELEASE}-tls": 8884}
    assert of(docs, "MeshTLSAuthentication")[f"{CERT_RELEASE}-tls"]["spec"]["identityRefs"] == [
        {"kind": "ServiceAccount", "name": "traefik", "namespace": "traefik"}]
    assert outside_routes(docs, "mqtt-helsinki-cameras") == ["mqtts"]
    assert of(docs, "IngressRouteTCP")["mqtt-helsinki-cameras"]["spec"]["routes"][0]["services"][0]["port"] == 1884
    for release in RELEASES[1:]:
        assert f"{release}-cert" not in of(docs, "Deployment"), release


def test_the_certificate_template_filled_is_the_certificate_door_the_chart_renders(tmp_path):
    """The Portal stamps `objects-certificate.yaml` beside the broker's objects: filled with the
    door's release, host and the allowance's clientCaRef, it is what the chart renders."""
    docs = cert_docs(tmp_path)
    (template,) = [d for d in docs if d["kind"] == "ConfigMap" and d["metadata"]["name"] == TEMPLATE]
    text = template["data"]["objects-certificate.yaml"]
    for sentinel, value in (("__CLIENT_CA__", CA["name"]),
                            ("__CLIENT_CA_KEY__", CA["key"]), ("__TLS_REVISION__", "")):
        text = text.replace(sentinel, value)
    limits = {"maxMessageBytes": 65536, "messagesPerSecond": 10, "maxConnections": 1000}
    stamped = fill(text, CERT_RELEASE, "helsinki", "cameras", limits)
    rendered = [d for d in docs if d["metadata"].get("labels", {}).get("app.kubernetes.io/instance") == CERT_RELEASE]
    assert comparable(stamped) == comparable(rendered)
    assert "client_ca_certs" not in template["data"]["objects.yaml"]


V3_LEAF = "basicConstraints = CA:FALSE\nsubjectKeyIdentifier = hash\nauthorityKeyIdentifier = keyid\n"


def issue(directory: Path, name: str, subject: str, ca: str | None, *extra: str) -> None:
    """A P-256 key and certificate `name`, self-signed when `ca` is None, else signed by it."""
    key, crt = directory / f"{name}.key", directory / f"{name}.crt"
    curve = ["-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes", "-keyout", str(key)]
    if ca is None:
        command = ["openssl", "req", "-x509", *curve, "-days", "2", "-subj", subject, "-out", str(crt)]
    else:
        csr = directory / f"{name}.csr"
        subprocess.run(["openssl", "req", *curve, "-subj", subject, "-out", str(csr)], capture_output=True, check=True)
        # OpenSSL 3.0 (the CI runner) issues a v1 certificate without -extfile, which rustls refuses.
        extensions = directory / f"{name}.ext"
        extensions.write_text(V3_LEAF)
        command = ["openssl", "x509", "-req", "-in", str(csr), "-CA", str(directory / f"{ca}.crt"),
                   "-CAkey", str(directory / f"{ca}.key"), "-CAcreateserial", "-extfile", str(extensions),
                   "-out", str(crt), *extra]
    subprocess.run(command, capture_output=True, check=True)


def issue_expired(directory: Path, name: str, subject: str, ca: str) -> None:
    """A certificate of `ca` valid only in January 2020. `openssl x509 -not_after` needs OpenSSL
    3.4; the CI runner has 3.0, so this goes through `openssl ca -startdate -enddate`."""
    key, csr, crt = directory / f"{name}.key", directory / f"{name}.csr", directory / f"{name}.crt"
    subprocess.run(["openssl", "req", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:P-256", "-nodes",
                    "-keyout", str(key), "-subj", subject, "-out", str(csr)], capture_output=True, check=True)
    db = directory / f"{name}.ca"
    db.mkdir()
    (db / "index.txt").write_text("")
    (db / "serial").write_text("01\n")
    (db / "ca.cnf").write_text(
        f"[ca]\ndefault_ca = d\n[d]\ndatabase = {db}/index.txt\nnew_certs_dir = {db}\nserial = {db}/serial\n"
        "default_md = sha256\npolicy = p\nunique_subject = no\nx509_extensions = v3\n"
        f"[p]\norganizationName = optional\ncommonName = supplied\n[v3]\n{V3_LEAF}")
    subprocess.run(["openssl", "ca", "-batch", "-notext", "-config", str(db / "ca.cnf"),
                    "-cert", str(directory / f"{ca}.crt"), "-keyfile", str(directory / f"{ca}.key"),
                    "-startdate", "20200101000000Z", "-enddate", "20200201000000Z",
                    "-in", str(csr), "-out", str(crt)], capture_output=True, check=True)


@pytest.mark.xdist_group("docker-rmqtt")
@pytest.mark.skipif(shutil.which("docker") is None or shutil.which("openssl") is None, reason="no container runtime")
def test_the_certificate_door_admits_only_a_valid_certificate_of_the_project_ca(tmp_path):
    """T-5875: the rendered certificate door of the pinned RMQTT, reached as Traefik's passthrough
    reaches it (PROXY v2, then TLS). A device certificate the project CA signed reaches the
    gateway with the subject DN as its username, whatever username the device sent, and no
    password; a device with no certificate, another CA's or an expired one is refused in the
    handshake and never reaches the gateway. RMQTT 0.24 has no CRL setting: a revoked device is
    refused by the gateway on its ServiceAccount (ADR-N-048 §2.12)."""
    decider = ThreadingHTTPServer(("127.0.0.1", 0), Decider)
    threading.Thread(target=decider.serve_forever, daemon=True).start()
    Decider.rate = None
    Decider.deny.clear()
    for log in (Decider.connects, Decider.addresses, Decider.usernames):
        log.clear()
    brokers = [{"project": "helsinki", "name": "cameras", "clientCa": CA}]
    values = {**BASE, "brokers": brokers, "authUrl": f"http://127.0.0.1:{decider.server_port}/internal/mqtt/auth"}
    result = helm(values, tmp_path)
    assert result.returncode == 0, result.stderr
    data = of([d for d in yaml.safe_load_all(result.stdout) if d], "ConfigMap")[CERT_RELEASE]["data"]
    tls = free_port()
    conf, certs, ca, keys = tmp_path / "conf", tmp_path / "tls", tmp_path / "ca", tmp_path / "keys"
    for directory in (conf / "plugins", certs, ca, keys):
        directory.mkdir(parents=True)
    (conf / "rmqtt.toml").write_text(data["rmqtt.toml"].replace("0.0.0.0:8884", f"127.0.0.1:{tls}"))
    for name in ("rmqtt-auth-http.toml", "rmqtt-acl.toml"):
        (conf / "plugins" / name).write_text(data[name])
    issue(keys, "project-ca", "/CN=helsinki device CA", None)
    issue(keys, "other-ca", "/CN=praha device CA", None)
    issue(keys, "good", "/O=helsinki/CN=sensor-1", "project-ca", "-days", "1")
    issue(keys, "other", "/O=helsinki/CN=sensor-1", "other-ca", "-days", "1")
    issue_expired(keys, "expired", "/O=helsinki/CN=sensor-1", "project-ca")
    issue(keys, "server", f"/CN={CERT_HOST}", "project-ca", "-days", "1")
    shutil.copy(keys / "server.crt", certs / "tls.crt")
    shutil.copy(keys / "server.key", certs / "tls.key")
    shutil.copy(keys / "project-ca.crt", ca / "ca.crt")
    for path in (tmp_path, conf, conf / "plugins", certs, ca):
        path.chmod(0o755)
    for path in (*certs.iterdir(), *ca.iterdir(), *conf.rglob("*.toml")):
        path.chmod(0o644)
    name = f"t5875-rmqtt-{tls}"
    started = subprocess.run(
        ["docker", "run", "-d", "--rm", "--name", name, "--network", "host", "--read-only",
         "-v", f"{conf}:/etc/rmqtt:ro", "-v", f"{certs}:/etc/rmqtt-tls:ro", "-v", f"{ca}:/etc/rmqtt-ca:ro",
         image(), "-f", "/etc/rmqtt/rmqtt.toml"],
        capture_output=True, text=True, check=False)
    try:
        assert started.returncode == 0, started.stderr
        deadline = time.monotonic() + 60
        while socket.socket().connect_ex(("127.0.0.1", tls)) != 0:
            if time.monotonic() > deadline:
                logs = subprocess.run(["docker", "logs", name], capture_output=True, text=True, check=False)
                pytest.fail(f"RMQTT did not listen on its TLS port: {logs.stdout}{logs.stderr}")
            time.sleep(0.2)

        def connect(client: str | None) -> bytes:
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
            context.load_verify_locations(keys / "project-ca.crt")
            if client:
                context.load_cert_chain(keys / f"{client}.crt", keys / f"{client}.key")
            raw = socket.create_connection(("127.0.0.1", tls), timeout=5)
            raw.sendall(proxy_v2("203.0.113.9", 40001))
            with context.wrap_socket(raw, server_hostname=CERT_HOST) as session:
                # MQTT 3.1.1 CONNECT, client id "helsinki-sensor-1", username "spoofed", no password.
                payload = b"\x00\x04MQTT\x04\x82\x00\x3c" + b"".join(
                    struct.pack("!H", len(v)) + v for v in (b"helsinki-sensor-1", b"spoofed"))
                session.sendall(b"\x10" + bytes([len(payload)]) + payload)
                return session.recv(4)

        assert connect("good") == b"\x20\x02\x00\x00"
        assert Decider.usernames == ["O=helsinki, CN=sensor-1"]
        assert Decider.connects == ["helsinki-sensor-1"] and Decider.addresses == ["203.0.113.9"]
        for refused in (None, "other", "expired"):
            with pytest.raises((ssl.SSLError, ConnectionError)):
                connect(refused)
        assert Decider.connects == ["helsinki-sensor-1"]
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True, check=False)
        decider.shutdown()


def test_a_stamped_broker_certificate_names_the_environments_issuer(local):
    # The Portal stamps this template; a hard-coded letsencrypt-prod left every broker on a
    # self-signed cluster waiting for a Secret no issuer could fill (T-6438).
    template = one(local, "ConfigMap", "broker-template")["data"]
    certificates = [f for f in template.values() if "kind: Certificate" in f]
    assert certificates
    assert all('name: "selfsigned-ca"' in f for f in certificates)
