"""Selftests of the subscriptions suite against a fake jc and a fake MQTT broker on loopback (the
live proof is a rehearsal run): a jc that delivers by tenant and grant passes, a jc that answers
101 to everything and delivers every write to every socket fails the control rows.

    python3 -m pytest bins/jc/smoke/subscriptions/tests -q
"""

import argparse
import base64
import hashlib
import importlib.util
import json
import os
import socketserver
import struct
import sys
import threading
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
SUITE_DIR = HERE.parent
sys.path.insert(0, str(SUITE_DIR))
sys.path.insert(0, str(SUITE_DIR.parent))
sys.path.append(str(SUITE_DIR.parent / "endpoints"))

import export as ex
import mqttclient
import wsclient
from common import FAIL, PASS, SKIP

_spec = importlib.util.spec_from_file_location("smoke_subscriptions_suite", SUITE_DIR / "suite.py")
suite = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(suite)

DC = "https://uri.etsi.org/ngsi-ld/default-context/"
SECRET = "fixture-client-secret"
CONTEXT = "https://example.org/air/context.jsonld"


def sa_token(client: str) -> str:
    """The pipelines token as the realm mints it: bound to the project's write Endpoint (`nows`,
    the same space as the public `open` one), never to `open` itself."""
    claims = {"azp": client, "aud": ["portal-internal", client.replace("-pipelines", "-nows")]}
    return "h." + base64.urlsafe_b64encode(json.dumps(claims).encode()).decode().rstrip("=") + ".s"


# ------------------------------------------------------------------------------------ fixture export

def write_export(root: Path) -> Path:
    files = root / "gitea" / "files" / "joinedcontext"
    for project, space, tenant in (("hel", "air", "hel-air"), ("bb", "air", "bb-air")):
        d = files / project / "spaces" / space
        (d / "endpoints").mkdir(parents=True)
        (d / "policies").mkdir()
        (d / "space.yaml").write_text(f"kind: ContextSpace\nmetadata: {{name: {space}, namespace: {project}}}\n"
                                      f"spec: {{urnSegment: {tenant}}}\n")
        (d / "policies" / "public.yaml").write_text(
            f"kind: Policy\nmetadata: {{name: public-read, namespace: {project}}}\n"
            f"spec:\n  contextSpaceRef: {space}\n  assignee: {{kind: role, id: public}}\n"
            "  operations: [retrieveOps]\n  information:\n    - entities: [{type: AirQualityObserved}]\n")
        (d / "policies" / "write.yaml").write_text(
            f"kind: Policy\nmetadata: {{name: pipelines-write, namespace: {project}}}\n"
            f"spec:\n  contextSpaceRef: {space}\n  assignee: {{kind: serviceAccount, id: pipelines}}\n"
            "  operations: [upsertBatch, createBatch]\n  information:\n    - entities: [{type: AirQualityObserved}]\n")
        for name, audience, reps in (("open", "public", "[ngsi-ld, websocket]"),
                                     ("members", "organization", "[ngsi-ld, websocket]"),
                                     ("nows", "public", "[ngsi-ld]")):
            (d / "endpoints" / f"{name}.yaml").write_text(
                f"kind: Endpoint\nmetadata: {{name: {name}, namespace: {project}}}\n"
                f"spec:\n  contextSpaceRef: {space}\n  slug: {project}-{name}\n  audience: {audience}\n"
                f"  enabledRepresentations: {reps}\n")
    realm = {"realm": "smartcity", "clients": [
        {"clientId": f"{p}-pipelines", "secret": f"$(env:KC_{p.upper()}_PIPELINES_SECRET)"} for p in ("hel", "bb")]}
    (root / "keycloak").mkdir(parents=True)
    (root / "keycloak" / "realm-smartcity.json").write_text(json.dumps(realm))
    b = files / "hel" / "mqttbrokers"
    b.mkdir(parents=True)
    (b / "vehicles.yaml").write_text(
        "kind: MqttBroker\nmetadata: {name: vehicles, namespace: hel}\nspec:\n  roles:\n"
        "    - name: trackers\n      serviceAccounts: [vehicle-demo]\n      publish: [\"vehicles/${clientid}/position\"]\n")
    return root


