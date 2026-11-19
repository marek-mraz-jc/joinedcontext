"""jc's edge trust (J-1): which peers may name the caller in X-Forwarded-For.

jc sits behind APISIX, so without `trusted_proxies` every request comes from the edge's pod and
kaisatsu's per-address sign-in lock and daiya's public rate limit become one bucket for everybody.
The list comes from the environment (the edge's pod network) and never widens to everything.
APISIX pods take any address of that network, so the trust holds because jc's NetworkPolicies
let no pod but APISIX reach its public listener (8080) or the Linkerd inbound proxy (4143).
"""

import copy
import ipaddress
import re
import shutil
import subprocess
import tomllib
from pathlib import Path

import pytest
from conftest import TREE_IGNORE, need_environment

PROJECT_ROOT = Path(__file__).resolve().parent.parent
ENVIRONMENTS = PROJECT_ROOT / "ci/example-deployments/environments"

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")


def jc_environments() -> list[str]:
    """Every example environment whose component list names jc."""
    return sorted(
        p.parent.name
        for p in ENVIRONMENTS.glob("*/global.yaml.gotmpl")
        if any(line.strip() == "- jc" for line in p.read_text().splitlines())
    )


def jc_toml(docs) -> dict:
    (cm,) = [d for d in docs if d.get("kind") == "ConfigMap" and "jc.toml" in (d.get("data") or {})]
    return tomllib.loads(cm["data"]["jc.toml"])


def client_of(proxies: list[str], peer: str, forwarded: str | None) -> str:
    """jc-router's rule (crates/shared/jc-router resolve_client_addr, tested there by
    client_addr_trusts_forwarded_for_only_from_trusted_proxies): an untrusted peer is the client;
    behind a trusted peer, the right-most hop that is not a proxy."""
    nets = [ipaddress.ip_network(n) for n in proxies]
    trusted = lambda ip: any(ipaddress.ip_address(ip) in n for n in nets)
    if not trusted(peer):
        return peer
    hops = [h.strip() for h in (forwarded or "").split(",") if h.strip()]
    for hop in reversed(hops):
        if not trusted(hop):
            return hop
    return hops[0] if hops else peer


def test_jc_renders_in_some_environment():
    assert jc_environments(), "no example environment lists jc: the render tests below check nothing"


def env_values(docs, name: str) -> list[str]:
    """Every value the rendered objects give the variable `name` (container env or ConfigMap data)."""
    out = []
    for d in docs:
        out += [v for k, v in (d.get("data") or {}).items() if k == name]
        spec = ((d.get("spec") or {}).get("template") or {}).get("spec") or {}
        for c in spec.get("containers") or []:
            out += [e.get("value") for e in c.get("env") or [] if e.get("name") == name]
    return out


@requires_helmfile
@pytest.mark.parametrize("env", jc_environments())
def test_each_private_broker_host_names_its_own_project(rendered, env):
    """jc's ekiben admits a broker's 1883 only to streams of the project an entry names
    (`project=name:port`); an entry without one opens nothing (jc-mqtt-runner-scope)."""
    for value in env_values(rendered(env), "JC_EKIBEN_PRIVATE_HOSTS"):
        for entry in filter(None, value.split(",")):
            project, _, host = entry.partition("=")
            assert project and host.endswith(":1883"), f"{env}: {entry}"
            assert host.startswith(f"mqtt-{project}-"), f"{env}: {entry} is another project's broker"


def jc_server_images(docs) -> set[str]:
    """The jc-server image of the rendered jc Deployment (its containers and init containers)."""
    out = set()
    for d in docs:
        if d.get("kind") == "Deployment" and d["metadata"]["name"] == "jc":
            spec = d["spec"]["template"]["spec"]
            out |= {c["image"] for c in spec.get("containers", []) + spec.get("initContainers", [])
                    if "/marek-mraz-jc/jc" in c["image"]}
    return out


@requires_helmfile
def test_a_jc_dev_pin_change_leaves_jc_prods_render_alone(rendered, rendered_variant):
    """jc-dev and jc-prod pin jc apart (T-6723): `serverJcDev` moves with jc main for jc-dev,
    `server` only when @prod-deployer moves jc-prod from a jc-dev green batch."""
    fake = "sha256:" + "0" * 64

    def edit(tree):
        p = tree / "components/jc/images.yaml"
        text = p.read_text()
        start = text.index("  serverJcDev:")
        head, tail = text[:start], text[start:]
        p.write_text(head + re.sub(r"digest: '[^']+'", f"digest: '{fake}'", tail, count=1))

    assert jc_server_images(rendered_variant("jc-prod", edit)) == jc_server_images(rendered("jc-prod"))
    dev = jc_server_images(rendered_variant("jc-dev", edit))
    assert dev and all(i.endswith(fake) for i in dev), dev


@requires_helmfile
def test_jc_retain_reaches_the_meshed_postgres(rendered):
    """jc-dev's hourly history cut (retainHours) connects to a Postgres whose inbound proxy takes only
    mesh-authenticated clients (T-6959: unmeshed, every run was refused at connect). The pod is meshed
    with a native sidecar so the Job still completes, and the policies open the proxy path."""
    docs = rendered("jc-dev")
    (cron,) = [d for d in docs if d.get("kind") == "CronJob" and d["metadata"]["name"] == "jc-retain"]
    notes = cron["spec"]["jobTemplate"]["spec"]["template"]["metadata"]["annotations"]
    assert notes.get("linkerd.io/inject") == "enabled", notes
    assert notes.get("config.alpha.linkerd.io/proxy-enable-native-sidecar") == "true", notes
    pols = {d["metadata"]["name"]: d["spec"] for d in docs if d.get("kind") == "NetworkPolicy"}
    retain = lambda peer: (peer.get("podSelector") or {}).get("matchLabels", {}).get("app.kubernetes.io/name") == "jc-retain"
    pg = pols["postgres-linkerd-from-jc"]
    assert any(retain(peer) and 4143 in [p["port"] for p in rule["ports"]]
               for rule in pg["ingress"] for peer in rule["from"]), pg
    egress = pols["jc-retain-linkerd-control-plane"]["egress"]
    assert {8080, 8086, 8090} <= {p["port"] for rule in egress for p in rule.get("ports", [])}, egress
    assert any(p["port"] == 4143 for rule in egress for p in rule.get("ports", [])), egress


