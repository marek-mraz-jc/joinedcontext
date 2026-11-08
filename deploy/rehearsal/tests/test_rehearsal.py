"""Tests of the rehearsal kit.

Unit tests run anywhere (`pytest deploy/rehearsal/tests`). The live tests start their own
rehearsal environment `rt` (jc + PostgreSQL in docker) and run only with REHEARSAL_LIVE=1 and a
built jc (JC_BIN or $CARGO_TARGET_DIR/debug/jc).
"""

from __future__ import annotations

import datetime as dt
import http.server
import json
import os
import subprocess
import sys
import threading
import time
import urllib.parse
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(HERE))
import rehearsal as rh  # noqa: E402

NOW = dt.datetime(2026, 11, 8, 12, 0, tzinfo=dt.timezone.utc)


def instance(hours: float, tenant="t", eid="urn:ngsi-ld:X:1"):
    return {"tenant": tenant, "id": eid, "type": "X", "attr": "v",
            "instance": {"type": "Property", "value": hours, "observedAt": rh.iso(NOW - dt.timedelta(hours=hours))}}


def test_an_instance_older_than_24_hours_is_refused_and_a_newer_one_kept():
    cutoff = rh.cutoff_for(NOW)
    assert rh.iso(cutoff) == "2026-11-07T12:00:00Z"
    kept, refused = rh.split_by_cutoff([instance(25), instance(23), instance(0.1)], cutoff)
    assert [k["instance"]["value"] for k in kept] == [23, 0.1]
    assert len(refused) == 1 and "older than the cut-off 2026-11-07T12:00:00Z" in refused[0]["reason"]


def test_an_instance_without_a_time_or_with_a_zone_less_time_is_refused():
    no_time = instance(1)
    del no_time["instance"]["observedAt"]
    zoneless = instance(1)
    zoneless["instance"]["observedAt"] = "2026-11-07T10:00:00"
    kept, refused = rh.split_by_cutoff([no_time, zoneless], rh.cutoff_for(NOW))
    assert kept == [] and len(refused) == 2


def test_people_and_their_credentials_never_reach_the_import():
    realm = {"realm": "dev", "clients": [{"clientId": "c"}], "groups": [{"name": "g"}],
             "users": [
                 {"username": "demo.steward@hel.fi", "credentials": [{"type": "password", "value": "x"}],
                  "groups": ["/g"]},
                 {"username": "service-account-c", "serviceAccountClientId": "c",
                  "clientRoles": {"realm-management": ["query-users"]},
                  "credentials": [{"type": "password", "value": "y"}]},
             ],
             "federatedUsers": [{"username": "f"}]}
    clean = rh.strip_people(realm)
    assert [u["username"] for u in clean["users"]] == ["service-account-c"]
    assert "credentials" not in clean["users"][0]
    assert clean["users"][0]["clientRoles"] == {"realm-management": ["query-users"]}
    assert "federatedUsers" not in clean and clean["groups"] == realm["groups"]
    tested = rh.add_tester(clean)
    tester = tested["users"][-1]
    assert tester["username"] == rh.TESTER
    assert tester["credentials"][0]["value"] == "$(env:REHEARSAL_TESTER_PASSWORD)"


def test_only_mappers_kaisatsu_does_not_know_are_left_out_and_named():
    realm = {"clientScopes": [{"name": "address", "protocolMappers": [
        {"name": "address", "protocolMapper": "oidc-address-mapper"},
        {"name": "aud", "protocolMapper": "oidc-audience-mapper"}]}]}

    def check(r):
        for i, sc in enumerate(r["clientScopes"]):
            for mi, m in enumerate(sc["protocolMappers"]):
                if m["protocolMapper"] != "oidc-audience-mapper":
                    return (f"$.clientScopes[{i}].protocolMappers[{mi}].protocolMapper: "
                            f"unknown mapper kind '{m['protocolMapper']}'")
        return ""
    clean, dropped = rh.drop_refused_mappers(realm, check)
    assert [m["name"] for m in clean["clientScopes"][0]["protocolMappers"]] == ["aud"]
    assert dropped == [{"in": "clientScopes/address", "mapper": "address", "kind": "oidc-address-mapper"}]
    with pytest.raises(SystemExit):
        rh.drop_refused_mappers({"x": 1}, lambda r: "$.clients[0].secret: a literal secret")


