"""Selftests of the assistant suite against a fake jc assistant on loopback that calls the suite's
model stub as jc's proxy would (the live proof is a rehearsal run): a jc that reads only what a
deployment may read passes, one that reads a closed Endpoint or serves every id fails.

    python3 -m pytest bins/jc/smoke/assistant/tests -q
"""

import argparse
import hashlib
import importlib.util
import json
import socket
import sys
import threading
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

HERE = Path(__file__).resolve().parent
SUITE_DIR = HERE.parent
sys.path.insert(0, str(SUITE_DIR))
sys.path.insert(0, str(SUITE_DIR.parent))
sys.path.append(str(SUITE_DIR.parent / "endpoints"))

import export as ex
import modelstub
from common import FAIL, PASS, SKIP

_spec = importlib.util.spec_from_file_location("smoke_assistant_suite", SUITE_DIR / "suite.py")
suite = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(suite)

DC = "https://uri.etsi.org/ngsi-ld/default-context/"


def write_export(root: Path) -> Path:
    files = root / "gitea" / "files" / "joinedcontext" / "hel"
    d = files / "spaces" / "air"
    (d / "endpoints").mkdir(parents=True)
    (d / "policies").mkdir()
    (d / "space.yaml").write_text("kind: ContextSpace\nmetadata: {name: air, namespace: hel}\nspec: {urnSegment: hel-air}\n")
    (d / "policies" / "public.yaml").write_text(
        "kind: Policy\nmetadata: {name: public-read, namespace: hel}\nspec:\n  contextSpaceRef: air\n"
        "  assignee: {kind: role, id: public}\n  operations: [retrieveOps]\n"
        "  information:\n    - entities: [{type: AirQualityObserved}]\n")
    (d / "policies" / "staff.yaml").write_text(
        "kind: Policy\nmetadata: {name: staff-read, namespace: hel}\nspec:\n  contextSpaceRef: air\n"
        "  assignee: {kind: role, id: staff}\n  operations: [retrieveOps]\n"
        "  information:\n    - entities: [{type: Payroll}]\n")
    for name, audience in (("open", "public"), ("payroll", "organization")):
        (d / "endpoints" / f"{name}.yaml").write_text(
            f"kind: Endpoint\nmetadata: {{name: {name}, namespace: hel}}\nspec:\n  contextSpaceRef: air\n"
            f"  slug: hel-{name}\n  audience: {audience}\n  enabledRepresentations: [ngsi-ld, mcp]\n")
    a = files / "assistant" / "deployments"
    a.mkdir(parents=True)
    (a / "public.yaml").write_text("kind: AssistantDeployment\nmetadata: {name: public, namespace: hel}\n"
                                   "spec:\n  publicId: hel-public\n  channel: public\n  connectors: [{endpoint: open}]\n")
    (a / "knowledge.yaml").write_text("kind: AssistantDeployment\nmetadata: {name: knowledge, namespace: hel}\n"
                                      "spec:\n  publicId: hel-knowledge\n  channel: internal\n")
    return root


INDEX = {"tenants": {"hel-air": {
    "AirQualityObserved": {"iri": DC + "AirQualityObserved", "count": 3, "sample": "urn:ngsi-ld:AirQualityObserved:a1",
                           "location": False, "expires": False, "patterns": {}},
    "Payroll": {"iri": DC + "Payroll", "count": 2, "sample": "urn:ngsi-ld:Payroll:secret-1", "location": False,
                "expires": False, "patterns": {}}}}}
ROWS = {"open": ("AirQualityObserved", "urn:ngsi-ld:AirQualityObserved:a1"),
        "payroll": ("Payroll", "urn:ngsi-ld:Payroll:secret-1")}


