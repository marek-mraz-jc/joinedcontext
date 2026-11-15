#!/usr/bin/env python3
# SPDX-License-Identifier: EUPL-1.2
"""Check that rows of every public Endpoint x type link the CKAN dataset of their space and type (T-4802).

    scripts/smoke-source-links.py <domain> <env> [out-dir]

Reads only public data, anonymously: the gateway's DCAT feed, the hub's named MCP servers
(`list_endpoints`) for the public Endpoints and CKAN's package_search, paced as smoke-catalogue.py;
then each Endpoint's types and its first entity of each type another Endpoint serves too, 6 at once
within 10 minutes as smoke-catalogue.py's count pass (T-5903).
Two Endpoints serve one (space, type) when their first entity of the type is the same URN.

Per (Endpoint, type) it compares the dataset the assistant's resolver finds, the rule of
joinedcontext-platform crates/assistant/src/lib.rs (`dataset_pattern`, `dataset_page`,
`endpoint_page`) over the text the assistant indexes per dataset (crates/assistant/src/ckan.rs),
with the datasets that publish that space and type: the Endpoint's own dataset when it serves the
type, else any dataset of another Endpoint over the same space and type. ko: a dataset exists and
the resolver finds none or another one, or none exists and the resolver links one.

Writes `<out-dir>/smoke-source-links-<env>-<time>.tsv`, and `.json` with the pages and type lists
the assistant's fixture test reads; exits 1 on any ko.
"""

import csv
import importlib.util
import json
import re
import sys
import threading
import urllib.parse
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from datetime import UTC, datetime
from pathlib import Path

_spec = importlib.util.spec_from_file_location("smoke_catalogue", Path(__file__).with_name("smoke-catalogue.py"))
smoke = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(smoke)

# The assistant's address pattern (ckan.rs ADDRESS) and plain names (lib.rs is_slug, dataset_pattern).
ADDRESS = re.compile(r"""https?://[^\s'"<>()]+/api/endpoint/[^\s'"<>()]+""")
TYPE_NAME = re.compile(r"^[A-Za-z][A-Za-z0-9_]{0,99}$")
SLUG = re.compile(r"^[a-z0-9-]{1,63}$")
COLUMNS = ["endpoint", "type", "resolved", "expected", "verdict"]
# ponytail: the named MCP servers the seeds declare (kind McpServer), each listed up to its 100-row
# page (the tool's maximum); a server a deployment lacks
# answers 404 and adds nothing. Add a name when a project brings a new one.
SERVERS = ["bbsk/otvorene-data", "bbsk/kraj-a-mesto", "banskabystrica/otvorene-data",
           "zilina/otvorene-data", "praha/otvorene-data", "helsinki/otvorene-data"]
LIST_ENDPOINTS = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
    "name": "list_endpoints", "arguments": {"limit": 100}}}).encode()


def page_text(package: dict) -> str:
    """The addresses the assistant's CKAN page of `package` carries (ckan.rs `dataset_html`)."""
    parts = []
    for resource in package.get("resources") or []:
        url = resource.get("url") or ""
        if "/api/endpoint/" not in url:
            continue
        parts.append(url)
        for found in ADDRESS.findall(resource.get("description") or ""):
            parts.append(found.rstrip(".,;:"))
    return "\n".join(parts)


def dataset_endpoint(package: dict) -> str:
    for extra in package.get("extras") or []:
        if extra.get("key") == "endpoint":
            return smoke.endpoint_slug(extra.get("value") or "")
    return ""


def resolve(pages: dict, slugs: list, kind: str, own: str) -> str:
    """The page the resolver (`rows_dataset_projects`) cites for rows of `kind` read from `own`: a
    dataset page naming the type under `own`, else under another of `slugs`, else the one dataset
    page naming `own`; "" for none."""
    for under in ([own], [s for s in slugs if s != own]):
        pattern = typed(under, kind)
        found = sorted(url for url, text in pages.items() if pattern and pattern.search(text))
        if found:
            return found[-1]
    naming = re.compile("/api/endpoint/" + re.escape(own) + "([^a-z0-9-]|$)")
    named = [url for url, text in pages.items() if naming.search(text)]
    return named[0] if len(named) == 1 else ""


def kept(text: str) -> str:
    """The lines of a page the resolver reads: the typed addresses and the NGSI-LD root."""
    lines = dict.fromkeys(line for line in text.splitlines()
                          if "entities?type=" in line or "/odata/v4/" in line or line.endswith("/ngsi-ld/v1/"))
    return "\n".join(lines)


