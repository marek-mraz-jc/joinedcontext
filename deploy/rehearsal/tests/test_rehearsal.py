"""Tests of the rehearsal kit.

Unit tests run anywhere (`pytest deploy/rehearsal/tests`). The live tests start their own
rehearsal environment `rt` (jc + PostgreSQL in docker) and run only with REHEARSAL_LIVE=1 and a
built jc (JC_BIN or $CARGO_TARGET_DIR/debug/jc).
"""

from __future__ import annotations

import argparse
import datetime as dt
import http.server
import json
import os
import subprocess
import re
import sys
import threading
import time
import urllib.parse
from pathlib import Path

import pytest
import yaml

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


def test_a_cluster_import_fences_catalogues_to_the_instances_own_host_and_nothing_else():
    ckan = b"kind: CkanInstance\nspec:\n  url: https://opendata.dev.joinedcontext.com\n"
    own = rh.catalogue_url("https://jcprod-opendata.dev.joinedcontext.com")
    assert rh.fence_writes(ckan, own) == b"kind: CkanInstance\nspec:\n  url: https://jcprod-opendata.dev.joinedcontext.com\n"
    for bad in ["http://jcprod-opendata.dev.joinedcontext.com", "https://x.dev.joinedcontext.com/path",
                "https://u@x.example", "https://x.example:8443", "https://x.example\n  apiKey: y", "https://localhost"]:
        with pytest.raises(SystemExit):
            rh.catalogue_url(bad)



def test_an_imported_endpoint_without_a_ckan_publish_leaves_jcs_catalogue_as_prods():
    head = b"apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: e\nspec:\n  slug: s\n"
    cat = b"  catalog:\n    # the city's own words\n    license: CC_BY_4_0\n    themes: [SOCI]\n"
    # No spec.publish.ckan: prod lists nothing, so jc gets harvest: false in the block.
    out = rh.hide_unpublished(head + cat + b"  audience: public\n")
    assert yaml.safe_load(out)["spec"]["catalog"] == {"harvest": False, "license": "CC_BY_4_0", "themes": ["SOCI"]}
    assert yaml.safe_load(out)["spec"]["audience"] == "public" and b"# the city's own words" in out
    flip = rh.hide_unpublished(head + b"  catalog:\n    harvest: true\n    license: x\n")
    assert yaml.safe_load(flip)["spec"]["catalog"] == {"harvest": False, "license": "x"}
    flow = rh.hide_unpublished(head + b"  catalog: { license: x, harvest: true }\n")
    assert yaml.safe_load(flow)["spec"]["catalog"] == {"harvest": False, "license": "x"}
    # A publish block without ckan publishes nothing on prod either.
    other = rh.hide_unpublished(head + b"  publish:\n    feeds: {}\n" + cat)
    assert yaml.safe_load(other)["spec"]["catalog"]["harvest"] is False
    # With spec.publish.ckan prod lists it: unchanged, as is an Endpoint without a catalog block.
    published = head + b"  publish:\n    ckan:\n      instanceRef: { kind: CkanInstance, name: zilina }\n" + cat
    assert rh.hide_unpublished(published) == published
    assert rh.hide_unpublished(head) == head
    pipeline = b"kind: Pipeline\nspec:\n  catalog:\n    license: x\n"
    assert rh.hide_unpublished(pipeline) == pipeline

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


# ------------------------------------------------------------------------------------------------
# live: one source for the secrets of the clients jc signs in as, across a restart (F5)

RS_PORT, RS_PG_PORT = 42118, 55448
JC_CLIENTS = {"portal-api": "KC_PORTAL_API_SECRET", "portal-reconciler": "KC_PORTAL_RECONCILER_SECRET",
              "context-gateway": "KC_CONTEXT_GATEWAY_SECRET", "jc-assistant": "KC_JC_ASSISTANT_SECRET"}


def grant(base, realm, client, secret):
    st, _ = rh.Http(base).json("POST", f"/id/realms/{realm}/protocol/openid-connect/token", form={
        "grant_type": "client_credentials", "client_id": client, "client_secret": secret})
    return st


def test_a_restart_signs_jc_in_with_the_realms_secret_of_each_of_its_clients(tmp_path):
    if not LIVE:
        pytest.skip("REHEARSAL_LIVE=1 starts a jc and a PostgreSQL container")
    home = tmp_path / "home"
    keycloak = home / "rs" / "export" / "keycloak"
    keycloak.mkdir(parents=True)
    (keycloak / "realm-rsrealm.json").write_text(json.dumps({"realm": "rsrealm", "enabled": True, "clients": [
        {"clientId": c, "enabled": True, "publicClient": False, "serviceAccountsEnabled": True,
         "clientAuthenticatorType": "federated-jwt", "secret": f"$(env:{v})"} for c, v in JC_CLIENTS.items()]}))
    run_env = dict(os.environ, REHEARSAL_HOME=str(home))
    up = [str(HERE / "up.sh"), "rs", str(RS_PORT), str(RS_PG_PORT)]
    base, rs = f"http://localhost:{RS_PORT}", home / "rs"
    files = dict(rh.STATE_CLIENT_SECRETS)
    try:
        subprocess.run(up, env=run_env, check=True, timeout=900)
        old = {}
        for client, var in JC_CLIENTS.items():
            old[client] = (rs / "secrets" / "realm" / var).read_text().strip()
            assert (rs / "state" / files[client]).read_text().strip() == old[client], client
            assert grant(base, "rsrealm", client, old[client]) == 200, client
        # The realms step adds jc's proxy client with a fresh realm secret, handed to its state file.
        proxy = (rs / "secrets" / "realm" / "KC_AGENT_PROXY_SECRET").read_text().strip()
        assert (rs / "state" / files["agent-proxy"]).read_text().strip() == proxy
        assert grant(base, "rsrealm", "agent-proxy", proxy) == 200
        # The realm's value changes (a rotated export): the next start signs every client in with it.
        (rs / "secrets" / "realm" / JC_CLIENTS["jc-assistant"]).write_text("rotated-" + old["jc-assistant"][:8])
        subprocess.run(up, env=run_env, check=True, timeout=900)
        new = (rs / "secrets" / "realm" / JC_CLIENTS["jc-assistant"]).read_text().strip()
        assert (rs / "state" / files["jc-assistant"]).read_text().strip() == new
        assert grant(base, "rsrealm", "jc-assistant", new) == 200
        assert grant(base, "rsrealm", "jc-assistant", old["jc-assistant"]) == 401
        for client in ("portal-api", "portal-reconciler", "context-gateway"):
            assert grant(base, "rsrealm", client, old[client]) == 200, client
    finally:
        for name in ("jc.pid", "retain.pid"):
            pid = rs / name
            if pid.exists():
                subprocess.run(["kill", pid.read_text().strip()], check=False)
        subprocess.run(["docker", "rm", "-f", "rh-pg-rs"], check=False, capture_output=True)



def test_a_bundles_description_written_after_its_manifest_does_not_stop_the_import(tmp_path):
    # prod-final-20261109T1958Z: EXPORT.md 4 ms after manifest.json; a data file still stops it.
    (tmp_path / "antares").mkdir()
    (tmp_path / "antares" / "entities.jsonl").write_text("")
    os.utime(tmp_path / "antares" / "entities.jsonl", (1, 1))
    (tmp_path / "manifest.json").write_text("{}")
    os.utime(tmp_path / "manifest.json", (2, 2))
    (tmp_path / "EXPORT.md").write_text("# bundle")
    assert rh.bundle_settled(tmp_path) == ""
    (tmp_path / "antares" / "EXPORT.md").write_text("")
    assert "antares/EXPORT.md" in rh.bundle_settled(tmp_path)


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



def test_the_import_keeps_a_kpis_whole_history_and_cuts_every_other_type():
    cutoff = dt.datetime(2026, 11, 8, 18, tzinfo=dt.UTC)
    line = lambda t, at: {"tenant": "a", "id": f"urn:ngsi-ld:{t}:1", "type": t, "attr": "v",
                          "instance": {"type": "Property", "value": 1, "observedAt": at}}
    kept, refused = rh.split_by_cutoff([line("KeyPerformanceIndicator", "2023-12-31T00:00:00Z"),
                                        line("BikeHireDockingStation", "2026-11-07T00:00:00Z"),
                                        line("BikeHireDockingStation", "2026-11-09T00:00:00Z"),
                                        {**line("X", "2025-01-01T00:00:00Z"), "type": ["X", "KeyPerformanceIndicator"]}],
                                       cutoff)
    assert [(k["type"], k["instance"]["observedAt"]) for k in kept] == [
        ("KeyPerformanceIndicator", "2023-12-31T00:00:00Z"), ("BikeHireDockingStation", "2026-11-09T00:00:00Z"),
        (["X", "KeyPerformanceIndicator"], "2025-01-01T00:00:00Z")]
    assert [r["kind"] for r in refused] == ["cut-off"]



SPACE = """apiVersion: joinedcontext.com/v1alpha1
kind: ContextSpace
metadata:
  name: city
spec:
  dataModelRef: city
  retention:
    - entityType: Tree
      history: 'off'
    - entityType: BikeHireDockingStation
      history: P30D
      # a level the pipeline repeats
      record: onChange
      heartbeat: PT1H
      attributes:
        dateModified: { record: never }
    - entityType: KeyPerformanceIndicator
      history: P30D
  # the space's place
  quality: []
"""