def test_an_unmapped_flow_is_left_out_only_when_nothing_uses_it():
    def check(r):
        for i, f in enumerate(r["authenticationFlows"]):
            if f["alias"] not in ("custom browser",):
                return f"$.authenticationFlows[{i}].alias: unmappable authentication flow '{f['alias']}'"
        return ""
    realm = {"browserFlow": "custom browser", "authenticationFlows": [
        {"alias": "custom browser", "authenticationExecutions": []},
        {"alias": "saml ecp", "authenticationExecutions": []}]}
    clean, dropped = rh.drop_refused_mappers(realm, check)
    assert [f["alias"] for f in clean["authenticationFlows"]] == ["custom browser"]
    assert dropped[0]["flow"] == "saml ecp"
    bound = {"browserFlow": "Organization", "authenticationFlows": [{"alias": "Organization"}]}
    with pytest.raises(SystemExit):
        rh.drop_refused_mappers(bound, check)
    def unknown_step(r):
        for i, f in enumerate(r["authenticationFlows"]):
            for ei, e in enumerate(f.get("authenticationExecutions") or []):
                if e["authenticator"] in ("client-x509",):
                    return (f"$.authenticationFlows[{i}].authenticationExecutions[{ei}].authenticator: "
                            f"unknown authenticator '{e['authenticator']}'")
        return ""
    clients = {"clientAuthenticationFlow": "clients", "clients": [{"clientAuthenticatorType": "client-secret"}],
               "authenticationFlows": [{"alias": "clients", "authenticationExecutions": [
                   {"authenticator": "client-secret", "requirement": "ALTERNATIVE"},
                   {"authenticator": "client-x509", "requirement": "ALTERNATIVE"}]}]}
    clean, dropped = rh.drop_refused_mappers(clients, unknown_step)
    assert [e["authenticator"] for e in clean["authenticationFlows"][0]["authenticationExecutions"]] == ["client-secret"]
    assert dropped[0]["authenticator"] == "client-x509"
    clients["clients"].append({"clientAuthenticatorType": "client-x509"})
    clients["authenticationFlows"][0]["authenticationExecutions"].append(
        {"authenticator": "client-x509", "requirement": "ALTERNATIVE"})
    with pytest.raises(SystemExit):
        rh.drop_refused_mappers(clients, unknown_step)
    docker = {"dockerAuthenticationFlow": "docker auth", "clients": [],
              "authenticationFlows": [{"alias": "docker auth", "authenticationExecutions": [
                  {"authenticator": "client-x509", "requirement": "REQUIRED"}]}]}
    clean, _ = rh.drop_refused_mappers(docker, unknown_step)
    assert clean["authenticationFlows"] == [] and "dockerAuthenticationFlow" not in clean
    docker = {"dockerAuthenticationFlow": "docker auth", "clients": [{"protocol": "docker-v2"}],
              "authenticationFlows": [{"alias": "docker auth", "authenticationExecutions": [
                  {"authenticator": "client-x509", "requirement": "REQUIRED"}]}]}
    with pytest.raises(SystemExit):
        rh.drop_refused_mappers(docker, unknown_step)
    broker = {"firstBrokerLoginFlow": "first broker login", "identityProviders": [{"providerId": "kubernetes"}],
              "clients": [], "authenticationFlows": [{"alias": "first broker login", "authenticationExecutions": [
                  {"authenticator": "client-x509", "requirement": "REQUIRED"}]}]}
    clean, _ = rh.drop_refused_mappers(broker, unknown_step)
    assert clean["authenticationFlows"] == [] and "firstBrokerLoginFlow" not in clean
    broker["identityProviders"] = [{"providerId": "oidc"}]
    broker["firstBrokerLoginFlow"] = "first broker login"
    broker["authenticationFlows"] = [{"alias": "first broker login", "authenticationExecutions": [
        {"authenticator": "client-x509", "requirement": "REQUIRED"}]}]
    with pytest.raises(SystemExit):
        rh.drop_refused_mappers(broker, unknown_step)
    client = {"browserFlow": "custom browser", "clients": [{"authenticationFlowBindingOverrides": {"browser": "x"}}],
              "authenticationFlows": [{"alias": "x"}]}
    with pytest.raises(SystemExit):
        rh.drop_refused_mappers(client, check)


def test_seed_files_are_cut_into_repositories_as_layout_2():
    seed = {
        "organizations__hel.yaml": "kind: Organization\n",
        "projects__helsinki__project.yaml": "apiVersion: joinedcontext.com/v1alpha1\nkind: Project\nspec:\n  organizationRef: hel\n",
        "projects__helsinki__spaces__helsinki__space.yaml": "kind: ContextSpace\n",
    }
    repos = rh.layout2(seed, "joinedcontext", "configuration")
    assert set(repos) == {("joinedcontext", "configuration"), ("joinedcontext", "helsinki")}
    assert repos[("joinedcontext", "helsinki")]["spaces/helsinki/space.yaml"] == "kind: ContextSpace\n"
    org = repos[("joinedcontext", "configuration")]
    assert "repository:\n    name: helsinki" in org["projects/helsinki.yaml"]
    assert org[".jc/layout"] == "2\n" and "organizations/hel.yaml" in org
    with pytest.raises(ValueError):
        rh.layout2({"projects__x__project.yaml": "kind: Project\n"}, "o", "r")


def test_the_importer_writes_only_to_a_loopback_jc():
    for base in ["http://localhost:1", "http://127.0.0.1:1", "http://[::1]:1"]:
        rh.loopback_only(base)
    for base in ["https://portal.dev.joinedcontext.com", "http://10.0.0.5:8080", "http://116.203.134.249"]:
        with pytest.raises(SystemExit):
            rh.loopback_only(base)