def typed(slugs: list, kind: str):
    """The assistant's `dataset_pattern`: a page publishing `kind` from one of `slugs`."""
    if not (TYPE_NAME.match(kind) and slugs and all(SLUG.match(s) for s in slugs)):
        return None
    return re.compile("/api/endpoint/(" + "|".join(slugs) + r")/(ngsi-ld/v1/entities\?type=|odata/v4/)"
                      + kind + "([^A-Za-z0-9_]|$)")


def publishing(pages: dict, owned: dict, slug: str, kind: str) -> list:
    """The datasets of the Endpoint `slug` that publish `kind`: its one dataset, or of several
    (the harvested datasets of one shared Endpoint) those whose page names the type."""
    mine = owned.get(slug, [])
    if len(mine) == 1:
        return mine
    pattern = typed([slug], kind)
    return [url for url in mine if pattern and pattern.search(pages.get(url, ""))]


def expected(pages: dict, owned: dict, served: dict, first: dict, slug: str, kind: str) -> list:
    """The datasets that publish the (space, type) of `kind` read from `slug`: its own Endpoint's
    first, else those of the Endpoints whose first entity of the type is the same."""
    own = publishing(pages, owned, slug, kind)
    if own:
        return sorted(own)
    same = first.get((slug, kind))
    return sorted(url for other in owned if other != slug and kind in served.get(other, [])
                  and same and first.get((other, kind)) == same
                  for url in publishing(pages, owned, other, kind))


def hub_endpoints(client, domain: str) -> dict:
    """The public Endpoints the hub's named servers lend, by slug, with the types each lists:
    the doors over a space that no catalogue record names are among them (Registre kraja)."""
    found = {}
    for server in SERVERS:
        answer = client.fetch(f"https://api.{domain}/api/mcp/{server}", method="POST", body=LIST_ENDPOINTS,
                              headers={"Content-Type": "application/json",
                                       "Accept": "application/json, text/event-stream"})
        body = answer.json() if answer.status == 200 else None
        content = ((body or {}).get("result") or {}).get("content") or []
        text = content[0].get("text", "") if content and isinstance(content[0], dict) else ""
        lines = [line.split(" | ") for line in text.splitlines()]
        if not lines or lines[0][:5] != ["slug", "title", "space", "access", "types"]:
            continue
        for cells in lines[1:]:
            if len(cells) >= 5 and SLUG.match(cells[0]) and cells[3] == "public":
                found[cells[0]] = [t for t in cells[4].split(",") if TYPE_NAME.match(t)]
    return found


class Reads:
    """GETs in parallel through smoke-catalogue's count pass: a lasting 429 or its deadline ends the run."""

    def __init__(self, counter):
        self.counter, self.stop, self.why = counter, threading.Event(), ""

    def halt(self, why: str):
        if not self.stop.is_set():
            self.why = why
            self.stop.set()
        raise smoke.OutOfBudget(self.why)

    def one(self, url: str):
        if self.stop.is_set():
            raise smoke.OutOfBudget(self.why)
        if self.counter._clock() - self.counter.start > smoke.COUNT_DEADLINE:
            self.halt(f"the {smoke.COUNT_DEADLINE:.0f} s deadline passed")
        answer = self.counter.get(url)
        if answer.status == 429:
            self.halt(f"429 after {smoke.RETRIES} retries on {url}")
        return answer

    def all(self, urls: dict) -> dict:
        with ThreadPoolExecutor(smoke.COUNT_WORKERS) as pool:
            futures = {key: pool.submit(self.one, url) for key, url in urls.items()}
            try:
                return {key: future.result() for key, future in futures.items()}
            except smoke.OutOfBudget:
                self.stop.set()
                raise


