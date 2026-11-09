"""Smoke suite `catalogue`: the rehearsal's open data catalogue (dataroom) against prod's CKAN.

   bins/jc/smoke/run catalogue --base-url <url> --out <dir> [--export <bundle>]
       [--prod <fixtures/prod-ckan-*.json>] [--renames <RENAMES.md>] [--path /opendata]

Prod is never contacted: fetch_prod_ckan.py captured its catalogue once (anonymous GETs). Every call
here is an anonymous GET on the rehearsal, as a visitor of the open data portal makes it:

  organisations     organization_list holds each organisation prod's does
  package_list      each name prod lists (a name prod lists and answers 404 for is no dataset)
  package_search    per organisation, the count and the names prod's search answers
  package_show      every prod dataset: found, in prod's organisation, with prod's resource count
  dcat              /catalog.jsonld (every page): a catalog, prod's total, each prod dataset with
                    prod's distribution count
  control           package_show of a name nobody made answers no dataset, so a dataroom that
                    answers everything fails

With --export of another environment (the CI gate's synthetic dev slice) the comparisons are named
skips and only the control and the feed's shape run.

Names go through RENAMES.md (kind `dataset`, evidence/jc-dev/RENAMES.md): a renamed dataset is
looked up under its new name, and its old name must still answer package_show with the new one. A
`dataset (extra)` row names a dataset jc holds and prod never did: package_search and the DCAT total
count it on top of prod's, and a jc name no row lists fails its organisation's search.
Ids and timestamps never compare: jc mints its own. The checks on jc alone (each Endpoint's
dataset published, per organisation isolation) are the endpoints, names and multiorg suites'."""

from __future__ import annotations

import argparse
import json
import sys
import urllib.parse
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(HERE.parent))
sys.path.insert(0, str(HERE.parent / "names"))

from common import Http, Results, as_json, loopback_only, read_token
from compare import load_renames
from fetch_prod_ckan import dcat_pages, distributions_by_package

FIXTURES = HERE / "fixtures"
DEFAULT_RENAMES = Path("/workspace/evidence/jc-dev/RENAMES.md")
NOBODY = "smoke-catalogue-no-such-dataset-7f3a"


class Ckan:
    """Anonymous CKAN action reads on the rehearsal's dataroom."""

    def __init__(self, base: str, path: str):
        self.http = Http(base.rstrip("/") + path, timeout=120)

    def action(self, name: str, **q):
        st, _, raw = self.http.call("GET", f"/api/3/action/{name}" + (f"?{urllib.parse.urlencode(q)}" if q else ""))
        doc = as_json(raw)
        ok = st == 200 and isinstance(doc, dict) and doc.get("success") is True
        return st, (doc.get("result") if ok else None), doc

    def get(self, path: str):
        st, _, raw = self.http.call("GET", path)
        if st != 200:
            raise RuntimeError(f"GET {path}: {st}")
        doc = as_json(raw)
        if doc is None:
            raise RuntimeError(f"GET {path}: not JSON")
        return doc


def search_all(c: Ckan, **q) -> tuple[int, list | None]:
    out, start = [], 0
    while True:
        st, r, _ = c.action("package_search", rows=1000, start=start, **q)
        if r is None:
            return st, None
        out += r.get("results") or []
        start += 1000
        if start >= (r.get("count") or 0) or not r.get("results"):
            return st, out


def org_of(pk: dict | None) -> str | None:
    return ((pk or {}).get("organization") or {}).get("name")