@requires_helmfile
@pytest.mark.parametrize("env", jc_environments())
def test_jc_toml_names_its_profile(rendered, env):
    """jc refuses a jc.toml without `profile` (P-29, T-6717): every environment names production."""
    assert jc_toml(rendered(env)).get("profile") == "production", f"{env}: jc.toml names no production profile"


@requires_helmfile
@pytest.mark.parametrize("env", jc_environments())
def test_only_jc_dev_requires_a_second_factor(rendered, env):
    """jc-dev requires a TOTP of every person (owner 2026-11-14, T-6894); another environment leaves the
    key out, so a jc older than the floor still starts there."""
    got = jc_toml(rendered(env)).get("kaisatsu", {}).get("second_factor")
    assert got == ("required" if env == "jc-dev" else None), f"{env}: second_factor {got!r}"


@requires_helmfile
@pytest.mark.parametrize("env", jc_environments())
def test_jc_trusts_forwarded_for_only_from_the_edge(rendered, env):
    proxies = jc_toml(rendered(env)).get("trusted_proxies")
    assert proxies, f"{env}: jc.toml has no trusted_proxies, every caller is the edge's address"
    for net in proxies:
        assert ipaddress.ip_network(net).prefixlen > 0, f"{env}: {net} trusts every address"
    # A caller outside the edge's network, the Service network and the node itself name nobody.
    for outsider in ("203.0.113.7", "10.43.0.10", "203.0.113.10"):
        assert client_of(proxies, outsider, "198.51.100.1") == outsider, f"{env}: {outsider} set the client address"
    # The edge's pod forwards the real caller.
    assert client_of(proxies, "10.42.1.7", "198.51.100.1, 10.42.0.1") == "198.51.100.1"


def jc_container(docs) -> dict:
    (dep,) = [d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "jc"]
    (container,) = [c for c in dep["spec"]["template"]["spec"]["containers"] if c["name"] in ("jc", "jc-server")]
    return container


def jc_upstreams(docs) -> list[str]:
    """Every APISIX upstream node naming one of jc's Services, in the rendered standalone config."""
    nodes = []
    for d in docs:
        if d.get("kind") in ("Secret", "ConfigMap") and d["metadata"]["name"].startswith("apisix-standalone"):
            text = (d.get("stringData") or d.get("data") or {}).get("apisix.yaml", "")
            nodes += [line.strip() for line in text.splitlines() if "jc.jc-" in line or "jc-pods." in line or ".svc.cluster.local:8080" in line and line.strip().startswith('"jc')]
    return nodes


@requires_helmfile
@pytest.mark.parametrize("env", jc_environments())
def test_jc_default_values_open_no_import_window(rendered, env):
    # The import window (CLUSTER-IMPORT.md 6) lets an anonymous write in: it must never be the default.
    docs = rendered(env)
    assert jc_toml(docs)["listen"] == "0.0.0.0:8080", f"{env}: jc listens on loopback only outside the window"
    container = jc_container(docs)
    env_vars = {e["name"]: e.get("value") for e in container.get("env", [])}
    assert "ANTARES_POLICY" not in env_vars, f"{env}: ANTARES_POLICY={env_vars.get('ANTARES_POLICY')} overrides jc's joinedcontext policy"
    assert "readinessProbe" in container and "livenessProbe" in container, f"{env}: jc renders without probes"
    services = {d["metadata"]["name"] for d in docs if d.get("kind") == "Service"}
    assert {"jc", "jc-pods"} <= services, f"{env}: jc's Services are missing: {services}"


