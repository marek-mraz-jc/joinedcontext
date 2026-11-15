"""The dashboards add-on: what the render has to prove before anyone applies it (T-0039).

An add-on is opt-in per environment, so this one is rendered in `addons` — the environment
that exists to render add-ons — and what is asserted here is everything that is decidable
without a cluster: that the component refuses to run without a Prometheus, that the dashboards
reach the pod, that the login is the realm's, and that the edge may actually reach it.
"""

import json
import os
import re
import shutil
import subprocess
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest
import yaml

PROJECT_ROOT = Path(__file__).resolve().parent.parent
COMPONENT = PROJECT_ROOT / "components/grafana"
# What a render needs, copied into a temporary tree the way test_networkpolicies does, so a
# test that changes an environment value changes nobody else's render.
TREE = ("components", "defaults", "ci", "scripts", "helmfile-root.yaml.gotmpl",
        "helmfile-components.yaml.gotmpl")

requires_helmfile = pytest.mark.skipif(
    shutil.which("helmfile") is None, reason="helmfile not installed"
)


@pytest.fixture(scope="module")
def addons(rendered):
    return rendered("addons")


def by_name(docs, kind, name):
    match = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert match, f"{kind}/{name} is not in the render"
    return match[0]


def container(docs):
    return by_name(docs, "Deployment", "grafana")["spec"]["template"]["spec"]["containers"][0]


def test_the_workload_the_route_names_is_the_workload_that_renders(addons):
    """docs Deployment/10 section 2 publishes `grafana:3000`, and one number has to hold from
    the container port to the Service to the route's upstream: the edge answers 502 for a
    Service that listens somewhere else, with the pod healthy."""
    service = by_name(addons, "Service", "grafana")["spec"]
    # The target is the container port by name, so the two are checked against each other
    # rather than against a number written twice.
    assert [p["port"] for p in service["ports"]] == [3000]
    target = service["ports"][0]["targetPort"]
    named = {p["name"]: p["containerPort"] for p in container(addons)["ports"]}
    assert named[target] == 3000, named

    config = yaml.safe_load(
        by_name(addons, "ConfigMap", "apisix-standalone-base")["data"]["apisix.yaml"]
    )
    route = next(r for r in config["routes"] if r["id"] == "grafana")
    assert route["uri"] == "/grafana*"
    upstream = next(u for u in config["upstreams"] if u["id"] == "grafana")
    assert list(upstream["nodes"]) == ["grafana.addons.svc.cluster.local:3000"]


def test_the_edge_is_allowed_to_reach_it(addons):
    """APISIX runs under a default-deny egress and its own component names only the upstreams
    every installation has. An add-on brings its own line, or the route resolves and the
    connection is dropped on the way out: 504 at the edge with the pod 1/1 (T-0419)."""
    policy = by_name(addons, "NetworkPolicy", "apisix-egress-to-grafana")["spec"]
    peers = [
        (peer["podSelector"]["matchLabels"], port["port"])
        for rule in policy["egress"]
        for peer in rule["to"]
        for port in rule["ports"]
    ]
    assert any(
        labels.get("app.kubernetes.io/name") == "grafana" and port == 3000
        for labels, port in peers
    ), peers

    meshed = by_name(addons, "NetworkPolicy", "apisix-linkerd-egress-to-grafana")["spec"]
    assert any(
        port["port"] == 4143 for rule in meshed["egress"] for port in rule["ports"]
    ), "the outbound proxy connects to the peer's inbound proxy, not to the service port"


def test_it_talks_to_prometheus_and_the_realm_and_to_nothing_else(addons):
    """A dashboard viewer reads one Prometheus and logs people in. Anything else it could
    reach is blast radius after a break-in (OPS-38)."""
    egress = by_name(addons, "NetworkPolicy", "grafana")["spec"]["egress"]
    reached = {
        (
            peer.get("podSelector", {}).get("matchLabels", {}).get("app.kubernetes.io/name")
            or peer.get("podSelector", {}).get("matchLabels", {}).get("app.kubernetes.io/instance")
            or peer.get("podSelector", {}).get("matchLabels", {}).get("k8s-app"),
            port["port"],
        )
        for rule in egress
        for peer in rule["to"]
        for port in rule["ports"]
    }
    assert reached == {
        ("kube-dns", 53),
        ("prometheus", 9090),
        ("keycloak-app", 8080),
    }, reached


