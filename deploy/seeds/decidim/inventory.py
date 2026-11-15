#!/usr/bin/env python3
"""Decidim inventory for T-6315: public instances, their sizes, and today's harvest on dev and prod.

    python3 -I seeds/decidim/inventory.py [--deep-only] --out <dir>

Reads only public answers: the decidim.org installations list, each instance's robots.txt and its
anonymous GraphQL API (/api), and the public slovensko-participacia Endpoint on dev and prod.
Every GraphQL query asks ids and type names only: no author, nickname, avatar or text leaves an
instance (the author check asks `author { __typename }`, the type name, never a value). One request
per second per host, 30 s timeout, 8 MiB answer cap, a host whose robots.txt disallows /api for
every agent is skipped.
"""

import argparse
import concurrent.futures
import json
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
import urllib.robotparser

# The harvester's own User-Agent (README), so an instance's robots.txt rule for us applies here too.
UA = "joinedcontext-platform/1.0 (+https://dev.joinedcontext.com; public participation data)"
CAP = 8 * 1024 * 1024
LIST_URL = "https://decidim.org/installations/"
# Found outside the decidim.org list (web search 2026-11-01).
EXTRA = [
    {
        "title": "Spotter Rozhodování (CZ)",
        "url": "https://decidim.spotter.cz/",
        "lat": 49.8,
        "lng": 15.5,
    },
    {
        "title": "Metadecidim",
        "url": "https://meta.decidim.org/",
        "lat": 41.39,
        "lng": 2.17,
    },
]
# Deep counts beyond SK/CZ: the reference instances a Smart City buyer asks about.
DEEP_EXTRA = {"meta.decidim.org", "www.decidim.barcelona", "omastadi.hel.fi"}
ENDPOINT_SLUG = "nak2jyucmfc6k72hvcltngzerhcc3kfv"
CLUSTERS = {
    "dev": "api.dev.joinedcontext.com",
    "prod": "api.smartcity.joinedcontext.com",
}
TYPES = [
    "ParticipatoryProcess",
    "Proposal",
    "BudgetProject",
    "Result",
    "Meeting",
    "Survey",
    "Debate",
]
CONNECTIONS = {
    "Proposals": "proposals",
    "Meetings": "meetings",
    "Accountability": "results",
    "Debates": "debates",
    "Surveys": "surveys",
}
MAX_PAGES = 50  # 5,000 ids per component at most

_last: dict[str, float] = {}