def run(a, res: Results) -> dict:
    prod = json.loads(Path(a.prod).read_text())
    rename_rows = [r for r in load_renames(a.renames) if r.kind == "dataset"]
    renames = {r.old: r for r in rename_rows if not r.extra}
    extras = {r.new for r in rename_rows if r.extra}
    jc_name = {n: (renames[n].new if n in renames else n) for n in prod["datasets"]}
    c = Ckan(a.base_url, a.path)
    call = lambda s: f"GET {a.path}/api/3/action/{s}"

    st, body, _ = c.action("package_show", id=NOBODY)
    res.check("control.package_show-unknown", "dataroom", call(f"package_show?id={NOBODY}"),
              "no dataset (404 or success false)", f"{st}, {'a dataset' if body else 'none'}", body is None)

    why = not_prod(a.export)
    if why:
        for id in ("organisations", "package_list", "package_search", "package_show", "dcat.total", "dcat.dataset"):
            res.skip(id, "dataroom", "prod's catalogue as the reference", "as prod", why)
        try:
            nodes, feed = dcat_pages(c, "/catalog.jsonld")
            res.check("dcat.catalog", "dataroom", f"GET {a.path}/catalog.jsonld?page=1..",
                      "a dcat:Catalog node on each page", f"{feed['catalogNodes']} over {feed['pages']} pages",
                      feed["catalogNodes"] >= 1)
        except (RuntimeError, ValueError) as err:
            res.check("dcat.feed", "dataroom", f"GET {a.path}/catalog.jsonld", "the paged DCAT feed", str(err)[:160], False)
        return {"prod": Path(a.prod).name, "compared": why}

    st, orgs, _ = c.action("organization_list")
    got = set(orgs or [])
    for o in prod["organizations"]:
        res.check(f"organisations.{o}", o, call("organization_list"), "listed, as on prod",
                  "listed" if o in got else (f"{st}, not listed" if orgs is not None else f"{st}, unreadable"), o in got)

    st, names, _ = c.action("package_list")
    listed = set(names or [])
    unfound = set(prod.get("listedNotFound") or [])
    for n in prod["packageList"]:
        if n in unfound:
            res.check(f"package_list.{n}", "prod", "fixture: prod's package_show of the name", "a dataset on prod",
                      "prod lists it and answers 404 for it: no dataset to carry", True,
                      "prod's package_list names a dataset its package_show does not find")
            continue
        want = jc_name.get(n, n)
        res.check(f"package_list.{n}", "dataroom", call("package_list"), f"lists {want}" + (" (RENAMES.md)" if want != n else ""),
                  "listed" if want in listed else (f"{st}, not listed" if names is not None else f"{st}, unreadable"),
                  want in listed)

    by_org: dict[str, set] = {}
    for n, d in prod["datasets"].items():
        if d.get("inSearch", True):
            by_org.setdefault(d["organization"], set()).add(jc_name[n])
    for o, count in sorted(prod["perOrganization"].items()):
        st, rows = search_all(c, fq=f'organization:"{o}"')
        got_names = {r.get("name") for r in rows or []}
        want = by_org.get(o, set())
        missing = sorted(want - got_names)
        accepted = sorted((got_names - want) & extras)
        unexpected = sorted(got_names - want - extras)
        res.check(f"package_search.{o}", o, call(f"package_search?fq=organization:{o}"),
                  f"{count} datasets, prod's names" + (f", and the RENAMES.md extras {', '.join(accepted)}" if accepted else ""),
                  f"{st}, {len(rows) if rows is not None else 'unreadable'} datasets"
                  + (f", missing {len(missing)}: {', '.join(missing[:5])}" if missing else "")
                  + (f", not prod's and no RENAMES.md extra {len(unexpected)}: {', '.join(unexpected[:5])}" if unexpected else ""),
                  rows is not None and len(rows) == count + len(accepted) and not missing and not unexpected)

    st, everything = search_all(c)
    ids = {r.get("id"): r.get("name") for r in everything or []}
    for n, d in prod["datasets"].items():
        want = jc_name[n]
        st, pk, _ = c.action("package_show", id=want)
        detail = f"{st}" if pk is None else f"organisation {org_of(pk)}, {len(pk.get('resources') or [])} resources"
        ok = pk is not None and org_of(pk) == d["organization"] and len(pk.get("resources") or []) == d["resources"]
        res.check(f"package_show.{n}", d["organization"] or "", call(f"package_show?id={want}"),
                  f"organisation {d['organization']}, {d['resources']} resources"
                  + (f" (renamed from {n})" if want != n else ""), detail, ok)
        if want != n:
            st, old, _ = c.action("package_show", id=n)
            res.check(f"rename.{n}", d["organization"] or "", call(f"package_show?id={n}"),
                      f"the old name answers {want}", f"{st}, {old.get('name') if old else 'none'}",
                      bool(old) and old.get("name") == want, f"RENAMES.md: {renames[n].keeps}")

    try:
        nodes, feed = dcat_pages(c, "/catalog.jsonld")
    except (RuntimeError, ValueError) as err:
        res.check("dcat.feed", "dataroom", f"GET {a.path}/catalog.jsonld?page=1..", "the paged DCAT feed", str(err)[:160], False)
        return {"prod": Path(a.prod).name}
    pf = prod["dcat"]
    res.check("dcat.catalog", "dataroom", f"GET {a.path}/catalog.jsonld?page=1..", "a dcat:Catalog node on each page",
              f"{feed['catalogNodes']} catalog nodes over {feed['pages']} pages", feed["catalogNodes"] >= 1)
    dist = distributions_by_package(nodes)
    by_name = {ids.get(k, k): v for k, v in dist.items()}
    in_feed = sorted(extras & set(by_name))
    res.check("dcat.total", "dataroom", f"GET {a.path}/catalog.jsonld", f"hydra:totalItems {pf['totalItems']}, as prod's"
              + (f", and the RENAMES.md extras {', '.join(in_feed)}" if in_feed else ""),
              str(feed["totalItems"]), feed["totalItems"] == pf["totalItems"] + len(in_feed))
    for n, d in prod["datasets"].items():
        if d.get("dcatDistributions") is None:
            continue
        want = jc_name[n]
        got = by_name.get(want)
        res.check(f"dcat.dataset.{n}", d["organization"] or "", f"GET {a.path}/catalog.jsonld (every page)",
                  f"a dcat:Dataset with {d['dcatDistributions']} distributions",
                  "absent" if got is None else f"{got} distributions", got == d["dcatDistributions"])
    return {"prod": Path(a.prod).name, "prodFetchedAt": prod.get("fetchedAt"), "jcPages": feed["pages"]}