def test_the_dashboards_are_committed_json_and_reach_the_pod(addons):
    """Seven dashboards, provisioned from files rather than fetched at start-up: the egress
    policy above allows no outbound call that could fetch one, which is the point."""
    dashboards = by_name(addons, "ConfigMap", "grafana-dashboards")["data"]
    assert sorted(dashboards) == [
        "assistant.json",
        "context-broker.json",
        "context-gateway.json",
        "edge-apisix.json",
        "k6-load.json",
        "portal.json",
        "slo.json",
    ]
    for name, raw in dashboards.items():
        parsed = json.loads(raw)
        assert parsed["panels"], f"{name} renders no panel"
        assert parsed["editable"] is False, f"{name} may be edited in the UI"

    pod = by_name(addons, "Deployment", "grafana")["spec"]["template"]["spec"]
    volume = next(v for v in pod["volumes"] if v["name"] == "dashboards-platform")
    assert volume["configMap"]["name"] == "grafana-dashboards"
    mounts = {m["mountPath"] for m in container(addons)["volumeMounts"]}
    provider = yaml.safe_load(
        by_name(addons, "ConfigMap", "grafana")["data"]["dashboardproviders.yaml"]
    )
    path = provider["providers"][0]["options"]["path"]
    assert path in mounts, f"the provider reads {path} and nothing is mounted there"


def test_no_dashboard_queries_a_metric_no_component_exports():
    """A panel is a claim that somebody exports that series. These prefixes are the ones
    this platform actually has: APISIX's own, Antares's own, and — since T-0463 — the Context
    Gateway's and the Portal's, the Portal's assistant series (`jc_agent_`) since T-2771, and the synthetic probe's (`jc_probe_`,
    T-5103) with the SLO rules recorded from them (`jc:probe_`, T-5104), and k6's (`k6_`), which the
    load runs remote-write (T-5901, conformance tests/k6/load.sh)."""
    allowed = ("apisix_", "antares_", "jc_gateway_", "jc_portal_", "jc_agent_", "jc_probe_", "jc:probe_", "k6_")
    for path in sorted((COMPONENT / "charts/dashboards/files").glob("*.json")):
        for panel in json.loads(path.read_text())["panels"]:
            for target in panel["targets"]:
                expression = target["expr"]
                assert any(
                    prefix in expression for prefix in allowed
                ), f"{path.name}: {expression} queries no series this platform exports"


# What each exporter publishes, from `telemetry.rs` in the platform and the Portal. A panel
# that reads a duration must read it as buckets: both exporters render histograms, and a
# `histogram_quantile` over `_bucket` is the only form that survives the HPA adding a replica.
GATEWAY_SERIES = (
    "jc_gateway_requests_total",
    "jc_gateway_request_duration_seconds_bucket",
    "jc_gateway_pdp_decisions_total",
    "jc_gateway_broker_request_duration_seconds_bucket",
)
PORTAL_SERIES = (
    "jc_portal_requests_total",
    "jc_portal_request_duration_seconds_bucket",
    "jc_portal_changes_total",
)


def expressions(name):
    path = COMPONENT / "charts/dashboards/files" / name
    return [t["expr"] for panel in json.loads(path.read_text())["panels"] for t in panel["targets"]]


def test_the_gateway_dashboard_draws_what_only_the_gateway_can_see():
    """A policy decision and a broker round trip are the two the edge can never answer: a
    request allowed by a policy and one allowed because nothing looked are the same 200 at
    APISIX (T-0464, OPS-16)."""
    drawn = expressions("context-gateway.json")
    for series in GATEWAY_SERIES:
        assert any(series in expression for expression in drawn), f"{series} is not drawn"
    # And the edge panels stay, because only APISIX sees a request the gateway never received.
    assert any("apisix_" in expression for expression in drawn)