def test_the_import_bounds_each_high_rate_types_history_in_the_space_manifest():
    out = rh.bound_retention(SPACE.encode(), "city", {"BikeHireDockingStation", "KeyPerformanceIndicator",
                                                       "AirQualityObserved", "TrafficFlowObserved"})
    doc = yaml.safe_load(out)
    rules = {r["entityType"]: r for r in doc["spec"]["retention"]}
    assert rules["Tree"] == {"entityType": "Tree", "history": "off"}
    # bikes: one day; the rule's longer heartbeat (fewer rows) and its attribute recording stay
    assert rules["BikeHireDockingStation"] == {"entityType": "BikeHireDockingStation", "history": "P1D",
                                               "record": "onChange", "heartbeat": "PT1H",
                                               "attributes": {"dateModified": {"record": "never"}}}
    # a KPI keeps its whole history: the export's 30 days would delete the periods the import keeps
    assert rules["KeyPerformanceIndicator"] == {"entityType": "KeyPerformanceIndicator", "history": "forever",
                                                "record": "onChange", "heartbeat": "P1D",
                                                "reason": "one value per period, written once"}
    assert rules["AirQualityObserved"] == {"entityType": "AirQualityObserved", "history": "P7D",
                                           "record": "onChange", "heartbeat": "PT1H"}
    assert rules["TrafficFlowObserved"]["heartbeat"] == "PT15M"
    assert doc["spec"]["quality"] == [] and b"# a level the pipeline repeats" in out and b"# the space's place" in out
    assert out.index(b"TrafficFlowObserved") < out.index(b"# the space's place")
    # bbsk-otvorene-data's KPI: 30 days; a space without these types, another kind: unchanged
    bbsk = yaml.safe_load(rh.bound_retention(SPACE.encode(), "bbsk-otvorene-data", {"KeyPerformanceIndicator"}))
    assert [r for r in bbsk["spec"]["retention"] if r["entityType"] == "KeyPerformanceIndicator"][0]["history"] == "P30D"
    assert rh.bound_retention(SPACE.encode(), "city", set()) == SPACE.encode()
    endpoint = SPACE.replace("kind: ContextSpace", "kind: Endpoint").encode()
    assert rh.bound_retention(endpoint, "city", {"BikeHireDockingStation"}) == endpoint
    # a space with no retention block gets one
    bare = rh.bound_retention(b"kind: ContextSpace\nmetadata:\n  name: c\nspec:\n  dataModelRef: c\n", "c",
                              {"WeatherObserved"})
    assert yaml.safe_load(bare)["spec"] == {"retention": [{"entityType": "WeatherObserved", "history": "P7D",
                                                           "record": "onChange", "heartbeat": "PT1H"}],
                                            "dataModelRef": "c"}
    # a list at the key's own indent ("  - ") is edited in place too
    flat = SPACE.replace("    - ", "  - ").replace("\n      ", "\n    ").replace("\n        ", "\n      ")
    assert yaml.safe_load(rh.bound_retention(flat.encode(), "city", {"BikeHireDockingStation"})) \
        ["spec"]["retention"][1]["history"] == "P1D"


def test_the_import_bounds_the_spaces_of_every_repository_not_only_the_first(tmp_path):
    export = tmp_path / "export"
    (export / "antares").mkdir(parents=True)
    (export / "antares" / "entities.jsonl").write_text("".join(
        json.dumps({"tenant": t, "entity": {"id": f"urn:ngsi-ld:BikeHireDockingStation:{t}", "type": "BikeHireDockingStation"}}) + "\n"
        for t in ("first", "second")))
    repos = []
    for repo in ("a", "b"):
        space = "first" if repo == "a" else "second"
        d = export / "gitea" / "files" / "joinedcontext" / repo / "spaces" / space
        d.mkdir(parents=True)
        (d / "space.yaml").write_text(SPACE.replace("name: city", f"name: {space}"))
        repos.append({"owner": "joinedcontext", "name": repo})
    (export / "gitea" / "repos.json").write_text(json.dumps(repos))

    class Refusing:  # every forge call fails: only the files the import would write are of interest
        def json(self, *a, **k):
            return 500, {}

    report = {}
    rh.import_shako(Refusing(), "token", export, report)
    assert report["retentionBounded"] == ["joinedcontext/a/spaces/first/space.yaml",
                                          "joinedcontext/b/spaces/second/space.yaml"]

def test_the_import_leaves_out_a_role_binding_whose_people_the_export_removed(tmp_path):
    """prod-final-20261109T1958Z holds 6 RoleBindings with `subjects:` empty: jc-core refuses each,
    and one refused binding stops every role of the repository compiling (App roles 403)."""
    head = "apiVersion: joinedcontext.com/v1alpha1\nkind: RoleBinding\nmetadata:\n  name: {n}\n  namespace: org\nspec:\n"
    empty = head.format(n="approvers") + "  subjects:\n  role: approver\n  scope: { organization: hel }\n"
    held = head.format(n="editors") + "  subjects:\n    - group: editors\n  role: editor\n  scope: { organization: hel }\n"
    d = tmp_path / "gitea" / "files" / "joinedcontext" / "configuration" / "users" / "assignments"
    d.mkdir(parents=True)
    (d / "approvers.yaml").write_text(empty)
    (d / "editors.yaml").write_text(held)
    (d / "notes.yaml").write_text("kind: Role\nspec: {subjects: []}\n")
    (tmp_path / "gitea" / "repos.json").write_text(json.dumps([{"owner": "joinedcontext", "name": "configuration"}]))

    class Refusing:
        def json(self, *a, **k):
            return 500, {}

    report = {}
    rh.import_shako(Refusing(), "token", tmp_path, report)
    assert report["bindingsWithoutPeople"] == ["joinedcontext/configuration/users/assignments/approvers.yaml"]
    assert rh.binding_without_people(empty.encode()) and not rh.binding_without_people(held.encode())
    assert not rh.binding_without_people(b"kind: Role\nspec: {subjects: []}\n")
    assert not rh.binding_without_people(b": not yaml: [")


PG_IMAGE = "postgis/postgis:17-3.5"


@pytest.mark.skipif(subprocess.run(["docker", "image", "inspect", PG_IMAGE], capture_output=True).returncode != 0,
                    reason=f"no docker or no {PG_IMAGE} on this host")
def test_retain_keeps_a_kpis_whole_history_in_real_postgres(tmp_path):
    """retain against a real PostgreSQL with Antares' history tables: an old KPI row stays in both
    tables, an old sensor row goes, an audit row stays, a fresh row stays."""
    name, port = "rh-pg-retain-test", 55449
    subprocess.run(["docker", "rm", "-f", name], capture_output=True)
    subprocess.run(["docker", "run", "-d", "--name", name, "-p", f"127.0.0.1:{port}:5432",
                    "-e", "POSTGRES_PASSWORD=retain-test", "-e", "POSTGRES_USER=antares", PG_IMAGE],
                   check=True, capture_output=True)
    pw = tmp_path / "pw"
    pw.write_text("retain-test")
    try:
        schema = """
        CREATE TABLE temporal_entities (tenant_id text, id text, types text[]);
        CREATE TABLE iri_ids (tenant_id text, id bigint, iri text);
        CREATE TABLE attr_instances (tenant_id text, entity_id text, observed_at timestamptz, provenance jsonb);
        CREATE TABLE attr_rows (tenant_id text, entity_no bigint, observed_at timestamptz, provenance jsonb);
        INSERT INTO temporal_entities VALUES
          ('a', 'urn:k', ARRAY['https://uri.etsi.org/ngsi-ld/default-context/KeyPerformanceIndicator']),
          ('a', 'urn:s', ARRAY['https://smartdatamodels.org/dataModel.Transportation/BikeHireDockingStation']),
          ('b', 'urn:k', ARRAY['https://uri.etsi.org/ngsi-ld/default-context/NotAKeyPerformanceIndicatorX']);
        INSERT INTO iri_ids VALUES ('a', 1, 'urn:k'), ('a', 2, 'urn:s');
        INSERT INTO attr_instances VALUES ('a', 'urn:k', '2023-12-31', NULL), ('a', 'urn:s', '2026-11-01', NULL),
          ('a', 'urn:s', '2026-11-01', '{"audit": true}'), ('a', 'urn:s', '2026-11-09T12:00:00Z', NULL),
          ('b', 'urn:k', '2026-11-01', NULL);
        INSERT INTO attr_rows VALUES ('a', 1, '2025-01-01', NULL), ('a', 2, '2026-11-01', NULL);
        """
        for _ in range(60):
            if rh_psql(port, pw, "SELECT 1").returncode == 0:
                break
            time.sleep(1)
        assert rh_psql(port, pw, schema).returncode == 0
        now = dt.datetime(2026, 11, 9, 13, tzinfo=dt.UTC)
        cutoff = rh.cutoff_for(now)
        assert rh.old_history("127.0.0.1", port, "antares", "antares", str(pw), cutoff) == 3
        args = argparse.Namespace(host="127.0.0.1", port=port, user="antares", db="antares", password_file=str(pw))
        assert rh.retain_once(args, now)["deleted"] == 3
        left = rh_psql(port, pw, "SELECT tenant_id, entity_id, observed_at::date FROM attr_instances ORDER BY 1, 3;"
                                 "SELECT entity_no FROM attr_rows").stdout.split()
        assert left == ["a|urn:k|2023-12-31", "a|urn:s|2026-11-01", "a|urn:s|2026-11-09", "1"]
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True)


@pytest.mark.skipif(subprocess.run(["docker", "image", "inspect", PG_IMAGE], capture_output=True).returncode != 0,
                    reason=f"no docker or no {PG_IMAGE} on this host")
