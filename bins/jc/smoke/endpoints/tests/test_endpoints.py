"""Unit tests of the endpoints suite: expectations from a tiny export, and the whole suite against a
fake gateway that answers right (green) or breaks one rule at a time (red).

    python3 -m pytest bins/jc/smoke/endpoints/tests -q
"""

import json
import sys
import threading
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent.parent))

import export as ex  # noqa: E402
import suite  # noqa: E402

DC = "https://uri.etsi.org/ngsi-ld/default-context/"
OPEN, CLOSED = "a" * 32, "b" * 32

ENDPOINT = """apiVersion: joinedcontext.com/v1alpha1
kind: Endpoint
metadata: {{name: {name}, namespace: city}}
spec:
  contextSpaceRef: air
  slug: {slug}
  audience: {audience}
  enabledRepresentations: [ngsi-ld, ogc-features, odata]
{extra}"""

POLICY = """apiVersion: joinedcontext.com/v1alpha1
kind: Policy
metadata: {{name: {name}, namespace: city}}
spec:
  contextSpaceRef: {{kind: ContextSpace, name: air}}
  assigner: did:web:city.example
  assignee: {assignee}
  operations: [retrieveOps, retrieveEntityTypes, queryTemporal, retrieveTemporal]
  information:
    - entities:
        - type: AirQualityObserved
{pattern}"""


