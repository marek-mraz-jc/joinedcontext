"""A scripted OpenAI-compatible model on loopback for the assistant suite (the pattern of
platform crates/assistant/tests/chat_tests.rs, one level further out: jc's own model proxy calls
it as JC_MODEL_BASE, so the chat, the proxy and the gateway reads are jc's own).

POST /chat/completions (and /v1/chat/completions) answers by what the call is told:
  - the router ("You prepare the searches"): the route the suite set with `plan(route)`; with
    `pick` each read names the Endpoint as the router's catalogue lists it (the hinted one when
    listed, else the first that lists the type), as a model reading the catalogue would,
  - the follow-up judge ("You check whether the passages"): the passages answer,
  - any other call (the answer): ANSWER, streamed when the call asks for a stream; with
    `chart(args)` set and the chart tool offered, first one `show_visualization` call of those
    arguments on the answer's first table (the number the tool's refusal names, when it is not 1).
Every request body is kept in `calls` (the suite reads what reached the model). No key is
checked: the key file the proxy holds is a dummy."""

from __future__ import annotations

import json
import re
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROUTER = "You prepare the searches"
JUDGE = "You check whether the passages"
ANSWER = "Smoke answer from the data read [1]."
CHART_TOOL = "show_visualization"


class Stub:
    def __init__(self, port: int = 0):
        self.lock = threading.Lock()
        self.calls: list[dict] = []
        self.route: dict = {"route": "direct", "lang": "en", "queries": [], "data": [], "followups": []}
        self.pick = False
        self.routed: list[str] = []
        self.charted: dict | None = None
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

    def plan(self, route: dict, pick: bool = False) -> None:
        with self.lock:
            self.route, self.pick, self.routed = route, pick, []
            self.calls = []

    def chart(self, args: dict | None) -> None:
        """`show_visualization` arguments (kind, label, value, title) the next answers call once."""
        with self.lock:
            self.charted = args

    def chart_call(self, body: dict) -> dict | None:
        """The chart call this answer call makes, or None: only when the tool is offered, before
        the call's own result; a refusal naming the tables retries once on the first it names."""
        offered = any((t.get("function") or {}).get("name") == CHART_TOOL for t in body.get("tools") or [])
        with self.lock:
            args = dict(self.charted) if self.charted else None
        if not offered or not args:
            return None
        results = [m for m in body.get("messages") or [] if m.get("role") == "tool"]
        if not results:
            n = 1
        elif len(results) == 1 and "no table" in str(results[0].get("content")):
            numbers = re.findall(r"\b\d+\b", str(results[0].get("content")).split("no table", 1)[1])[1:]
            if not numbers:
                return None
            n = int(numbers[0])
        else:
            return None
        args["data"] = n
        return {"id": f"chart{len(results)}", "type": "function",
                "function": {"name": CHART_TOOL, "arguments": json.dumps(args)}}

    @staticmethod
    def catalogue(told: str) -> list[tuple[str, list[str]]]:
        """(endpoint, types) of each line of the router's live-data list: `- name (title): T1, T2; attrs`."""
        out = []
        for m in re.finditer(r"^- (\S+)(?: \([^\n]*?\))?: ([^;\n]*)", told, re.MULTILINE):
            out.append((m.group(1), [t.strip() for t in m.group(2).split(",") if t.strip()]))
        return out

    def routed_for(self, body: dict) -> dict:
        route = json.loads(json.dumps(self.route))
        if self.pick:
            texts = [part.get("text", "") if isinstance(part, dict) else str(part)
                     for m in body.get("messages") or [] for part in
                     (m.get("content") if isinstance(m.get("content"), list) else [m.get("content") or ""])]
            lines = self.catalogue("\n".join(texts))
            for read in route.get("data") or []:
                fits = [n for n, types in lines if read.get("type") in types]
                if fits:
                    read["endpoint"] = read["endpoint"] if read.get("endpoint") in fits else fits[0]
        self.routed = [r.get("endpoint", "") for r in route.get("data") or []]
        return route

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
                text = json.dumps(self.routed_for(body), ensure_ascii=False)
        elif kind == "judge":
            text = json.dumps({"answers": True, "missing": "", "queries": []})
        else:
            text = ANSWER
            call = self.chart_call(body)
            if call:
                return self.send(h, body, None, call)
        self.send(h, body, text, None)

    @staticmethod
    def send(h, body: dict, text: str | None, call: dict | None) -> None:
        usage = {"prompt_tokens": 100, "completion_tokens": 10, "total_tokens": 110}
        if body.get("stream"):
            h.send_response(200)
            h.send_header("content-type", "text/event-stream")
            h.end_headers()
            delta = {"role": "assistant", "content": text} if call is None else \
                {"role": "assistant", "tool_calls": [dict(call, index=0)]}
            for chunk in ({"choices": [{"index": 0, "delta": delta}]},
                          {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop" if call is None else "tool_calls"}]},
                          {"choices": [], "usage": usage}):
                h.wfile.write(b"data: " + json.dumps(chunk).encode() + b"\n\n")
            h.wfile.write(b"data: [DONE]\n\n")
            return
        message = {"role": "assistant", "content": text}
        if call is not None:
            message["tool_calls"] = [call]
        raw = json.dumps({"id": "smoke", "object": "chat.completion", "model": body.get("model", "stub"),
                          "choices": [{"index": 0, "message": message,
                                       "finish_reason": "stop" if call is None else "tool_calls"}],
                          "usage": usage}).encode()
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
