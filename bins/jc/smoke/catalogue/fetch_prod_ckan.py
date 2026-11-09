#!/usr/bin/env python3
"""Prod's open data catalogue, fetched once by hand for the `catalogue` suite (which never contacts prod).

   python3 bins/jc/smoke/catalogue/fetch_prod_ckan.py --out bins/jc/smoke/catalogue/fixtures/prod-ckan-<date>.json

Anonymous GETs on prod's CKAN only, at most one a second (parity.py throttle_prod): organization_list,
package_list, package_search (every page) and package_search per organisation (every page, the query
the suite sends), package_show of each listed name the search does not return and of each name the
search returns twice, and the DCAT feed /catalog.jsonld (every page). Kept per dataset: name,
organisation, resource count, and the distribution count of its DCAT node (found by the package id,
which is not kept). Ids, timestamps and texts are not kept: jc mints its own, so they never compare.

A name counts once. prod's search index can hold a document package_show no longer answers beside
the live one under the same name (2026-11-09: bratislava-pocet-nevybavenych-ziadosti-do-ms-podla,
an orphan of 2026-10-26 with 7 resources beside the live dataset's 15): the live one is the one
package_show answers, and `searchDuplicates` names each such name with its row count."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import sys
import time
import urllib.parse
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))

from common import Http

DCAT = "http://www.w3.org/ns/dcat#"
HYDRA = "http://www.w3.org/ns/hydra/core#"


class Paced:
    def __init__(self, base: str, gap: float):
        self.http, self.gap, self.last = Http(base, timeout=120), gap, 0.0

    def get(self, path: str, missing_ok: bool = False):
        wait = self.last + self.gap - time.monotonic()
        if wait > 0:
            time.sleep(wait)
        self.last = time.monotonic()
        st, _, raw = self.http.call("GET", path)
        if st == 404 and missing_ok:
            return None
        if st != 200:
            raise SystemExit(f"GET {path}: {st}")
        return json.loads(raw)


def action(p: Paced, name: str, **q):
    doc = p.get(f"/api/3/action/{name}" + (f"?{urllib.parse.urlencode(q)}" if q else ""))
    if not doc.get("success"):
        raise SystemExit(f"{name}: {doc.get('error')}")
    return doc["result"]


def expand(doc) -> list:
    """The nodes of a JSON-LD document with every `prefix:local` key and type written out by the
    document's @context (prod's feed is expanded, a compacted one reads the same after this)."""
    ctx = doc.get("@context", {}) if isinstance(doc, dict) else {}
    prefixes = {k: v for k, v in (ctx.items() if isinstance(ctx, dict) else []) if isinstance(v, str)}

    def iri(term: str) -> str:
        head, sep, tail = term.partition(":")
        return prefixes[head] + tail if sep and head in prefixes and not tail.startswith("//") else term

    def node_of(n: dict) -> dict:
        node = {}
        for k, v in n.items():
            if k == "@type":
                node[k] = [iri(t) for t in (v if isinstance(v, list) else [v])]
            elif k.startswith("@"):
                node[k] = v
            else:
                node[iri(k)] = [node_of(x) if isinstance(x, dict) else x for x in (v if isinstance(v, list) else [v])]
        return node

    graph = doc if isinstance(doc, list) else doc.get("@graph", [doc] if "@id" in doc else [])
    out = [node_of(n) for n in graph if isinstance(n, dict)]
    # A compacted feed may nest its datasets inside the catalog node: every typed node counts.
    found, todo = [], list(out)
    while todo:
        n = todo.pop()
        if isinstance(n, list):
            todo += n
        elif isinstance(n, dict):
            if "@type" in n:
                found.append(n)
            todo += [v for k, v in n.items() if not k.startswith("@")]
    return found


def first_value(node: dict, key: str):
    v = (node.get(key) or [None])[0]
    return v.get("@value", v.get("@id")) if isinstance(v, dict) else v


def dcat_pages(p, path: str = "/catalog.jsonld"):
    """Every node of every page of a ckanext-dcat style paged JSON-LD feed, and its hydra totals."""
    nodes, page, total, catalogs = [], 1, None, 0
    while page <= 100:
        graph = expand(p.get(f"{path}?page={page}"))
        nodes += graph
        last = None
        for n in graph:
            types = n.get("@type") or []
            if f"{DCAT}Catalog" in types:
                catalogs += 1
            if f"{HYDRA}PagedCollection" in types or f"{HYDRA}PartialCollectionView" in types:
                total = first_value(n, f"{HYDRA}totalItems")
                last = first_value(n, f"{HYDRA}lastPage") or first_value(n, f"{HYDRA}last")
        if not last or str(last).endswith(f"page={page}"):
            break
        page += 1
    return nodes, {"pages": page, "totalItems": total, "catalogNodes": catalogs}


def distributions_by_package(nodes: list) -> dict:
    """{package id or name: distribution count} of the dataset nodes (`.../dataset/<x>`)."""
    out = {}
    for n in nodes:
        if f"{DCAT}Dataset" not in (n.get("@type") or []):
            continue
        key = str(n.get("@id", "")).rstrip("/").rsplit("/dataset/", 1)[-1]
        out[key] = len(n.get(f"{DCAT}distribution") or [])
    return out


def search_all(p: Paced, **q) -> list:
    """Every row of a package_search, 1000 a page: the suite's own query."""
    out, start = [], 0
    while True:
        r = action(p, "package_search", rows=1000, start=start, **q)
        out += r["results"]
        start += 1000
        if start >= r["count"] or not r["results"]:
            return out


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--base", default="https://opendata.smartcity.joinedcontext.com")
    ap.add_argument("--out", required=True)
    ap.add_argument("--gap", type=float, default=1.0)
    a = ap.parse_args(argv)
    if a.gap < 1:
        ap.error("--gap below 1 s: prod is read at most once a second")
    p = Paced(a.base, a.gap)
    orgs = action(p, "organization_list")
    names = action(p, "package_list")
    packages = search_all(p)
    names_by_id = {pk["id"]: pk["name"] for pk in packages}
    per_org = {o: len({pk["name"] for pk in search_all(p, fq=f'organization:"{o}"')}) for o in orgs}
    nodes, feed = dcat_pages(p)
    dist = distributions_by_package(nodes)
    seen = Counter(pk["name"] for pk in packages)
    duplicates = {n: c for n, c in sorted(seen.items()) if c > 1}
    live = {n: action(p, "package_show", id=n)["id"] for n in duplicates}
    packages = [pk for pk in packages if pk["name"] not in live or pk["id"] == live[pk["name"]]]
    datasets = {}
    for pk in packages:
        datasets[pk["name"]] = {
            "organization": (pk.get("organization") or {}).get("name"),
            "resources": len(pk.get("resources") or []),
            "dcatDistributions": dist.get(pk.get("id"), dist.get(pk["name"])),
        }
    # package_list can name datasets the search does not return: package_show tells a dataset the
    # index missed from a name nothing answers (prod 2026-11-09: 72 names, every one 404).
    unshown = []
    for name in sorted(set(names) - set(datasets)):
        doc = p.get(f"/api/3/action/package_show?{urllib.parse.urlencode({'id': name})}", missing_ok=True)
        if not doc or not doc.get("success"):
            unshown.append(name)
            continue
        pk = doc["result"]
        datasets[name] = {"organization": (pk.get("organization") or {}).get("name"),
                          "resources": len(pk.get("resources") or []),
                          "dcatDistributions": dist.get(pk.get("id"), dist.get(name)), "inSearch": False}
    feed["datasetNodes"] = sum(1 for d in datasets.values() if d["dcatDistributions"] is not None)
    # The feed's datasets by name, an orphan's node counted under the name it shares.
    feed["distinctDatasets"] = len({names_by_id.get(k, k) for k in dist})
    doc = {"fetchedAt": dt.datetime.now(dt.UTC).strftime("%Y-%m-%dT%H:%M:%SZ"), "base": a.base,
           "organizations": sorted(orgs), "packageList": sorted(names), "searchCount": len(packages),
           "listedNotFound": unshown, "searchDuplicates": duplicates,
           "perOrganization": per_org, "dcat": feed, "datasets": dict(sorted(datasets.items()))}
    Path(a.out).parent.mkdir(parents=True, exist_ok=True)
    Path(a.out).write_text(json.dumps(doc, indent=1, ensure_ascii=False) + "\n")
    print(f"wrote {a.out}: {len(orgs)} organisations, {len(names)} names, {len(packages)} packages, "
          f"{feed["datasetNodes"]} datasets in the DCAT feed over {feed['pages']} pages")
    return 0


if __name__ == "__main__":
    sys.exit(main())
