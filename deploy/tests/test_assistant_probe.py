"""scripts/assistant-probe.py against a stub public assistant (T-4809): it earns a visitor token
with the proof of work, asks each question with it, and counts MODEL_UNREACHABLE and every non-200
apart from a whole answer."""

import importlib.util
import io
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "assistant-probe.py"
spec = importlib.util.spec_from_file_location("assistant_probe", SCRIPT)
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)

DONE = "event: conversation\ndata: {}\n\nevent: answer\ndata: {\"text\": \"Dobrý deň\"}\n\nevent: done\ndata: {}\n\n"
UNREACHABLE = f"event: conversation\ndata: {{}}\n\nevent: error\ndata: {{\"detail\": \"{probe.MODEL_UNREACHABLE}.\"}}\n\n"


@pytest.fixture
def assistant():
    """A stub whose chat answers come from `answers` in order; it records each chat's token."""
    seen = {"tokens": [], "answers": []}

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass

        def reply(self, status, body, kind="application/json"):
            data = body.encode()
            self.send_response(status)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def do_GET(self):
            assert self.path == "/api/v1/d/data/challenge", self.path
            self.reply(200, json.dumps({"challenge": "c.1", "difficulty": 8}))

        def do_POST(self):
            body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
            if self.path == "/api/v1/d/data/token":
                proof = probe.hashlib.sha256(f"c.1:{body['nonce']}".encode()).digest()
                assert probe.zero_bits(proof) >= 8
                return self.reply(200, json.dumps({"token": "visitor-1"}))
            assert self.path == "/api/v1/d/data/chat", self.path
            seen["tokens"].append(self.headers.get("X-JC-Visitor"))
            status, stream = seen["answers"].pop(0)
            self.reply(status, stream, "text/event-stream")

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    seen["base"] = f"http://127.0.0.1:{server.server_port}"
    yield seen
    server.shutdown()


def test_each_verdict_is_counted_and_logged(assistant):
    assistant["answers"] = [(200, DONE), (200, UNREACHABLE), (503, "no"), (200, DONE)]
    log = io.StringIO()
    counts = {"ok": 0, "model_unreachable": 0, "other": 0}
    probe.probe(assistant["base"], "data", 0, 4, log, counts)
    assert counts == {"ok": 2, "model_unreachable": 1, "other": 1}
    assert probe.summary(counts) == "assistant probe: 4 asked, 2 ok, 1 model_unreachable, 1 other"
    # One token, earned once, carried by every question.
    assert assistant["tokens"] == ["visitor-1"] * 4
    lines = [json.loads(line) for line in log.getvalue().splitlines()]
    assert [line["verdict"] for line in lines] == ["ok", "model_unreachable", "other", "ok"]
    assert "visitor-1" not in log.getvalue()


def test_an_unreachable_assistant_is_other_and_never_raises():
    counts = {"ok": 0, "model_unreachable": 0, "other": 0}
    probe.probe("http://127.0.0.1:9", "data", 0, 2, io.StringIO(), counts)
    assert counts == {"ok": 0, "model_unreachable": 0, "other": 2}


@pytest.mark.parametrize(
    "status, stream, want",
    [
        (200, DONE, "ok"),
        (200, UNREACHABLE, "model_unreachable"),
        (502, probe.MODEL_UNREACHABLE, "model_unreachable"),
        (200, "event: conversation\ndata: {}\n\n", "other"),
        (429, "", "other"),
    ],
)
def test_the_verdict_of_one_answer(status, stream, want):
    assert probe.verdict(status, stream) == want
