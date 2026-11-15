"""jc-prod's App hosts (bins/jc edge-session-design S4a, CUTOVER section 4 'App-host slice').

Prod's old Portal stamped one Ingress and one Certificate per App, `{name}.apps.{domain}` with
Secret `app-{name}-tls`. jc stamps none: the edge routes every App host to jc, whose Portal answers
App `{name}` alone there, and each App host keeps its own certificate (`global.ingress.appHosts`)
so prod's copied Secrets are served the second DNS moves.
"""

import re

import pytest

from conftest import load_yaml

ENV = "jc-prod"
DOMAIN = "smartcity.joinedcontext.com"
# Prod's App Ingresses, kubectl get ingress -n smartcity (2026-11-09T20:2xZ): 38, one host each.
PROD_APPS = {
    "air-quality", "air-weather-explorer", "alerts-desk", "alerts-heatmap", "banskabystrica-data",
    "banskabystrica-mapa", "banskabystrica-ovzdusie", "banskabystrica-skoly", "banskabystrica-zaznamy",
    "bbsk-data", "bbsk-mapa", "bbsk-mosty", "bbsk-podujatia", "bbsk-ukazovatele", "bbsk-zaznamy",
    "bike-rebalancing", "bike-stations", "bike-weather-demand", "data-quality-inspector",
    "district-compare", "event-day-planner", "helsinki-alerts", "helsinki-bikes", "helsinki-data",
    "helsinki-events", "helsinki-kartta", "hsl-transport", "kpi-forecast", "news-topics", "praha-data",
    "praha-mapa", "praha-mesto", "praha-odpad", "transit-reach", "zilina-mapa", "zilina-ukazovatele",
    "zilina-vyskum", "zilina-zaznamy",
}


def of_kind(docs, kind):
    return [d for d in docs if d and d.get("kind") == kind]


def edge_file(docs) -> dict:
    """The APISIX standalone file the render seeds (ConfigMap or Secret)."""
    for doc in docs:
        if doc and doc.get("kind") in ("ConfigMap", "Secret"):
            for value in (doc.get("data") or doc.get("stringData") or {}).values():
                if isinstance(value, str) and "routes:" in value and "#END" in value:
                    return load_yaml(value.replace("#END", ""))
    pytest.fail("no APISIX standalone file in the render")


@pytest.fixture(scope="module")
def docs(rendered):
    return rendered(ENV)


def test_every_prod_app_host_keeps_its_ingress_rule_secret_and_certificate(docs):
    edge = [i for i in of_kind(docs, "Ingress") if i["metadata"]["name"] == "apisix"]
    assert len(edge) == 1
    rules = {r["host"] for r in edge[0]["spec"]["rules"]}
    tls = {t["secretName"]: t["hosts"] for t in edge[0]["spec"]["tls"]}
    certificates = {c["metadata"]["name"]: c["spec"] for c in of_kind(docs, "Certificate")}
    for app in PROD_APPS:
        host = f"{app}.apps.{DOMAIN}"
        assert host in rules, host
        assert tls[f"app-{app}-tls"] == [host]
        spec = certificates[f"app-{app}"]
        assert spec["secretName"] == f"app-{app}-tls"
        assert spec["dnsNames"] == [host]
        assert spec["issuerRef"]["name"] == "letsencrypt-prod"
    # No wildcard reaches a certificate: HTTP-01 cannot prove one, and the edge certificate keeps
    # prod's exact names, so the copied apisix-edge-tls stays Ready.
    for name, spec in certificates.items():
        assert not any("*" in n for n in spec["dnsNames"]), name
    assert sorted(certificates["apisix-edge"]["dnsNames"]) == sorted(
        [DOMAIN] + [f"{h}.{DOMAIN}" for h in ("api", "assistant", "data", "idm", "opendata", "portal", "status", "tiles", "viz")]
    )
    assert not any("*" in host for host in rules)