@requires_helmfile
def test_jc_import_window_routes_nothing_to_jc(rendered_variant):
    def window_on(tree):
        values = tree / "deployment/environments/jc-prod/global.yaml.gotmpl"
        text = values.read_text()
        assert "    mode: host\n" in text, "jc-prod names no host mode"
        values.write_text(text.replace("    mode: host\n", "    mode: host\n    importWindow: true\n", 1))

    docs = rendered_variant("jc-prod", window_on)
    toml = jc_toml(docs)
    assert toml["listen"] == "127.0.0.1:8080", "the window listens beyond the pod's loopback"
    # The kit's slug paths on http://localhost:8080, the Portal's discovery on the same loopback.
    assert (toml["mode"], toml["base_host"], toml["scheme"]) == ("slug", "localhost:8080", "http")
    container = jc_container(docs)
    assert {e["name"]: e.get("value") for e in container.get("env", [])}.get("ANTARES_POLICY") == "allow-all"
    # The image's own JCBIN_LISTEN (0.0.0.0:8080) beats jc.toml unless the pod names it.
    assert {e["name"]: e.get("value") for e in container.get("env", [])}.get("JCBIN_LISTEN") == "127.0.0.1:8080"
    jc_services = [d["metadata"]["name"] for d in docs if d.get("kind") == "Service" and d["metadata"]["name"] in ("jc", "jc-pods")]
    assert not jc_services, f"the window renders Service(s) {jc_services}: their endpoints would reach jc"
    ingresses = [d["metadata"]["name"] for d in docs if d.get("kind") == "Ingress" and "jc" in str(d["spec"].get("rules", [])) and "jc-prod" == d["metadata"].get("namespace") and d["metadata"]["name"].startswith("jc")]
    assert not ingresses, f"the window renders jc Ingress(es) {ingresses}"
    assert not jc_upstreams(docs), f"the window routes the edge to jc: {jc_upstreams(docs)}"
    # No App runtime path in the window: no API token, no kube-api egress, no apps-db or store login.
    (deployment,) = [d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "jc"]
    assert deployment["spec"]["template"]["spec"]["automountServiceAccountToken"] is False
    assert not [d for d in docs if d.get("kind") == "NetworkPolicy" and d["metadata"]["name"] == "jc-server-kube-api"]
    names = {e["name"] for e in container.get("env", [])}
    assert not names & {"APPS_DB_PASSWORD", "JC_PORTAL_APPS_DB_URL", "JC_PORTAL_ARTIFACT_STORE_ACCESS_KEY", "JC_PORTAL_ARTIFACT_STORE_SECRET_KEY"}
    # Red check of the reader: the default render routes the edge to jc.
    default = rendered_variant("jc-prod", lambda tree: None)
    assert jc_upstreams(default), "the upstream reader finds no jc route in the default render"
    (default_jc,) = [d for d in default if d.get("kind") == "Deployment" and d["metadata"]["name"] == "jc"]
    assert default_jc["spec"]["template"]["spec"]["automountServiceAccountToken"] is True, "jc-prod's default render holds no App runtime token"


@requires_helmfile
@pytest.mark.parametrize("env", ["jc-prod", "jc-dev"])
def test_jc_catalogue_carries_prods_publisher_badges_and_logo(rendered, env):
    # Owner 2026-11-10 15:28Z "logos are missing": old prod drew each publisher's badge and words
    # from the CKAN chart's `organizations` and the footer mark and favicon from global.branding.
    dataroom = jc_toml(rendered(env))["dataroom"]
    pubs = dataroom["publishers"]
    assert len(pubs) == 8 and pubs["praha"]["initials"] == "PHA" and pubs["praha"]["colour"] == "#b4232a"
    assert pubs["hel-fi"]["description"]["sk"].startswith("Otvorené údaje regiónu Helsinki")
    assert sum(bool(p.get("description")) for p in pubs.values()) == 6, "none for banskabystrica and bbsk, as on prod"
    logo = (ENVIRONMENTS / env / "branding/logo.svg").read_text()
    branding = dataroom["branding"]
    assert (branding["logo"], branding["favicon"]) == ("logo.svg", "logo.svg")
    assert branding["logo_svg"].strip() == logo.strip()


@requires_helmfile
@pytest.mark.parametrize("env", ["jc-prod", "jc-dev"])
def test_jc_streams_sign_in_as_their_pipelines_own_client(rendered, env):
    # T-6899: jc-dev rendered no JC_PORTAL_PIPELINE_IDENTITY, so its streams signed in as the
    # imported <project>-pipelines, whose audience lacked banskabystrica-otvorene-data: every od-*
    # upsert answered 401 (realm state: 4 audience mappers, none 4ysiqy53...).
    env_of = {e["name"]: e.get("value") for e in jc_container(rendered(env))["env"]}
    assert env_of.get("JC_PORTAL_PIPELINE_IDENTITY") == "pipeline-secret"


@requires_helmfile
@pytest.mark.parametrize("env", jc_environments())
def test_jc_portal_serves_as_a_role_apart_from_the_one_that_migrates(rendered, env):
    # T-7179: the Portal migrates as jc_portal, the owner, and serves as jc_portal_app, so the
    # running Portal cannot drop the operators' audit triggers. Each password from its own Secret.
    container = jc_container(rendered(env))
    env_of = {e["name"]: e for e in container["env"]}
    serving, migrating = (env_of[k].get("value", "") for k in ("JC_PORTAL_DATABASE_URL", "JC_PORTAL_MIGRATION_DATABASE_URL"))
    assert serving.startswith("postgresql://jc_portal_app:$(PORTAL_APP_DB_PASSWORD)@") and serving.endswith("/jc_portal")
    assert migrating.startswith("postgresql://jc_portal:$(PORTAL_DB_PASSWORD)@") and migrating.endswith("/jc_portal")
    refs = {k: env_of[k]["valueFrom"]["secretKeyRef"]["name"] for k in ("PORTAL_APP_DB_PASSWORD", "PORTAL_DB_PASSWORD")}
    assert refs == {"PORTAL_APP_DB_PASSWORD": "db-jc-portal-app", "PORTAL_DB_PASSWORD": "db-jc-portal"}