def test_the_prod_run_log_loads_into_the_portals_own_tables_once(tmp_path):
    """import-runlog against the Portal's migrations in a real PostgreSQL: every run and line goes
    in, a JSON text with quotes, backslashes and a newline arrives unchanged, a second load adds
    nothing, and a control byte refuses the file."""
    import gzip
    name, port = "rh-pg-runlog-test", 55448
    subprocess.run(["docker", "rm", "-f", name], capture_output=True)
    subprocess.run(["docker", "run", "-d", "--name", name, "-p", f"127.0.0.1:{port}:5432",
                    "-e", "POSTGRES_PASSWORD=runlog-test", "-e", "POSTGRES_USER=antares", PG_IMAGE],
                   check=True, capture_output=True)
    pw = tmp_path / "pw"
    pw.write_text("runlog-test")
    migrations = HERE.parent.parent / "crates" / "portal" / "migrations"
    try:
        for _ in range(60):
            if rh_psql(port, pw, "SELECT 1").returncode == 0:
                break
            time.sleep(1)
        for m in ("0020_pipeline_outcomes.sql", "0029_run_changes.sql", "0050_run_stream_hash.sql"):
            assert rh_psql(port, pw, (migrations / m).read_text()).returncode == 0, m
        runlog = tmp_path / "runlog"
        runlog.mkdir()
        runs = [{"project": "helsinki", "pipeline": "vision-counts", "run": f"2026-10-28T10:2{i}Z",
                 "first_at": "2026-10-28T10:24:23Z", "last_at": f"2026-10-28T10:2{i}:52Z", "sent": 1024,
                 "rejected": 0, "failed": 0, "created": None, "updated": None, "unchanged": None,
                 "stream_hash": None} for i in range(3)]
        (runlog / "pipeline_runs.jsonl").write_text("".join(json.dumps(r) + "\n" for r in runs))
        tricky = 'a "quoted" \\ path\nsecond line'
        lines = [{"id": 7, "project": "helsinki", "pipeline": "vision-counts", "run": "2026-10-28T10:22Z",
                  "at": "2026-10-28T10:24:52Z", "record_id": "urn:ngsi-ld:X:1", "step": None,
                  "outcome": "sent", "message": tricky}]
        with gzip.open(runlog / "pipeline_log.newest-run.jsonl.gz", "wt") as fh:
            fh.write("".join(json.dumps(r) + "\n" for r in lines))
        load = lambda: rh.import_runlog("127.0.0.1", port, "antares", "antares", str(pw), runlog)  # noqa: E731
        assert load() == {"pipeline_runs": {"file": 3, "added": 3}, "pipeline_log": {"file": 1, "added": 1}}
        assert load() == {"pipeline_runs": {"file": 3, "added": 0}, "pipeline_log": {"file": 1, "added": 0}}
        got = rh_psql(port, pw, "SELECT message FROM pipeline_log").stdout
        assert got.rstrip("\n") == tricky
        assert rh_psql(port, pw, "SELECT max(sent) FROM pipeline_runs").stdout.strip() == "1024"
        with pytest.raises(SystemExit):
            rh.runlog_sql("pipeline_runs", ['{"a": "x"}\x01'])
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True)


def rh_psql(port, pw, sql):
    env = dict(os.environ, PGPASSWORD=pw.read_text())
    return subprocess.run(["psql", "-AXt", "-v", "ON_ERROR_STOP=1", "-h", "127.0.0.1", "-p", str(port), "-U", "antares",
                           "-d", "antares"], input=sql, env=env, capture_output=True, text=True)

def test_a_datasource_reads_the_same_endpoint_on_the_rehearsal():
    gw = "http://context-gateway.smartcity.svc.cluster.local:8080/api/endpoint/jtjnhkaau64zmzz5csmcwem52icvufy2"
    assert rh.rehearsal_endpoint_url(gw, "http://localhost:42102/") == \
        "http://127.0.0.1:42102/api/endpoint/jtjnhkaau64zmzz5csmcwem52icvufy2"
    assert rh.rehearsal_endpoint_url(gw, "http://127.0.0.1:42102") == \
        "http://127.0.0.1:42102/api/endpoint/jtjnhkaau64zmzz5csmcwem52icvufy2"
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


def test_the_realms_step_hands_the_new_proxy_clients_secret_to_jcs_state_file(tmp_path):
    """jc-prod 2026-11-11: the deployer's files name the proxy's fresh realm secret, so the start
    copies it into proxy-client-secret, and the realm file reads it from the secrets mount."""
    export = tmp_path / "export"
    (export / "keycloak").mkdir(parents=True)
    (export / "keycloak" / "realm-smartcity.json").write_text(json.dumps(prod_shaped_assistant_realm()))
    out = tmp_path / "state" / "realms"
    rh.main(["realms", "--export", str(export), "--out", str(out), "--secrets-dir", "/run/jc/realm-secrets"])
    state = out.parent
    assert "KC_AGENT_PROXY_SECRET" in (state / "realm-vars.txt").read_text().split()
    assert "proxy-client-secret KC_AGENT_PROXY_SECRET" in (state / "realm-state-secrets.txt").read_text().splitlines()
    realm = json.loads((out / "smartcity.json").read_text())
    (proxy,) = [c for c in realm["clients"] if c["clientId"] == "agent-proxy"]
    assert proxy["secret"] == "$(file:KC_AGENT_PROXY_SECRET)"
    (assistant,) = [c for c in realm["clients"] if c["clientId"] == "jc-assistant"]
    assert "agent-proxy" in [m["config"]["included.custom.audience"] for m in assistant["protocolMappers"]]



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


def test_prometheus_files_label_jc_and_point_daiya_at_the_proxy(tmp_path):
    # prometheus.sh: jc's admin listener as a jc-prod target (its token by file, never inline),
    # Prometheus itself as another namespace, daiya's org-1 datasource `prometheus` on the proxy.
    rh.prometheus_files("127.0.0.1:55642", "127.0.0.1:55742", "127.0.0.1:55743",
                        tmp_path / "prometheus.yml", tmp_path / "ds.yaml")
    jobs = {j["job_name"]: j for j in json.loads((tmp_path / "prometheus.yml").read_text())["scrape_configs"]}
    assert jobs["jc"]["static_configs"] == [{"targets": ["127.0.0.1:55642"], "labels": {"namespace": "jc-prod"}}]
    assert jobs["jc"]["authorization"] == {"credentials_file": "/etc/rh/token"}
    assert jobs["prometheus"]["static_configs"][0]["labels"] == {"namespace": "monitoring"}
    (ds,) = json.loads((tmp_path / "ds.yaml").read_text())["datasources"]
    assert (ds["uid"], ds["type"], ds["orgId"], ds["url"]) == ("prometheus", "prometheus", 1, "http://127.0.0.1:55743")


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


def test_the_smoke_counts_each_portal_made_dashboard_in_its_project_org(tmp_path):
    class Daiya(http.server.BaseHTTPRequestHandler):
        def answer(self, body):
            raw = json.dumps(body).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def do_GET(self):  # noqa: N802
            if self.path.startswith("/dashboards/api/orgs/name/"):
                return self.answer({"id": 6})
            org = self.headers.get("X-Grafana-Org-Id")
            self.answer([{"uid": "pv-ab12"}] if org == "6" else [{"uid": "pv-elsewhere"}])

        def log_message(self, *a):
            pass

    server = http.server.HTTPServer(("127.0.0.1", 0), Daiya)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    src = tmp_path / "grafana" / "dashboards"
    src.mkdir(parents=True)
    (src / "org3-project-helsinki.json").write_text(json.dumps({"uid": "project-helsinki"}))
    (src / "org3-pv-ab12.json").write_text(json.dumps({"uid": "pv-ab12"}))
    (src / "org3-pv-ef56.json").write_text(json.dumps({"uid": "pv-ef56"}))
    (src / "org9-pv-cd34.json").write_text(json.dumps({"uid": "pv-cd34"}))
    try:
        found, lost = rh.portal_made_found(rh.Http(f"http://127.0.0.1:{server.server_port}"), {},
                                           rh.portal_made(tmp_path))
    finally:
        server.shutdown()
    assert (found, sorted(lost)) == (1, ["org3-pv-ef56.json", "org9-pv-cd34.json"])


DEV_BUNDLE = Path("/workspace/evidence/jc-rehearsal/export/dev")


@pytest.mark.skipif(not (DEV_BUNDLE / "grafana/dashboards").is_dir(), reason="the dev bundle is not on this host")
def test_the_dev_bundle_holds_two_portal_made_dashboards_of_helsinki():
    made = rh.portal_made(DEV_BUNDLE)
    assert len(made) == 2 and {p for _, p in made} == {"helsinki"}


def test_a_jc_that_ignores_sigterm_is_killed_before_the_next_starts():
    """The 2026-11-09 hang: a deadlocked jc never finishes its graceful shutdown and keeps its port."""
    stuck = subprocess.Popen(["bash", "-c", "trap '' TERM; while :; do sleep 0.2; done"])
    time.sleep(0.3)
    r = subprocess.run([str(HERE / "stop-jc.sh"), str(stuck.pid)], capture_output=True, text=True, timeout=60)
    stuck.wait(timeout=5)
    assert r.returncode == 0 and "SIGKILL" in r.stderr and stuck.returncode == -9


def test_synth_puts_the_sample_apps_in_the_apps_organization():
    job = ('            - name: ORG\n              value: "joinedcontext"\n'
           '            - name: APPS_ORG\n              value: "joinedcontext-apps"\n')
    assert rh.apps_owner_of(job, "joinedcontext") == "joinedcontext-apps"
    assert rh.apps_owner_of('            - name: ORG\n              value: "x"\n', "x") == "x"