def test_a_catalogue_instance_publishes_only_into_the_rehearsal():
    ckan = (b"apiVersion: joinedcontext.com/v1alpha1\nkind: CkanInstance\nmetadata:\n  name: zilina\n"
            b"spec:\n  url: https://opendata.dev.joinedcontext.com\n  organizationDefault: zilina\n")
    fenced = rh.fence_writes(ckan)
    assert b"dev.joinedcontext.com" not in fenced
    assert b"  url: https://catalogue.rehearsal.invalid\n" in fenced
    endpoint = b"kind: Endpoint\nspec:\n  catalog:\n    publisher:\n      url: https://dev.joinedcontext.com/\n"
    assert rh.fence_writes(endpoint) == endpoint


def test_entity_batches_keep_their_tenant_and_stay_under_the_caps():
    lines = ([{"tenant": "a", "entity": {"id": f"urn:ngsi-ld:X:{i}"}} for i in range(1203)]
             + [{"tenant": "b", "entity": {"id": "urn:ngsi-ld:X:b"}}]
             + [{"tenant": "a", "entity": {"id": "urn:ngsi-ld:X:a2"}}])
    batches = list(rh._tenant_batches(iter(lines)))
    assert all(len(b) <= rh.MAX_BATCH_ENTITIES for _, b in batches)
    assert [(t, len(b)) for t, b in batches] == [("a", 500), ("a", 500), ("a", 203), ("b", 1), ("a", 1)]


def test_the_import_never_sends_an_instance_older_than_the_window(tmp_path, monkeypatch):
    """The real importer against a stub broker: the 25 h instance is refused in the report and
    never reaches the broker; the window ends at the export time, not at the import."""
    monkeypatch.setattr(rh, "DISK_RESERVE", 0)  # this test is about the window, not the host disk
    seen = []

    class Broker(http.server.BaseHTTPRequestHandler):
        def do_POST(self):  # noqa: N802
            body = self.rfile.read(int(self.headers["Content-Length"]))
            seen.append((self.path, self.headers.get("NGSILD-Tenant"), json.loads(body)))
            self.send_response(201 if "temporal" in self.path else 204)
            self.end_headers()

        def log_message(self, *a):
            pass

    server = http.server.HTTPServer(("127.0.0.1", 0), Broker)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    export = tmp_path / "export"
    (export / "antares").mkdir(parents=True)
    later = NOW + dt.timedelta(hours=5)
    (export / "antares" / "entities.jsonl").write_text(json.dumps(
        {"tenant": "t", "entity": {"id": "urn:ngsi-ld:X:1", "type": "X"}}) + "\n")
    (export / "antares" / "temporal.jsonl").write_text("".join(json.dumps(instance(d)) + "\n" for d in (25, 23, 1)))
    (export / "manifest.json").write_text(json.dumps({"env": "unit", "exportedAt": rh.iso(NOW)}))  # written last
    (tmp_path / "antares-admin-token").write_text("unit-admin-token\n")
    report = tmp_path / "report.json"
    def run(now):
        seen.clear()
        rh.main(["import", "--export", str(export), "--base", f"http://127.0.0.1:{server.server_port}",
                 "--state", str(tmp_path), "--report", str(report), "--only", "antares", "--now", rh.iso(now)])
        return json.loads(report.read_text()), sorted(
            i["value"] for path, _, body in seen if "temporal" in path for i in body["v"])
    r, sent = run(NOW)
    assert r["cutoff"] == "2026-11-07T12:00:00Z"
    assert r["antares"]["temporal"] == 2 and len(r["antares"]["temporalRefused"]) == 1
    assert sent == [1, 23]
    assert all(tenant == "t" for _, tenant, _ in seen)
    # Imported 5 h after the export: the 23 h instance is 28 h old now and stays out.
    r, sent = run(later)
    assert r["cutoff"] == "2026-11-07T17:00:00Z" and sent == [1]
    server.shutdown()


# ------------------------------------------------------------------------------------------------
# live: the rolling 24 h rule in a running rehearsal environment

LIVE = os.environ.get("REHEARSAL_LIVE") == "1"
PORT, PG_PORT = 42119, 55449


@pytest.fixture(scope="module")
def env(tmp_path_factory):
    if not LIVE:
        pytest.skip("REHEARSAL_LIVE=1 starts a jc and a PostgreSQL container")
    home = tmp_path_factory.mktemp("rehearsal")
    run_env = dict(os.environ, REHEARSAL_HOME=str(home), REHEARSAL_SWEEP_SECS="3", REHEARSAL_RETAIN_SECS="3")
    subprocess.run([str(HERE / "up.sh"), "rt", str(PORT), str(PG_PORT)], env=run_env, check=True, timeout=600)
    yield home / "rt"
    for pid in ("jc.pid", "retain.pid"):
        f = home / "rt" / pid
        if f.exists():
            subprocess.run(["kill", f.read_text().strip()], check=False)
    subprocess.run(["docker", "rm", "-f", "rh-pg-rt"], check=False, capture_output=True)