@requires_helmfile
def test_jc_antares_serves_apart_from_its_owner_once_the_role_split_is_on(rendered, rendered_variant):
    # T-7310: off, the broker logs in as the owner jc_antares as before (an older image knows no
    # ANTARES_MIGRATION_DATABASE_URL); on, it migrates as jc_antares, serves as jc_antares_app and
    # drains and reaps as jc_antares_maint, each password from its own Secret.
    off = {e["name"]: e for e in jc_container(rendered("addons"))["env"]}
    assert off["ANTARES_DATABASE_URL"]["value"].startswith("postgresql://jc_antares:$(BROKER_DB_PASSWORD)@")
    assert "ANTARES_MIGRATION_DATABASE_URL" not in off and "ANTARES_MAINTENANCE_DATABASE_URL" not in off

    def split_on(tree):
        path = tree / "components/jc/default-environment.yaml.gotmpl"
        text = path.read_text()
        assert "antaresRoleSplit: false" in text
        path.write_text(text.replace("antaresRoleSplit: false", "antaresRoleSplit: true"))

    docs = rendered_variant("addons", split_on)
    on = {e["name"]: e for e in jc_container(docs)["env"]}
    urls = {k: on[k]["value"] for k in ("ANTARES_DATABASE_URL", "ANTARES_MIGRATION_DATABASE_URL",
                                        "ANTARES_MAINTENANCE_DATABASE_URL")}
    assert urls["ANTARES_DATABASE_URL"].startswith("postgresql://jc_antares_app:$(BROKER_APP_DB_PASSWORD)@")
    assert urls["ANTARES_MIGRATION_DATABASE_URL"].startswith("postgresql://jc_antares:$(BROKER_DB_PASSWORD)@")
    assert urls["ANTARES_MAINTENANCE_DATABASE_URL"].startswith(
        "postgresql://jc_antares_maint:$(BROKER_MAINT_DB_PASSWORD)@")
    assert all(u.endswith("/jc_antares") for u in urls.values())
    refs = {k: on[k]["valueFrom"]["secretKeyRef"]["name"]
            for k in ("BROKER_DB_PASSWORD", "BROKER_APP_DB_PASSWORD", "BROKER_MAINT_DB_PASSWORD")}
    assert refs == {"BROKER_DB_PASSWORD": "db-jc-antares", "BROKER_APP_DB_PASSWORD": "db-jc-antares-app",
                    "BROKER_MAINT_DB_PASSWORD": "db-jc-antares-maint"}
    # The roles behind them: rows only, the maintenance one in antares_maintenance.
    jobs = {d["metadata"]["name"]: d for d in docs if d.get("kind") == "Job" and "create-role" in d["metadata"]["name"]}
    def job_env(name):
        return {e["name"]: e.get("value") for e in jobs[name]["spec"]["template"]["spec"]["containers"][0]["env"]}
    app = job_env("jc-antares-db-create-role-jc-antares-app-dml")
    maint = job_env("jc-antares-db-create-role-jc-antares-maint-dml")
    assert (app["ROLE_TYPE"], app["ROLE_MEMBER_OF"]) == ("dml", "[]")
    assert (maint["ROLE_TYPE"], maint["ROLE_MEMBER_OF"]) == ("dml", '["antares_maintenance"]')
    assert app["DB_OWNER"] == maint["DB_OWNER"] == "jc_antares"