def test_realm_secrets_become_file_references_under_the_secrets_dir(tmp_path):
    realm = {"realm": "r", "clients": [{"clientId": "a", "secret": "$(env:A_SECRET)"},
                                        {"clientId": "b", "secret": "$(B_SECRET)"}]}
    out = rh.to_file_refs(realm)
    # Relative names, read under kaisatsu's import secrets dir: no path of this home in the file.
    assert [c["secret"] for c in out["clients"]] == ["$(file:A_SECRET)", "$(file:B_SECRET)"]
    (tmp_path / "realm").mkdir()
    (tmp_path / "realm" / "A_SECRET").write_text("s3\n")
    assert rh.secret_file(out["clients"][0]["secret"], tmp_path).read_text() == "s3\n"
    assert rh.secret_file("$(env:A_SECRET)", tmp_path) == tmp_path / "realm-A_SECRET"


def test_every_jc_of_one_config_is_found_whatever_the_pid_file_says(tmp_path):
    mine, other = tmp_path / "a" / "jc.toml", tmp_path / "b" / "jc.toml"
    sleep = [sys.executable, "-c", "import time; time.sleep(60)"]
    p1 = subprocess.Popen(sleep + ["--config", str(mine)])
    p2 = subprocess.Popen(sleep + ["--config", str(other)])
    try:
        out = subprocess.run([str(HERE / "jcs-of.sh"), str(mine)], capture_output=True, text=True, timeout=30).stdout
        assert out.split() == [str(p1.pid)]
    finally:
        p1.kill(); p2.kill()


def test_a_service_token_jc_holds_in_memory_is_read_from_the_kit_secrets(tmp_path):
    (tmp_path / "state").mkdir(); (tmp_path / "secrets").mkdir()
    (tmp_path / "secrets" / "shako-token").write_text("given\n")
    (tmp_path / "state" / "antares-admin-token").write_text("on-disk\n")
    assert rh.read_secret(tmp_path / "state", "shako-token") == "given"
    assert rh.read_secret(tmp_path / "state", "antares-admin-token") == "on-disk"


def test_held_pipelines_start_every_module_but_ekiben():
    """The count suites read the import alone (supervisor 2026-11-10): with REHEARSAL_HOLD_PIPELINES=1
    jc starts without ekiben, so no Portal queue and no restored stream writes a row during or
    after the import (gate 2026-11-10T131506Z-g6: 108 endpoints rows above the export)."""
    fn = subprocess.run(["sed", "-n", "/^jc_role()/,/^}/p", str(HERE / "up.sh")], capture_output=True, text=True, check=True).stdout
    assert fn

    def role(**env):
        return subprocess.run(["bash", "-c", fn + "jc_role"], env={"PATH": os.environ["PATH"], **env},
                              capture_output=True, text=True, check=True).stdout.strip()

    src = (HERE / "../../bins/jc/src/module.rs").read_text()
    every = re.search(r"pub const ALL: \[Module; \d+\] = \[(.*?)\];", src, re.DOTALL)[1]
    names = {"ModelTools": "model-tools"}
    modules = [names.get(m, m.lower()) for m in re.findall(r"Module::(\w+)", every)]
    assert role() == "all"
    assert role(REHEARSAL_HOLD_PIPELINES="1").split(",") == [m for m in modules if m != "ekiben"]
    assert role(REHEARSAL_HOLD_PIPELINES="0") == "all"
    assert role(REHEARSAL_HOLD_PIPELINES="1", REHEARSAL_ROLE="kaisatsu,portal") == "kaisatsu,portal"
    assert 'nohup "$jc_bin" --role "$role"' in (HERE / "up.sh").read_text()


def test_every_secret_up_sh_makes_is_long_enough_for_a_jc_token(tmp_path):
    # dataroom refuses a Portal publisher token under 32 printable characters; up.sh's secret()
    # makes every token, so each one it makes has 64.
    fn = subprocess.run(["sed", "-n", "/^secret()/,/^}/p", str(HERE / "up.sh")], capture_output=True, text=True).stdout
    (tmp_path / "secrets").mkdir()
    script = f'home={tmp_path}\n{fn}\nfor i in $(seq 30); do cat "$(secret t$i)"; echo; done'
    out = subprocess.run(["bash", "-c", script], capture_output=True, text=True, check=True).stdout.split()
    assert len(out) == 30 and all(len(t) == 64 and t.isalnum() for t in out)


def test_each_pipelines_client_names_the_secret_the_runner_reads():
    realm = {"clients": [
        {"clientId": "praha-pipelines", "secret": "$(env:KC_PRAHA_PIPELINES_SECRET)"},
        {"clientId": "bbsk-pipelines", "secret": "$(KC_BBSK_PIPELINES_SECRET)"},
        {"clientId": "portal", "secret": "$(env:KC_PORTAL_SECRET)"},
        {"clientId": "x-pipelines", "secret": "literal"},
        {"clientId": "y-pipelines"},
    ]}
    assert rh.pipeline_clients(realm) == [("bbsk-pipelines", "KC_BBSK_PIPELINES_SECRET"),
                                          ("praha-pipelines", "KC_PRAHA_PIPELINES_SECRET")]


def test_an_imported_realm_hands_its_client_secrets_to_jcs_state_files():
    realm = {"clients": [
        {"clientId": "portal-api", "secret": "$(env:KC_PORTAL_API_SECRET)"},
        {"clientId": "portal-reconciler", "secret": "$(env:KC_PORTAL_RECONCILER_SECRET)"},
        {"clientId": "context-gateway", "secret": "$(env:KC_CONTEXT_GATEWAY_SECRET)"},
        {"clientId": "jc-assistant", "secret": "$(env:KC_JC_ASSISTANT_SECRET)"},
        {"clientId": "agent-proxy", "secret": "$(env:KC_AGENT_PROXY_SECRET)"},
        {"clientId": "edge", "secret": "$(env:KC_EDGE_SECRET)"},
    ]}
    # Every client jc signs in as takes the realm's secret: jc's own state value would clash with
    # it in the one realm (jc-smoke-live F5: the assistant got no service token).
    assert rh.state_client_secrets(realm) == [
        ("assistant-client-secret", "KC_JC_ASSISTANT_SECRET"),
        ("gateway-client-secret", "KC_CONTEXT_GATEWAY_SECRET"),
        ("portal-admin-client-secret", "KC_PORTAL_RECONCILER_SECRET"),
        ("portal-client-secret", "KC_PORTAL_API_SECRET"),
        ("proxy-client-secret", "KC_AGENT_PROXY_SECRET")]
    # prod's realm has no agent-proxy (jc-prod 2026-11-11): the realms step adds it with a fresh
    # realm secret, so jc's proxy-client-secret takes the realm's value too.
    prod = {"clients": [c for c in realm["clients"] if c["clientId"] != "agent-proxy"]}
    assert ("proxy-client-secret", "KC_AGENT_PROXY_SECRET") not in rh.state_client_secrets(prod)
    assert ("proxy-client-secret", "KC_AGENT_PROXY_SECRET") in rh.state_client_secrets(rh.add_agent_proxy(prod)[0])


def test_the_rehearsal_signs_in_at_the_realm_of_the_export(tmp_path):
    assert rh.realm_of(tmp_path) == "jc"
    (tmp_path / "realm-name.txt").write_text("smartcity\n")
    assert rh.realm_of(tmp_path) == "smartcity"
    (tmp_path / "portal-admin-client-secret").write_text("s\n")
    seen = []

    class H:
        def json(self, method, path, form=None):
            seen.append(path)
            return 200, {"access_token": "t"}

    assert rh.admin_token(H(), tmp_path) == "t"
    assert seen == ["/id/realms/smartcity/protocol/openid-connect/token"]


def test_an_imported_realm_gets_the_test_persons_admin_group_once():
    out = rh.add_tester({"realm": "smartcity", "groups": [{"name": "bbsk"}]})
    assert [g["name"] for g in out["groups"]] == ["bbsk", "platform-admins"]
    again = rh.add_tester({"groups": out["groups"]})
    assert [g["name"] for g in again["groups"]].count("platform-admins") == 1


def test_up_sh_gives_jc_every_secret_it_holds_only_in_memory():
    """jc keeps these in memory and removes a stale state copy (jc-secrets), so the suites and the
    smoke read them only from the file up.sh gives jc."""
    text = (HERE / "up.sh").read_text()
    for var in ("JCBIN_SHAKO_TOKEN_FILE", "JCBIN_DATAROOM_PORTAL_TOKEN_FILE", "JCBIN_EKIBEN_TOKEN_FILE",
                "JCBIN_DAIYA_PORTAL_PASSWORD_FILE"):
        assert re.search(rf"\bexport {var}\b|{var}=.*; export {var}\b", text), var


def test_up_sh_hands_jc_the_deployments_age_identity_in_place_and_refuses_an_open_one(tmp_path):
    key = tmp_path / "age.key"
    key.write_text("AGE-SECRET-KEY-1X\n")
    text = (HERE / "up.sh").read_text()
    block = text[text.index('if [ -n "${REHEARSAL_AGE_KEY_FILE:-}" ]'):]
    block = block[:block.index("\nfi\n") + 4]
    run = lambda mode: subprocess.run(["bash", "-c", f"set -e; {block}\necho \"$JCBIN_PORTAL_AGE_KEY_FILE\""],
                                      env=dict(os.environ, REHEARSAL_AGE_KEY_FILE=str(key)),
                                      capture_output=True, text=True) if not key.chmod(mode) else None
    ok = run(0o600)
    assert ok.returncode == 0 and ok.stdout.strip() == str(key)
    bad = run(0o644)
    assert bad.returncode == 1 and "refused" in bad.stderr