class Assistant:
    """/d/... and /api/v1/d/... for the deployments; a chat asks the model at `model` like jc's
    proxy: the router, then the answer with the rows read. `leaky` reads any routed Endpoint and
    serves every id."""

    def __init__(self, model_port: int, leaky=False, deaf=False):
        public = {"hel-public": {"open"}}
        fake = self

        class H(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def send(self, status, body=b"", ctype="application/json"):
                self.send_response(status)
                self.send_header("content-type", ctype)
                self.send_header("content-length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def known(self, pid):
                return leaky or pid in public

            def do_GET(self):
                p = self.path.split("?")[0].split("/")
                if self.path == "/d/widget.js":
                    return self.send(200, b"x", "text/javascript")
                if self.path == "/d/widget.css":
                    return self.send(200, b"x", "text/css")
                if len(p) == 4 and p[1] == "d":
                    if not self.known(p[2]):
                        return self.send(404, b"<html>missing</html>", "text/html")
                    return self.send(200, b"<html>widget</html>" if p[3] == "widget" else b"x", "text/html")
                if len(p) == 6 and p[3] == "d":
                    if not self.known(p[4]):
                        return self.send(404)
                    if p[5] == "challenge":
                        return self.send(200, json.dumps({"challenge": "c.1.8.s", "difficulty": 8}).encode())
                    return self.send(200, b"{}")
                self.send(404)

            def do_POST(self):
                p = self.path.split("/")
                body = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)) or b"{}")
                if len(p) != 6 or not self.known(p[4]):
                    return self.send(404)
                if p[5] == "token":
                    ok = suite.zero_bits(hashlib.sha256(f"{body['challenge']}:{body['nonce']}".encode()).digest()) >= 8
                    return self.send(200 if ok else 400, json.dumps({"token": "v"} if ok else {}).encode())
                if self.headers.get("x-jc-visitor") != "v":
                    return self.send(401)
                if self.headers.get("origin") not in (None,) and not leaky:
                    return self.send(403)
                if deaf:  # the model unreachable: an error and done, nothing read
                    raw = b'event: error\ndata: {"status": 503}\n\nevent: done\ndata: {}\n\n'
                    return self.send(200, raw, "text/event-stream")
                # The router's catalogue lists what the deployment may read, as jc's does.
                shown = "".join(f"- {n} (title): {ROWS[n][0]}; id, type\n" for n in sorted(public.get(p[4], set())))
                route = fake.model({"messages": [{"role": "system", "content": modelstub.ROUTER},
                                                 {"role": "user", "content": [{"type": "text", "text": shown}]}]})
                item = (json.loads(route).get("data") or [{}])[0]
                events = []
                if item.get("endpoint") in public.get(p[4], set()) or (leaky and item.get("endpoint") in ROWS):
                    st, eid = ROWS[item["endpoint"]]
                    events.append(("data", {"type": st, "columns": ["id"], "rows": [[eid]]}))
                answer = fake.model({"messages": [{"role": "user", "content": json.dumps(events)}]})
                events += [("answer", {"text": answer}), ("done", {})]
                raw = "".join(f"event: {n}\ndata: {json.dumps(d)}\n\n" for n, d in events).encode()
                self.send(200, raw, "text/event-stream")

        self.model_port = model_port
        self.srv = ThreadingHTTPServer(("127.0.0.1", 0), H)
        self.srv.daemon_threads = True
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()
        self.base = f"http://127.0.0.1:{self.srv.server_address[1]}"

    def model(self, body) -> str:
        req = urllib.request.Request(f"http://127.0.0.1:{self.model_port}/chat/completions",
                                     data=json.dumps(body).encode(), headers={"content-type": "application/json"})
        with urllib.request.urlopen(req, timeout=5) as r:
            return json.loads(r.read())["choices"][0]["message"]["content"]

    def close(self):
        self.srv.shutdown()
        self.srv.server_close()


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def run_suite(tmp_path, leaky=False, log=None, off=False, deaf=False, kept=False):
    root = write_export(tmp_path / "export")
    files = root / "gitea" / "files"
    port = free_port()
    jc = Assistant(port, leaky, deaf)
    if off:
        jc.close()
    a = argparse.Namespace(base_url=jc.base, model_stub_port=port, log_file=log, prod_widget=False, timeout=10,
                           kept_component=kept)
    s = suite.Suite(a, suite.load_deployments(files), suite.Targets(ex.load_endpoints(files), INDEX))
    try:
        s.run()
    finally:
        if not off:
            jc.close()
    return {r["id"]: r for r in s.r.rows}, s


def test_a_jc_that_reads_only_granted_endpoints_passes(tmp_path):
    r, s = run_suite(tmp_path)
    fails = [x for x in s.r.rows if x["result"] == FAIL]
    assert not fails, fails
    for rid in ("chat.hel-public.granted", "chat.hel-public.closed-refused", "chat.hel-public.foreign-origin",
                "page.hel-public.widget", "page.hel-knowledge.widget", "page.hel-knowledge.config",
                "control.unknown-id", "bundle.widget.js"):
        assert r[rid]["result"] == PASS, rid
    assert r["prod.widget"]["result"] == SKIP and "--prod-widget" in r["prod.widget"]["reason"]
    # An internal deployment has no public chat to ask.
    assert not any(k.startswith("chat.hel-knowledge") for k in r)


def test_a_jc_that_reads_a_closed_endpoint_and_serves_every_id_fails(tmp_path):
    r, _ = run_suite(tmp_path, leaky=True)
    for rid in ("chat.hel-public.closed-refused", "chat.hel-public.foreign-origin", "control.unknown-id",
                "page.hel-knowledge.widget"):
        assert r[rid]["result"] == FAIL, rid
    assert r["chat.hel-public.granted"]["result"] == PASS