def test_the_portal_dashboard_draws_proposals_by_lane():
    """The lane is the Portal's own classification — green applied, amber and red waiting for
    a person — and nothing outside the process knows which one a change landed in (CC-63)."""
    drawn = expressions("portal.json")
    for series in PORTAL_SERIES:
        assert any(series in expression for expression in drawn), f"{series} is not drawn"
    assert any(
        "jc_portal_changes_total" in expression and "by (lane)" in expression
        for expression in drawn
    ), drawn


# The assistant's series, from the Portal's `telemetry.rs` (T-2771).
ASSISTANT_SERIES = (
    "jc_agent_answer_duration_seconds_bucket",
    "jc_agent_model_call_duration_seconds_bucket",
    "jc_agent_runs_finished_total",
    "jc_agent_tool_steps_total",
    "jc_agent_model_tokens_total",
)


def test_the_assistant_dashboard_draws_latency_endings_failures_and_tokens():
    """Where an answer's seconds went, how runs end and expire, which tools fail and what the
    model costs per day: a slow assistant is traced from here to its run (T-2771, AG-72)."""
    drawn = expressions("assistant.json")
    for series in ASSISTANT_SERIES:
        assert any(series in expression for expression in drawn), f"{series} is not drawn"
    assert any("histogram_quantile(0.95" in e and "jc_agent_answer_duration_seconds_bucket" in e for e in drawn)
    assert any('status="expired"' in e for e in drawn)


def test_no_duration_is_read_as_a_summary_quantile():
    """`quantile="0.95"` on a duration is a per-replica number that cannot be combined with
    another replica's, so a panel written that way silently draws one pod (T-0464)."""
    for path in sorted((COMPONENT / "charts/dashboards/files").glob("*.json")):
        for expression in expressions(path.name):
            assert "quantile=" not in expression, f"{path.name}: {expression}"


def test_nothing_is_authored_in_the_ui(addons):
    """Everything provisioned is read-only, and the login form is off: a datasource or a
    dashboard changed in the UI is configuration that exists nowhere in Git (CC-02)."""
    config = by_name(addons, "ConfigMap", "grafana")["data"]
    datasource = yaml.safe_load(config["datasources.yaml"])["datasources"][0]
    assert datasource["editable"] is False
    assert datasource["url"].startswith("http://prometheus-operated.")

    provider = yaml.safe_load(config["dashboardproviders.yaml"])["providers"][0]
    assert provider["editable"] is False and provider["disableDeletion"] is True

    ini = config["grafana.ini"]
    assert "disable_login_form = true" in ini
    assert "allow_sign_up = false" in ini


def test_the_realm_owns_the_login_and_the_secret_is_a_reference(addons):
    """The client secret is generated by the secrets component and reaches the pod as an
    environment variable; nothing that could be a credential is rendered into the config
    (CC-06)."""
    ini = by_name(addons, "ConfigMap", "grafana")["data"]["grafana.ini"]
    assert "client_id = grafana" in ini
    # The browser follows auth_url, the pod makes the other two calls.
    assert "auth_url = https://idm.addons.example.org/" in ini
    assert "token_url = http://keycloak-app-keycloakx-http." in ini
    assert "client_secret" not in ini

    env = {e["name"]: e for e in container(addons)["env"]}
    reference = env["GF_AUTH_GENERIC_OAUTH_CLIENT_SECRET"]["valueFrom"]["secretKeyRef"]
    assert reference == {"name": "keycloak-client-grafana", "key": "client-secret"}
    assert "value" not in env["GF_AUTH_GENERIC_OAUTH_CLIENT_SECRET"]


def test_the_pod_is_pinned_unprivileged_and_needs_no_api_access(addons):
    """The production baseline, asserted here because this component is in no environment the
    baseline suite renders."""
    pod = by_name(addons, "Deployment", "grafana")["spec"]["template"]["spec"]
    assert pod["automountServiceAccountToken"] is False
    assert pod["securityContext"]["runAsNonRoot"] is True
    assert not pod.get("initContainers"), "an init container is an image to pin for nothing"

    grafana = container(addons)
    assert "@sha256:" in grafana["image"], grafana["image"]
    assert grafana["securityContext"]["readOnlyRootFilesystem"] is True
    assert grafana["securityContext"]["allowPrivilegeEscalation"] is False
    assert grafana["resources"]["requests"] and grafana["resources"]["limits"]
    assert not [d for d in addons if d.get("kind") in ("Role", "ClusterRole")
                and d["metadata"]["name"] == "grafana"]