def test_only_the_clients_jc_signs_in_as_trade_federated_jwt_for_their_secret():
    k8s = {"jwt.credential.sub": "system:serviceaccount:smartcity:portal", "jwt.credential.issuer": "kubernetes"}
    realm = {"clients": [
        {"clientId": "portal-api", "clientAuthenticatorType": "federated-jwt", "secret": "$(env:A)",
         "attributes": {**k8s, "post.logout.redirect.uris": "+"}},
        {"clientId": "context-gateway", "clientAuthenticatorType": "federated-jwt", "attributes": dict(k8s)},
        {"clientId": "pl-zilina-osm", "clientAuthenticatorType": "federated-jwt", "attributes": dict(k8s)},
        {"clientId": "praha-pipelines", "clientAuthenticatorType": "client-secret"},
        # prod's own proxy: token exchange over the Endpoints; jc never signs in as it.
        {"clientId": "helsinki-agent-proxy", "clientAuthenticatorType": "federated-jwt",
         "attributes": {**k8s, "standard.token.exchange.enabled": "true"}},
    ]}
    out, changed = rh.jc_clients_by_secret(realm)
    by = {c["clientId"]: c for c in out["clients"]}
    assert changed == ["context-gateway", "portal-api"]
    assert by["helsinki-agent-proxy"] == realm["clients"][4]
    with_assistant = {"clients": out["clients"] + [{"clientId": "jc-assistant"}]}
    assert rh.add_agent_proxy(with_assistant)[0]["clients"][4] == realm["clients"][4]
    assert by["portal-api"]["clientAuthenticatorType"] == "client-secret"
    assert by["portal-api"]["attributes"] == {"post.logout.redirect.uris": "+"}
    assert by["portal-api"]["secret"] == "$(env:A)"
    assert by["context-gateway"]["secret"] == "$(env:KC_CONTEXT_GATEWAY_SECRET)"
    assert ("gateway-client-secret", "KC_CONTEXT_GATEWAY_SECRET") in rh.state_client_secrets(out)
    assert by["pl-zilina-osm"] == realm["clients"][2]
    assert by["praha-pipelines"] == realm["clients"][3]
    assert realm["clients"][0]["clientAuthenticatorType"] == "federated-jwt"


def test_the_assistants_client_may_ask_the_shared_embedders_scope_and_nothing_else_moves():
    realm = {"clients": [{"clientId": "jc-assistant", "secret": "$(file:x)", "optionalClientScopes": ["address"]},
                         {"clientId": "portal-api", "optionalClientScopes": []}],
             "clientScopes": [{"name": "address"}]}
    out, changed = rh.add_embedder_scope(realm)
    assert changed
    by_id = {c["clientId"]: c for c in out["clients"]}
    assert by_id["jc-assistant"] == {"clientId": "jc-assistant", "secret": "$(file:x)",
                                     "optionalClientScopes": ["address", "embedder"]}
    assert by_id["portal-api"]["optionalClientScopes"] == []
    (scope,) = [s for s in out["clientScopes"] if s["name"] == "embedder"]
    assert scope["protocolMappers"][0]["config"]["included.custom.audience"] == "embedder"
    assert realm["clients"][0]["optionalClientScopes"] == ["address"], "the input is not changed"
    # A second pass, and a realm without the assistant, change nothing.
    assert rh.add_embedder_scope(out) == (out, False)
    assert rh.add_embedder_scope({"clients": [{"clientId": "portal-api"}]})[1] is False


def prod_shaped_assistant_realm():
    """jc-prod's smartcity realm as far as the proxy goes (prod-final export, names only)."""
    aud = lambda name, a: {"name": name, "protocolMapper": "oidc-audience-mapper",
                           "config": {"included.custom.audience": a, "access.token.claim": "true"}}
    return {"realm": "smartcity", "clients": [
        {"clientId": "jc-assistant", "clientAuthenticatorType": "client-secret", "secret": "$(env:KC_JC_ASSISTANT_SECRET)",
         "protocolMappers": [aud("agent-proxy-audience", "helsinki-agent-proxy"), aud("functions-audience", "jc-functions")]},
        {"clientId": "helsinki-agent-proxy", "clientAuthenticatorType": "federated-jwt",
         "attributes": {"standard.token.exchange.enabled": "true"}, "protocolMappers": [aud("context-gateway-audience", "context-gateway")]},
        {"clientId": "portal-api", "secret": "$(env:KC_PORTAL_API_SECRET)"},
    ], "clientScopes": [{"name": "basic"}], "users": [{"serviceAccountClientId": "portal-api", "clientRoles": {"x": ["y"]}}]}


def test_the_assistants_proxy_client_comes_into_an_imported_realm_and_nothing_else_moves():
    realm = prod_shaped_assistant_realm()
    before = json.loads(json.dumps(realm))
    out, added = rh.add_agent_proxy(realm)
    assert added == ["client agent-proxy", "jc-assistant audience agent-proxy"]
    assert realm == before, "the input is not changed"
    by = {c["clientId"]: c for c in out["clients"]}
    assert set(by) == {"jc-assistant", "helsinki-agent-proxy", "portal-api", "agent-proxy"}
    # Exactly jc's own proxy client (bins/jc/src/setup.rs assistant_realm_file), a fresh secret variable.
    assert by["agent-proxy"] == {
        "clientId": "agent-proxy", "name": "Model proxy", "enabled": True, "protocol": "openid-connect",
        "publicClient": False, "clientAuthenticatorType": "client-secret", "secret": "$(env:KC_AGENT_PROXY_SECRET)",
        "standardFlowEnabled": False, "implicitFlowEnabled": False, "directAccessGrantsEnabled": False,
        "serviceAccountsEnabled": True, "fullScopeAllowed": False, "redirectUris": [], "webOrigins": [],
        "defaultClientScopes": ["basic"], "optionalClientScopes": [], "protocolMappers": []}
    assert rh.unsafe_proxy_client(out, by["agent-proxy"]) == []
    # One mapper more on jc-assistant itself: the old audiences stay, agent-proxy joins them.
    auds = lambda c: sorted(m["config"]["included.custom.audience"] for m in c["protocolMappers"])
    assert auds(by["jc-assistant"]) == sorted(auds(before["clients"][0]) + ["agent-proxy"])
    assert by["jc-assistant"]["protocolMappers"][:2] == before["clients"][0]["protocolMappers"]
    assert {k: v for k, v in by["jc-assistant"].items() if k != "protocolMappers"} == \
        {k: v for k, v in before["clients"][0].items() if k != "protocolMappers"}
    # helsinki-agent-proxy stays federated-jwt with its exchange; scopes, users, other clients unmoved.
    assert by["helsinki-agent-proxy"] == before["clients"][1] and by["portal-api"] == before["clients"][2]
    assert {k: v for k, v in out.items() if k != "clients"} == {k: v for k, v in before.items() if k != "clients"}
    # jc's state file takes the realm's fresh secret (no state value of jc's own, F5).
    assert ("proxy-client-secret", "KC_AGENT_PROXY_SECRET") in rh.state_client_secrets(out)
    assert "KC_AGENT_PROXY_SECRET" in rh.SECRET_VAR.findall(json.dumps(out))
    assert rh.add_agent_proxy(out) == (out, [])
    assert rh.add_agent_proxy({"clients": [{"clientId": "portal-api"}]}) == ({"clients": [{"clientId": "portal-api"}]}, [])


def test_an_existing_proxy_client_is_kept_with_its_secret_or_refused_when_it_is_more_than_introspection():
    safe = {**rh.PROXY_CLIENT_REP, "secret": "$(env:KC_THEIRS)"}
    realm = prod_shaped_assistant_realm()
    realm["clients"].append(safe)
    out, added = rh.add_agent_proxy(realm)
    assert added == ["jc-assistant audience agent-proxy"]
    assert [c for c in out["clients"] if c["clientId"] == "agent-proxy"] == [safe], "its secret is never rewritten"
    for field, value in [("standardFlowEnabled", True), ("implicitFlowEnabled", True), ("directAccessGrantsEnabled", True),
                         ("fullScopeAllowed", True), ("publicClient", True), ("redirectUris", ["https://x/*"]),
                         ("webOrigins", ["+"]), ("clientAuthenticatorType", "federated-jwt"),
                         ("attributes", {"standard.token.exchange.enabled": "true"})]:
        bad = prod_shaped_assistant_realm()
        bad["clients"].append({**safe, field: value})
        with pytest.raises(SystemExit, match=field.split(".")[0]):
            rh.add_agent_proxy(bad)
    for roles in [{"clientRoles": {"realm-management": ["view-users"]}}, {"realmRoles": ["admin"]}]:
        bad = prod_shaped_assistant_realm()
        bad["clients"].append(safe)
        bad["users"].append({"serviceAccountClientId": "agent-proxy", **roles})
        with pytest.raises(SystemExit, match="service account"):
            rh.add_agent_proxy(bad)
    ok = prod_shaped_assistant_realm()
    ok["clients"].append(safe)
    ok["users"].append({"serviceAccountClientId": "agent-proxy", "realmRoles": ["default-roles-smartcity"]})
    assert rh.add_agent_proxy(ok)[1] == ["jc-assistant audience agent-proxy"]
    # Absent fields read as Keycloak's defaults: a bare client has the standard flow and full scope.
    bare = prod_shaped_assistant_realm()
    bare["clients"].append({"clientId": "agent-proxy"})
    with pytest.raises(SystemExit, match="standardFlowEnabled"):
        rh.add_agent_proxy(bare)
    twice = prod_shaped_assistant_realm()
    twice["clients"] += [safe, safe]
    with pytest.raises(SystemExit, match="2 clients"):
        rh.add_agent_proxy(twice)