@requires_helmfile
def test_jc_prod_carries_the_cutover_parts(rendered):
    # Supervisor 2026-11-10 06:3xZ: prod parity on jc-prod. bbsk. keeps its own certificate, off
    # the copied edge certificate's SAN list; App functions run on jc-prod's realm and gateway; the
    # 24 h temporal rule runs hourly and keeps KPI history and audit rows.
    docs = rendered("jc-prod")
    (cert,) = [d for d in docs if d.get("kind") == "Certificate" and d["metadata"]["name"] == "apisix-edge"]
    assert "bbsk.smartcity.joinedcontext.com" not in cert["spec"]["dnsNames"], "bbsk. on the edge SANs: cert-manager would re-order the copied certificate"
    tls = [t for d in docs if d.get("kind") == "Ingress" for t in d["spec"].get("tls", [])]
    assert {"hosts": ["bbsk.smartcity.joinedcontext.com"], "secretName": "demo-bbsk-tls"} in tls
    (functions,) = [d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "jc-functions"]
    env = {e["name"]: e.get("value") for e in functions["spec"]["template"]["spec"]["containers"][0]["env"]}
    assert env["JC_OIDC_ISSUER"] == "https://idm.smartcity.joinedcontext.com/realms/smartcity"
    assert env["JC_GATEWAY_URL"] == "https://api.smartcity.joinedcontext.com"
    assert {e["name"]: e.get("value") for e in jc_container(docs)["env"]}["JC_FUNCTIONS_URL"].startswith("http://jc-functions.jc-prod.")
    # jc-prod keeps prod's history: no hourly 24 h sweep (it would delete prod history for good).
    assert not [d for d in docs if d.get("kind") == "CronJob" and d["metadata"]["name"] == "jc-retain"]
    # tmpfs keeps each file in a 4 KiB page: prod's realm names 903 client secrets plus the
    # pipeline clients, which overflowed a 2Mi realm-secrets volume (seed-realm, 2026-11-10).
    (jc,) = [d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "jc"]
    (vol,) = [v for v in jc["spec"]["template"]["spec"]["volumes"] if v["name"] == "realm-secrets"]
    # 904 realm secrets plus a copy per pipeline client (841 lines) at a 4 KiB page each: ~7 MiB.
    assert vol["emptyDir"]["sizeLimit"] == "16Mi"
    # The gateway's checkouts (70 MB in prod's rehearsal) live in the state tmpfs; 1Mi gave ENOSPC.
    (state,) = [v for v in jc["spec"]["template"]["spec"]["volumes"] if v["name"] == "state"]
    assert state["emptyDir"] == {"medium": "Memory", "sizeLimit": "192Mi"}
    assert not [m for m in jc_container(docs)["volumeMounts"] if m["mountPath"].startswith("/var/lib/jc/state/")]
    env = {e["name"]: e.get("value") for e in jc["spec"]["template"]["spec"]["containers"][0]["env"]}
    assert (env["JCBIN_LISTEN"], env["JCBIN_STATE_DIR"]) == ("0.0.0.0:8080", "/var/lib/jc/state")
    # Antares' in-process bus: one jc process per database, so no rollout overlaps two.
    assert (env["ANTARES_BUS"], env["ANTARES_ALLOW_SHARED_LOCAL"]) == ("local", "1")
    # The import's shako calls were refused (401) while jc held a per-boot token: jc reads jc-setup's.
    assert env["JCBIN_SHAKO_TOKEN_FILE"] == "/run/jc/setup/shako-token"
    assert env["ANTARES_EXPORT_FILES"] == "2", "the export cap rose to 6 in jc 446484ae; the node has no room yet"
    # The runner's files: without them every GTFS-RT stream was refused (lstat .../streams/proto).
    assert env["JCBIN_EKIBEN_RESOURCES_FILE"] == "/etc/jc/ekiben-resources.yaml"
    (cm,) = [d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "jc"]
    assert "helsinki__gtfs" in cm["data"]["ekiben-resources.yaml"] and "message FeedMessage" in cm["data"]["gtfs-realtime.proto"]
    assert {"name": "config", "mountPath": "/var/lib/jc-data/streams/proto/gtfs-realtime.proto", "subPath": "gtfs-realtime.proto",
            "readOnly": True} in jc_container(docs)["volumeMounts"]
    # Unset, jc took the Portal's host as the org domain and pipelines minted second copies.
    assert {k: env[k] for k in ("JC_PORTAL_ORG_DOMAIN", "JC_GATEWAY_ORG_DOMAIN", "JC_ASSISTANT_ORG_DOMAIN")} == dict.fromkeys(
        ("JC_PORTAL_ORG_DOMAIN", "JC_GATEWAY_ORG_DOMAIN", "JC_ASSISTANT_ORG_DOMAIN"), "hel.fi")
    # Old prod's Portal served the map Apps' tiles; unset, every map App said "No base map".
    assert (env["JC_BASEMAP_URL"], env["JC_BASEMAP_ATTRIBUTION"]) == (
        "https://tile.openstreetmap.org/{z}/{x}/{y}.png", "© OpenStreetMap contributors")
    assert env["JC_BASEMAP_CACHE_MAX_BYTES"] == "67108864"
    # Without it the gateway refused the pipelines' writes: "token is not bound to this resource".
    assert env["JC_PORTAL_PIPELINE_IDENTITY"] == "pipeline-secret"
    # The edge blocks /git: the Portal reaches its forge on jc's loopback, the public base stays.
    assert [m["host"] for m in jc_toml(docs)["mounts"]["shako"]] == ["smartcity.joinedcontext.com", "localhost:8080"]
    assert (env["JC_SHAKO_URL"], env["JC_SHAKO_OWNER"], env["JC_SHAKO_REPO"]) == ("http://localhost:8080/git", "joinedcontext", "configuration")
    mounts = jc_toml(docs)["mounts"]
    assert [m["host"] for m in mounts["antares"]] == ["localhost:8080"]
    # ekiben's API stays inside the pod; only its ingest listener has a public entry (PL-122).
    assert [(m["host"], m["path_prefix"]) for m in mounts["ekiben"]] == [
        ("localhost:8080", "/streams"), ("portal.smartcity.joinedcontext.com", "/ingest")]
    assert (env["JC_PORTAL_PIPELINE_RUNNER_URL"], env["JC_PORTAL_BROKER_URL"]) == ("http://localhost:8080/streams", "http://localhost:8080/broker")
    # Host mode routes by name: the kubelet's probes name the hosts jc answers them on.
    probes = jc["spec"]["template"]["spec"]["containers"][0]
    assert probes["readinessProbe"]["httpGet"]["httpHeaders"] == [{"name": "Host", "value": "portal.smartcity.joinedcontext.com"}]
    assert probes["livenessProbe"]["httpGet"]["httpHeaders"] == [{"name": "Host", "value": "localhost:8080"}]
    # Each harvest reads its Endpoints on this instance's gateway and links under its catalogue.
    for h in jc_toml(docs)["dataroom"]["harvest"]:
        assert "api.smartcity.joinedcontext.com" in h["allowed_hosts"]
        assert h.get("datasets_base", "https://opendata.smartcity.joinedcontext.com/") == "https://opendata.smartcity.joinedcontext.com/"
    assert jc["spec"]["strategy"]["type"] == "Recreate" and jc["spec"]["replicas"] == 1


def with_retain(tree):
    """jc-prod with the dev move's 24 h retain sweep turned on (retainHours: 24)."""
    path = tree / "deployment/environments/jc-prod/global.yaml.gotmpl"
    text = path.read_text()
    assert "\n    catalogueHarvest: true\n" in text
    path.write_text(text.replace("\n    catalogueHarvest: true\n", "\n    catalogueHarvest: true\n    retainHours: 24\n", 1))


@requires_helmfile
def test_the_retain_sweep_keeps_audit_rows_and_kpis(rendered_variant):
    docs = rendered_variant("jc-prod", with_retain)
    (retain,) = [d for d in docs if d.get("kind") == "CronJob" and d["metadata"]["name"] == "jc-retain"]
    sql = retain["spec"]["jobTemplate"]["spec"]["template"]["spec"]["containers"][0]["command"][-1]
    assert sql.count("interval '24 hours'") == 2 and sql.count("KeyPerformanceIndicator") == 2
    assert sql.count('\'{"audit": true}\'') == 2, "retain deletes audit rows"