def temporal_values(h, tenant, eid, attr):
    st, body = h.json("GET", f"/broker/ngsi-ld/v1/temporal/entities/{urllib.parse.quote(eid, safe='')}"
                             f"?attrs={attr}&timerel=after&timeAt=2000-01-01T00:00:00Z",
                      headers={"NGSILD-Tenant": tenant})
    assert st == 200, body
    inst = body.get(attr) or []
    inst = inst if isinstance(inst, list) else [inst]
    return sorted(i["value"] for i in inst)


def test_antares_deletes_history_older_than_24_hours_and_never_the_current_state(env):
    h = rh.Http(f"http://localhost:{PORT}")
    now = rh.utcnow()
    tenant, eid = "rt-space", "urn:ngsi-ld:Sensor:rt-1"
    old = rh.iso(now - dt.timedelta(hours=25))
    # The current state itself was observed 25 h ago: it must survive.
    st, body = h.json("POST", "/broker/ngsi-ld/v1/entities", headers={"NGSILD-Tenant": tenant}, body={
        "id": eid, "type": "Sensor", "reading": {"type": "Property", "value": 42, "observedAt": old}})
    assert st in (201, 204, 409), body
    st, body = h.json("POST", "/broker/ngsi-ld/v1/temporal/entities", headers={"NGSILD-Tenant": tenant}, body={
        "id": eid, "type": "Sensor", "history": [
            {"type": "Property", "value": 25, "observedAt": old},
            {"type": "Property", "value": 23, "observedAt": rh.iso(now - dt.timedelta(hours=23))}]})
    assert st in (201, 204), body
    deadline = time.time() + 60
    while time.time() < deadline and temporal_values(h, tenant, eid, "history") != [23]:
        time.sleep(1)
    assert temporal_values(h, tenant, eid, "history") == [23], "the 25 h instance stays or the 23 h one went"
    st, current = h.json("GET", f"/broker/ngsi-ld/v1/entities/{urllib.parse.quote(eid, safe='')}",
                         headers={"NGSILD-Tenant": tenant})
    assert st == 200 and current["reading"]["value"] == 42, current



def test_a_bundle_still_being_written_is_not_imported(tmp_path):
    (tmp_path / "antares").mkdir()
    data = tmp_path / "antares" / "entities.jsonl"
    data.write_text("")
    manifest = tmp_path / "manifest.json"
    manifest.write_text("{}")
    os.utime(data, (1, 1))
    assert rh.bundle_settled(tmp_path) == ""
    os.utime(data, None)
    os.utime(manifest, (1, 1))
    assert "written after manifest.json" in rh.bundle_settled(tmp_path)
    (tmp_path / "antares" / ".parts").mkdir()
    (tmp_path / "antares" / ".parts" / "t.done").write_text("")
    os.utime(data, (1, 1))
    assert rh.bundle_settled(tmp_path) == ""
    os.utime(data, None)
    with pytest.raises(SystemExit):
        rh.main(["import", "--export", str(tmp_path), "--base", "http://127.0.0.1:1", "--state", str(tmp_path),
                 "--report", str(tmp_path / "r.json")])


def test_the_broker_after_the_import_refuses_an_anonymous_write():
    assert rh.policy_problems(401, "joinedcontext") == []
    assert rh.policy_problems(201, "joinedcontext") == ["an anonymous write answered 201, not 401"]
    assert rh.policy_problems(401, "allow-all") == ["the broker's policy engine is 'allow-all', not 'joinedcontext'"]
    assert len(rh.policy_problems(403, None)) == 2


def test_the_old_history_check_fails_when_it_cannot_read(tmp_path):
    pw = tmp_path / "pw"
    pw.write_text("x")
    with pytest.raises(RuntimeError):
        rh.old_history("127.0.0.1", 1, "antares", "antares", str(pw), dt.datetime(2026, 11, 7, tzinfo=dt.UTC))


def test_a_datasource_reads_the_same_endpoint_on_the_rehearsal():
    gw = "http://context-gateway.smartcity.svc.cluster.local:8080/api/endpoint/jtjnhkaau64zmzz5csmcwem52icvufy2"
    assert rh.rehearsal_endpoint_url(gw, "http://localhost:42102/") == \
        "http://localhost:42102/api/endpoint/jtjnhkaau64zmzz5csmcwem52icvufy2"
    assert rh.rehearsal_endpoint_url("http://prometheus:9090", "http://localhost:42102") == "http://prometheus:9090"


