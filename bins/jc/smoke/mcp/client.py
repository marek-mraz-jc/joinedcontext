"""An MCP client over streamable HTTP for the smoke suite: one JSON-RPC message per POST, the
answer read as JSON or as an SSE stream, and the tool result's data read from structuredContent
(or its text, inside the untrusted-data fence the hub writes around broker answers)."""

from __future__ import annotations

import itertools
import json
import re
import time

from common import Http, short

ACCEPT = "application/json, text/event-stream"
PROTOCOL = "2025-06-18"
_ids = itertools.count(1)

# Every NGSI-LD write the hub and an Endpoint's MCP know by name: none is offered anonymously.
WRITE_TOOLS = {
    "create_entity", "batch_create", "batch_upsert", "batch_update", "batch_delete", "delete_entity",
    "update_attributes", "append_attributes", "merge_entity", "replace_attribute", "update_attribute",
    "delete_attribute", "create_subscription", "update_subscription", "delete_subscription",
}


def parse_body(raw: bytes):
    """A JSON-RPC answer from a JSON body or the last `data:` event of an SSE body."""
    text = raw.decode(errors="replace").strip()
    if not text:
        return None
    if text[0] in "{[":
        try:
            return json.loads(text)
        except ValueError:
            return None
    last = None
    for line in text.splitlines():
        if line.startswith("data:"):
            try:
                last = json.loads(line[5:].strip())
            except ValueError:
                continue
    return last


_FENCE = re.compile(r"\{.*\}|\[.*\]", re.S)


def payload(result) -> object:
    """The data a tool result carries: structuredContent, else the first JSON value in its text."""
    if not isinstance(result, dict):
        return None
    if isinstance(result.get("structuredContent"), (dict, list)):
        return result["structuredContent"]
    for part in result.get("content") or []:
        text = part.get("text") if isinstance(part, dict) else None
        if not text:
            continue
        try:
            return json.loads(text)
        except ValueError:
            m = _FENCE.search(text)
            if m:
                try:
                    return json.loads(m.group(0))
                except ValueError:
                    pass
    return None


def text_of(result) -> str:
    if not isinstance(result, dict):
        return ""
    return " ".join(str(p.get("text", "")) for p in result.get("content") or [] if isinstance(p, dict))


def entities_of(data) -> list:
    """Entities under `entities` / `entity` / a bare list, at any of the usual nestings."""
    if isinstance(data, list):
        return [e for e in data if isinstance(e, dict)]
    if not isinstance(data, dict):
        return []
    for key in ("entities", "items", "results"):
        if isinstance(data.get(key), list):
            return [e for e in data[key] if isinstance(e, dict)]
    if isinstance(data.get("entity"), dict):
        return [data["entity"]]
    for v in data.values():
        if isinstance(v, dict) and isinstance(v.get("entities"), list):
            return [e for e in v["entities"] if isinstance(e, dict)]
    return []


def total_of(data):
    if isinstance(data, dict):
        for key in ("total", "count", "totalCount"):
            if isinstance(data.get(key), int):
                return data[key]
        for v in data.values():
            if isinstance(v, dict) and isinstance(v.get("total"), int):
                return v["total"]
    return None


class Answer:
    def __init__(self, status: int, headers: dict, raw: bytes):
        self.status, self.headers, self.raw = status, headers, raw
        self.message = parse_body(raw) if status == 200 else None
        m = self.message if isinstance(self.message, dict) else {}
        self.error = m.get("error")
        self.result = m.get("result")

    @property
    def ok(self) -> bool:
        """A tool result that is no error, or a non-tool result."""
        if self.status != 200 or self.error or self.result is None:
            return False
        return not (isinstance(self.result, dict) and self.result.get("isError"))

    @property
    def refused(self) -> bool:
        """Refused in any of MCP's channels: an HTTP refusal, a JSON-RPC error or a tool error."""
        if self.status in (401, 403, 404):
            return True
        return self.status == 200 and (bool(self.error) or (
            isinstance(self.result, dict) and bool(self.result.get("isError"))))

    @property
    def data(self):
        return payload(self.result)

    def brief(self, n: int = 200) -> str:
        if self.error:
            return f"{self.status} error {self.error.get('code')}: {str(self.error.get('message'))[:n]}"
        if isinstance(self.result, dict) and self.result.get("isError"):
            return f"{self.status} isError: {text_of(self.result)[:n]}"
        if self.status == 200 and self.result is not None:
            return f"{self.status} ok {short(json.dumps(self.result).encode(), n)}"
        return f"{self.status} {short(self.raw, n)}"


class Mcp:
    """One MCP door (`/api/mcp`, `/api/endpoint/{slug}/mcp`, `/api/v1/mcp`, `/c/{id}`)."""

    def __init__(self, http: Http, path: str, token: str | None = None, retries: int = 4):
        self.http, self.path, self.token, self.retries = http, path, token, retries

    def send(self, method: str, params: dict | None = None, notification: bool = False) -> Answer:
        msg = {"jsonrpc": "2.0", "method": method}
        if not notification:
            msg["id"] = next(_ids)
        if params is not None:
            msg["params"] = params
        headers = {"Accept": ACCEPT, "MCP-Protocol-Version": PROTOCOL}
        for attempt in range(self.retries + 1):
            st, h, raw = self.http.call("POST", self.path, token=self.token, headers=headers, body=msg)
            if st != 429 or attempt == self.retries:
                return Answer(st, h, raw)
            # The Endpoint's own bucket: wait what it names, then ask again.
            try:
                wait = float(h.get("retry-after") or h.get("ratelimit-reset") or 2)
            except ValueError:
                wait = 2
            time.sleep(min(max(wait, 1), 30))
        return Answer(st, h, raw)

    def initialize(self) -> Answer:
        return self.send("initialize", {"protocolVersion": PROTOCOL, "capabilities": {},
                                        "clientInfo": {"name": "jc-smoke-mcp", "version": "1"}})

    def tools(self, params: dict | None = None) -> tuple[Answer, list[dict]]:
        a = self.send("tools/list", params or {})
        tools = (a.result or {}).get("tools") if isinstance(a.result, dict) else None
        return a, [t for t in tools or [] if isinstance(t, dict)]

    def call(self, name: str, arguments: dict) -> Answer:
        return self.send("tools/call", {"name": name, "arguments": arguments})
