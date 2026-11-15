#!/usr/bin/env python3
# SPDX-License-Identifier: EUPL-1.2
"""Read every public Endpoint through every surface it serves, type by type (T-6502).

    scripts/sweep-endpoints.py <domain> <env> <out.tsv> [reads-per-minute [workers]]

smoke-catalogue.py counts every dataset, but only 500 budgeted reads a run go past the count, by
dataset and for its first type. This walks all of it, anonymously and read-only: the inventory is the
DCAT feed plus the MCP hub's list_endpoints, the types are what the Endpoint serves plus what its
models name, and each distribution link of its card is opened. One run reads about 15 addresses per
Endpoint, so it stays out of the hourly smoke; run it by hand after a release.

Writes one row per environment, Endpoint, type and surface. Exits 1 on any row with status 5xx,
no-answer or a broken link; `empty` and `differs` rows are printed and need a reason, not a fix.
"""

import csv
import importlib.util
import json
import sys
import threading
import time
import urllib.parse
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("smoke_catalogue", HERE / "smoke-catalogue.py")
smoke = importlib.util.module_from_spec(spec)
sys.modules["smoke_catalogue"] = smoke
spec.loader.exec_module(smoke)

WORKERS = 6
RATE = 400  # reads a minute, the edge's 600 per address less room for the other agents (T-5903)
SLOW_RETRY = 3.0  # a 5xx or a cut read is read once more after this many seconds
LIMITED_WAIT = 20.0
LIMITED_TRIES = 6
JSON_ACCEPT = {"Accept": "application/json", "User-Agent": "jc-sweep-endpoints"}
MCP_HEADERS = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream",
               "User-Agent": "jc-sweep-endpoints"}
FAILING = {"5xx", "no-answer", "broken"}
COLUMNS = ["env", "endpoint", "type", "surface", "status", "rows", "note"]
# What `link` skips: the addresses the typed checks read themselves, and the WebSocket.
READ_BY_TYPE = ("ngsi-ld/v1/", "odata/v4/", "ogc/features/", "file.csv", "ws", "mcp")


class Pace:
    """One read every `gap` seconds across the threads."""

    def __init__(self, rate: int, clock=time.monotonic, sleep=time.sleep):
        self.gap, self.clock, self.sleep = 60 / rate, clock, sleep
        self.lock = threading.Lock()
        self.next = 0.0
        self.reads = 0

    def wait(self):
        with self.lock:
            now = self.clock()
            at = max(now, self.next)
            self.next = at + self.gap
            self.reads += 1
        if at > now:
            self.sleep(at - now)


class Reader:
    def __init__(self, pace: Pace, send=smoke.send, sleep=time.sleep, ws=smoke.open_socket):
        self.pace, self.send, self.sleep, self.ws = pace, send, sleep, ws

    def read(self, url: str, method="GET", headers=None, body=None, limit=smoke.MAX_PAGE):
        """The answer and what happened on the way: a 429 waits, a 5xx or a cut read gets one more try."""
        retried = ""
        for _ in range(LIMITED_TRIES):
            self.pace.wait()
            answer = self.send(method, url, headers or JSON_ACCEPT, body, limit)
            # A 503 with Retry-After is the gateway's own share of slow reads, the sweep's load and no fault.
            if answer.status == 429 or (answer.status == 503 and answer.retry_after):
                self.sleep(float(answer.retry_after) if answer.retry_after.isdigit() else LIMITED_WAIT)
                continue
            if (answer.status >= 500 or answer.status == 0) and not retried:
                retried = f"first read {answer.why()}"
                self.sleep(SLOW_RETRY)
                continue
            return answer, retried
        return answer, retried


def status_of(answer) -> str:
    if answer.status == 0:
        return "no-answer"
    if answer.status >= 500:
        return "5xx"
    return "ok" if answer.status == 200 else str(answer.status)