def test_every_app_host_goes_to_jc_with_no_x_frame_options(docs):
    edge = edge_file(docs)
    routes = {r["id"]: r for r in edge["routes"]}
    apps = routes["jc-apps"]
    assert apps["host"] == f"*.apps.{DOMAIN}"
    assert apps["uri"] == "/*"
    upstreams = {u["id"]: u for u in edge["upstreams"]}
    assert list(upstreams["jc-apps"]["nodes"]) == ["jc.jc-prod.svc.cluster.local:8080"]
    plugins = {p["id"]: p["plugins"] for p in edge["plugin_configs"]}
    mine, jc = plugins[apps["plugin_config_id"]], plugins["jc"]
    # The same forged-header clearing, rate limit and internal-mount refusals as every jc host.
    for name in ("serverless-pre-function", "limit-count", "uri-blocker", "proxy-rewrite"):
        assert mine[name] == jc[name], name
    # The Portal frames an App (AP-122): SAMEORIGIN would refuse it; the App names its framers.
    assert "X-Frame-Options" not in mine["response-rewrite"]["headers"]["set"]
    assert jc["response-rewrite"]["headers"]["set"]["X-Frame-Options"] == "SAMEORIGIN"


def test_an_apps_old_path_on_the_apex_moves_to_its_host(docs):
    route = {r["id"]: r for r in edge_file(docs)["routes"]}["jc-apps-moved"]
    assert route["host"] == DOMAIN
    redirect = route["plugins"]["redirect"]
    assert redirect["ret_code"] == 308 and redirect["append_query_string"] is True
    pattern, target = redirect["regex_uri"]
    for path, want in [
        ("/apps/bike-stations/", f"https://bike-stations.apps.{DOMAIN}/"),
        ("/apps/bike-stations", f"https://bike-stations.apps.{DOMAIN}/"),
        ("/apps/kpi-forecast/assets/x.js", f"https://kpi-forecast.apps.{DOMAIN}/assets/x.js"),
    ]:
        match = re.match(pattern, path)
        assert match, path
        got = target
        for i, group in enumerate(match.groups(), start=1):
            got = got.replace(f"${i}", group or "")
        assert got == want, path
    # A name no App could have never builds a host.
    for path in ("/apps/Evil.example/x", "/apps/../x", "/apps/a.b/"):
        assert not re.match(pattern, path), path


def test_jc_mounts_the_portal_on_every_app_host_and_caches_fetched_builds(docs):
    maps = [c for c in of_kind(docs, "ConfigMap") if "jc.toml" in (c.get("data") or {})]
    assert maps, "no jc.toml in the jc-prod render"
    toml = maps[0]["data"]["jc.toml"]
    entries = re.findall(r'\[\[mounts\.portal\]\]\s*\nhost = "([^"]+)"', toml)
    assert entries == [f"portal.{DOMAIN}", f"*.apps.{DOMAIN}"], entries
    deployments = [d for d in of_kind(docs, "Deployment") if d["metadata"]["name"] == "jc"]
    env = {e["name"]: e.get("value") for c in deployments[0]["spec"]["template"]["spec"]["containers"] for e in c.get("env", [])}
    # On the data volume: /var/lib/jc is the state dir, a 1 MiB memory emptyDir no build fits in.
    assert env["JC_PORTAL_APPS_CACHE_DIR"] == "/var/lib/jc-data/apps-cache"


def test_jc_keeps_the_reconcilers_last_render_on_the_data_volume(docs):
    """T-6708: the last good render of each project repository survives a restart, so it is on
    the persistent data volume, never the 1 MiB memory state dir."""
    deployments = [d for d in of_kind(docs, "Deployment") if d["metadata"]["name"] == "jc"]
    pod = deployments[0]["spec"]["template"]["spec"]
    env = {e["name"]: e.get("value") for c in pod["containers"] for e in c.get("env", [])}
    state = env["JC_PORTAL_RECONCILER_STATE_DIR"]
    mounts = [m["mountPath"] for c in pod["containers"] for m in c.get("volumeMounts", [])]
    assert state.startswith("/var/lib/jc-data/"), state
    assert "/var/lib/jc-data" in mounts, mounts
