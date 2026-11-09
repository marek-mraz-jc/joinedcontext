"""Smoke suite `assistant`: the exported AssistantDeployments on jc's assistant module, with a
scripted model on loopback, and production's widget as the reference.

   bins/jc/smoke/run assistant --base-url <url> --export <bundle> --out <dir>
       [--model-stub-port <port>] [--log-file <jc.log>] [--prod-widget]

1. prod: with --prod-widget one anonymous GET of production's widget page
   (https://assistant.smartcity.joinedcontext.com/d/data/widget, 200). Nothing else of prod is
   asked: a chat there spends prod's model key and writes a transcript.
2. pages, per deployment: an anonymous channel (public, ckan) serves /d/{publicId}/widget,
   /d/{publicId}/embed.js and /api/v1/d/{publicId}/config; an internal one answers 404 on each;
   the bundle /d/widget.js and /d/widget.css is served; an unknown publicId 404s (the control).
3. chat, per anonymous deployment, when jc's model proxy calls the stub (jc started with
   JC_MODEL_BASE=http://127.0.0.1:<port> and a dummy JCBIN_MODEL_KEY_FILE; the suite serves the
   stub on --model-stub-port while it runs): the visitor flow (challenge, SHA-256 proof, token,
   POST .../chat) of one public question the router sends to a granted Endpoint answers with a
   data table of the routed type and the stub's answer; the same question routed to a closed
   Endpoint of the deployment's project reads nothing of it; a chat from a foreign Origin is 403.

The module is off without the embedding model and ONNX Runtime (JC_ASSISTANT_MODEL_DIR,
ORT_DYLIB_PATH: scripts/ci/e5-small.sh and the onnxruntime library), [assistant.database] or the
model key file; then parts 2 and 3 are named skips for the model and ONNX, and failures with the
reason jc logged for anything else (--log-file reads the `assistant: off` line)."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import secrets
import sys
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
SMOKE = HERE.parent
for p in (HERE, SMOKE, SMOKE / "endpoints"):
    sys.path.insert(0, str(p))

import export as ex
import modelstub
from common import Http, Results, as_json, loopback_only, short

PROD_WIDGET = "https://assistant.smartcity.joinedcontext.com/d/data/widget"
ANONYMOUS = {"public", "ckan"}
FOREIGN_ORIGIN = "https://smoke-assistant.invalid"
# Why the module is off, when it is one of the files this suite may skip for (the task allows it).
MODEL_FILES = re.compile(r"JC_ASSISTANT_MODEL_DIR|ORT_DYLIB_PATH|embedding model|ONNX Runtime")


def load_deployments(files: Path) -> list[dict]:
    out = []
    for f in sorted(files.glob("*/*/assistant/deployments/*.yaml")):
        for d in ex._load_yaml(f):
            if d.get("kind") == "AssistantDeployment":
                meta, spec = d.get("metadata") or {}, d.get("spec") or {}
                out.append({"project": meta.get("namespace") or f.parts[-4], "name": meta["name"],
                            "publicId": spec.get("publicId") or meta["name"],
                            "channel": spec.get("channel") or "public", "spec": spec})
    return out


def zero_bits(digest: bytes) -> int:
    bits = 0
    for b in digest:
        if b == 0:
            bits += 8
            continue
        return bits + 8 - b.bit_length()
    return bits


def solve(challenge: str, bits: int) -> str:
    n = 0
    while zero_bits(hashlib.sha256(f"{challenge}:{n}".encode()).digest()) < bits:
        n += 1
    return str(n)


def events(raw: bytes) -> list[tuple[str, object]]:
    """The SSE events of a chat answer as (name, JSON data)."""
    out = []
    for block in raw.decode(errors="replace").split("\n\n"):
        name = next((ln[7:] for ln in block.splitlines() if ln.startswith("event: ")), "")
        data = "\n".join(ln[6:] for ln in block.splitlines() if ln.startswith("data: "))
        if name:
            out.append((name, as_json(data.encode()) if data else None))
    return out


class Targets:
    """The Endpoints a deployment's question is routed to, from the export."""

    def __init__(self, endpoints, index):
        self.by_name = {(e.project, e.name): e for e in endpoints}
        self.endpoints, self.index = endpoints, index

    def tenant_types(self, e):
        return (self.index.get("tenants") or {}).get(e.tenant, {})

    def granted(self, d: dict):
        """(router name, Endpoint, short type) of a granted public Endpoint, or (None, why)."""
        spec, names = d["spec"], []
        for c in spec.get("connectors") or []:
            ep = str(c.get("endpoint") or "")
            project, _, name = ep.rpartition("/")
            names.append((project or d["project"], name))
        server = str(spec.get("mcpServer") or "")
        if server:
            project = server.split("/")[0]
            names += sorted((e.project, e.name) for e in self.endpoints if e.project == project and not e.closed)
        for key in names:
            e = self.by_name.get(key)
            if not e or e.closed:
                continue
            types, _ = ex.expected_types(e, e.public_grants(), self.tenant_types(e))
            if types:
                st = max(types, key=lambda t: self.tenant_types(e)[t]["count"])
                return f"{e.project}/{e.name}", e, st
        return None, "no connector or McpServer member with a public grant on exported rows"

    def closed(self, d: dict):
        """(router name, Endpoint, short type, sample id) of a closed Endpoint the visitor may not
        read, in the deployment's project, or (None, why)."""
        for e in sorted(self.endpoints, key=lambda x: x.slug):
            if e.project != d["project"] or not e.closed:
                continue
            tt = self.tenant_types(e)
            types, _ = ex.expected_types(e, e.grants, tt)
            # A type no public grant of the project opens: its rows can only come from here.
            public = {t for x in self.endpoints if x.tenant == e.tenant and not x.closed
                      for t in ex.expected_types(x, x.public_grants(), tt)[0]}
            types = {t: n for t, n in types.items() if t not in public} or types
            if types:
                st = max(types, key=lambda t: tt[t]["count"])
                return f"{e.project}/{e.name}", e, st, tt[st].get("sample") or ""
        return None, "no closed Endpoint with exported rows in the project", None, None