def endpoints_of(root: Path):
    eps = ex.load_endpoints(root / "gitea" / "files")
    return {e.slug: e for e in eps}, eps


# ------------------------------------------------------------------------------------ fake jc

def read_frame(rfile):
    b0, b1 = rfile.read(2)
    n = b1 & 0x7F
    if n == 126:
        n = struct.unpack("!H", rfile.read(2))[0]
    elif n == 127:
        n = struct.unpack("!Q", rfile.read(8))[0]
    key = rfile.read(4)
    payload = bytes(b ^ key[i % 4] for i, b in enumerate(rfile.read(n)))
    return b0 & 0x0F, payload


class Jc:
    """The doors the suite uses. `lax`: every upgrade 101, every write to every socket."""

    def __init__(self, endpoints, lax=False):
        self.eps, self.lax = endpoints, lax
        self.lock = threading.Lock()
        self.sockets = []  # [(endpoint, conn, subs {id: (type, q)}, seq [n])]
        self.held = 0
        jc = self

        class H(socketserver.StreamRequestHandler):
            def handle(self):
                jc.serve(self)

        self.srv = socketserver.ThreadingTCPServer(("127.0.0.1", 0), H)
        self.srv.daemon_threads = True
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()
        self.base = f"http://127.0.0.1:{self.srv.server_address[1]}"

    def answer(self, h, status, body=b"", headers=()):
        h.wfile.write(f"HTTP/1.1 {status} X\r\nContent-Length: {len(body)}\r\nConnection: close\r\n".encode()
                      + b"".join(f"{k}: {v}\r\n".encode() for k, v in headers) + b"\r\n" + body)

    def serve(self, h):
        line = h.rfile.readline().decode()
        if not line:
            return
        method, path, _ = line.split(" ", 2)
        hdrs = {}
        while True:
            x = h.rfile.readline().decode().strip()
            if not x:
                break
            k, _, v = x.partition(":")
            hdrs[k.strip().lower()] = v.strip()
        body = h.rfile.read(int(hdrs.get("content-length", "0") or 0))
        if path.startswith("/api/endpoint/") and path.endswith("/ws"):
            return self.ws(h, path.split("/")[3], hdrs)
        if path == "/id/realms/smartcity/protocol/openid-connect/token":
            form = dict(x.split("=", 1) for x in body.decode().split("&"))
            ok = form.get("client_secret") == SECRET
            return self.answer(h, 200 if ok else 401, json.dumps({"access_token": sa_token(form["client_id"])}
                                                                 if ok else {}).encode())
        parts = path.split("?")[0].split("/")
        if method == "GET" and parts[4:7] == ["ngsi-ld", "v1", "entities"] and len(parts) == 8:
            sample = {"id": "x", "type": "AirQualityObserved", "@context": CONTEXT,
                      "location": {"type": "GeoProperty", "value": {"type": "Point", "coordinates": [0, 0]}},
                      "pm10": {"type": "Property", "value": 7.1}}
            return self.answer(h, 200, json.dumps(sample).encode())
        if method == "POST" and parts[4:7] == ["ngsi-ld", "v1", "entityOperations"]:
            e = self.eps.get(parts[3])
            claims = suite.jwt_claims(hdrs.get("authorization", "")[7:])
            bound = e is not None and e.slug in claims.get("aud", [])
            if not self.lax and not bound:  # jc_core: token is not bound to this resource
                return self.answer(h, 401, b'{"title": "Unauthorized"}')
            if parts[7] == "delete":
                return self.answer(h, 204 if hdrs.get("jc-reason") else 400)
            for ent in json.loads(body):
                # The body's @context expands the short type, as the broker stores it; a write
                # without one, or of no attribute the view shows, notifies nobody (matching.rs).
                if not self.lax and (ent.get("@context") != CONTEXT or "pm10" not in ent):
                    continue
                self.notify(e.tenant if e else None, {**ent, "type": DC + ent["type"]})
            return self.answer(h, 201, b"[]")
        if self.lax and path == "/internal/mqtt/auth":  # the internal listener's door on the public base
            return self.answer(h, 200, b'{"result": "allow", "superuser": false}')
        return self.answer(h, 200 if self.lax else 404, b"{}")

    def ws(self, h, slug, hdrs):
        e = self.eps.get(slug)
        upgrade = hdrs.get("upgrade", "").lower() == "websocket"
        if not self.lax:
            if e is None or "websocket" not in e.representations:
                return self.answer(h, 404, b'{"title": "Not Found"}')
            if e.closed:
                return self.answer(h, 401, b'{"title": "Unauthorized"}')
            if not upgrade:
                return self.answer(h, 426, b'{"guide": "open a WebSocket"}')
            with self.lock:
                if self.held >= suite.PER_CALLER:
                    return self.answer(h, 429, b'{"title": "Too Many Requests"}')
                self.held += 1
        elif not upgrade:
            return self.answer(h, 200, b'{"guide": "anything"}')
        accept = base64.b64encode(hashlib.sha1((hdrs["sec-websocket-key"] + wsclient.GUID).encode()).digest()).decode()
        h.wfile.write(("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                       f"Sec-WebSocket-Accept: {accept}\r\nSec-WebSocket-Protocol: ngsi-ld-ws\r\n\r\n").encode())
        conn = {"e": e, "h": h, "subs": {}, "seq": [0], "w": threading.Lock()}
        with self.lock:
            self.sockets.append(conn)
        self.send(conn, {"metadata": {"x-op": "x-welcome", "x-connection": "c", "x-resume-token": "t"}})
        try:
            while True:
                opcode, payload = read_frame(h.rfile)
                if opcode == 0x8:
                    break
                m = json.loads(payload)
                meta, xid = m["metadata"], m["metadata"].get("x-id")
                if meta["x-op"] == "x-subscribe":
                    sid = f"urn:ngsi-ld:Subscription:{os.urandom(3).hex()}"
                    b = m["body"]
                    conn["subs"][sid] = (b["entities"][0]["type"], b.get("q"))
                    self.send(conn, {"metadata": {"x-op": "x-result", "x-id": xid, "x-status": 201}, "body": {"id": sid}})
                elif meta["x-op"] == "x-unsubscribe":
                    conn["subs"].pop(meta.get("x-subscription"), None)
                    self.send(conn, {"metadata": {"x-op": "x-result", "x-id": xid, "x-status": 204}})
        except (ValueError, OSError):
            pass
        finally:
            with self.lock:
                self.sockets.remove(conn)
                if not self.lax:
                    self.held -= 1

    def send(self, conn, m):
        with conn["w"]:
            conn["h"].wfile.write(wsclient.frame(0x1, json.dumps(m).encode(), mask=False))
            conn["h"].wfile.flush()

    def notify(self, tenant, ent):
        mark = ent.get(DC + suite.MARK, {}).get("value")
        with self.lock:
            conns = list(self.sockets)
        for c in conns:
            for sid, (typ, q) in list(c["subs"].items()):
                if not self.lax:
                    if c["e"].tenant != tenant or typ != ent["type"] or not typ.endswith("AirQualityObserved"):
                        continue
                    if q and q != f"{suite.MARK}=={mark}":
                        continue
                c["seq"][0] += 1
                self.send(c, {"metadata": {"x-op": "x-notification", "x-seq": c["seq"][0], "x-subscription": sid},
                              "body": {"subscriptionId": sid, "data": [{"id": ent["id"], "type": ent["type"]}]}})

    def close(self):
        self.srv.shutdown()
        self.srv.server_close()


# ------------------------------------------------------------------------------------ helpers

def index_of(eps):
    slot = {"iri": DC + "AirQualityObserved", "count": 5, "sample": "x", "location": False, "expires": False,
            "patterns": {}}
    return {"tenants": {e.tenant: {"AirQualityObserved": dict(slot)} for e in eps}}


def args(base, tmp, **kw):
    realm = tmp / "realm"
    realm.mkdir(exist_ok=True)
    for p in ("HEL", "BB"):
        (realm / f"KC_{p}_PIPELINES_SECRET").write_text(SECRET + "\n")
    a = argparse.Namespace(base_url=base, realm_secrets_dir=str(realm), per_class=1, wait=3.0, silence=0.5, settle=0.1,
                           timeout=5.0, mqtt_broker=[], mqtt_token_dir=None)
    for k, v in kw.items():
        setattr(a, k, v)
    return a


def rows(s):
    return {r["id"]: r for r in s.r.rows}


@pytest.fixture
def world(tmp_path):
    root = write_export(tmp_path / "export")
    by_slug, eps = endpoints_of(root)
    return tmp_path, root, by_slug, eps


# ------------------------------------------------------------------------------------ tests

def test_the_plan_takes_one_public_websocket_endpoint_per_class_on_a_whole_granted_type(world):
    _, _, _, eps = world
    chosen, skipped = suite.plan(eps, index_of(eps), 1)
    assert sorted(e.slug for _, e, _, _, _ in chosen) == ["bb-open", "hel-open"]
    assert {(st, w) for _, _, st, _, w in chosen} == {("AirQualityObserved", "pipelines")} and not skipped
    # A type held only by an id pattern, or a grant narrowed by q, is no probe target.
    for e in eps:
        for g in e.grants:
            g.narrowed = "q"
    assert suite.plan(eps, index_of(eps), 1)[0] == []


def test_a_jc_that_delivers_by_tenant_and_grant_passes_every_check(world):
    tmp, root, by_slug, eps = world
    jc = Jc(by_slug)
    try:
        s = suite.Suite(args(jc.base, tmp), eps, index_of(eps), suite.load_brokers(root / "gitea" / "files"),
                        suite.load_clients(root))
        s.run()
    finally:
        jc.close()
    r = rows(s)
    fails = [x for x in s.r.rows if x["result"] == FAIL]
    assert not fails, fails
    for slug in ("hel-open", "bb-open"):
        assert r[f"deliver.{slug}"]["result"] == PASS and r[f"deliver.{slug}.envelope"]["result"] == PASS
        assert r[f"hub.{slug}.second-subscriber"]["result"] == PASS
        assert r[f"hub.{slug}.filter"]["result"] == PASS
        assert r[f"hub.{slug}.unsubscribe"]["result"] == PASS
        assert r[f"outside.{slug}.other-endpoint"]["result"] == PASS
    assert r["refused.hel-members.anonymous"]["result"] == PASS
    assert r["refused.per-caller-cap"]["result"] == PASS
    assert "cleanup.probes" not in r, r.get("cleanup.probes")
    assert r["transport.hel-nows"]["result"] == PASS and r["control.unknown-slug"]["result"] == PASS
    assert r["mqtt.auth-not-public"]["result"] == PASS
    # The broker stays a kept component: named skips, never silent.
    mq = [x for x in s.r.rows if x["id"].startswith("mqtt.hel/vehicles.")]
    assert mq and all(x["result"] == SKIP and x["reason"].startswith("kept component") for x in mq)


def test_a_jc_that_answers_everything_fails_the_control_rows(world):
    tmp, root, by_slug, eps = world
    jc = Jc(by_slug, lax=True)
    try:
        s = suite.Suite(args(jc.base, tmp), eps, index_of(eps), [], suite.load_clients(root))
        s.run()
    finally:
        jc.close()
    r = rows(s)
    for rid in ("control.unknown-slug", "transport.hel-nows", "transport.hel-members", "refused.hel-members.anonymous",
                "refused.per-caller-cap", "hub.hel-open.filter", "outside.hel-open.other-endpoint",
                "mqtt.auth-not-public"):
        assert r[rid]["result"] == FAIL, rid
    # A delivery that reaches everyone still reaches A: the positive row alone proves nothing.
    assert r["deliver.hel-open"]["result"] == PASS


def test_a_jc_that_delivers_nothing_fails_delivery_and_proves_no_silence(world):
    tmp, root, by_slug, eps = world
    jc = Jc(by_slug)
    jc.notify = lambda tenant, ent: None
    s = suite.Suite(args(jc.base, tmp), eps, index_of(eps), [], suite.load_clients(root))
    try:
        s.run()
        # A writer whose Policy grants no delete leaves its probes: a named row.
        s.probes = [(by_slug["hel-open"], "urn:ngsi-ld:AirQualityObserved:left", "bad")]
        s.cleanup()
    finally:
        jc.close()
    r = rows(s)
    assert r["cleanup.probes"]["result"] == SKIP and "left" in r["cleanup.probes"]["reason"]
    assert r["deliver.hel-open"]["result"] == FAIL
    assert r["hub.hel-open.filter"]["result"] == FAIL and "unproven" in r["hub.hel-open.filter"]["reason"]
    assert r["outside.hel-open.other-endpoint"]["result"] == FAIL


def test_transports_only_reads_part_one_and_names_the_other_parts_skipped(world):
    """Live mode's flag: anonymous GETs of part 1; no socket, no probe write."""
    tmp, root, by_slug, eps = world
    jc = Jc(by_slug)
    try:
        s = suite.Suite(args(jc.base, tmp, transports_only=True), eps, index_of(eps), [], suite.load_clients(root))
        s.run()
    finally:
        jc.close()
    r = rows(s)
    assert {k for k, x in r.items() if x["result"] != SKIP} == {"control.unknown-slug"} | {
        f"transport.{e.slug}" for e in eps}
    assert all(r[k]["result"] == SKIP and "--transports-only" in r[k]["reason"] for k in ("deliver", "refusals", "mqtt"))
    assert not s.probes


def test_without_the_realm_secrets_delivery_is_a_named_skip(world):
    tmp, _, _, eps = world
    s = suite.Suite(args("http://127.0.0.1:9", tmp, realm_secrets_dir=None), eps, {}, [])
    s.delivery()
    assert [x["result"] for x in s.r.rows] == [SKIP] and "--realm-secrets-dir" in s.r.rows[0]["reason"]


def test_a_writer_without_a_token_is_a_named_skip_and_the_secret_never_lands_in_a_row(world):
    tmp, root, by_slug, eps = world
    jc = Jc(by_slug)
    try:
        a = args(jc.base, tmp)
        (Path(a.realm_secrets_dir) / "KC_BB_PIPELINES_SECRET").write_text("wrong\n")
        s = suite.Suite(a, eps, index_of(eps), [], suite.load_clients(root))
        s.delivery()
    finally:
        jc.close()
    r = rows(s)
    assert r["deliver.hel-open"]["result"] == PASS
    assert r["deliver.bb-open"]["result"] == SKIP and "401" in r["deliver.bb-open"]["reason"]
    assert not any(SECRET in json.dumps(x) or sa_token("hel-pipelines") in json.dumps(x) for x in s.r.rows)
    assert "through hel/nows" in r["deliver.hel-open"]["call"]


# ------------------------------------------------------------------------------------ MQTT

class Broker:
    """CONNACK 0 for the token `good` with username == client id, 5 otherwise; PUBACK on
    vehicles/<client>/position, a disconnect on any other topic."""

    def __init__(self):
        class H(socketserver.StreamRequestHandler):
            def handle(self):
                client = None
                while True:
                    first = self.rfile.read(1)
                    if not first:
                        return
                    n, mult = 0, 1
                    while True:
                        b = self.rfile.read(1)[0]
                        n += (b & 0x7F) * mult
                        mult *= 128
                        if not b & 0x80:
                            break
                    body = self.rfile.read(n)
                    kind = first[0] >> 4
                    if kind == mqttclient.CONNECT:
                        fields, pos = [], 10
                        for _ in range(3):
                            k = struct.unpack("!H", body[pos:pos + 2])[0]
                            fields.append(body[pos + 2:pos + 2 + k].decode())
                            pos += 2 + k
                        client, user, pw = fields
                        ok = pw == "good" and user == client
                        self.wfile.write(bytes([0x20, 2, 0, 0 if ok else 5]))
                        if not ok:
                            return
                    elif kind == mqttclient.PUBLISH:
                        k = struct.unpack("!H", body[:2])[0]
                        topic = body[2:2 + k].decode()
                        if topic != f"vehicles/{client}/position":
                            return
                        self.wfile.write(bytes([0x40, 2]) + body[2 + k:4 + k])
                    elif kind == mqttclient.DISCONNECT:
                        return

        self.srv = socketserver.ThreadingTCPServer(("127.0.0.1", 0), H)
        self.srv.daemon_threads = True
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()
        self.url = f"mqtt://127.0.0.1:{self.srv.server_address[1]}"


def test_remaining_length_and_connect_are_encoded_as_mqtt_311():
    assert [mqttclient.remaining(n) for n in (0, 127, 128, 16383, 16384)] == \
        [b"\x00", b"\x7f", b"\x80\x01", b"\xff\x7f", b"\x80\x80\x01"]
    p = mqttclient.connect_packet("c", "u", "p")
    assert p[0] == 0x10 and p[2:8] == b"\x00\x04MQTT" and p[8] == 4 and p[9] == 0xC2


def test_a_given_broker_runs_every_check_with_tokens_from_files(tmp_path):
    root = write_export(tmp_path / "export")
    b = Broker()
    tokens = tmp_path / "tokens" / "hel"
    tokens.mkdir(parents=True)
    (tokens / "vehicle-demo").write_text("good\n")
    a = args("http://127.0.0.1:9", tmp_path, mqtt_broker=[f"hel/vehicles={b.url}"],
             mqtt_token_dir=str(tmp_path / "tokens"))
    s = suite.Suite(a, [], {}, suite.load_brokers(root / "gitea" / "files"))
    s.mqtt()
    b.srv.shutdown()
    r = rows(s)
    for rid in ("mqtt.hel/vehicles.connect", "mqtt.hel/vehicles.refused-token",
                "mqtt.hel/vehicles.trackers.publish.0", "mqtt.hel/vehicles.trackers.refused-topic"):
        assert r[rid]["result"] == PASS, (rid, r[rid])
    # Nothing answers on :9, so the public /internal/mqtt/auth row has no answer: it fails.
    assert r["mqtt.auth-not-public"]["result"] == FAIL
    assert not any("good" in json.dumps(x) for x in s.r.rows), "a token in a result row"


def test_a_broker_off_loopback_is_refused():
    a = args("http://127.0.0.1:9", Path("/tmp"), mqtt_broker=["hel/vehicles=mqtt://broker.dev.example:1883"])
    s = suite.Suite(a, [], {}, [{"project": "hel", "name": "vehicles", "roles": []}])
    with pytest.raises(SystemExit):
        s.mqtt()


def test_a_topic_filter_is_made_concrete_for_the_client():
    assert suite.concrete("vehicles/${clientid}/position", "veh-1") == "vehicles/veh-1/position"
    assert suite.concrete("ovzdusie/+/air/#", "s") == "ovzdusie/smoke/air/smoke"