def test_each_pipelines_own_client_trades_federated_jwt_for_its_secret_and_keeps_its_audience():
    k8s = {"jwt.credential.sub": "system:serviceaccount:smartcity-pipeline-identities:pl-bb-park",
           "jwt.credential.issuer": "kubernetes"}
    aud = [{"protocolMapper": "oidc-audience-mapper", "config": {"included.custom.audience": "slugA"}}]
    realm = {"clients": [
        {"clientId": "bb-pl-park", "clientAuthenticatorType": "federated-jwt", "secret": "$(env:KC_BB_PL_PARK_SECRET)",
         "attributes": {**k8s, "joinedcontext.serviceaccount": "bb/pl-park"}, "protocolMappers": aud},
        {"clientId": "bb-pl-nosecret", "clientAuthenticatorType": "federated-jwt",
         "attributes": {**k8s, "joinedcontext.serviceaccount": "bb/pl-nosecret"}},
        # A workload client that is no Pipeline's keeps the cluster's token.
        {"clientId": "bb-cams", "clientAuthenticatorType": "federated-jwt",
         "attributes": {**k8s, "joinedcontext.serviceaccount": "bb/cams"}},
        # An account attribute naming another client is not this client's pipeline.
        {"clientId": "bb-pl-other", "clientAuthenticatorType": "federated-jwt",
         "attributes": {**k8s, "joinedcontext.serviceaccount": "bb/pl-park"}},
        {"clientId": "bb-pipelines", "clientAuthenticatorType": "client-secret", "secret": "$(env:KC_BB_PIPELINES_SECRET)"},
    ]}
    out, changed = rh.jc_clients_by_secret(realm)
    by = {c["clientId"]: c for c in out["clients"]}
    assert changed == ["bb-pl-nosecret", "bb-pl-park"]
    park = by["bb-pl-park"]
    assert park["clientAuthenticatorType"] == "client-secret"
    assert park["attributes"] == {"joinedcontext.serviceaccount": "bb/pl-park"}
    assert park["protocolMappers"] == aud and park["secret"] == "$(env:KC_BB_PL_PARK_SECRET)"
    assert by["bb-pl-nosecret"]["secret"] == "$(env:KC_BB_PL_NOSECRET_SECRET)"
    assert by["bb-cams"] == realm["clients"][2] and by["bb-pl-other"] == realm["clients"][3]
    assert rh.pipeline_clients(out) == [("bb-pipelines", "KC_BB_PIPELINES_SECRET"),
                                        ("bb-pl-nosecret", "KC_BB_PL_NOSECRET_SECRET"),
                                        ("bb-pl-park", "KC_BB_PL_PARK_SECRET")]


def test_an_export_without_service_accounts_gets_the_portals_from_jcs_shipped_realm():
    shipped = json.loads(rh.SHIPPED_REALM.read_text())
    realm = {"clients": [{"clientId": "portal-api"}, {"clientId": "portal-reconciler"}],
             "users": [{"username": "service-account-portal-api", "serviceAccountClientId": "portal-api",
                        "clientRoles": {"x": ["y"]}}]}
    out, added = rh.add_service_accounts(realm, shipped)
    assert added == ["portal-reconciler"]
    rec = [u for u in out["users"] if u.get("serviceAccountClientId") == "portal-reconciler"][0]
    assert rec["clientRoles"]["realm-management"] == ["manage-users", "query-groups"]
    assert [u["clientRoles"] for u in out["users"] if u["serviceAccountClientId"] == "portal-api"] == [{"x": ["y"]}]
    assert rh.add_service_accounts({"clients": []}, shipped)[1] == []


def test_client_urls_of_the_source_host_get_a_twin_on_the_rehearsal_and_keep_their_own():
    p, d = "https://portal.smartcity.joinedcontext.com", "https://smartcity.joinedcontext.com"
    realm = {"clients": [
        {"clientId": "portal-api", "rootUrl": p, "redirectUris": [f"{p}/api/v1/auth/callback"], "webOrigins": [p]},
        {"clientId": "edge", "redirectUris": [f"{p}/*", f"{d}/apps/*"],
         "attributes": {"post.logout.redirect.uris": f"{p}/*##https://elsewhere.example/*"}},
        {"clientId": "other", "redirectUris": ["https://evil.example/smartcity.joinedcontext.com/x"]},
    ]}
    out, n = rh.add_rehearsal_origin(realm, "http://localhost:42102/")
    by = {c["clientId"]: c for c in out["clients"]}
    assert by["portal-api"]["redirectUris"] == [f"{p}/api/v1/auth/callback", "http://localhost:42102/api/v1/auth/callback"]
    assert by["portal-api"]["webOrigins"] == [p, "http://localhost:42102"]
    assert by["edge"]["redirectUris"] == [f"{p}/*", f"{d}/apps/*", "http://localhost:42102/*", "http://localhost:42102/apps/*"]
    assert by["edge"]["attributes"]["post.logout.redirect.uris"] == f"{p}/*##https://elsewhere.example/*##http://localhost:42102/*"
    assert by["other"] == realm["clients"][2]
    assert n == 5
    assert rh.add_rehearsal_origin({"clients": [{"clientId": "x"}]}, "http://localhost:1") == ({"clients": [{"clientId": "x"}]}, 0)


def test_portal_api_gets_one_port_less_loopback_callback_and_nothing_else():
    """The restore suite's copy runs on its own port from the restored realm file (gate
    2026-11-10T203500Z-gc: names `session` "GET /api/v1/auth/login -> 400", 814 rows). kaisatsu
    admits any loopback port for a port-less http://127.0.0.1/<exact path> (oidc redirect.rs), so
    portal-api gets that one URL: exact path, no wildcard, no web origin, no other client, no
    non-loopback host, and the realm import without --public-base (the cluster import) adds none."""
    p = "https://portal.smartcity.joinedcontext.com"
    realm = {"clients": [
        {"clientId": "portal-api", "rootUrl": p, "redirectUris": [f"{p}/api/v1/auth/callback"], "webOrigins": [p],
         "attributes": {"post.logout.redirect.uris": f"{p}/"}},
        {"clientId": "edge", "redirectUris": [f"{p}/*"], "webOrigins": [p]},
        {"clientId": "app-x", "redirectUris": ["https://x.apps.smartcity.joinedcontext.com/*"]},
    ]}
    twinned, _ = rh.add_rehearsal_origin(realm, "http://127.0.0.1:42330")
    out, url = rh.add_loopback_callback(twinned, "http://127.0.0.1:42330")
    assert url == "http://127.0.0.1/api/v1/auth/callback"
    before, after = {c["clientId"]: c for c in twinned["clients"]}, {c["clientId"]: c for c in out["clients"]}
    assert after["portal-api"]["redirectUris"] == before["portal-api"]["redirectUris"] + [url]
    assert {k: v for k, v in after["portal-api"].items() if k != "redirectUris"} == \
        {k: v for k, v in before["portal-api"].items() if k != "redirectUris"}
    assert {k: v for k, v in after.items() if k != "portal-api"} == {k: v for k, v in before.items() if k != "portal-api"}
    added = [u for c in out["clients"] for k in ("redirectUris", "webOrigins") for u in c.get(k) or []
             if "127.0.0.1/" in u or u.endswith("127.0.0.1")]
    assert added == [url] and "*" not in url
    # idempotent, and a second call on another port adds nothing more
    assert rh.add_loopback_callback(out, "http://127.0.0.1:47173") == (out, None)
    # localhost base: the same twin under localhost; never on a non-loopback or https base
    assert rh.add_loopback_callback(realm, "http://localhost:42102/")[1] == "http://localhost/api/v1/auth/callback"
    for base in ("https://127.0.0.1:42330", "http://10.0.0.5:42330", "http://portal.smartcity.joinedcontext.com",
                 "http://127.0.0.1.evil.example:1"):
        assert rh.add_loopback_callback(realm, base) == (realm, None)
    # a realm whose portal-api registers no callback (or has no portal-api) gets none
    assert rh.add_loopback_callback({"clients": [{"clientId": "portal-api"}]}, "http://127.0.0.1:1")[1] is None


def org_realm():
    p = "https://portal.dev.joinedcontext.com"
    return {"clients": [
        {"clientId": "portal-api", "rootUrl": p, "redirectUris": [f"{p}/api/v1/auth/callback"], "webOrigins": [p],
         "attributes": {"post.logout.redirect.uris": f"{p}/"}},
        {"clientId": "portal-ui", "redirectUris": [f"{p}/*"], "webOrigins": [p]},
        {"clientId": "edge", "redirectUris": [f"{p}/*"], "webOrigins": [p]},
    ]}


def test_an_organization_localhost_portal_host_gets_its_exact_callbacks(tmp_path):
    """Gate 2026-11-11T041800Z-gr, multiorg host mode: the imported dev realm is the deployment's,
    so jc writes no redirect for portal.barcelona.localhost:23000 and kaisatsu answered the
    sign-in 400. Each [[organizations]] Portal host gets the exact portal-api callback, web origin
    and logout target and the exact portal-ui sso-check callback; no wildcard, no other client."""
    extra = tmp_path / "orgs.toml"
    extra.write_text('[[organizations]]\nname = "hel"\ncatalogue_id = "00000000-0000-7000-8000-0000000000a1"\n'
                     'hosts = { portal = "portal.hel.localhost:23000", opendata = "opendata.hel.localhost:23000", '
                     'api = "api.hel.localhost:23000", mcp = "mcp.hel.localhost:23000" }\n')
    hosts = rh.organization_portals(str(extra))
    assert hosts == ["portal.hel.localhost:23000"]
    realm = org_realm()
    out, added = rh.add_organization_portals(realm, "http://127.0.0.1:23000", hosts)
    by = {c["clientId"]: c for c in out["clients"]}
    h = "http://portal.hel.localhost:23000"
    assert by["portal-api"]["redirectUris"][-1] == f"{h}/api/v1/auth/callback"
    assert by["portal-api"]["webOrigins"][-1] == h
    assert by["portal-api"]["attributes"]["post.logout.redirect.uris"].split("##")[-1] == f"{h}/"
    assert by["portal-ui"]["redirectUris"][-1] == f"{h}/api/v1/auth/sso-check/done"
    assert by["edge"] == realm["clients"][2]
    assert sorted(added) == sorted([f"{h}/api/v1/auth/callback", h, f"{h}/", f"{h}/api/v1/auth/sso-check/done", h])
    assert not any("*" in u for u in added)
    # idempotent
    assert rh.add_organization_portals(out, "http://127.0.0.1:23000", hosts) == (out, [])