def told(evs) -> str:
    """What the error and tool events said, for a failing row (the cause, not only the names)."""
    said = [f"{n}: {json.dumps(x, ensure_ascii=False)[:160]}" for n, x in evs if n in ("error", "tool")]
    return "; " + "; ".join(said) if said else ""


class Suite:
    def __init__(self, a, deployments, targets: Targets):
        self.a, self.deployments, self.targets = a, deployments, targets
        self.http = Http(a.base_url, timeout=a.timeout)
        self.r = Results("assistant", a.base_url)
        self.off = None  # why the module is off, when it is

    # ------------------------------------------------------------ 1. prod

    def prod(self):
        if not self.a.prod_widget:
            self.r.skip("prod.widget", "prod", f"GET {PROD_WIDGET}", "200",
                        "no --prod-widget: production's widget is not asked (an anonymous GET; never a chat)")
            return
        try:
            with urllib.request.urlopen(urllib.request.Request(PROD_WIDGET, headers={"User-Agent": "jc-smoke"}),
                                        timeout=20) as r:
                st, body = r.status, r.read(65536)
        except urllib.error.HTTPError as e:
            st, body = e.code, b""
        except (urllib.error.URLError, OSError) as e:
            st, body = 0, str(e).encode()
        self.r.check("prod.widget", "prod", f"GET {PROD_WIDGET}", "200 HTML", f"{st} {short(body, 60)}",
                     st == 200 and b"<html" in body.lower())

    # ------------------------------------------------------------ 2. pages

    def module_state(self):
        st, _, _ = self.http.call("GET", "/d/widget.js")
        if st == 200:
            return
        why = ""
        if self.a.log_file and Path(self.a.log_file).exists():
            for line in Path(self.a.log_file).read_text(errors="replace").splitlines():
                if "assistant: off" in line:
                    m = re.search(r"why=(.*)$", line)
                    why = m.group(1).strip().strip('"') if m else line[-300:]
        self.off = why or f"GET /d/widget.js answered {st} and no --log-file says why"

    def skip_or_fail(self, id, target, call, expected):
        """A row for a check the module being off prevents: a skip for the model files alone."""
        if MODEL_FILES.search(self.off or ""):
            self.r.skip(id, target, call, expected,
                        f"assistant module off: {self.off} (fetch scripts/ci/e5-small.sh and the onnxruntime library)")
        else:
            self.r.check(id, target, call, expected, f"assistant module off: {self.off}", False,
                         "the copy lacks production's assistant")

    def pages(self):
        unknown = "smoke-unknown-" + secrets.token_hex(4)
        if self.off:
            self.skip_or_fail("bundle.widget.js", "bundle", "GET /d/widget.js", "200 JavaScript")
            for d in self.deployments:
                self.skip_or_fail(f"page.{d['publicId']}", f"{d['project']}/{d['name']}",
                                  f"GET /d/{d['publicId']}/widget", "200 or 404 by channel")
            return
        for path, kind in (("/d/widget.js", "javascript"), ("/d/widget.css", "css")):
            st, h, body = self.http.call("GET", path)
            self.r.check(f"bundle.{path[3:]}", "bundle", f"GET {path}", f"200 {kind}",
                         f"{st} {h.get('content-type', '')}", st == 200 and kind in h.get("content-type", "") and bool(body))
        st, _, body = self.http.call("GET", f"/d/{unknown}/widget")
        self.r.check("control.unknown-id", "/d/<unknown>/widget", "GET", "404", f"{st} {short(body, 60)}", st == 404)
        for d in self.deployments:
            pid, t = urllib.parse.quote(d["publicId"], safe=""), f"{d['project']}/{d['name']}"
            served = d["channel"] in ANONYMOUS
            for rid, path in (("widget", f"/d/{pid}/widget"), ("embed", f"/d/{pid}/embed.js"),
                              ("config", f"/api/v1/d/{pid}/config")):
                st, h, body = self.http.call("GET", path)
                if served:
                    ok = st == 200 and (rid != "widget" or b"<html" in body.lower())
                    want = "200"
                else:
                    ok, want = st == 404, f"404 (channel {d['channel']} is not anonymous)"
                self.r.check(f"page.{d['publicId']}.{rid}", t, f"GET {path}", want, f"{st} {short(body, 80)}", ok,
                             f"channel {d['channel']}")

    # ------------------------------------------------------------ 3. chat

    def visitor(self, pid: str):
        """A visitor token, or (None, why)."""
        st, _, raw = self.http.call("GET", f"/api/v1/d/{pid}/challenge")
        ch = as_json(raw) or {}
        if st != 200 or not ch.get("challenge"):
            return None, f"challenge {st} {short(raw, 100)}"
        nonce = solve(ch["challenge"], int(ch.get("difficulty") or 0))
        st, _, raw = self.http.call("POST", f"/api/v1/d/{pid}/token", body={"challenge": ch["challenge"], "nonce": nonce})
        tok = (as_json(raw) or {}).get("token")
        return (tok, "") if st == 200 and tok else (None, f"token {st} {short(raw, 100)}")

    def ask(self, stub, pid: str, endpoint: str, st: str, question: str, origin: str | None = None):
        stub.plan({"route": "data", "lang": "en", "queries": [question], "followups": [],
                   "data": [{"endpoint": endpoint, "type": st}]})
        tok, why = self.visitor(pid)
        if not tok:
            return 0, [], why
        headers = {"X-JC-Visitor": tok, "Accept": "text/event-stream"}
        if origin:
            headers["Origin"] = origin
        code, _, raw = self.http.call("POST", f"/api/v1/d/{pid}/chat", headers=headers, body={"message": question})
        return code, events(raw), short(raw, 200)

    def chat(self):
        anonymous = [d for d in self.deployments if d["channel"] in ANONYMOUS]
        if self.off or not self.a.model_stub_port:
            why = f"assistant module off: {self.off}" if self.off else \
                "no --model-stub-port: jc's JC_MODEL_BASE must name the suite's stub (never a real model key)"
            for d in anonymous:
                for rid in ("granted", "closed-refused", "foreign-origin"):
                    if self.off:
                        self.skip_or_fail(f"chat.{d['publicId']}.{rid}", f"{d['project']}/{d['name']}",
                                          f"POST /api/v1/d/{d['publicId']}/chat", rid)
                    else:
                        self.r.skip(f"chat.{d['publicId']}.{rid}", f"{d['project']}/{d['name']}",
                                    f"POST /api/v1/d/{d['publicId']}/chat", rid, why)
            return
        stub = modelstub.Stub(self.a.model_stub_port)
        try:
            for d in anonymous:
                self.one(stub, d)
        finally:
            stub.close()

    def one(self, stub, d):
        pid, t = urllib.parse.quote(d["publicId"], safe=""), f"{d['project']}/{d['name']}"
        call = f"POST /api/v1/d/{pid}/chat"
        name, e, st = self.targets.granted(d)
        if not name:
            self.r.skip(f"chat.{d['publicId']}.granted", t, call, "a data table and the answer", e)
        else:
            q = f"What are the current values of {st}?"
            code, evs, raw = self.ask(stub, pid, name, st, q)
            tables = [x for n, x in evs if n == "data" and isinstance(x, dict)
                      and ex.short_type(str(x.get("type", ""))) == st and x.get("rows")]
            answer = next((x.get("text") for n, x in evs if n == "answer" and isinstance(x, dict)), None)
            ok = code == 200 and bool(tables) and answer == modelstub.ANSWER
            names = sorted({n for n, _ in evs})
            self.r.check(f"chat.{d['publicId']}.granted", t, f"{call}: {q} (routed to {name})",
                         f"200, a data table of {st}, the stub's answer",
                         f"{code}; events {names}; {len(tables)} table(s); answer {'ok' if answer == modelstub.ANSWER else answer!r}"
                         + ("" if evs else f"; {raw}") + told(evs), ok, f"Endpoint {e.slug}")
        name, e, st, sample = self.targets.closed(d)
        if not name:
            self.r.skip(f"chat.{d['publicId']}.closed-refused", t, call, "nothing of a closed Endpoint read", e)
        else:
            q = f"List the {st} rows."
            code, evs, raw = self.ask(stub, pid, name, st, q)
            read = [x for n, x in evs if n == "data"]
            leaked = bool(sample) and any(sample in json.dumps(c, ensure_ascii=False) for c in stub.answer_calls())
            ok = code == 200 and not read and not leaked and any(n == "done" for n, _ in evs)
            self.r.check(f"chat.{d['publicId']}.closed-refused", t, f"{call}: {q} (routed to {name}, {e.audience})",
                         "200 and done, no data table, the sample row never reaches the model",
                         f"{code}; {len(read)} table(s); sample in a model call: {leaked}" + ("" if evs else f"; {raw}"), ok,
                         f"Endpoint {e.slug}")
        tok, why = self.visitor(pid)
        if not tok:
            self.r.check(f"chat.{d['publicId']}.foreign-origin", t, call, "403", why, False)
            return
        code, _, raw = self.http.call("POST", f"/api/v1/d/{pid}/chat", body={"message": "hello"},
                                      headers={"X-JC-Visitor": tok, "Origin": FOREIGN_ORIGIN})
        self.r.check(f"chat.{d['publicId']}.foreign-origin", t, f"{call} Origin {FOREIGN_ORIGIN}", "403",
                     f"{code} {short(raw, 80)}", code == 403)

    def run(self):
        self.prod()
        self.module_state()
        self.pages()
        self.chat()