def test_each_project_dashboard_goes_into_its_own_organization(tmp_path):
    """The real importer against a stub daiya: a project's folder, datasource and dashboard carry
    its organization (made once, 409 -> by name), the datasource names the rehearsal's gateway, an
    existing datasource is replaced, and a platform model stays in the main organization."""
    seen = []

    class Daiya(http.server.BaseHTTPRequestHandler):
        def _answer(self, status, body):
            raw = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def _record(self):
            n = int(self.headers.get("Content-Length") or 0)
            body = json.loads(self.rfile.read(n)) if n else None
            seen.append((self.command, self.path, self.headers.get("X-Grafana-Org-Id"), body))
            return body

        def do_POST(self):  # noqa: N802
            self._record()
            if self.path == "/dashboards/api/orgs":
                return self._answer(409, {"message": "exists"})
            if self.path == "/dashboards/api/datasources":
                return self._answer(409, {"message": "exists"})
            self._answer(200, {"status": "success"})

        def do_PUT(self):  # noqa: N802
            self._record()
            self._answer(200, {})

        def do_GET(self):  # noqa: N802
            self._record()
            self._answer(200, {"id": 7, "name": "helsinki"})

        def log_message(self, *a):
            pass

    server = http.server.HTTPServer(("127.0.0.1", 0), Daiya)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_port}"
    state = tmp_path / "state"
    state.mkdir()
    (state / "dashboards-password").write_text("pw")
    d = tmp_path / "dash"
    d.mkdir()
    (d / "helsinki.folder.json").write_text(json.dumps({"uid": "project-helsinki", "title": "Helsinki"}))
    (d / "helsinki.datasource.air.json").write_text(json.dumps({
        "uid": "air", "name": "air", "type": "ngsild-grafana-datasource",
        "url": "http://context-gateway.dev.svc.cluster.local:8080/api/endpoint/abc"}))
    (d / "helsinki.dashboard.json").write_text(json.dumps({"folderUid": "project-helsinki", "dashboard": {"uid": "h", "title": "H"}}))
    (d / "portal.json").write_text(json.dumps({"uid": "portal", "title": "Portal"}))
    report = {}
    try:
        rh.import_daiya(rh.Http(base), state, tmp_path, None, report, d)
    finally:
        server.shutdown()
    assert {r["result"] for r in report["daiya"]} <= {"imported", "unchanged"}, report
    org_calls = [s for s in seen if "/orgs" in s[1]]
    assert [c[:2] for c in org_calls] == [("POST", "/dashboards/api/orgs"), ("GET", "/dashboards/api/orgs/name/helsinki")]
    project = [s for s in seen if s[1].startswith(("/dashboards/api/folders", "/dashboards/api/datasources",
                                                   "/dashboards/api/dashboards/db")) and s[3].get("dashboard", {}).get("uid") != "portal"]
    assert project and all(s[2] == "7" for s in project), project
    put = [s for s in seen if s[0] == "PUT"]
    assert put and put[0][1] == "/dashboards/api/datasources/uid/air"
    assert put[0][3]["url"] == f"{base}/api/endpoint/abc"
    model = [s for s in seen if (s[3] or {}).get("dashboard", {}).get("uid") == "portal"]
    assert model and model[0][2] is None


def test_only_the_guard_refusing_the_own_listener_counts_as_expected():
    base = "http://localhost:42101"
    own = base + "/api/endpoint/abc"
    refused = {"error": {"code": "cr.ssrf_blocked", "message": "disallowed URL scheme 'http' (https required)"}}
    assert rh.panel_class(502, refused, own, base) == "expected"
    loop = {"title": "Outbound Request Blocked", "detail": "blocked loopback address 127.0.0.1"}
    assert rh.panel_class(502, loop, own, base) == "expected"
    # the same refusal for a datasource elsewhere, another error, or a refusal text alone: red
    assert rh.panel_class(502, refused, "http://prometheus:9090", base) == "red"
    assert rh.panel_class(500, {"error": "the panel's datasource did not answer"}, own, base) == "red"
    assert rh.panel_class(502, "disallowed URL scheme 'http'", own, base) == "red"
    assert rh.panel_class(200, {"frames": []}, own, base) == "data"


@pytest.mark.parametrize("script, ok", [
    ("echo 'config file jc.toml: unknown field `database`' >> {log}; exit 2", False),
    ("echo 'jc listening' >> {log}; sleep 1; exit 1", False),
    ("echo 'jc listening' >> {log}; sleep 30", True),
])
def test_up_stops_when_jc_exits_at_start(tmp_path, script, ok):
    log = tmp_path / "jc.log"
    log.write_text("")
    jc = subprocess.Popen(["bash", "-c", script.format(log=log)])
    try:
        r = subprocess.run([str(HERE / "wait-jc.sh"), str(jc.pid), str(log), "20"],
                           env=dict(os.environ, REHEARSAL_SETTLE_SECS="3"), capture_output=True, text=True, timeout=60)
    finally:
        jc.kill()
        jc.wait()
    assert (r.returncode == 0) == ok, r.stderr


def test_an_import_that_would_fill_the_shared_disk_does_not_start():
    # the dev bundle of 2026-11-08 on 35 GB free: refused, naming the numbers
    why = rh.disk_problem(1_822_836, 35 * 10**9)
    assert "1,822,836 entities" in why and "35.0 GB are free" in why
    assert rh.disk_problem(1_822_836, 100 * 10**9) == ""
    assert rh.disk_problem(260, 30 * 10**9) == ""


def clean_import():
    return {"entities": 10, "exportEntities": 10, "refusedCallCount": 0, "temporal": 5, "temporalKept": 5,
            "temporalRefusedByKind": {"cut-off": 3}}