def fetch(url, body=None, headers=None):
    host = urllib.parse.urlsplit(url).hostname or ""
    wait = _last.get(host, 0) + 1.0 - time.monotonic()
    if wait > 0:
        time.sleep(wait)
    _last[host] = time.monotonic()
    req = urllib.request.Request(
        url, data=body, headers={"User-Agent": UA, **(headers or {})}
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return r.status, r.read(CAP + 1)[:CAP], dict(r.headers)
    except urllib.error.HTTPError as e:
        return e.code, e.read(CAP)[:CAP], dict(e.headers or {})


def gql(base, query, variables=None):
    status, data, _ = fetch(
        base + "/api",
        json.dumps({"query": query, "variables": variables or {}}).encode(),
        {"Content-Type": "application/json", "Accept": "application/json"},
    )
    if status != 200:
        raise RuntimeError(f"HTTP {status}")
    answer = json.loads(data)
    if answer.get("data") is None:
        raise RuntimeError(
            (answer.get("errors") or [{}])[0].get("message", "no data")[:80]
        )
    return answer


def robots_allow(base):
    try:
        status, data, _ = fetch(base + "/robots.txt")
    except (OSError, ValueError):
        return True
    if status != 200:
        return True
    rp = urllib.robotparser.RobotFileParser()
    rp.parse(data.decode("utf-8", "replace").splitlines())
    return rp.can_fetch(UA, base + "/api")


SHALLOW = "{ decidim { version } participatoryProcesses { id } assemblies { id } }"
# One space at a time: an unfiltered components list of a large instance runs past the server's
# 30 s timeout (tvormespolu answers 500, T-5065).
COMPONENTS = "query($id: ID!) {{ s: {}(id: $id) {{ components {{ __typename id }} }} }}"


def count_component(base, ctype, cid):
    """Ids of one component, paged; None when the API hides it from anonymous clients."""
    if ctype == "Budgets":
        a = gql(
            base,
            "query($id: ID!) { c: component(id: $id) { ... on Budgets { budgets { id projects { id } } } } }",
            {"id": cid},
        )
        c = (a["data"] or {}).get("c")
        if c is None or c.get("budgets") is None:
            return None
        return sum(len(b.get("projects") or []) for b in c["budgets"] if b)
    conn = CONNECTIONS[ctype]
    total, after = 0, None
    for _ in range(MAX_PAGES):
        a = gql(
            base,
            "query($id: ID!, $after: String) { c: component(id: $id) { ... on "
            + ctype
            + " { items: "
            + conn
            + "(first: 100, after: $after) { pageInfo { hasNextPage endCursor } nodes { id } } } } }",
            {"id": cid, "after": after},
        )
        c = (a["data"] or {}).get("c")
        if c is None or c.get("items") is None:
            return None if total == 0 else total
        total += len([n for n in c["items"]["nodes"] or [] if n])
        if not c["items"]["pageInfo"]["hasNextPage"]:
            break
        after = c["items"]["pageInfo"]["endCursor"]
    return total


def author_exposed(base, cid):
    """True when the API names a proposal's author type to anonymous clients (value never read)."""
    try:
        a = gql(
            base,
            "query($id: ID!) { c: component(id: $id) { ... on Proposals { proposals(first: 1) "
            "{ nodes { author { __typename } } } } } }",
            {"id": cid},
        )
    except RuntimeError:
        return None
    nodes = (((a["data"] or {}).get("c") or {}).get("proposals") or {}).get(
        "nodes"
    ) or []
    nodes = [n for n in nodes if n]  # a hidden proposal answers null
    return any(n.get("author") for n in nodes) if nodes else None


def probe(inst, deep):
    base = origin(inst["url"])
    row = {
        "title": inst["title"],
        "base": base,
        "archived": "web.archive.org" in inst["url"],
    }
    try:
        if not robots_allow(base):
            return {**row, "status": "robots.txt disallows /api"}
        d = gql(base, SHALLOW)["data"]
    except (
        OSError,
        ValueError,
        RuntimeError,
    ) as e:  # dead host, no API, refused: recorded, never fatal
        return {**row, "status": f"no public API ({type(e).__name__}: {str(e)[:60]})"}
    row.update(
        status="ok",
        version=(d.get("decidim") or {}).get("version") or "hidden",
        processes=len(d.get("participatoryProcesses") or []),
        assemblies=len(d.get("assemblies") or []),
    )
    if deep:
        comps = []
        for field, key in (
            ("participatoryProcess", "participatoryProcesses"),
            ("assembly", "assemblies"),
        ):
            for space in d.get(key) or []:
                try:
                    s = (
                        gql(base, COMPONENTS.format(field), {"id": space["id"]})[
                            "data"
                        ].get("s")
                        or {}
                    )
                except RuntimeError:
                    continue
                comps += [c for c in (s.get("components") or []) if c]
        kinds = {}
        for c in comps:
            kinds[c["__typename"]] = kinds.get(c["__typename"], 0) + 1
        row["components"] = kinds
        counts = {}
        for c in comps:
            t = c["__typename"]
            if t in CONNECTIONS or t == "Budgets":
                try:
                    n = count_component(base, t, c["id"])
                except RuntimeError:
                    n = None
                key = "BudgetProject" if t == "Budgets" else t
                counts[key] = counts.get(key, 0) + (n or 0)
                counts.setdefault("hidden", [])
                if n is None:
                    counts["hidden"].append(t)
        row["counts"] = counts
        first = next((c["id"] for c in comps if c["__typename"] == "Proposals"), None)
        row["author_exposed"] = author_exposed(base, first) if first else None
    return row


def origin(url):
    """The instance's own origin; a list entry pointing at web.archive.org names the dead site."""
    url = re.sub(r"^https?://web\.archive\.org/web/\d+/", "", url)
    return re.sub(r"^(https?://[^/]+).*$", r"\1", url)


def safe(inst, deep):
    try:
        return probe(inst, deep)
    except (
        OSError,
        ValueError,
        RuntimeError,
        KeyError,
        TypeError,
        AttributeError,
    ) as e:  # an odd answer shape on one instance never ends the sweep
        return {
            "title": inst["title"],
            "base": origin(inst["url"]),
            "status": f"probe failed ({type(e).__name__}: {str(e)[:60]})",
        }


def in_sk_cz(i):
    host = urllib.parse.urlsplit(origin(i["url"])).hostname or ""
    return host.endswith((".sk", ".cz")) or (
        47.7 < i.get("lat", 0) < 51.1 and 12.0 < i.get("lng", 0) < 22.6
    )


def in_eu(i):
    return 34 < i.get("lat", 0) < 72 and -11 < i.get("lng", 0) < 35


def harvest_check():
    out = {}
    for name, host in CLUSTERS.items():
        row = {}
        for t in TYPES:
            st, _, h = fetch(
                f"https://{host}/api/endpoint/{ENDPOINT_SLUG}/ngsi-ld/v1/entities?type={t}&count=true&limit=0"
            )
            count = {k.lower(): v for k, v in h.items()}.get("ngsild-results-count")
            row[t] = int(count) if st == 200 and count is not None else f"HTTP {st}"
        st, data, _ = fetch(
            f"https://{host}/api/endpoint/{ENDPOINT_SLUG}/ngsi-ld/v1/entities?type=BudgetProject,Proposal"
            "&limit=1000&options=sysAttrs,keyValues&pick=modifiedAt"
        )
        row["lastModified"] = (
            max((e.get("modifiedAt", "") for e in json.loads(data)), default="-")
            if st == 200
            else "-"
        )
        out[name] = row
    d = gql(
        "https://tvormespolu.banskabystrica.sk",
        "{ participatoryProcesses { id } assemblies { id } }",
    )["data"]
    out["source"] = {
        "ParticipatoryProcess": len(d["participatoryProcesses"]) + len(d["assemblies"])
    }
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True, help="directory for inventory.json")
    ap.add_argument(
        "--deep-only", action="store_true", help="skip the EU-wide shallow sweep"
    )
    ap.add_argument(
        "--host",
        help="probe only this instance host (deep), e.g. tvormespolu.banskabystrica.sk",
    )
    args = ap.parse_args()
    _, page, _ = fetch(LIST_URL)
    listed = [
        json.loads(m)
        for m in re.findall(
            r'\{"id":"[^{}]*?"url":"[^{}]*?\}', page.decode("utf-8", "replace")
        )
    ]
    seen, insts = set(), []
    for i in listed + EXTRA:
        if i["url"] not in seen:
            seen.add(i["url"])
            insts.append(i)
    deep = [
        i
        for i in insts
        if in_sk_cz(i) or urllib.parse.urlsplit(origin(i["url"])).hostname in DEEP_EXTRA
    ]
    if args.host:
        deep = [
            i
            for i in insts
            if urllib.parse.urlsplit(origin(i["url"])).hostname == args.host
        ]
    shallow = (
        []
        if args.deep_only or args.host
        else [i for i in insts if in_eu(i) and i not in deep]
    )
    with concurrent.futures.ThreadPoolExecutor(8) as pool:
        rows = list(pool.map(lambda i: safe(i, True), deep)) + list(
            pool.map(lambda i: safe(i, False), shallow)
        )
    result = {"listed": len(listed), "harvest": harvest_check(), "instances": rows}
    with open(f"{args.out}/inventory.json", "w") as out:
        json.dump(result, out, indent=1, ensure_ascii=False)
    ok = [r for r in rows if r["status"] == "ok"]
    print(
        f"listed {len(listed)}, probed {len(rows)} (deep {len(deep)}), public API {len(ok)}"
    )
    print(json.dumps(result["harvest"], ensure_ascii=False))
    for r in rows:
        if r.get("counts") is not None:
            print(
                r["base"],
                r.get("version"),
                r.get("processes"),
                r.get("assemblies"),
                r["counts"],
                "author:",
                r.get("author_exposed"),
            )
    return 0


if __name__ == "__main__":
    sys.exit(main())