def main(argv=None):
    p = argparse.ArgumentParser(prog="run assistant", description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--base-url", required=True)
    p.add_argument("--export", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--model-stub-port", type=int, help="the loopback port jc's JC_MODEL_BASE names")
    p.add_argument("--log-file", help="jc's log: why the module is off")
    p.add_argument("--prod-widget", action="store_true", help=f"GET {PROD_WIDGET} once (anonymous)")
    p.add_argument("--timeout", type=float, default=120)
    p.add_argument("--cache", default=str(Path.home() / ".cache" / "jc-smoke"))
    p.add_argument("--state", help="unused (accepted for the common runner)")
    p.add_argument("--admin-token-file", help="unused (accepted for the common runner)")
    p.add_argument("--person-token-file", help="unused (accepted for the common runner)")
    a = p.parse_args(argv)
    loopback_only(a.base_url)
    export = Path(a.export)
    files = export / "gitea" / "files"
    endpoints = ex.load_endpoints(files)
    index = ex.load_index(export, endpoints, Path(a.cache)) if a.model_stub_port else {}
    s = Suite(a, load_deployments(files), Targets(endpoints, index))
    s.run()
    s.r.write(Path(a.out), {"export": str(export), "deployments": len(s.deployments),
                            "module": f"off: {s.off}" if s.off else "on"})
    t = s.r.totals()
    print(f"assistant: {t['pass']} pass, {t['fail']} fail, {t['skip']} skip")
    return 1 if t["fail"] else 0


if __name__ == "__main__":
    sys.exit(main())