def test_a_jc_that_never_reaches_the_model_proves_no_refusal(tmp_path):
    r, _ = run_suite(tmp_path, deaf=True)
    assert r["chat.hel-public.closed-refused"]["result"] == FAIL
    assert "model asked: False" in r["chat.hel-public.closed-refused"]["actual"]
    assert r["chat.hel-public.granted"]["result"] == FAIL and "503" in r["chat.hel-public.granted"]["actual"]


def test_a_module_off_for_the_model_files_is_a_named_skip_and_otherwise_a_failure(tmp_path):
    log = tmp_path / "jc.log"
    log.write_text("2026-11-09T10:00:00Z WARN jc::modules::assistant: assistant: off "
                   "why=JC_ASSISTANT_MODEL_DIR (the embedding model) is unset\n")
    r, _ = run_suite(tmp_path, log=str(log), off=True)
    assert r["chat.hel-public.granted"]["result"] == SKIP and "embedding model" in r["chat.hel-public.granted"]["reason"]
    log.write_text("WARN assistant: off why=[assistant.database] is not configured\n")
    r, _ = run_suite(tmp_path / "b", log=str(log), off=True)
    assert r["chat.hel-public.granted"]["result"] == FAIL and "assistant.database" in r["chat.hel-public.granted"]["actual"]
    assert r["page.hel-public"]["result"] == FAIL


def test_a_kept_component_is_a_named_skip_whatever_turned_the_module_off(tmp_path):
    """The rehearsal's gate: the assistant stays a component on dev (tasks/GOAL-jc-dev.md), so a
    module off for its database is a named skip there, never a failure or a known.txt line."""
    log = tmp_path / "jc.log"
    log.write_text("WARN assistant: off why=[assistant.database] is not configured\n")
    r, s = run_suite(tmp_path, log=str(log), off=True, kept=True)
    assert not [x for x in s.r.rows if x["result"] == FAIL], s.r.rows
    for rid in ("chat.hel-public.granted", "page.hel-public", "bundle.widget.js"):
        assert r[rid]["result"] == SKIP and r[rid]["reason"].startswith("kept component: assistant"), r[rid]


def test_the_proof_has_the_leading_zero_bits_asked():
    nonce = suite.solve("abc.1.12.sig", 12)
    assert suite.zero_bits(hashlib.sha256(f"abc.1.12.sig:{nonce}".encode()).digest()) >= 12
    assert suite.zero_bits(b"\x00\x0f") == 12 and suite.zero_bits(b"\x80") == 0


def test_the_stub_routes_by_the_catalogue_only_when_asked_to_pick():
    stub = modelstub.Stub()
    try:
        told = {"messages": [{"role": "system", "content": modelstub.ROUTER}, {"role": "user", "content": [
            {"type": "text", "text": "Live data:\n- bbsk-verejne (Region, public): KeyPerformanceIndicator; id\n"
                                     "- bbsk-otvorene-data (tables): Bridge, Grant; id, name\n"}]}]}
        stub.plan({"data": [{"endpoint": "app-bbsk-data", "type": "Bridge"}]}, pick=True)
        assert stub.routed_for(told)["data"][0]["endpoint"] == "bbsk-otvorene-data"
        stub.plan({"data": [{"endpoint": "bbsk-mesto", "type": "Bridge"}]})
        assert stub.routed_for(told)["data"][0]["endpoint"] == "bbsk-mesto"
    finally:
        stub.close()


def test_the_stub_streams_when_asked_and_tells_the_router_from_the_answer():
    stub = modelstub.Stub()
    try:
        stub.plan({"route": "data", "data": [{"endpoint": "p/e", "type": "T"}]})
        url = f"http://127.0.0.1:{stub.port}/chat/completions"

        def post(body):
            req = urllib.request.Request(url, data=json.dumps(body).encode(), headers={"content-type": "application/json"})
            with urllib.request.urlopen(req, timeout=5) as r:
                return r.read()

        routed = json.loads(post({"messages": [{"role": "system", "content": [{"text": modelstub.ROUTER}]}]}))
        assert json.loads(routed["choices"][0]["message"]["content"])["data"][0]["endpoint"] == "p/e"
        streamed = post({"stream": True, "messages": [{"role": "user", "content": "q"}]}).decode()
        assert modelstub.ANSWER in streamed and streamed.rstrip().endswith("data: [DONE]")
        assert len(stub.answer_calls()) == 1
    finally:
        stub.close()


@pytest.mark.parametrize("raw,names", [(b"event: data\ndata: {\"a\": 1}\n\nevent: done\ndata: {}\n\n", ["data", "done"]),
                                       (b"", [])])
def test_events_are_read_from_the_sse_body(raw, names):
    assert [n for n, _ in suite.events(raw)] == names