@requires_helmfile
def test_the_component_refuses_to_render_without_a_prometheus(tmp_path):
    """Enabled and pointed at nothing is the one state that looks healthy and shows nothing,
    so it stops the render instead (the audit collector refuses the same way without a
    bucket)."""
    for entry in TREE:
        source = PROJECT_ROOT / entry
        (shutil.copytree if source.is_dir() else shutil.copy)(source, tmp_path / entry)
    shutil.copytree(tmp_path / "defaults/deployment", tmp_path / "deployment")
    shutil.copy(tmp_path / "ci/example-deployments/helmfile.yaml", tmp_path / "deployment/helmfile.yaml")
    shutil.copytree(tmp_path / "ci/example-deployments/environments",
                    tmp_path / "deployment/environments", dirs_exist_ok=True)

    # The root helmfile reads `global.yaml.gotmpl` itself and passes the result down, so that
    # file is where an environment's value lives; a second file beside it is merged with lower
    # precedence and would change nothing.
    environment = tmp_path / "deployment/environments/addons/global.yaml.gotmpl"
    text = environment.read_text()
    assert "prometheusUrl: 'http" in text, "fixture moved"
    environment.write_text(text.replace("prometheusUrl: 'http://prometheus-operated.addons.svc.cluster.local:9090'",
                                        "prometheusUrl: ''"))

    result = subprocess.run(
        ["helmfile", "-f", "deployment/helmfile.yaml", "-e", "addons", "template",
         "--skip-deps", "-q", "--selector", "component=grafana"],
        cwd=str(tmp_path), capture_output=True, text=True,
    )
    assert result.returncode != 0, "a Grafana with no datasource rendered fine"
    assert "no prometheusUrl is set" in result.stdout + result.stderr


# Project dashboards (T-6504). `addons` renders one project; dev's list is read as a file, so these
# hold before the component joins dev's `components:`.
DEV_PROJECTS = yaml.safe_load(
    (PROJECT_ROOT / "ci/example-deployments/environments/dev/grafana.yaml.gotmpl").read_text()
)["grafana"]["grafana"]["projects"]
DASHBOARD_DIR = COMPONENT / "charts/dashboards/projects"
PROVISION = COMPONENT / "charts/dashboards/provision.sh"
PLUGIN_ID = "ngsild-grafana-datasource"
# The upstream digest under images/grafana; images.yaml pins what image-grafana.yml built on it.
BASE_DIGEST = "sha256:ae509d9ae8c1021ca32ec370a9ca975d40165eda9fccd1e1102d317ff8787cd7"


def projects_config(docs):
    return by_name(docs, "ConfigMap", "grafana-projects")["data"]


def test_a_person_opens_only_the_organizations_of_their_groups(addons):
    """Each project is an organization of its own, and a person without one is refused at the
    door: a folder cannot be closed in this edition, so the organization is the wall."""
    ini = by_name(addons, "ConfigMap", "grafana")["data"]["grafana.ini"]
    assert "org_mapping = platform-admin:1:Admin /helsinki:2:Viewer\n" in ini
    assert "role_attribute_strict = true" in ini
    assert "&& 'Admin' || groups[?@ == '/helsinki'] | [0] && 'Viewer'" in ini
    assert "use_refresh_token = true" in ini