def sweep(client, domain: str) -> tuple[list, dict]:
    packages, start = [], 0
    while True:
        query = urllib.parse.urlencode({"rows": 1000, "start": start})
        answer = client.fetch(f"https://opendata.{domain}/api/3/action/package_search?{query}",
                              limit=smoke.MAX_INVENTORY)
        result = (answer.json() or {}).get("result") if answer.status == 200 else None
        if not isinstance(result, dict):
            raise SystemExit(f"  FAIL  source links: package_search answered {answer.why()}")
        packages += result.get("results", [])
        start += 1000
        if start >= result.get("count", 0) or not result.get("results"):
            break
    answer = client.fetch(f"https://api.{domain}/catalog.jsonld", limit=smoke.MAX_INVENTORY)
    feed = answer.json() if answer.status == 200 else None
    if not isinstance(feed, dict):
        raise SystemExit(f"  FAIL  source links: catalog.jsonld answered {answer.why()}")
    lent = hub_endpoints(client, domain)
    public = sorted(({smoke.endpoint_slug(d.get("@id", "")) for d in feed.get("dcat:dataset", [])} | set(lent))
                    - {""})
    base = f"https://opendata.{domain}/dataset/"
    pages = {base + p["name"]: page_text(p) for p in packages if p.get("name")}
    owned = {}
    for package in packages:
        slug = dataset_endpoint(package)
        if slug and package.get("name"):
            owned.setdefault(slug, []).append(base + package["name"])
    # Every Endpoint's types and first entities: ~1400 limit=1 reads on dev, 6 at once as smoke-catalogue's
    # count pass reads (T-5768); paced at 50 a minute they outlasted the run's 500 requests (T-5903).
    reads = Reads(smoke.Counts(domain, client._send, client._clock, client._sleep))

    def gateway(slug):
        return f"https://api.{domain}/api/endpoint/{slug}/ngsi-ld/v1"

    try:
        served = dict.fromkeys(public)
        for slug, answer in reads.all({s: gateway(s) + "/types" for s in public if s not in lent}).items():
            listed = (answer.json() or {}).get("typeList") if answer.status == 200 else None
            served[slug] = [t for t in listed or [] if isinstance(t, str) and TYPE_NAME.match(t)]
        served.update({s: lent[s] for s in public if s in lent})
        # The first entity only tells Endpoints over one type apart: a type one Endpoint alone serves
        # has no sibling and no other dataset whatever it is, so it costs no read.
        shared = {kind for kind, n in Counter(k for kinds in served.values() for k in set(kinds)).items() if n > 1}
        first = {}
        for key, answer in reads.all({(s, k): gateway(s) + "/entities?" + urllib.parse.urlencode(
                {"type": k, "limit": 1, "pick": "id"}) for s in public for k in served[s] if k in shared}).items():
            body = answer.json() if answer.status == 200 else None
            if isinstance(body, list) and body and isinstance(body[0], dict):
                first[key] = body[0].get("id")
    finally:
        client.count += reads.counter.count
    rows = []
    for slug in public:
        for kind in served[slug]:
            same = first.get((slug, kind))
            siblings = sorted({slug} | {o for o in public if same and first.get((o, kind)) == same})
            want = expected(pages, owned, served, first, slug, kind)
            got = resolve(pages, siblings, kind, slug)
            good = got in want if want else got == ""
            rows.append({"endpoint": slug, "type": kind, "resolved": got or "-",
                         "expected": ",".join(want) or "-", "verdict": "ok" if good else "ko",
                         "siblings": siblings})
    fixture = {"pages": [{"url": url, "text": kept(text)} for url, text in sorted(pages.items())],
               "owners": owned, "served": served,
               "rows": [{k: r[k] for k in ("endpoint", "type", "siblings", "expected")} for r in rows]}
    return rows, fixture


def run(domain: str, env: str, out: Path, client=None, now: str | None = None) -> int:
    client = client or smoke.Client()
    now = now or datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
    try:
        rows, fixture = sweep(client, domain)
    except smoke.OutOfBudget as stop:
        print(f"  FAIL  source links: the run's budget ended after {client.count} requests"
              + (f": {stop}" if str(stop) else ""))
        return 1
    out.mkdir(parents=True, exist_ok=True)
    path = out / f"smoke-source-links-{env}-{now}.tsv"
    with path.open("w", newline="") as handle:
        writer = csv.DictWriter(handle, COLUMNS, delimiter="\t", extrasaction="ignore", lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)
    path.with_suffix(".json").write_text(json.dumps(fixture, indent=1, sort_keys=True) + "\n")
    bad = [r for r in rows if r["verdict"] == "ko"]
    for row in bad:
        print(f"  FAIL  source link {row['endpoint']} {row['type']}: resolved {row['resolved']}, "
              f"expected {row['expected']}")
    alone = sum(r["siblings"] == [r["endpoint"]] for r in rows)
    print(f"  {'FAIL' if bad else 'ok  '}  source links: {len(rows)} Endpoint x type, {len(bad)} ko, "
          f"{alone} with no other Endpoint over their space and type; "
          f"{client.count} requests; {path}")
    return 1 if bad else 0


def main(argv: list[str]) -> int:
    if len(argv) not in (3, 4):
        print(__doc__.strip().splitlines()[2].strip(), file=sys.stderr)
        return 2
    return run(argv[1], argv[2], Path(argv[3] if len(argv) == 4 else smoke.DEFAULT_OUT))


if __name__ == "__main__":
    sys.exit(main(sys.argv))