@pytest.mark.parametrize("change, word", [
    ({"entities": 0}, "0 of 10 exported entities"),
    ({"refusedCallCount": 4439}, "4,439 broker calls refused"),
    ({"temporal": 4}, "4 of 5 temporal instances"),
    ({"temporalRefusedByKind": {"cut-off": 3, "no-observedAt": 1}}, "no-observedAt"),
])
def test_a_short_or_refused_import_is_a_problem_and_the_cut_off_alone_is_not(change, word):
    assert rh.antares_problems(clean_import()) == []
    problems = rh.antares_problems({**clean_import(), **change})
    assert len(problems) == 1 and word in problems[0], problems


def stub_import(tmp_path, admit):
    """The importer against a stub broker that admits a subject-less write only with the admin
    bearer `admit` (jc's broker since T-5032); returns (rc, report, Authorization headers seen)."""
    seen = []

    class Broker(http.server.BaseHTTPRequestHandler):
        def do_POST(self):  # noqa: N802
            self.rfile.read(int(self.headers["Content-Length"]))
            seen.append(self.headers.get("Authorization"))
            ok = self.headers.get("Authorization") == f"Bearer {admit}"
            body = b"" if ok else json.dumps({"status": 401, "detail": "a request without a subject needs "
                                                                          "the broker's admin bearer token"}).encode()
            self.send_response((201 if "temporal" in self.path else 204) if ok else 401)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *a):
            pass

    server = http.server.HTTPServer(("127.0.0.1", 0), Broker)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    export = tmp_path / "export"
    (export / "antares").mkdir(parents=True)
    (export / "antares" / "entities.jsonl").write_text("".join(json.dumps(
        {"tenant": "t", "entity": {"id": f"urn:ngsi-ld:X:{i}", "type": "X"}}) + "\n" for i in range(3)))
    (export / "antares" / "temporal.jsonl").write_text(json.dumps(instance(1)) + "\n")
    (export / "manifest.json").write_text(json.dumps({"env": "unit", "exportedAt": rh.iso(NOW)}))
    state = tmp_path / "state"
    state.mkdir()
    (state / "antares-admin-token").write_text("the-admin-token\n")
    report = tmp_path / "report.json"
    try:
        rc = rh.main(["import", "--export", str(export), "--base", f"http://127.0.0.1:{server.server_port}",
                      "--state", str(state), "--report", str(report), "--only", "antares", "--now", rh.iso(NOW)])
    finally:
        server.shutdown()
    return rc, json.loads(report.read_text()), seen


def test_the_import_sends_the_broker_admin_token_and_fails_when_the_broker_refuses(tmp_path, monkeypatch):
    monkeypatch.setattr(rh, "DISK_RESERVE", 0)
    rc, r, seen = stub_import(tmp_path / "ok", "the-admin-token")
    assert rc == 0 and r["problems"] == [] and r["antares"]["entities"] == 3 and r["antares"]["temporal"] == 1
    assert seen and set(seen) == {"Bearer the-admin-token"}
    # the dev run of 2026-11-08 21:44: every call 401, 0 entities, and the import said rc 0
    rc, r, _ = stub_import(tmp_path / "refused", "another-token")
    assert rc == 1 and r["antares"]["entities"] == 0
    assert any("0 of 3 exported entities" in p for p in r["problems"])


def test_import_sh_stops_the_allow_all_jc_when_the_import_fails(tmp_path):
    """import.sh with stand-ins: up.sh starts a 'jc' (sleep), rehearsal.py import fails; the
    script must fail and the allow-all jc must be gone."""
    kit = tmp_path / "kit"
    kit.mkdir()
    (kit / "import.sh").write_text((HERE / "import.sh").read_text())
    home = tmp_path / "home"
    (kit / "up.sh").write_text(f"#!/bin/bash\nmkdir -p {home}/e\nsetsid sleep 300 >/dev/null 2>&1 &\necho $! > {home}/e/jc.pid\n")
    (kit / "rehearsal.py").write_text("import sys\nsys.exit(1)\n")
    for f in ("import.sh", "up.sh"):
        (kit / f).chmod(0o755)
    r = subprocess.run([str(kit / "import.sh"), "e", "1", "2", str(tmp_path)], capture_output=True, text=True,
                       env=dict(os.environ, REHEARSAL_HOME=str(home)), timeout=60)
    pid = int((home / "e" / "jc.pid").read_text())
    time.sleep(0.5)
    alive = Path(f"/proc/{pid}").exists() and "Z" not in Path(f"/proc/{pid}/stat").read_text().split()[2]
    if alive:
        os.kill(pid, 9)
    assert r.returncode != 0 and "import FAILED" in r.stderr
    assert not alive, "jc on allow-all stayed up after a failed import"