def test_the_one_unsigned_plugin_is_the_pinned_one(addons):
    """The plugin runs in every viewer's browser, so one id is allow-listed, and the image builds
    it from a commit and an archive hash somebody read, on bases pinned by digest."""
    ini = by_name(addons, "ConfigMap", "grafana")["data"]["grafana.ini"]
    assert f"allow_loading_unsigned_plugins = {PLUGIN_ID}\n" in ini
    assert "preinstall_disabled = true" in ini

    dockerfile = (PROJECT_ROOT / "images/grafana/Dockerfile").read_text()
    assert re.search(r"ADD --checksum=sha256:[0-9a-f]{64} ", dockerfile)
    assert re.search(
        r"https://github\.com/joinedcontext/Grafana-NGSI-LD-data-source/archive/[0-9a-f]{40}\.tar\.gz",
        dockerfile,
    )
    assert "--ignore-scripts" in dockerfile
    assert f"/usr/share/grafana/jc-plugins/{PLUGIN_ID}" in dockerfile
    bases = re.findall(r"^FROM (\S+)", dockerfile, re.M)
    assert len(bases) == 2 and all(re.search(r"@sha256:[0-9a-f]{64}$", b) for b in bases)
    # Only ever the base of what is pinned, or the plugin moves Grafana behind the trivy reading.
    pinned = yaml.safe_load((COMPONENT / "images.yaml").read_text())["grafana"]["grafana"]
    assert bases[1].endswith(":" + pinned["tag"] + "@" + BASE_DIGEST), pinned
    # The built image, or stock Grafana starts without the plugin and every project panel fails.
    assert pinned["repository"] == "ghcr.io/marek-mraz-jc/joinedcontext-grafana"
    assert pinned["digest"] != BASE_DIGEST


def test_the_image_gate_reads_the_expiring_excuses_and_prints_the_digest_first():
    """The binary is upstream's, so its reviewed rows hold here too, with their expiry; a digest
    printed after a red gate would leave nothing to pin or to read a new finding on."""
    workflow = (PROJECT_ROOT / ".github/workflows/image-grafana.yml").read_text()
    assert 'row["expires"] >= today' in workflow and "trivyignores: .trivyignore" in workflow
    assert workflow.index("pin this in components/grafana") < workflow.index('exit-code: "1"')


def test_the_hook_runs_from_the_mounted_script(addons):
    grafana = container(addons)
    assert grafana["lifecycle"]["postStart"]["exec"]["command"] == [
        "/bin/bash", "/etc/grafana/projects/provision.sh",
    ]
    assert projects_config(addons)["provision.sh"] == PROVISION.read_text()


def test_a_datasource_carries_the_persons_token_and_none_of_its_own(addons):
    """No shared token reads a private Endpoint: the proxy forwards the signed-in person's."""
    config = projects_config(addons)
    sources = {k: json.loads(v) for k, v in config.items() if ".datasource." in k}
    assert len(sources) == 3
    for source in sources.values():
        assert source["jsonData"]["oauthPassThru"] is True
        assert re.fullmatch(
            r"http://context-gateway\.[a-z-]+\.svc\.cluster\.local:8080/api/endpoint/[a-z2-7]{26,}",
            source["url"],
        )
        assert source["type"] == PLUGIN_ID
        assert "secureJsonData" not in source and "httpHeaderName1" not in source["jsonData"]


@pytest.mark.parametrize("project", DEV_PROJECTS, ids=lambda p: p["name"])
def test_a_project_dashboard_reads_only_its_own_endpoints(project):
    """A second project's Endpoint on this dashboard would read as no datasource at all, and a
    token in the JSON would reach every viewer."""
    text = (DASHBOARD_DIR / f"{project['name']}.json").read_text()
    dashboard = json.loads(text)
    assert dashboard["uid"] == f"project-{project['name']}" and dashboard["editable"] is False
    used = {t["datasource"]["uid"] for p in dashboard["panels"] for t in p["targets"]}
    assert used == {e["name"] for e in project["endpoints"]}
    assert all(p["description"] for p in dashboard["panels"])
    assert not re.search(r"(?i)authorization|bearer|secureJson|password|token", text)


@pytest.mark.parametrize(
    "endpoint",
    [dict(e, project=p["name"]) for p in DEV_PROJECTS for e in p["endpoints"]],
    ids=lambda e: e["name"],
)
def test_each_endpoint_is_seeded_and_in_the_tokens_audience(endpoint):
    """The gateway refuses a token that does not name the Endpoint, so a slug missing from the
    client is a dashboard of 401s, and a slug no seed holds is one of 404s."""
    seeds = (PROJECT_ROOT / "components/context-gateway/seed" / endpoint["project"]).glob("*.yaml")
    manifests = [d for f in seeds for d in yaml.safe_load_all(f.read_text()) if d]
    match = [
        m for m in manifests
        if m.get("kind") == "Endpoint" and m["metadata"]["name"] == endpoint["name"]
    ]
    assert match and match[0]["spec"]["slug"] == endpoint["slug"]

    client = yaml.safe_load((COMPONENT / "keycloak-clients.yaml").read_text())["grafana"]
    audiences = [
        m["config"]["included.custom.audience"]
        for m in client["rawValues"]["protocolMappers"]
        if m["protocolMapper"] == "oidc-audience-mapper"
    ]
    assert endpoint["slug"] in audiences