@pytest.mark.parametrize("host", ["portal.hel.example.com:23000", "portal.hel.localhost.evil.example",
                                  "127.0.0.1.evil.example", "10.0.0.5:23000", "*.localhost:23000", "localhost"])
def test_a_non_loopback_organization_portal_host_is_refused(host):
    with pytest.raises(SystemExit):
        rh.add_organization_portals(org_realm(), "http://127.0.0.1:23000", ["portal.hel.localhost:23000", host])


def test_a_non_loopback_base_adds_no_organization_portal_url():
    realm = org_realm()
    for base in ("https://127.0.0.1:23000", "http://10.0.0.5:23000", "http://portal.smartcity.joinedcontext.com", None):
        assert rh.add_organization_portals(realm, base, ["portal.hel.localhost:23000"]) == (realm, [])


def test_the_port_less_callback_comes_only_with_the_rehearsal_base():
    """`realms` adds the twin only beside --public-base, which up.sh alone passes (the cluster
    import of CLUSTER-IMPORT.md and the prod export never do)."""
    text = (HERE / "rehearsal.py").read_text()
    body = text[text.index("def cmd_realms"):]
    body = body[:body.index("\ndef ")]
    guard = body.index("if a.public_base:")
    assert guard < body.index("add_loopback_callback(") < body.index("add_service_accounts(")
    assert text.count("add_loopback_callback(") == 2  # the def and this one call
    assert "--public-base" not in (HERE / "CLUSTER-IMPORT.md").read_text()


def test_an_app_client_gets_its_loopback_app_host_twin():
    """The role-gated App pages sign in on `http://<app>.apps.localhost:<port>/callback` (gate
    2026-11-10T120138Z-apps-redirect: 22 app.page rows, 400 invalid redirect_uri); prod's App client
    holds only `https://<app>.apps.<domain>/*`. An IP-literal base mounts App hosts under localhost."""
    a = "https://air-quality.apps.smartcity.joinedcontext.com"
    realm = {"clients": [
        {"clientId": "portal-api", "rootUrl": "https://portal.smartcity.joinedcontext.com"},
        {"clientId": "app-air-quality", "redirectUris": [f"{a}/*"], "webOrigins": [a]},
        {"clientId": "other", "redirectUris": ["https://x.apps.smartcity.joinedcontext.com.evil.example/*",
                                               "https://a.b.apps.smartcity.joinedcontext.com/*"]},
    ]}
    for base, host in (("http://127.0.0.1:42102", "localhost:42102"), ("http://localhost:42102/", "localhost:42102")):
        out, n = rh.add_rehearsal_origin(realm, base)
        by = {c["clientId"]: c for c in out["clients"]}
        assert by["app-air-quality"]["redirectUris"] == [f"{a}/*", f"http://air-quality.apps.{host}/*"]
        assert by["app-air-quality"]["webOrigins"] == [a, f"http://air-quality.apps.{host}"]
        assert by["other"] == realm["clients"][2]
        assert n == 2


def test_every_kit_secret_is_long_enough_for_the_strictest_reader(tmp_path):
    """up.sh secret(): dataroom refuses a Portal publisher token under 32 printable characters, and
    a base64 secret with its / + = stripped came out shorter about two times in three."""
    text = (HERE / "up.sh").read_text()
    fn = text[text.index("secret() {"):]
    fn = fn[:fn.index("\n}\n") + 3]
    script = fn + 'for i in $(seq 1 200); do secret "s$i" > /dev/null; done\n'
    (tmp_path / "secrets").mkdir()
    subprocess.run(["bash", "-c", script], check=True, env=dict(os.environ, home=str(tmp_path)))
    values = [p.read_text() for p in (tmp_path / "secrets").iterdir()]
    assert len(values) == 200
    assert all(len(v) >= 32 and v.isascii() and v.isprintable() and " " not in v for v in values)
    assert all((tmp_path / "secrets" / f"s{i}").stat().st_mode & 0o777 == 0o600 for i in range(1, 201))


CATALOG = (b"apiVersion: joinedcontext.com/v1alpha1\nkind: Endpoint\nmetadata:\n  name: catalog\n"
           b"  namespace: zilina\n  title: { sk: K }\nspec:\n  publish:\n    ckan:\n"
           b"      instanceRef: { kind: CkanInstance, name: zilina }\n      license: cc-zero\n"
           b"  catalog:\n    license: CC0\n")


def test_each_catalog_endpoint_publishes_its_own_ckan_name_and_the_old_one_is_an_alias():
    import yaml
    named = rh.name_datasets(CATALOG)
    ckan = yaml.safe_load(named)["spec"]["publish"]["ckan"]
    assert ckan == {"name": "zilina-catalog", "instanceRef": {"kind": "CkanInstance", "name": "zilina"},
                    "license": "cc-zero"}
    assert rh.name_datasets(named) == named
    # A name already set, another city, another Endpoint, another kind: left as they are.
    for data in (named.replace(b"zilina-catalog", b"own"), CATALOG.replace(b"namespace: zilina", b"namespace: bbsk"),
                 CATALOG.replace(b"  name: catalog\n", b"  name: air\n"),
                 CATALOG.replace(b"kind: Endpoint", b"kind: CkanInstance")):
        assert rh.name_datasets(data) == data
    names = sorted(rh.CKAN_NAMES.values())
    assert len(set(names)) == len(names) == 3
    assert set(rh.DATASET_ALIASES.values()) <= set(names)
    assert not set(rh.DATASET_ALIASES) & set(names)


def test_the_hub_sources_are_the_deployments_and_a_malformed_list_stops(tmp_path):
    f = tmp_path / rh.GATEWAY_VALUES
    f.parent.mkdir(parents=True)
    f.write_text('env:\n  JC_GATEWAY_ASSISTANT_URL: "http://x"\n  JC_GATEWAY_HUB_KNOWLEDGE_SOURCES: "bbsk/web, praha/web"\n')
    assert rh.hub_sources(tmp_path) == "bbsk/web,praha/web"
    f.write_text("env:\n  OTHER: 1\n")
    assert rh.hub_sources(tmp_path) == ""
    for bad in ("web", "bbsk/web,../x/y", "bbsk/web/x", "Bbsk/web"):
        f.write_text(f'  JC_GATEWAY_HUB_KNOWLEDGE_SOURCES: "{bad}"\n')
        with pytest.raises(SystemExit):
            rh.hub_sources(tmp_path)


def test_mains_realm_lines_go_over_the_export_and_nothing_else_moves():
    release = {"realm": "smartcity", "clients": [
        {"clientId": "portal-api", "optionalClientScopes": ["address"], "attributes": {"a": "1"},
         "protocolMappers": [{"name": "groups", "config": {"full.path": "false", "claim.name": "groups"}}]},
        {"clientId": "same", "redirectUris": ["https://x/*"]}],
        "clientScopes": [{"name": "mcp:portal"}], "roles": {"realm": [{"name": "r1"}]},
        "users": [{"username": "u"}]}
    main = {"realm": "smartcity", "clients": [
        {"clientId": "portal-api", "optionalClientScopes": ["address", "gateway-hub"],
         "attributes": {"a": "1", "standard.token.exchange.enabled": "true"},
         "protocolMappers": [{"name": "groups", "config": {"full.path": "true", "claim.name": "groups"}},
                             {"name": "jc_org", "config": {}}]},
        {"clientId": "same", "redirectUris": ["https://x/*"]},
        {"clientId": "grafana", "secret": "$(CLIENT_SECRET_GRAFANA)"}],
        "clientScopes": [{"name": "mcp:portal"}, {"name": "gateway-hub"}],
        "defaultOptionalClientScopes": ["connector", "offline_access"],
        "roles": {"realm": [{"name": "r1"}, {"name": "r2"}]},
        "users": [{"username": "u"}, {"username": "presenter@hel.fi"}]}
    export = {"realm": "smartcity", "clients": [
        {"clientId": "portal-api", "secret": "$(env:KC_PORTAL_API_SECRET)", "optionalClientScopes": ["address"],
         "attributes": {"a": "1", "live": "kept"},
         "protocolMappers": [{"name": "groups", "id": "m1", "config": {"full.path": "false", "claim.name": "groups"}},
                             {"name": "prod-only", "config": {}}]},
        {"clientId": "same", "redirectUris": ["https://prod-live/*"]},
        {"clientId": "prod-only"}],
        "clientScopes": [{"name": "mcp:portal", "id": "s1"}], "roles": {"realm": [{"name": "r1"}, {"name": "live"}]}}
    out, lines = rh.realm_lines(export, release, main)
    by = {c["clientId"]: c for c in out["clients"]}
    api = by["portal-api"]
    assert api["optionalClientScopes"] == ["address", "gateway-hub"]
    assert api["attributes"] == {"a": "1", "live": "kept", "standard.token.exchange.enabled": "true"}
    assert api["secret"] == "$(env:KC_PORTAL_API_SECRET)"
    mappers = {m["name"]: m for m in api["protocolMappers"]}
    assert mappers["groups"]["config"]["full.path"] == "true" and mappers["groups"]["id"] == "m1"
    assert set(mappers) == {"groups", "prod-only", "jc_org"}
    # What main did not change keeps prod's live value; what prod has alone stays.
    assert by["same"]["redirectUris"] == ["https://prod-live/*"] and "prod-only" in by
    assert by["grafana"]["secret"] == "$(CLIENT_SECRET_GRAFANA)"
    assert [s["name"] for s in out["clientScopes"]] == ["mcp:portal", "gateway-hub"]
    assert out["defaultOptionalClientScopes"] == ["connector", "offline_access"]
    assert [r["name"] for r in out["roles"]["realm"]] == ["r1", "live", "r2"]
    assert "users" not in out, "no person comes in with the realm lines"
    assert "clients[grafana] (added)" in lines and "defaultOptionalClientScopes" in lines
    assert export["clients"][0]["optionalClientScopes"] == ["address"], "the export is not changed in place"


