"""A scripted OpenAI-compatible model on loopback for the assistant suite (the pattern of
platform crates/assistant/tests/chat_tests.rs, one level further out: jc's own model proxy calls
it as JC_MODEL_BASE, so the chat, the proxy and the gateway reads are jc's own).

POST /chat/completions (and /v1/chat/completions) answers by what the call is told:
  - the router ("You prepare the searches"): the route the suite set with `plan(endpoint, type)`,
  - the follow-up judge ("You check whether the passages"): the passages answer,
  - any other call (the answer): ANSWER, streamed when the call asks for a stream.
Every request body is kept in `calls` (the suite reads what reached the model). No key is
checked: the key file the proxy holds is a dummy."""

from __future__ import annotations

import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROUTER = "You prepare the searches"
JUDGE = "You check whether the passages"
ANSWER = "Smoke answer from the data read [1]."


class Stub:
    def __init__(self, port: int = 0):
        self.lock = threading.Lock()
        self.calls: list[dict] = []
        self.route: dict = {"route": "direct", "lang": "en", "queries": [], "data": [], "followups": []}
        stub = self

        class H(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_GET(self):
                self.send_error(404)

            def do_POST(self):
                n = int(self.headers.get("content-length") or 0)
                try:
                    body = json.loads(self.rfile.read(n) or b"{}")
                except ValueError:
                    return self.send_error(400)
                if self.path.rstrip("/") not in ("/chat/completions", "/v1/chat/completions"):
                    return self.send_error(404)
                with stub.lock:
                    stub.calls.append(body)
                stub.reply(self, body)

        self.srv = ThreadingHTTPServer(("127.0.0.1", port), H)
        self.srv.daemon_threads = True
        self.port = self.srv.server_address[1]
        threading.Thread(target=self.srv.serve_forever, daemon=True).start()

    def plan(self, route: dict) -> None:
        with self.lock:
            self.route = route
            self.calls = []

    def kind(self, body: dict) -> str:
        told = json.dumps(body.get("messages") or [], ensure_ascii=False)
        if ROUTER in told:
            return "router"
        if JUDGE in told:
            return "judge"
        return "answer"

    def reply(self, h, body: dict) -> None:
        kind = self.kind(body)
        if kind == "router":
            with self.lock:
                text = json.dumps(self.route, ensure_ascii=False)
        elif kind == "judge":
            text = json.dumps({"answers": True, "missing": "", "queries": []})
        else:
            text = ANSWER
        usage = {"prompt_tokens": 100, "completion_tokens": 10, "total_tokens": 110}
        if body.get("stream"):
            h.send_response(200)
            h.send_header("content-type", "text/event-stream")
            h.end_headers()
            for chunk in ({"choices": [{"index": 0, "delta": {"role": "assistant", "content": text}}]},
                          {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
                          {"choices": [], "usage": usage}):
                h.wfile.write(b"data: " + json.dumps(chunk).encode() + b"\n\n")
            h.wfile.write(b"data: [DONE]\n\n")
            return
        raw = json.dumps({"id": "smoke", "object": "chat.completion", "model": body.get("model", "stub"),
                          "choices": [{"index": 0, "message": {"role": "assistant", "content": text},
                                       "finish_reason": "stop"}], "usage": usage}).encode()
        h.send_response(200)
        h.send_header("content-type", "application/json")
        h.send_header("content-length", str(len(raw)))
        h.end_headers()
        h.wfile.write(raw)

    def answer_calls(self) -> list[dict]:
        with self.lock:
            return [c for c in self.calls if self.kind(c) == "answer"]

    def close(self) -> None:
        self.srv.shutdown()
        self.srv.server_close()