def test_every_audience_is_a_dashboard_endpoint():
    """The token reaches what the dashboards read and no further."""
    client = yaml.safe_load((COMPONENT / "keycloak-clients.yaml").read_text())["grafana"]
    audiences = {
        m["config"]["included.custom.audience"]
        for m in client["rawValues"]["protocolMappers"]
        if m["protocolMapper"] == "oidc-audience-mapper"
    }
    assert audiences == {e["slug"] for p in DEV_PROJECTS for e in p["endpoints"]}


def test_the_gateway_is_reached_through_all_four_halves():
    """Caller egress and callee ingress, on the service port and the proxy's: one missing half is
    a connection dropped with both pods healthy."""
    plain = yaml.safe_load((COMPONENT / "networkpolicies.yaml").read_text())
    meshed = yaml.safe_load((COMPONENT / "networkpolicies-linkerd.yaml").read_text())
    gateway = {"app.kubernetes.io/name": "context-gateway-gateway"}

    def egress_ports(policy):
        return {
            port["port"]
            for rule in policy["egress"]
            for peer in rule["to"]
            if peer.get("podSelector", {}).get("matchLabels") == gateway
            for port in rule["ports"]
        }

    assert egress_ports(plain["grafana-egress-to-gateway"]) == {8080}
    assert egress_ports(meshed["grafana-linkerd-control-plane"]) == {4143}
    for policy, port in (
        (plain["context-gateway-from-grafana"], 8080),
        (meshed["context-gateway-linkerd-from-grafana"], 4143),
    ):
        assert policy["podSelector"] == gateway and policy["componentNamespace"] == "context-gateway"
        peer = policy["ingress"][0]["from"][0]
        assert peer["podSelector"]["matchLabels"] == {"app.kubernetes.io/name": "grafana"}
        assert port in {p["port"] for p in policy["ingress"][0]["ports"]}


def test_the_portal_reaches_the_admin_api_through_all_four_halves():
    """T-6509: the same four halves as the gateway's, with the Portal as the caller."""
    plain = yaml.safe_load((COMPONENT / "networkpolicies.yaml").read_text())
    meshed = yaml.safe_load((COMPONENT / "networkpolicies-linkerd.yaml").read_text())
    portal = {"app.kubernetes.io/name": "portal-portal"}
    grafana = {"app.kubernetes.io/name": "grafana"}

    for policy, port in ((plain["portal-egress-to-grafana"], 3000), (meshed["portal-linkerd-egress-to-grafana"], 4143)):
        assert policy["podSelector"] == portal and policy["componentNamespace"] == "portal"
        assert policy["egress"][0]["to"][0]["podSelector"]["matchLabels"] == grafana
        assert {p["port"] for p in policy["egress"][0]["ports"]} == {port}
    for policy, port in ((plain["grafana-from-portal"], 3000), (meshed["grafana-linkerd-from-portal"], 4143)):
        assert policy["podSelector"] == grafana
        assert policy["ingress"][0]["from"][0]["podSelector"]["matchLabels"] == portal
        assert port in {p["port"] for p in policy["ingress"][0]["ports"]}