def bundle(tmp: Path, now: str = "2026-11-08T12:00:00Z") -> Path:
    files = tmp / "gitea" / "files" / "org" / "city" / "spaces" / "air"
    (files / "endpoints").mkdir(parents=True)
    (files / "policies").mkdir()
    (files / "space.yaml").write_text("kind: ContextSpace\nmetadata: {name: air, namespace: city}\n"
                                      "spec: {urnSegment: city-air-pinned}\n")
    (files / "endpoints" / "open.yaml").write_text(ENDPOINT.format(
        name="open", slug=OPEN, audience="public",
        extra="  policyRef: urn:ngsi-ld:Policy:city.example:city-air-pinned:public-read\n"
              "  publish: {ckan: {instanceRef: {kind: CkanInstance, name: city}, name: city-air}}\n"))
    (files / "endpoints" / "closed.yaml").write_text(ENDPOINT.format(
        name="closed", slug=CLOSED, audience="organization", extra=""))
    (files / "policies" / "public-read.yaml").write_text(POLICY.format(
        name="public-read", assignee="{kind: role, id: public}", pattern="          idPattern: ':north-'\n"))
    (files / "policies" / "staff-read.yaml").write_text(POLICY.format(
        name="staff-read", assignee="{kind: group, id: staff}", pattern=""))
    (tmp / "antares").mkdir()
    rows = []
    for i, side in enumerate(["north", "north", "south"]):
        rows.append({"tenant": "city-air-pinned", "entity": {
            "id": f"urn:ngsi-ld:AirQualityObserved:{side}-{i}", "type": DC + "AirQualityObserved",
            "https://uri.etsi.org/ngsi-ld/location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [1, 2]}}}})
    rows.append({"tenant": "city-air-pinned", "entity": {"id": "urn:ngsi-ld:Road:r1", "type": DC + "Road"}})
    (tmp / "antares" / "entities.jsonl").write_text("".join(json.dumps(r) + "\n" for r in rows))
    (tmp / "antares" / "temporal.jsonl").write_text(json.dumps({
        "tenant": "city-air-pinned", "id": "urn:ngsi-ld:AirQualityObserved:north-0", "type": DC + "AirQualityObserved",
        "attr": DC + "pm10", "instance": {"type": "Property", "value": 3, "observedAt": now}}) + "\n")
    (tmp / "manifest.json").write_text(json.dumps({"exportedAt": now, "temporalCutoff": "2026-11-07T12:00:00Z"}))
    return tmp


def test_the_export_names_tenant_bindings_and_datasets(tmp_path):
    eps = {e.name: e for e in ex.load_endpoints(bundle(tmp_path) / "gitea" / "files")}
    assert eps["open"].tenant == "city-air-pinned"           # spec.urnSegment pins the tenant
    assert [g.name for g in eps["open"].grants] == ["public-read"]  # policyRef binds one Policy
    assert {g.name for g in eps["closed"].grants} == {"public-read", "staff-read"}  # none: every Policy
    assert eps["open"].datasets == ["city-air"] and eps["closed"].datasets == []
    assert eps["closed"].closed and not eps["open"].closed


def test_expected_counts_follow_the_policy_patterns(tmp_path):
    b = bundle(tmp_path)
    eps = {e.name: e for e in ex.load_endpoints(b / "gitea" / "files")}
    index = ex.build_index(b, ex.patterns_needed(eps.values()))
    tenant = index["tenants"]["city-air-pinned"]
    assert tenant["AirQualityObserved"]["count"] == 3
    want, narrowed = ex.expected_types(eps["open"], eps["open"].public_grants(), tenant)
    assert want == {"AirQualityObserved": 2} and narrowed == ""   # Road is no granted type
    assert tenant["AirQualityObserved"]["psample"][":north-"].endswith("north-0")
    staff = [g for g in eps["closed"].grants if g.name == "staff-read"]
    assert ex.expected_types(eps["closed"], staff, tenant)[0] == {"AirQualityObserved": 3}


# ------------------------------------------------------------------ fake gateway


class Fake:
    """Answers as a healthy gateway does for the bundle; `broken` names one rule it breaks."""

    def __init__(self, broken: str = ""):
        self.broken = broken

    def answer(self, method, path, headers):
        u = urllib.parse.urlparse(path)
        p, qs = u.path, urllib.parse.parse_qs(u.query)
        tok = headers.get("Authorization", "")
        if p == "/api/endpoints":
            if self.broken == "paged-list" and not qs.get("cursor"):
                return 200, {}, {"items": [{"slug": "c" * 32}], "nextCursor": "1"}
            return 200, {}, {"items": [{"slug": OPEN}]}
        if p.startswith("/opendata/api/3/action/package_show"):
            return 200, {}, {"success": True, "result": {"resources": [{"url": "x"}]}}
        slug = p.split("/")[3]
        rest = p.split("/", 4)[4] if p.count("/") >= 4 else ""
        if slug == CLOSED and not tok and self.broken != "closed-open":
            return 401, {}, {"title": "unauthorized"}
        if method == "POST" and self.broken == "no-create":
            return 405, {}, {"title": "OperationNotSupported"}
        if method == "POST":
            return (201, {}, {}) if self.broken == "write-open" else (401, {}, {"title": "no"})
        if rest == "ngsi-ld/v1/types":
            types = ["AirQualityObserved"] + (["Road"] if self.broken == "extra-type" else [])
            return 200, {}, {"typeList": types}
        if rest.startswith("ngsi-ld/v1/types/") or rest == "ngsi-ld/v1/attributes":
            return 200, {}, {"attributeNames": ["pm10"]}
        if rest.startswith("ngsi-ld/v1/temporal/entities/"):
            if qs.get("timerel") == ["after"]:
                return 200, {}, {"id": "x", "pm10": [{"type": "Property", "value": 3}]}
            old = [{"type": "Property", "value": 1}] if self.broken == "old-history" else []
            return 200, {}, {"id": "x", "pm10": old}
        if rest == "ngsi-ld/v1/entities" and "count" in qs:
            n = 3 if self.broken == "count" else 2
            return 200, {"NGSILD-Results-Count": str(n)}, []
        if rest == "ngsi-ld/v1/entities":
            return 200, {}, {"type": "FeatureCollection", "features": []}
        if rest.startswith("ngsi-ld/v1/entities/"):
            eid = urllib.parse.unquote(rest.split("/", 3)[3])
            return 200, {}, {"id": eid, "type": "AirQualityObserved"}
        if rest == "ogc/features/collections":
            return 200, {}, {"collections": [{"id": "AirQualityObserved"}]}
        if rest.startswith("ogc/features/collections/"):
            return 200, {}, {"type": "FeatureCollection", "numberMatched": 2, "features": []}
        if rest == "odata/v4/$metadata":
            return 200, {"Content-Type": "application/xml"}, b'<EntitySet Name="AirQualityObserved"/>'
        if rest.startswith("odata/v4/"):
            return 200, {}, {"@odata.count": 2, "value": []}
        return 404, {}, {}


@pytest.fixture
def serve():
    servers = []

    def start(fake):
        class H(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do(self):
                n = int(self.headers.get("Content-Length") or 0)
                self.rfile.read(n)
                st, hdrs, body = fake.answer(self.command, self.path, self.headers)
                raw = body if isinstance(body, bytes) else json.dumps(body).encode()
                self.send_response(st)
                for k, v in {"Content-Type": "application/json", **hdrs}.items():
                    self.send_header(k, v)
                self.send_header("Content-Length", str(len(raw)))
                self.end_headers()
                self.wfile.write(raw)

            do_GET = do_POST = do_DELETE = do

        srv = ThreadingHTTPServer(("127.0.0.1", 0), H)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        servers.append(srv)
        return f"http://127.0.0.1:{srv.server_port}"

    yield start
    for s in servers:
        s.shutdown()


def run(tmp_path, base, now):
    b = bundle(tmp_path / "x", now)
    out = tmp_path / "out"
    code = suite.main(["--base-url", base, "--export", str(b), "--out", str(out), "--workers", "1"])
    return code, json.loads((out / "results.json").read_text())


def now_iso():
    import datetime as dt
    return dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def test_a_healthy_gateway_is_green(tmp_path, serve):
    code, res = run(tmp_path, serve(Fake()), now_iso())
    fails = [r for r in res["checks"] if r["result"] == "fail"]
    assert code == 0 and not fails, fails
    ids = {r["id"] for r in res["checks"] if r["result"] == "pass"}
    assert {"anon.count", "anon.by-id.same", "anon.geojson", "anon.temporal.last-24h", "anon.temporal.none-older",
            "anon.ogc.items", "anon.odata.query", "anon.write-outside", "anon.refused", "dataset", "listing"} <= ids
    assert (tmp_path / "out" / "REPORT.md").read_text().startswith("# Smoke suite `endpoints`")


@pytest.mark.parametrize("broken,check", [
    ("count", "anon.count"), ("extra-type", "anon.types.match"), ("closed-open", "anon.refused"),
    ("write-open", "anon.write-outside"), ("old-history", "anon.temporal.none-older")])
def test_each_broken_rule_turns_its_check_red(tmp_path, serve, broken, check):
    code, res = run(tmp_path, serve(Fake(broken)), now_iso())
    red = {r["id"] for r in res["checks"] if r["result"] == "fail"}
    assert code == 1 and check in red, red


def test_a_paged_listing_is_read_to_its_end(tmp_path, serve):
    """/api/endpoints names at most 100 a page and the next page's cursor: an Endpoint on page 2
    is listed."""
    code, res = run(tmp_path, serve(Fake("paged-list")), now_iso())
    listing = [r for r in res["checks"] if r["id"] == "listing"]
    assert [r["result"] for r in listing] == ["pass"], listing


@pytest.mark.parametrize("ops,result", [("upsertBatch, createBatch, queryBatch", "pass"),
                                        ("createEntity", "fail"), ("redirectionOps", "fail")])
def test_a_view_serving_no_create_refuses_with_405(tmp_path, serve, ops, result):
    """A public grant of batch writes alone serves no POST /entities: the view's 405 refuses as
    surely as 401. A grant of createEntity (or redirectionOps) does serve it: 405 is then wrong."""
    b = bundle(tmp_path / "x", now_iso())
    pol = b / "gitea" / "files" / "org" / "city" / "spaces" / "air" / "policies" / "public-read.yaml"
    pol.write_text(pol.read_text().replace("retrieveTemporal]", f"retrieveTemporal, {ops}]"))
    out = tmp_path / "out"
    suite.main(["--base-url", serve(Fake("no-create")), "--export", str(b), "--out", str(out), "--workers", "1"])
    res = json.loads((out / "results.json").read_text())
    got = [r["result"] for r in res["checks"] if r["id"] == "anon.write-outside"]
    assert got == [result], got


def test_a_stopped_gateway_fails_and_never_passes(tmp_path):
    code, res = run(tmp_path, "http://127.0.0.1:9", now_iso())
    assert code == 1
    assert not [r for r in res["checks"] if r["result"] == "pass"]


def test_only_loopback_targets_are_accepted(tmp_path):
    with pytest.raises(SystemExit):
        suite.main(["--base-url", "https://portal.dev.joinedcontext.com", "--export", str(tmp_path), "--out", str(tmp_path)])


def test_an_app_endpoint_keeps_only_the_apps_own_grants(tmp_path):
    b = bundle(tmp_path)
    proj = b / "gitea" / "files" / "org" / "city"
    (proj / "apps" / "map").mkdir(parents=True)
    (proj / "apps" / "map" / "app.yaml").write_text("kind: App\nmetadata: {name: map, namespace: city}\n")
    (proj / "spaces" / "air" / "endpoints" / "app-map.yaml").write_text(ENDPOINT.format(
        name="app-map", slug="c" * 32, audience="public", extra=""))
    (proj / "spaces" / "air" / "policies" / "app-map-1.yaml").write_text(POLICY.format(
        name="app-map-1", assignee="{kind: role, id: 'endpoint:city/app-map'}", pattern=""))
    app = {e.name: e for e in ex.load_endpoints(b / "gitea" / "files")}["app-map"]
    assert [g.name for g in app.grants] == ["app-map-1"] and app.public_grants() == []


def test_a_person_holds_a_group_grant_by_bare_or_full_path():
    tok = "x." + suite.base64_json({"aud": ["s"], "sub": "u1", "groups": ["/city/staff"]}) + ".y"
    p = suite.Person(tok)
    assert p.aud == {"s"}
    assert p.holds(ex.Grant("g", {"kind": "group", "id": "staff"}, {"retrieveOps"}, []))
    assert p.holds(ex.Grant("u", {"kind": "user", "id": "u1"}, {"retrieveOps"}, []))
    assert not p.holds(ex.Grant("o", {"kind": "group", "id": "others"}, {"retrieveOps"}, []))