def inventory(reader: Reader, domain: str) -> dict:
    """{slug: where it was found}: the DCAT feed and every page of the hub's list_endpoints."""
    found = {}
    answer, _ = reader.read(f"https://api.{domain}/catalog.jsonld", limit=smoke.MAX_INVENTORY)
    for entry in (answer.json() or {}).get("dcat:dataset", []):
        if smoke.endpoint_slug(entry.get("@id", "")):
            found[smoke.endpoint_slug(entry["@id"])] = "feed"
    cursor = None
    while True:
        arguments = {"cursor": cursor} if cursor else {}
        call = {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": "list_endpoints", "arguments": arguments}}
        answer, _ = reader.read(f"https://api.{domain}/api/mcp", "POST", MCP_HEADERS, json.dumps(call).encode())
        page = ((answer.json() or {}).get("result") or {}).get("structuredContent") or {}
        for entry in page.get("endpoints", []):
            found[entry["slug"]] = "feed+mcp" if entry["slug"] in found else "mcp"
        cursor = page.get("nextCursor")
        if not cursor:
            return found


def card_links(card: dict, base: str) -> list:
    """The distribution addresses of an Endpoint's card, relative to its base."""
    links = [(d or {}).get("dcat:accessURL") or "" for d in card.get("dcat:distribution", [])]
    return sorted({link[len(base):] for link in links if link.startswith(base)})


class Sweep:
    def __init__(self, reader: Reader, env: str, domain: str, emit):
        self.reader, self.env, self.domain, self.emit = reader, env, domain, emit

    def row(self, slug, typ, surface, status, rows="", note=""):
        self.emit(dict(zip(COLUMNS, (self.env, slug, typ, surface, status, rows, note))))

    def plain(self, slug, typ, surface, answer, note="", rows=""):
        self.row(slug, typ, surface, status_of(answer), rows, " ".join(filter(None, [note, self.fault(answer)])))

    @staticmethod
    def fault(answer) -> str:
        if answer.status == 200:
            return ""
        title = (answer.json() or {}) if answer.body[:1] == b"{" else {}
        return " ".join(filter(None, [answer.why(), str(title.get("detail") or title.get("title") or "")]))[:160]

    def endpoint(self, slug: str):
        base = f"https://api.{self.domain}/api/endpoint/{slug}/"
        read = self.reader.read
        answer, retried = read(base[:-1], limit=smoke.MAX_INVENTORY)
        self.plain(slug, "-", "card", answer, retried)
        card = (answer.json() or {}) if answer.status == 200 else {}
        links = card_links(card, base)
        answer, retried = read(base + "ngsi-ld/v1/types")
        self.plain(slug, "-", "types", answer, retried)
        served = (answer.json() or {}).get("typeList") if answer.status == 200 else None
        served = [t for t in served if isinstance(t, str)] if isinstance(served, list) else []
        answer, retried = read(base + "schema/index.json")
        self.plain(slug, "-", "schema-index", answer, retried)
        models = (answer.json() or {}).get("models") if answer.status == 200 else None
        named = [t for m in models or [] if isinstance(m, dict) for t in m.get("types") or [] if t != "Any"]
        for link in links:
            if not link.startswith(READ_BY_TYPE):
                answer, retried = read(base + link, limit=1 << 16)
                # A card's own address answering 404 is the broken link a reader would follow.
                status = "broken" if answer.status == 404 else status_of(answer)
                self.row(slug, "-", "link", status, "", " ".join(filter(None, [link, retried, self.fault(answer)])))
        answer, retried = read(base + "mcp", "POST", MCP_HEADERS, smoke.MCP_LIST)
        tools = smoke.mcp_tools(answer) if answer.status == 200 else 0
        self.row(slug, "-", "mcp-tools", status_of(answer) if tools or answer.status != 200 else "empty",
                 tools, " ".join(filter(None, [retried, self.fault(answer)])))
        sets = self.names(base + "odata/v4", "value", "name") if any(x.startswith("odata") for x in links) else None
        collections = (self.names(base + "ogc/features/collections", "collections", "id")
                       if any(x.startswith("ogc") for x in links) else None)
        rows = {t: self.type_row(slug, base, t, sets, collections, links, t in served)
                for t in dict.fromkeys(served + named)}
        if not any(rows.values()):
            self.row(slug, "-", "endpoint", "empty", 0, f"no rows in {', '.join(rows) or 'any type'}")

    def names(self, url: str, member: str, key: str) -> set | None:
        answer, _ = self.reader.read(url)
        body = answer.json() if answer.status == 200 else None
        entries = body.get(member) if isinstance(body, dict) else None
        if not isinstance(entries, list):
            return None
        return {e.get(key) for e in entries if isinstance(e, dict)}

    def type_row(self, slug, base, typ, sets, collections, links, served) -> int:
        read = self.reader.read
        query = urllib.parse.urlencode({"type": typ, "count": "true", "limit": 1})
        answer, retried = read(base + "ngsi-ld/v1/entities?" + query)
        total = int(answer.results) if answer.results.isdigit() else 0
        note = "" if served else "model only, not in /types"
        status = status_of(answer)
        if status == "ok" and not total:
            status = "empty" if served else "ok"
        self.row(slug, typ, "ngsi-ld", status, total if answer.status == 200 else "",
                 " ".join(filter(None, [note, retried, self.fault(answer)])))
        if answer.status != 200 or not total:
            return total
        self.odata(slug, base, typ, sets, total)
        self.ogc(slug, base, typ, collections, total)
        if "file.csv" in links:
            answer, retried = read(base + "file.csv?" + urllib.parse.urlencode({"type": typ, "limit": 1}))
            lines = [x for x in answer.body.decode(errors="replace").splitlines() if x.strip()]
            status = status_of(answer)
            self.row(slug, typ, "csv", "empty" if status == "ok" and len(lines) < 2 else status, max(len(lines) - 1, 0),
                     " ".join(filter(None, [retried, self.fault(answer)])))
        if "ws" in links:
            self.reader.pace.wait()
            upgrade, subscribed, error = self.reader.ws("wss" + (base + "ws")[5:], typ)
            good = upgrade == 101 and subscribed == 201
            self.row(slug, typ, "websocket", "ok" if good else ("no-answer" if not upgrade else "5xx" if upgrade >= 500
                                                               else str(upgrade)),
                     "", "" if good else f"upgrade {upgrade}, subscribe {subscribed or '-'} {error}".strip())
        self.mcp_read(slug, base, typ)
        return total

    def odata(self, slug, base, typ, sets, total):
        if sets is None:
            self.row(slug, typ, "odata", "skip", "", "no OData distribution, or no service document")
            return
        if typ not in sets:
            self.row(slug, typ, "odata", "skip", "", "not in the service document")
            return
        query = urllib.parse.urlencode({"$top": 1, "$count": "true"})
        answer, retried = self.reader.read(base + f"odata/v4/{typ}?" + query)
        count = (answer.json() or {}).get("@odata.count") if answer.status == 200 else None
        self.compare(slug, typ, "odata", answer, count, total, retried)

    def ogc(self, slug, base, typ, collections, total):
        if collections is None:
            self.row(slug, typ, "ogc", "skip", "", "no OGC distribution, or no collections document")
            return
        if typ not in collections:
            self.row(slug, typ, "ogc", "skip", "", "not a collection")
            return
        answer, retried = self.reader.read(base + f"ogc/features/collections/{typ}/items?limit=1")
        body = (answer.json() or {}) if answer.status == 200 else {}
        count = body.get("numberMatched")
        if not isinstance(count, int) and body.get("features"):
            count = total  # the surface names no total: a feature came back
        self.compare(slug, typ, "ogc", answer, count, total, retried)

    def compare(self, slug, typ, surface, answer, count, total, retried):
        status = status_of(answer)
        note = " ".join(filter(None, [retried, self.fault(answer)]))
        if status == "ok" and not count:
            status = "empty"
        elif status == "ok" and count != total:
            status, note = "differs", f"NGSI-LD counts {total} {note}".strip()
        self.row(slug, typ, surface, status, count if isinstance(count, int) else "", note)

    def mcp_read(self, slug, base, typ):
        call = {"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": {"name": "query_entities", "arguments": {"type": [typ], "limit": 1}}}
        answer, retried = self.reader.read(base + "mcp", "POST", MCP_HEADERS, json.dumps(call).encode())
        result = (answer.json() or {}).get("result") if answer.status == 200 else None
        entities = ((result or {}).get("structuredContent") or {}).get("entities")
        status = status_of(answer)
        if status == "ok" and ((result or {}).get("isError") or not entities):
            status = "empty"
        note = "" if status == "ok" else str((result or {}).get("content") or self.fault(answer))[:160]
        self.row(slug, typ, "mcp-read", status, len(entities or []), " ".join(filter(None, [retried, note])))


def run(domain: str, env: str, out: Path, rate: int = RATE, reader: Reader | None = None, workers: int = WORKERS) -> int:
    reader = reader or Reader(Pace(rate))
    slugs = inventory(reader, domain)
    if not slugs:
        print(f"  FAIL  sweep {env}: the inventory is empty")
        return 1
    tally = {}
    lock = threading.Lock()
    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, COLUMNS, delimiter="\t", lineterminator="\n")
        writer.writeheader()

        def emit(row):
            with lock:
                writer.writerow(row)
                handle.flush()
                tally[row["status"]] = tally.get(row["status"], 0) + 1
                if row["status"] in FAILING or row["status"] in ("empty", "differs"):
                    print(f"  {row['status']:9} {env} {row['endpoint']} {row['type']} {row['surface']}: {row['note']}",
                          flush=True)

        sweep = Sweep(reader, env, domain, emit)
        for slug, source in slugs.items():
            sweep.row(slug, "-", "inventory", "ok", "", source)

        def safe(slug):
            try:
                sweep.endpoint(slug)
            except (AttributeError, KeyError, OSError, TypeError, ValueError) as error:  # one odd answer ends one Endpoint
                sweep.row(slug, "-", "sweep", "broken", "", f"{type(error).__name__}: {error}"[:160])

        with ThreadPoolExecutor(workers) as pool:
            list(pool.map(safe, sorted(slugs)))
    bad = sum(n for status, n in tally.items() if status in FAILING)
    print(f"  {'FAIL' if bad else 'ok  '}  sweep {env}: {len(slugs)} Endpoints, {reader.pace.reads} reads, {tally}")
    return 1 if bad else 0


def main(argv: list) -> int:
    if len(argv) not in (4, 5, 6):
        print(__doc__.strip().splitlines()[2].strip(), file=sys.stderr)
        return 2
    rate, workers = (*map(int, argv[4:]), RATE, WORKERS)[:2] if len(argv) == 6 else (
        int(argv[4]) if len(argv) == 5 else RATE, WORKERS)
    return run(argv[1], argv[2], Path(argv[3]), rate, workers=workers)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
