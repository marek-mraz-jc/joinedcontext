#!/usr/bin/env python3
"""Prod's MCP tool names, fetched once by hand for the `names` suite (which never contacts prod).

   python3 bins/jc/smoke/names/fetch_prod_tools.py --export <prod export> --out <prod-tools.json> \
       --gateway-repo <joinedcontext-platform clone> --broker-repo <Antares clone> \
       [--deployment <joinedcontext-deployment clone> --ref origin/release] \
       [--gateway-commit <sha>] [--broker-commit <sha>] [--base https://api.smartcity.joinedcontext.com]

Two parts:
- `public`: anonymous `initialize` + `tools/list` (every page) on the hub /api/mcp and on each
  McpServer of the export, /api/mcp/{project}/{name}. No credentials, at most one request a
  second. Anonymous tools/list holds read tools only (the gateway trims it, T-4168), so these
  rows are "public reads only".
- `source`: the write and admin tool names from prod's pinned source, read with `git show` at the
  commits prod runs (never a live call): context-gateway src/mcp/tools.rs (TOOLS,
  ANONYMOUS_TOOLS) at the gateway commit of deployment `release`
  components/context-gateway/images.yaml, and antares-mcp src/tools.rs (TOOLS) at the broker
  commit that images.yaml names for its tag (`Antares dev <sha> (<tag>)`)."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))

import sources
from common import Http
from prodnames import mcp_servers

PROTOCOL = "2025-06-18"
ACCEPT = "application/json, text/event-stream"


def git_show(repo: str, commit: str, path: str) -> str:
    return subprocess.run(["git", "-C", repo, "show", f"{commit}:{path}"], check=True,
                          capture_output=True, text=True).stdout


def pinned(deployment: str, ref: str) -> tuple[str | None, str | None]:
    """(gateway commit, broker commit) deployment `ref` runs."""
    gw = git_show(deployment, ref, "components/context-gateway/images.yaml")
    br = git_show(deployment, ref, "components/context-broker/images.yaml")
    g = re.search(r"^\s+gateway:\n(?:\s+.*\n)*?\s+commit:\s*'?([0-9a-f]{7,40})", gw, re.MULTILINE)
    # The broker is pinned by tag; images.yaml names the commit of that tag in a comment.
    tag = re.search(r"^\s+broker:\n(?:\s+.*\n)*?\s+tag:\s*'?([\w.-]+)", br, re.MULTILINE)
    b = tag and re.search(rf"Antares dev ([0-9a-f]{{7,40}}) \({re.escape(tag.group(1))}[,)]", br)
    return (g.group(1) if g else None), (b.group(1) if b else None)


def parse(raw: bytes):
    text = raw.decode(errors="replace").strip()
    if text[:1] in "{[":
        return json.loads(text)
    last = None
    for line in text.splitlines():
        if line.startswith("data:"):
            last = json.loads(line[5:].strip())
    return last


class Paced:
    """At most one request per `gap` seconds."""

    def __init__(self, http: Http, gap: float):
        self.http, self.gap, self.last = http, gap, 0.0

    def post(self, path: str, msg: dict):
        wait = self.last + self.gap - time.monotonic()
        if wait > 0:
            time.sleep(wait)
        self.last = time.monotonic()
        return self.http.call("POST", path, headers={"Accept": ACCEPT, "MCP-Protocol-Version": PROTOCOL},
                              body=msg)


def tools_of(p: Paced, path: str) -> dict:
    st, _, raw = p.post(path, {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": PROTOCOL, "capabilities": {},
        "clientInfo": {"name": "jc-smoke-names-fetch", "version": "1"}}})
    if st != 200:
        return {"status": st, "tools": None, "detail": raw[:200].decode(errors="replace")}
    names, cursor, n = [], None, 2
    while True:
        st, _, raw = p.post(path, {"jsonrpc": "2.0", "id": n, "method": "tools/list",
                                   "params": {"cursor": cursor} if cursor else {}})
        msg = parse(raw) if st == 200 else None
        result = (msg or {}).get("result") if isinstance(msg, dict) else None
        if not isinstance(result, dict):
            return {"status": st, "tools": None, "detail": raw[:200].decode(errors="replace")}
        names += [t["name"] for t in result.get("tools") or [] if isinstance(t, dict) and t.get("name")]
        cursor, n = result.get("nextCursor"), n + 1
        if not cursor or n > 20:
            return {"status": 200, "tools": sorted(set(names))}


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base", default="https://api.smartcity.joinedcontext.com")
    ap.add_argument("--export", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--gateway-repo", required=True)
    ap.add_argument("--broker-repo", required=True)
    ap.add_argument("--deployment", help="deployment clone: the pinned commits come from it")
    ap.add_argument("--ref", default="origin/release")
    ap.add_argument("--gateway-commit")
    ap.add_argument("--broker-commit")
    ap.add_argument("--gap", type=float, default=1.0, help="seconds between two requests (>= 1)")
    a = ap.parse_args(argv)
    if a.gap < 1:
        ap.error("--gap below 1 s: prod is read at most once a second")
    gw, br = a.gateway_commit, a.broker_commit
    if a.deployment and not (gw and br):
        pg, pb = pinned(a.deployment, a.ref)
        gw, br = gw or pg, br or pb
    if not (gw and br):
        ap.error("no gateway or broker commit: pass --deployment or --gateway-commit/--broker-commit")
    g_text = git_show(a.gateway_repo, gw, sources.GATEWAY_TOOLS)
    b_text = git_show(a.broker_repo, br, sources.BROKER_TOOLS)

    p = Paced(Http(a.base, timeout=60), a.gap)
    public = {"/api/mcp": tools_of(p, "/api/mcp")}
    for s in mcp_servers(Path(a.export) / "gitea" / "files"):
        public[s["path"]] = tools_of(p, s["path"])
        print(s["path"], public[s["path"]].get("status"), len(public[s["path"]].get("tools") or []), flush=True)
    doc = {"fetchedAt": dt.datetime.now(dt.UTC).strftime("%Y-%m-%dT%H:%M:%SZ"), "base": a.base,
           "public": public,
           "source": {"gateway": {"repo": "joinedcontext-platform", "commit": gw, "file": sources.GATEWAY_TOOLS},
                      "broker": {"repo": "Antares", "commit": br, "file": sources.BROKER_TOOLS},
                      "pinnedBy": f"joinedcontext-deployment {a.ref}" if a.deployment else "arguments",
                      "tools": sources.source_tools(g_text, b_text)}}
    Path(a.out).parent.mkdir(parents=True, exist_ok=True)
    Path(a.out).write_text(json.dumps(doc, indent=1) + "\n")
    print(f"wrote {a.out}: {len(public)} servers")
    return 0 if all(v.get("tools") is not None for v in public.values()) else 1


if __name__ == "__main__":
    sys.exit(main())