def test_people_come_with_ids_groups_and_roles_and_never_a_credential():
    lines = [
        {"id": "de300000-0000-4000-8000-000000000001", "username": "demo.steward@hel.fi",
         "email": "demo.steward@hel.fi", "firstName": "Demo", "enabled": True,
         "attributes": {"jc_org": ["hel"], "x": ["$(env:SECRET)"], "$(file:/etc/shadow)": ["y"]},
         "groups": ["/helsinki/stewards"], "realmRoles": ["portal-approver"], "totp": True,
         "requiredActions": ["VERIFY_EMAIL"],
         "credentials": [{"type": "password", "secretData": "{\"value\":\"hash\"}"}],
         "federatedIdentities": [{"identityProvider": "x"}], "access": {"manage": True}},
        {"id": "u-2", "username": "service-account-c", "serviceAccountClientId": "c"},
        {"id": "u-3", "username": "jane.roe@example.org", "email": "jane.roe@example.org",
         "firstName": "Jane", "lastName": "Roe", "emailVerified": True, "enabled": True,
         "groups": ["/bbsk"]},
    ]
    known = {"demo.steward", "de300000-0000-4000-8000-000000000001"}
    realm, counts = rh.add_people({"realm": "dev", "users": [{"username": "service-account-c"}]}, lines, known)
    assert counts == {"people": 2, "personas": 1, "pseudonymous": 1, "credentials": 0}
    assert [u["username"] for u in realm["users"]] == ["service-account-c", "demo.steward@hel.fi", "person-u-3"]
    steward, other = realm["users"][1], realm["users"][2]
    assert steward["email"] == "demo.steward@hel.fi" and steward["groups"] == ["/helsinki/stewards"]
    assert steward["realmRoles"] == ["portal-approver"] and steward["attributes"] == {"jc_org": ["hel"], "x": []}
    assert steward["requiredActions"] == ["VERIFY_EMAIL", "UPDATE_PASSWORD", "CONFIGURE_TOTP"]
    assert other == {"id": "u-3", "username": "person-u-3", "enabled": True, "groups": ["/bbsk"],
                     "attributes": {}, "requiredActions": ["UPDATE_PASSWORD"]}
    text = json.dumps(realm)
    for leak in ("credentials", "secretData", "hash", "federatedIdentities", "totp", "access", "$(",
                 "jane", "Roe", "example.org"):
        assert leak not in text, leak


def test_personas_are_the_demo_users_of_the_deployment(tmp_path):
    f = tmp_path / "demo-users.yaml"
    f.write_text("---\ndemo.steward:\n  id: de300000-0000-4000-8000-000000000001\n  firstName: Demo\n")
    assert rh.personas(f) == {"demo.steward", "de300000-0000-4000-8000-000000000001"}
    assert rh.personas(None) == set()


def test_people_stay_in_a_private_dir_and_a_private_file(tmp_path):
    export = tmp_path / "export"
    (export / "keycloak").mkdir(parents=True)
    (export / "keycloak" / "realm-dev.json").write_text(json.dumps({"realm": "dev"}))
    people = tmp_path / "people"
    people.mkdir(mode=0o755)
    (people / "users-dev.jsonl").write_text(json.dumps({"id": "u-3", "username": "jane.roe@example.org"}) + "\n")
    out = tmp_path / "state" / "realms"
    argv = ["realms", "--export", str(export), "--out", str(out), "--people", str(people)]
    os.chmod(people, 0o755)
    with pytest.raises(SystemExit):
        rh.main(argv)
    os.chmod(people, 0o700)
    rh.main(argv)
    assert (out / "dev.json").stat().st_mode & 0o777 == 0o600 and out.stat().st_mode & 0o777 == 0o700
    assert "person-u-3" in (out / "dev.json").read_text()
    assert json.loads((out.parent / "realm-dev-people.json").read_text())["people"] == 1



def test_datasources_are_provisioned_on_the_exact_own_address_in_org_1(tmp_path):
    src = tmp_path / "dash"
    src.mkdir()
    (src / "bbsk.datasource.bbsk-kpi.json").write_text(json.dumps({
        "name": "bbsk-kpi", "uid": "bbsk-kpi", "type": "ngsild-grafana-datasource", "access": "proxy",
        "url": "http://context-gateway.dev.svc.cluster.local:8080/api/endpoint/qfhhh5no5wz4lk3rfjisdtx3chiyfig3",
        "jsonData": {"tenant": "x"}}))
    out = tmp_path / "prov"
    assert rh.provisioning_files(src, 42101, out) == 1
    doc = json.loads((out / "datasources" / "rehearsal.yaml").read_text())
    (ds,) = doc["datasources"]
    assert ds["url"] == "http://127.0.0.1:42101/api/endpoint/qfhhh5no5wz4lk3rfjisdtx3chiyfig3"
    assert ds["orgId"] == 1 and ds["uid"] == "bbsk-kpi" and ds["jsonData"] == {"tenant": "x"}
    assert (out / "dashboards").is_dir()
    (src / "x.datasource.bad.json").write_text(json.dumps({"name": "b", "uid": "b", "type": "t",
                                                            "url": "http://evil.example/other"}))
    with pytest.raises(SystemExit):
        rh.provisioning_files(src, 42101, out)