class FakeGrafana(BaseHTTPRequestHandler):
    """Answers as the admin API does: an existing name is 409 and every write lands in an org."""

    orgs: dict = {}
    writes: list = []
    fail: str = ""

    def log_message(self, *_):
        pass

    def answer(self, status, body):
        raw = json.dumps(body, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        if self.path == "/health":
            return self.answer(200, {})
        name = self.path.rsplit("/", 1)[1]
        return self.answer(200, {"id": self.orgs[name], "name": name})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        if self.path == "/orgs":
            if body["name"] in self.orgs:
                return self.answer(409, {"message": "exists"})
            self.orgs[body["name"]] = len(self.orgs) + 2
            return self.answer(200, {"orgId": self.orgs[body["name"]]})
        org = self.headers["X-Grafana-Org-Id"]
        self.writes.append(("POST", self.path, org, body.get("uid") or body["dashboard"]["uid"]))
        if self.fail == self.path:
            return self.answer(500, {"message": "broken"})
        if self.writes.count(self.writes[-1]) > 1 and self.path != "/dashboards/db":
            return self.answer(409, {"message": "exists"})
        return self.answer(200, {})

    def do_PUT(self):
        self.rfile.read(int(self.headers["Content-Length"]))
        self.writes.append(("PUT", self.path, self.headers["X-Grafana-Org-Id"], ""))
        self.answer(200, {})


@pytest.fixture
def fake_grafana(tmp_path):
    server = ThreadingHTTPServer(("127.0.0.1", 0), FakeGrafana)
    FakeGrafana.orgs, FakeGrafana.writes, FakeGrafana.fail = {}, [], ""
    threading.Thread(target=server.serve_forever, daemon=True).start()
    shutil.copy(PROVISION, tmp_path / "provision.sh")
    for project in DEV_PROJECTS:
        name = project["name"]
        (tmp_path / f"{name}.folder.json").write_text(json.dumps({"uid": f"project-{name}"}))
        (tmp_path / f"{name}.dashboard.json").write_text(json.dumps({"dashboard": {"uid": f"project-{name}"}}))
        for endpoint in project["endpoints"]:
            (tmp_path / f"{name}.datasource.{endpoint['name']}.json").write_text(
                json.dumps({"uid": endpoint["name"]})
            )
    yield tmp_path, f"http://127.0.0.1:{server.server_address[1]}"
    server.shutdown()


def provision(directory, api):
    env = {
        "PATH": os.environ["PATH"], "GRAFANA_API": api,
        "GF_SECURITY_ADMIN_USER": "admin", "GF_SECURITY_ADMIN_PASSWORD": "not-in-argv",
    }
    return subprocess.run(
        ["bash", str(directory / "provision.sh")], env=env, capture_output=True, text=True, timeout=30
    )


def test_each_projects_objects_land_in_its_own_organization_and_a_repeat_changes_nothing(fake_grafana):
    """The deny of the second project: helsinki's datasources and dashboard exist in helsinki's
    organization alone, and a container restart runs the same script over the same database."""
    directory, api = fake_grafana
    for _ in range(2):
        result = provision(directory, api)
        assert result.returncode == 0, result.stderr
    assert FakeGrafana.orgs == {"bbsk": 2, "helsinki": 3}
    placed = {(org, uid) for method, _, org, uid in FakeGrafana.writes if method == "POST"}
    assert ("2", "project-bbsk") in placed and ("3", "project-helsinki") in placed
    for project in DEV_PROJECTS:
        org = str(FakeGrafana.orgs[project["name"]])
        for endpoint in project["endpoints"]:
            assert {o for o, uid in placed if uid == endpoint["name"]} == {org}
    assert any(m == "PUT" and p == "/datasources/uid/bbsk-kpi" for m, p, _, _ in FakeGrafana.writes)


def test_a_failed_step_fails_the_hook(fake_grafana):
    """A hook that fails restarts the container, in the events; a dashboard that silently stayed
    out would be a page of nothing."""
    directory, api = fake_grafana
    FakeGrafana.fail = "/dashboards/db"
    result = provision(directory, api)
    assert result.returncode == 1 and "broken" in result.stderr


def test_an_organization_off_its_number_stops_before_anything_lands_in_it(fake_grafana):
    """org_mapping names organizations by number: a project under another's number would open
    its dashboards to the wrong people."""
    directory, api = fake_grafana
    FakeGrafana.orgs["helsinki"] = 7
    result = provision(directory, api)
    assert result.returncode == 1 and "org_mapping says" in result.stderr
    assert not [w for w in FakeGrafana.writes if w[2] == "7"]


def test_the_password_stays_out_of_the_hooks_argv():
    assert re.search(r"-K <\(printf 'user = ", PROVISION.read_text())
    assert "-u " not in PROVISION.read_text()
