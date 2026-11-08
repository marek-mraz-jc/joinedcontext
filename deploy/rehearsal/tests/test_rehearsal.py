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
    fenced = rh.fence_writes(ckan, "http://localhost:42101/")
    assert b"dev.joinedcontext.com" not in fenced
    assert b"  url: http://localhost:42101/opendata\n" in fenced
    endpoint = b"kind: Endpoint\nspec:\n  catalog:\n    publisher:\n      url: https://dev.joinedcontext.com/\n"
    assert rh.fence_writes(endpoint, "http://localhost:1") == endpoint


def test_entity_batches_keep_their_tenant_and_stay_under_the_caps():
    lines = ([{"tenant": "a", "entity": {"id": f"urn:ngsi-ld:X:{i}"}} for i in range(1203)]
             + [{"tenant": "b", "entity": {"id": "urn:ngsi-ld:X:b"}}]
             + [{"tenant": "a", "entity": {"id": "urn:ngsi-ld:X:a2"}}])
    batches = list(rh._tenant_batches(iter(lines)))
    assert all(len(b) <= rh.MAX_BATCH_ENTITIES for _, b in batches)
    assert [(t, len(b)) for t, b in batches] == [("a", 500), ("a", 500), ("a", 203), ("b", 1), ("a", 1)]


def test_the_import_never_sends_an_instance_older_than_the_window(tmp_path):
    """The real importer against a stub broker: the 25 h instance is refused in the report and
    never reaches the broker; the window ends at the export time, not at the import."""
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
    (export / "manifest.json").write_text(json.dumps({"env": "unit", "exportedAt": rh.iso(NOW)}))
    later = NOW + dt.timedelta(hours=5)
    (export / "antares" / "entities.jsonl").write_text(json.dumps(
        {"tenant": "t", "entity": {"id": "urn:ngsi-ld:X:1", "type": "X"}}) + "\n")
    (export / "antares" / "temporal.jsonl").write_text("".join(json.dumps(instance(d)) + "\n" for d in (25, 23, 1)))
    report = tmp_path / "report.json"
    rh.main(["import", "--export", str(export), "--base", f"http://127.0.0.1:{server.server_port}",
             "--state", str(tmp_path), "--report", str(report), "--only", "antares", "--now", rh.iso(later)])
    server.shutdown()
    r = json.loads(report.read_text())
    assert r["cutoff"] == "2026-11-07T12:00:00Z"
    assert r["antares"]["temporal"] == 2 and len(r["antares"]["temporalRefused"]) == 1
    sent = [i["value"] for path, _, body in seen if "temporal" in path for i in body["v"]]
    assert sorted(sent) == [1, 23]
    assert all(tenant == "t" for _, tenant, _ in seen)


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