def test_org_1_panels_count_data_and_red_apart(tmp_path):
    class Daiya(http.server.BaseHTTPRequestHandler):
        def do_POST(self):  # noqa: N802
            q = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            uid = q["queries"][0]["datasource"]["uid"]
            body = {"results": {"A": {"status": 200, "frames": []}}} if uid == "good" else \
                {"message": "disallowed URL scheme 'http' (https required)"}
            raw = json.dumps(body).encode()
            self.send_response(200 if uid == "good" else 502)
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def log_message(self, *a):
            pass

    server = http.server.HTTPServer(("127.0.0.1", 0), Daiya)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    (tmp_path / "p.dashboard.json").write_text(json.dumps({"dashboard": {"panels": [
        {"id": 1, "type": "timeseries", "datasource": {"uid": "good"}, "targets": [{"refId": "A"}]},
        {"id": 2, "type": "row", "panels": [
            {"id": 3, "type": "stat", "datasource": "bad", "targets": [{"refId": "A"}]}]},
        {"id": 4, "type": "text"}]}}))
    try:
        o = rh.org1_panels(rh.Http(f"http://127.0.0.1:{server.server_port}"), {}, tmp_path)
    finally:
        server.shutdown()
    assert (o["data"], o["red"]) == (1, 1) and [r["panel"] for r in o["rows"]] == [1, 3]


def test_a_portal_made_dashboard_goes_into_the_org_of_its_source_project(tmp_path):
    seen = []

    class Daiya(http.server.BaseHTTPRequestHandler):
        def answer(self, st, body):
            raw = json.dumps(body).encode()
            self.send_response(st)
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def do_POST(self):  # noqa: N802
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            if self.path == "/dashboards/api/orgs":
                return self.answer(409, {"message": "exists"})
            seen.append((self.headers.get("X-Grafana-Org-Id"), body["dashboard"]["uid"], "id" in body["dashboard"]))
            self.answer(200, {"status": "success"})

        def do_GET(self):  # noqa: N802
            self.answer(200, {"id": 6, "name": self.path.rsplit("/", 1)[-1]})

        def log_message(self, *a):
            pass

    server = http.server.HTTPServer(("127.0.0.1", 0), Daiya)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    src = tmp_path / "grafana" / "dashboards"
    src.mkdir(parents=True)
    (src / "org3-project-helsinki.json").write_text(json.dumps({"uid": "project-helsinki"}))
    (src / "org3-pv-ab12.json").write_text(json.dumps({"id": 41, "uid": "pv-ab12", "title": "Air"}))
    (src / "org9-pv-cd34.json").write_text(json.dumps({"uid": "pv-cd34"}))
    try:
        rows = rh.import_portal_made(rh.Http(f"http://127.0.0.1:{server.server_port}"), {}, tmp_path, {})
    finally:
        server.shutdown()
    assert seen == [("6", "pv-ab12", False)]
    assert [(r["file"], r["project"], r["result"]) for r in rows] == [
        ("org3-pv-ab12.json", "helsinki", "imported"), ("org9-pv-cd34.json", None, "refused")]


@pytest.mark.parametrize("dist_age, bin_age, ok", [(-10, -5, False), (10, 20, True), (10, 5, False)])
def test_a_jc_with_a_stale_portal_ui_does_not_start(tmp_path, dist_age, bin_age, ok):
    """ui-fresh.sh: ui/dist older than the last UI commit, or a jc linked before ui/dist, fails."""
    clone = tmp_path / "clone"
    ui = clone / "crates/portal/ui"
    (ui / "src").mkdir(parents=True)
    (ui / "src" / "App.tsx").write_text("x")
    g = ["git", "-C", str(clone), "-c", "user.email=t@t", "-c", "user.name=t"]
    subprocess.run([*g, "init", "-q"], check=True)
    subprocess.run([*g, "add", "-A"], check=True)
    at = int(time.time()) - 1000
    subprocess.run([*g, "commit", "-qm", "ui"], check=True,
                   env=dict(os.environ, GIT_COMMITTER_DATE=f"@{at} +0000", GIT_AUTHOR_DATE=f"@{at} +0000"))
    (ui / "dist").mkdir()
    (ui / "dist" / "index.html").write_text("<html>")
    os.utime(ui / "dist" / "index.html", (at + dist_age, at + dist_age))
    jc = tmp_path / "jc"
    jc.write_text("")
    os.utime(jc, (at + bin_age, at + bin_age))
    r = subprocess.run([str(HERE / "ui-fresh.sh"), str(clone), str(jc)], capture_output=True, text=True)
    assert (r.returncode == 0) == ok, r.stderr


def test_a_public_endpoint_answers_as_the_source_did():
    src = {"efkkw7pka4bhc3igujpzhrs2ra": {"status": 403}}
    j = "application/json"
    assert rh.endpoint_answer_problem("aaaa", 200, j, b"[]", src) == ""
    assert "aaaa: 403" in rh.endpoint_answer_problem("aaaa", 403, j, b"{}", src)
    assert rh.endpoint_answer_problem("efkkw7pka4bhc3igujpzhrs2ra", 403, j, b"{}", src) == ""
    # the source refused it: a 200 in the rehearsal is a difference too (a grant opened)
    assert "(source 403)" in rh.endpoint_answer_problem("efkkw7pka4bhc3igujpzhrs2ra", 200, j, b"[]", src)
    assert rh.endpoint_answer_problem("aaaa", 200, "text/html", b"<html>", src)