@requires_helmfile
def test_jc_prod_opts_no_pod_out_of_the_mesh_without_a_reason(rendered_variant):
    # Kyverno's justify-linkerd-inject-opt-out refused jc-retain at admission on jc-prod
    # (2026-11-10 07:3xZ): a pod template with linkerd.io/inject disabled needs a reason.
    templates = []
    for d in rendered_variant("jc-prod", with_retain):
        spec = d.get("spec") or {}
        if d.get("kind") == "CronJob":
            spec = spec["jobTemplate"]["spec"]
        if d.get("kind") in ("Deployment", "StatefulSet", "DaemonSet", "Job", "CronJob"):
            templates.append((d["metadata"]["name"], spec["template"]["metadata"].get("annotations") or {}))
    # The reader sees CronJob templates (jc-retain, meshed since T-6959, so it is no opt-out sample).
    assert any(n == "jc-retain" for n, _ in templates), "the reader finds no CronJob template (jc-retain)"
    opted = [(n, a) for n, a in templates if a.get("linkerd.io/inject") == "disabled"]
    assert not [n for n, a in opted if not str(a.get("mesh.joinedcontext.com/opt-out-reason", "")).strip()]


def render_addons_with(tmp_path, proxies_yaml: str) -> subprocess.CompletedProcess:
    need_environment("addons")
    tree = tmp_path / "repo"
    shutil.copytree(PROJECT_ROOT, tree, ignore=TREE_IGNORE)
    values = tree / "deployment/environments/addons/jc.yaml.gotmpl"
    text = values.read_text()
    head, sep, _ = text.partition("    trustedProxies:\n")
    assert sep, "addons names no trustedProxies"
    values.write_text(head + proxies_yaml)
    return subprocess.run(
        [str(tree / "scripts/render.sh"), "addons", str(tmp_path / "out.yaml")],
        cwd=str(tree), capture_output=True, text=True, check=False,
    )


@requires_helmfile
@pytest.mark.parametrize(
    "proxies_yaml, refusal",
    [
        ("    trustedProxies: []\n", "name the pod network the edge"),
        ('    trustedProxies:\n      - "0.0.0.0/0"\n', "narrower than /0"),
        ('    trustedProxies:\n      - "::/0"\n', "narrower than /0"),
        ('    trustedProxies:\n      - "10.42.0.7"\n', "narrower than /0"),
    ],
)
def test_jc_refuses_to_render_without_a_narrow_edge_network(tmp_path, proxies_yaml, refusal):
    result = render_addons_with(tmp_path, proxies_yaml)
    assert result.returncode != 0, "rendered with trusted_proxies missing or wide open"
    assert refusal in result.stderr + result.stdout


def selects(selector: dict | None, labels: dict) -> bool:
    """A Kubernetes label selector, matchExpressions included; an operator it cannot read fails
    the test instead of reading as "selects nothing"."""
    selector = selector or {}
    if not all(labels.get(k) == v for k, v in (selector.get("matchLabels") or {}).items()):
        return False
    for expr in selector.get("matchExpressions") or []:
        key, op, values = expr["key"], expr["operator"], expr.get("values") or []
        if op == "In":
            ok = labels.get(key) in values
        elif op == "NotIn":
            ok = labels.get(key) not in values
        elif op == "Exists":
            ok = key in labels
        elif op == "DoesNotExist":
            ok = key not in labels
        else:
            raise AssertionError(f"unread selector operator {op!r} in {selector}")
        if not ok:
            return False
    return True


def opens_public_port(rule: dict, named: dict[str, int], public: set[int] | None = None) -> bool:
    """Whether an ingress rule covers jc's public listener or the Linkerd inbound proxy (or the
    ports in `public`). No ports means every port; a named port resolves through jc's container
    ports, and a name jc's pod does not declare (the injected proxy's, a typo) counts as public."""
    public = PUBLIC_PORTS if public is None else public
    entries = rule.get("ports")
    if not entries:
        return True
    for entry in entries:
        port = entry.get("port")
        if port is None:
            return True
        if isinstance(port, str):
            port = named.get(port)
            if port is None:
                return True
        end = entry.get("endPort", port)
        if any(port <= p <= end for p in public):
            return True
    return False


def workload(docs, name: str) -> dict:
    (dep,) = [d for d in docs if d.get("kind") in ("Deployment", "StatefulSet") and d["metadata"]["name"] == name]
    return dep


def pod_of(docs, name: str) -> tuple[str, dict]:
    dep = workload(docs, name)
    return dep["metadata"]["namespace"], dep["spec"]["template"]["metadata"]["labels"]


def named_ports(docs, name: str) -> dict[str, int]:
    containers = workload(docs, name)["spec"]["template"]["spec"]["containers"]
    return {p["name"]: p["containerPort"] for c in containers for p in c.get("ports") or [] if p.get("name")}


def workloads(docs, namespace: str):
    """(name, pod labels) of every workload rendered into `namespace`."""
    for d in docs:
        if d.get("kind") not in ("Deployment", "StatefulSet", "DaemonSet", "Job", "CronJob"):
            continue
        if d["metadata"].get("namespace") != namespace:
            continue
        spec = d["spec"].get("jobTemplate", {}).get("spec", d["spec"])
        yield d["metadata"]["name"], (spec.get("template", {}).get("metadata") or {}).get("labels") or {}