def not_prod(export: str | None) -> str:
    """Why prod's catalogue is not this rehearsal's reference ('' when it is): an export from
    another environment, or a synthetic one."""
    if not export:
        return ""
    m = Path(export) / "manifest.json"
    doc = json.loads(m.read_text()) if m.is_file() else {}
    if doc.get("env") != "prod" or doc.get("synthetic"):
        return (f"the rehearsal's export is {doc.get('env', 'unknown')}{' (synthetic)' if doc.get('synthetic') else ''}, "
                "not prod's: prod's catalogue is the reference of a prod rehearsal only")
    return ""


def latest_fixture() -> Path | None:
    found = sorted(FIXTURES.glob("prod-ckan-*.json"))
    return found[-1] if found else None


def main(argv=None) -> int:
    p = argparse.ArgumentParser(prog="run catalogue", description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--base-url", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--prod", default=str(latest_fixture() or ""), help="prod-ckan-*.json (default: the newest)")
    p.add_argument("--renames", default=str(DEFAULT_RENAMES) if DEFAULT_RENAMES.is_file() else None,
                   help="RENAMES.md (default evidence/jc-dev/RENAMES.md when present)")
    p.add_argument("--path", default="/opendata", help="dataroom's path on the rehearsal")
    p.add_argument("--export", help="the rehearsal's export bundle: prod's catalogue compares only with a prod one")
    p.add_argument("--admin-token-file", help="unused: every read is anonymous")
    p.add_argument("--person-token-file", help="unused: every read is anonymous")
    a = p.parse_args(argv)
    loopback_only(a.base_url)
    read_token(a.admin_token_file)
    if not a.prod:
        p.error("no prod-ckan-*.json: run fetch_prod_ckan.py once")
    res = Results("catalogue", a.base_url)
    meta = run(a, res)
    res.write(Path(a.out), meta)
    t = res.totals()
    print(f"catalogue: {t['pass']} pass, {t['fail']} fail, {t['skip']} skip -> {a.out}/REPORT.md")
    return 1 if t["fail"] else 0


if __name__ == "__main__":
    sys.exit(main())