def test_up_sh_gives_dataroom_prods_publish_gate_hidden_list():
    """jc.toml carries [dataroom.hidden] beside the aliases, and the two parse as one file."""
    import tomllib
    text = (HERE / "up.sh").read_text()
    m = re.search(r'^cat "\$here/(\S+catalogue-hidden\.toml)" >> "\$home/jc\.toml"$', text, re.M)
    assert m, "up.sh appends no [dataroom.hidden] to jc.toml"
    aliases = subprocess.run([sys.executable, "-I", str(HERE / "rehearsal.py"), "aliases"],
                             capture_output=True, text=True, check=True).stdout
    hidden = (HERE / m.group(1)).read_text()
    doc = tomllib.loads(aliases + "\n" + hidden)
    rows = sum(1 for line in hidden.splitlines() if line.startswith('"'))
    assert rows > 50 and len(doc["dataroom"]["hidden"]) == rows and doc["dataroom"]["aliases"]


def test_up_sh_gives_dataroom_prods_harvests_and_the_converted_list_beside_jc_toml():
    """jc-catalogue-17: jc.toml carries prod's [[dataroom.harvest]] entries, they parse with the
    aliases and the hidden list as one file, and each `converted_file` they name is copied beside
    jc.toml."""
    import tomllib
    text = (HERE / "up.sh").read_text()
    m = re.search(r'^cat "\$here/(\S+harvest-sources\.toml)" >> "\$home/jc\.toml"$', text, re.M)
    assert m, "up.sh appends no [[dataroom.harvest]] to jc.toml"
    copied = re.findall(r'^install -m 644 "\$here/(\S+)" "\$home/(\S+)"$', text, re.M)
    aliases = subprocess.run([sys.executable, "-I", str(HERE / "rehearsal.py"), "aliases"],
                             capture_output=True, text=True, check=True).stdout
    hidden = (HERE / "../../bins/jc/docs/catalogue-hidden.toml").read_text()
    doc = tomllib.loads(aliases + "\n" + hidden + "\n" + (HERE / m.group(1)).read_text())
    harvests = doc["dataroom"]["harvest"]
    assert {h["name"] for h in harvests} >= {"hub-bbsk", "nkod-banskabystrica"}
    for h in harvests:
        if "converted_file" in h:
            src = [s for s, dst in copied if dst == h["converted_file"]]
            assert src and (HERE / src[0]).is_file(), f"{h['name']}: {h['converted_file']} not copied beside jc.toml"
            json.loads((HERE / src[0]).read_text())


def test_up_sh_refuses_realm_lines_without_an_export_to_render(tmp_path):
    """jc-catalogue-17: REHEARSAL_REALM_LINES_* with no keycloak/ in the export kept the last
    rendered realm silently; up.sh now stops before jc starts."""
    text = (HERE / "up.sh").read_text()
    block = text[text.index("if [ ! -d \"$export_dir/keycloak\" ]"):]
    block = block[:block.index("\nfi\n") + 4]
    run = lambda env: subprocess.run(["bash", "-c", f"export_dir={tmp_path}\n{block}\necho started"],
                                     capture_output=True, text=True, env={"PATH": "/usr/bin:/bin", **env})
    refused = run({"REHEARSAL_REALM_LINES_FROM": "a", "REHEARSAL_REALM_LINES_TO": "b"})
    assert refused.returncode == 1 and "set REHEARSAL_EXPORT" in refused.stderr and "started" not in refused.stdout
    assert run({}).stdout.strip() == "started"
    (tmp_path / "keycloak").mkdir()
    assert run({"REHEARSAL_REALM_LINES_FROM": "a", "REHEARSAL_REALM_LINES_TO": "b"}).stdout.strip() == "started"


def test_the_kit_names_its_own_origin_by_the_literal_loopback_address():
    """Supervisor 2026-11-10: kaisatsu names its issuer by the host a request reached it on, and
    ekiben's guard lets the Portal's sweeps reach only the literal http://127.0.0.1:<port> (jc
    ekiben.rs host_target). With --base-host localhost the token URL was refused as SSRF and every
    *.expiry sweep stopped. jc's base host, the realm's public base, the Apps and the import all
    name one origin, 127.0.0.1; no script of the kit names its own origin as localhost."""
    up = (HERE / "up.sh").read_text()
    assert re.search(r'^origin="http://127\.0\.0\.1:\$port"$', up, re.M), "up.sh sets no 127.0.0.1 origin"
    assert '--base-host "${origin#http://}"' in up and '--public-base "$origin"' in up
    for name in ("up.sh", "apps.sh", "import.sh"):
        code = [line for line in (HERE / name).read_text().splitlines() if not line.lstrip().startswith("#")]
        own = [line for line in code if re.search(r"http://localhost[:/]|\"localhost:\$port\"", line)]
        assert not own, f"{name} names the rehearsal's origin as localhost: {own}"


def test_the_kit_runs_under_the_old_deployments_org_domain():
    """Pipelines mint urn:ngsi-ld:{Type}:{orgDomain}:{space}:{id}. Without JC_PORTAL_ORG_DOMAIN jc
    takes jc.invalid on a loopback host (provision.rs domain), so the rehearsal's pipeline runs
    wrote 5218 praha-mesto PointOfInterest beside the 19547 imported under hel.fi and an App's map
    read passed Antares' 10000-row ceiling. Portal, gateway and assistant read one domain."""
    up = (HERE / "up.sh").read_text()
    assert re.search(r'^org_domain=\$\{REHEARSAL_ORG_DOMAIN:-hel\.fi\}$', up, re.M), "up.sh sets no org domain"
    for key in ("JC_PORTAL_ORG_DOMAIN", "JC_GATEWAY_ORG_DOMAIN", "JC_ASSISTANT_ORG_DOMAIN"):
        assert f'{key}="$org_domain"' in up, f"up.sh exports no {key}"


def test_synthetic_entities_land_in_the_tenant_the_space_pins():
    model = "classes:\n  GtfsStop:\n    is_a: Entity\n"
    repos = {("joinedcontext", "zilina"): {
        "spaces/zilina-verejne/space.yaml": "kind: ContextSpace\nspec:\n  urnSegment: zilina-verejne\n",
        "spaces/zilina-verejne/datamodels/zilina-verejne.linkml.yaml": model,
        "spaces/catalog/space.yaml": "kind: ContextSpace\nspec:\n  urnSegment: zilina-catalog\n",
        "spaces/catalog/datamodels/catalog.linkml.yaml": "classes:\n  Dataset:\n    is_a: Entity\n"}}
    entities, temporal = rh.synth_entities(repos, rh.utcnow())
    assert {(e["tenant"], e["entity"]["type"]) for e in entities} == {
        ("zilina-verejne", "GtfsStop"), ("zilina-catalog", "Dataset")}
    assert temporal == []  # neither class declares a numeric slot


def test_synthetic_history_goes_on_a_numeric_slot_of_an_entity_class_only():
    model = """slots:
  name: {slot_uri: jc:name}
  currentValue: {any_of: [{range: float}, {range: string}]}
  start: {range: integer}
classes:
  KeyPerformanceIndicator:
    is_a: Entity
    slots: [name, currentValue]
  CalculationPeriod:
    slots: [start]
  Any:
    class_uri: linkml:Any
"""
    repos = {("joinedcontext", "helsinki"): {"spaces/helsinki-kpi/datamodels/kpi.linkml.yaml": model}}
    entities, temporal = rh.synth_entities(repos, rh.utcnow())
    assert {e["entity"]["type"] for e in entities} == {"KeyPerformanceIndicator"}
    assert all(set(e["entity"]) == {"id", "type", "name", "currentValue"} for e in entities)
    assert {t["attr"] for t in temporal} == {"currentValue"} and len(temporal) == 6


def test_a_sample_apps_manifest_goes_into_the_projects_repository_naming_its_source():
    manifest = "kind: App\nmetadata:\n  name: air-quality\nspec:\n  source:\n    git:\n      url: https://x/git/joinedcontext/helsinki_air-quality.git\n      ref: main\n"
    out = rh.sample_apps({"air-quality": {"app.yaml": manifest, "src/main.rs": "fn main() {}"}},
                         "joinedcontext-apps", "helsinki", "joinedcontext", "https://dev.example/git")
    assert out[("joinedcontext-apps", "helsinki_air-quality")] == {"src/main.rs": "fn main() {}"}
    held = out[("joinedcontext", "helsinki")]["apps/air-quality/app.yaml"]
    assert "      url: https://dev.example/git/joinedcontext-apps/helsinki_air-quality.git\n" in held
    assert "      ref: main\n" in held