def test_the_selector_and_port_readers_fail_closed():
    """The policy test is only as strict as these two readers."""
    labels = {"app.kubernetes.io/name": "jc-server", "tier": "edge"}
    assert selects({"matchExpressions": [{"key": "tier", "operator": "In", "values": ["edge"]}]}, labels)
    assert not selects({"matchExpressions": [{"key": "tier", "operator": "NotIn", "values": ["edge"]}]}, labels)
    assert selects({"matchExpressions": [{"key": "tier", "operator": "Exists"}]}, labels)
    assert not selects({"matchExpressions": [{"key": "tier", "operator": "DoesNotExist"}]}, labels)
    with pytest.raises(AssertionError, match="unread selector"):
        selects({"matchExpressions": [{"key": "tier", "operator": "Gt", "values": ["1"]}]}, labels)
    named = {"http": 8080, "admin": 9091}
    assert opens_public_port({"ports": [{"port": "http"}]}, named)
    assert not opens_public_port({"ports": [{"port": "admin"}]}, named)
    assert opens_public_port({"ports": [{"port": "linkerd-proxy"}]}, named)
    assert opens_public_port({"ports": [{"protocol": "TCP"}]}, named)
    assert opens_public_port({"ports": [{"port": 8000, "endPort": 8100}]}, named)
    assert opens_public_port({}, named)
    assert not opens_public_port({"ports": [{"port": 4191}]}, named)


# jc's public listener and the Linkerd inbound proxy meshed traffic to it arrives on.
PUBLIC_PORTS = {8080, 4143}


@requires_helmfile
@pytest.mark.parametrize("env", jc_environments())
def test_only_the_edge_reaches_jc_public_listener(rendered, env):
    """`trusted_proxies` names the whole pod network (APISIX pods take any address in it), so the
    trust holds only if no pod but APISIX can open a connection to jc's public listener at all."""
    check_only_the_edge_reaches_jc(rendered(env), env)


def mesh_admits_only_the_edge(docs, jc_ns: str, jc_labels: dict, named: dict[str, int], edge: tuple[str, str]) -> bool:
    """Whether jc's Linkerd proxy refuses every identity but the edge's on the public listener:
    one Server on 8080 with accessPolicy deny, no HTTPRoute under it, and every AuthorizationPolicy
    on it requiring a MeshTLSAuthentication that names exactly the edge's ServiceAccount. Then a
    peer a NetworkPolicy lets reach the inbound proxy (4143) for another port still cannot open
    8080 through it."""
    edge_ns, edge_sa = edge
    servers = [
        d for d in docs
        if d.get("kind") == "Server" and d["metadata"].get("namespace") == jc_ns
        and selects(d["spec"].get("podSelector"), jc_labels)
        and named.get(d["spec"].get("port"), d["spec"].get("port")) == 8080
    ]
    if len(servers) != 1 or servers[0]["spec"].get("accessPolicy") != "deny":
        return False
    server = servers[0]["metadata"]["name"]
    in_ns = lambda kind: [d for d in docs if d.get("kind") == kind and d["metadata"].get("namespace") == jc_ns]
    if any(any(ref.get("name") == server for ref in d["spec"].get("parentRefs") or []) for d in in_ns("HTTPRoute")):
        return False
    authentications = {d["metadata"]["name"]: d["spec"] for d in in_ns("MeshTLSAuthentication")}
    policies = [d for d in in_ns("AuthorizationPolicy") if d["spec"]["targetRef"].get("name") == server]
    if not policies:
        return False
    for policy in policies:
        refs = policy["spec"].get("requiredAuthenticationRefs") or []
        if not refs:
            return False
        for ref in refs:
            if ref.get("kind") != "MeshTLSAuthentication":
                return False
            identities = (authentications.get(ref.get("name")) or {}).get("identityRefs")
            if identities != [{"kind": "ServiceAccount", "name": edge_sa, "namespace": edge_ns}]:
                return False
    return True


def check_only_the_edge_reaches_jc(docs, env):
    jc_ns, jc_labels = pod_of(docs, "jc")
    named = named_ports(docs, "jc")
    edge_ns, edge_labels = pod_of(docs, "apisix")
    edge_sa = workload(docs, "apisix")["spec"]["template"]["spec"].get("serviceAccountName", "default")
    # With the proxy itself holding 8080 for the edge, a rule opening 4143 (the MQTT brokers' way to
    # jc's internal port) opens nothing of the public listener; 8080 itself stays the edge's alone.
    public = {8080} if mesh_admits_only_the_edge(docs, jc_ns, jc_labels, named, (edge_ns, edge_sa)) else PUBLIC_PORTS
    namespaces = {d["metadata"]["name"]: d["metadata"].get("labels") or {} for d in docs if d.get("kind") == "Namespace"}
    namespaces.setdefault(edge_ns, {}).setdefault("kubernetes.io/metadata.name", edge_ns)
    policies = [
        d for d in docs
        if d.get("kind") == "NetworkPolicy"
        and d["metadata"].get("namespace") == jc_ns
        and selects(d["spec"].get("podSelector"), jc_labels)
    ]
    ingress = [p for p in policies if "Ingress" in (p["spec"].get("policyTypes") or ["Ingress"])]
    assert ingress, f"{env}: no NetworkPolicy isolates jc's ingress, every pod reaches it"
    opened = 0
    for policy in ingress:
        for rule in policy["spec"].get("ingress") or []:
            if not opens_public_port(rule, named, public):
                continue
            ports = [p.get("port") for p in rule.get("ports") or []]
            name = policy["metadata"]["name"]
            peers = rule.get("from")
            assert peers, f"{env}: {name} opens {ports or 'every port'} to every source"
            for peer in peers:
                assert "ipBlock" not in peer, f"{env}: {name} opens jc to an address block"
                assert "podSelector" in peer, f"{env}: {name} opens jc to whole namespaces"
                ns_sel = peer.get("namespaceSelector")
                peer_ns = [n for n, labels in namespaces.items() if selects(ns_sel, labels)] if ns_sel is not None else [jc_ns]
                assert peer_ns == [edge_ns], f"{env}: {name} admits namespaces {peer_ns}"
                assert peer["podSelector"].get("matchLabels"), f"{env}: {name} admits every pod of {edge_ns}"
                assert selects(peer["podSelector"], edge_labels), f"{env}: {name} admits pods that are not APISIX"
                others = [w for w, labels in workloads(docs, edge_ns) if w != "apisix" and selects(peer["podSelector"], labels)]
                assert not others, f"{env}: {name} also admits {others}, which carry APISIX's labels"
                opened += 1
    assert opened, f"{env}: not even APISIX reaches jc"


def planted(namespace: str, pod_selector: dict, ports: list | None) -> dict:
    rule = {"from": [{"podSelector": {}}]}
    if ports is not None:
        rule["ports"] = ports
    return {
        "kind": "NetworkPolicy",
        "metadata": {"name": "planted", "namespace": namespace},
        "spec": {"podSelector": pod_selector, "policyTypes": ["Ingress"], "ingress": [rule]},
    }


@requires_helmfile
@pytest.mark.parametrize(
    "pod_selector, ports",
    [
        ({"matchExpressions": [{"key": "app.kubernetes.io/name", "operator": "In", "values": ["jc-server"]}]},
         [{"protocol": "TCP", "port": 8080}]),
        ({"matchLabels": {"app.kubernetes.io/name": "jc-server"}}, [{"protocol": "TCP", "port": "http"}]),
        ({"matchLabels": {"app.kubernetes.io/name": "jc-server"}}, [{"protocol": "TCP", "port": "linkerd-proxy"}]),
        ({"matchLabels": {"app.kubernetes.io/name": "jc-server"}}, [{"protocol": "TCP", "port": 4100, "endPort": 4200}]),
        ({}, None),
    ],
    ids=["by-expression", "named-http", "named-unknown", "port-range", "every-pod-every-port"],
)
def test_a_policy_opening_jc_to_every_pod_is_caught(rendered, pod_selector, ports):
    """Each way a NetworkPolicy can select jc or name its port reaches the check: none reads as
    "does not apply" (both used to, for matchExpressions and named ports). Planted where no
    Linkerd Server holds 8080 for the edge (jc-dev: no MQTT brokers, no App pods), so the inbound
    proxy port (4143, inside the port range) still reaches the public listener."""
    docs = rendered("jc-dev")
    jc_ns, jc_labels = pod_of(docs, "jc")
    edge_ns, _ = pod_of(docs, "apisix")
    edge_sa = workload(docs, "apisix")["spec"]["template"]["spec"].get("serviceAccountName", "default")
    assert not mesh_admits_only_the_edge(docs, jc_ns, jc_labels, named_ports(docs, "jc"), (edge_ns, edge_sa))
    with pytest.raises(AssertionError, match="admits every pod"):
        check_only_the_edge_reaches_jc([*docs, planted(jc_ns, pod_selector, ports)], "planted")


@requires_helmfile
def test_beside_the_mqtt_brokers_the_proxy_holds_the_public_listener_for_the_edge(rendered):
    """jc-prod opens jc's inbound proxy (4143) to the MQTT broker pods for the CONNECT check on
    8082. That keeps the edge's X-Forwarded-For trust only while jc's Linkerd Server on 8080 admits
    APISIX's identity alone: without that Server, with a second identity in it or with a policy
    that requires none, the check fails; 8080 opened to every pod still fails with it."""
    docs = rendered("jc-prod")
    jc_ns, _ = pod_of(docs, "jc")
    check_only_the_edge_reaches_jc(docs, "jc-prod")
    (server,) = [d for d in docs if d.get("kind") == "Server" and d["metadata"].get("namespace") == jc_ns and d["spec"]["port"] == 8080
                 and d["spec"]["podSelector"].get("matchLabels", {}).get("app.kubernetes.io/name") == "jc-server"]
    name = server["metadata"]["name"]

    without = [d for d in docs if d is not server]
    with pytest.raises(AssertionError, match="admits namespaces"):
        check_only_the_edge_reaches_jc(without, "no-server")

    def edited(kind: str, change) -> list[dict]:
        out = []
        for d in docs:
            if d.get("kind") == kind and d["metadata"].get("namespace") == jc_ns and name in d["metadata"]["name"]:
                d = copy.deepcopy(d)
                change(d["spec"])
            out.append(d)
        return out

    wider = edited("MeshTLSAuthentication", lambda s: s["identityRefs"].append({"kind": "Namespace", "name": "jc-prod-mqtt"}))
    with pytest.raises(AssertionError, match="admits namespaces"):
        check_only_the_edge_reaches_jc(wider, "second-identity")
    open_policy = edited("AuthorizationPolicy", lambda s: s.update(requiredAuthenticationRefs=[]))
    with pytest.raises(AssertionError, match="admits namespaces"):
        check_only_the_edge_reaches_jc(open_policy, "no-authentication")
    allowed = edited("Server", lambda s: s.update(accessPolicy="all-authenticated"))
    with pytest.raises(AssertionError, match="admits namespaces"):
        check_only_the_edge_reaches_jc(allowed, "server-allows")

    for ports in ([{"protocol": "TCP", "port": 8080}], [{"protocol": "TCP", "port": "http"}], None):
        with pytest.raises(AssertionError, match="admits every pod"):
            check_only_the_edge_reaches_jc([*docs, planted(jc_ns, {"matchLabels": {"app.kubernetes.io/name": "jc-server"}}, ports)], "planted")
