#!/usr/bin/env python3
"""Restores each catalogue dataset's first publication as its `issued` extra (T-4644).

The datasets were re-created on 15-17 Oct 2026, so CKAN's `metadata_created` is the day of the
re-creation, not of the first publication. For every dataset without an `issued` extra this script
takes, in this order:

1. the source publisher's own date: `dct:issued` of the NKOD record whose distribution is the
   dataset's source (or the record named in RECORDS), of the NKOD record of the same ŠÚ SR
   DATAcube table (its `dct:modified` when it has no `dct:issued`: the latest the table can have
   appeared), `metadata_created` of a HRI record, `created` of an ArcGIS Hub item;
2. otherwise the platform's first publication: the first commit of this repository that names the
   dataset's Endpoint slug or its name;
3. otherwise nothing: the dataset keeps `metadata_created`.

Every date is read from the source it names, never typed. A dry run (the default) prints one row per
dataset with the chosen date and its evidence; `--write` adds the `issued` extra with package_patch
and changes nothing else. A second run finds every dataset carrying `issued` and writes nothing.

    scripts/ckan-restore-issued.py https://opendata.dev.joinedcontext.com            # dry run
    CKAN_API_TOKEN=... scripts/ckan-restore-issued.py https://... --write       # the token from the
                                                                                # ckan-api-token secret
    scripts/ckan-restore-issued.py https://... --not-after 2026-09-28           # T-4733: no issued
                                                                                # later than that day
"""
import argparse
import datetime
import json
import os
import re
import subprocess
import sys
import urllib.parse
import urllib.request

NKOD = "https://data.slovensko.sk/api/sparql"
AGENT = "joinedcontext-ckan-restore-issued/1 (+https://joinedcontext.com)"
# Datasets whose source the platform names only in words (`rights`), with the NKOD record that
# publishes that source; the date is still read from the record.
RECORDS = {
    # SHMÚ's "Údaje z pozorovaní vykonaných meteorologickými stanicami", whose distribution is
    # https://opendata.shmu.sk/meteorology/climate/now/data, the pipeline's source.
    "banskabystrica-pocasie": "https://data.gov.sk/set/9bdd8179-9cb0-46c6-a0d0-a52e50d2e2bc",
}


def get_json(url, data=None, headers=None):
    request = urllib.request.Request(url, data=data, headers={"User-Agent": AGENT, **(headers or {})})
    with urllib.request.urlopen(request, timeout=60) as answer:
        return json.load(answer)


def sparql(query):
    data = urllib.parse.urlencode({"query": query}).encode()
    answer = get_json(NKOD, data, {"Accept": "application/sparql-results+json"})
    return [{k: v["value"] for k, v in row.items()} for row in answer["results"]["bindings"]]


def iso(value):
    """A date as `YYYY-MM-DDTHH:MM:SSZ` in UTC; None when it does not read as one."""
    try:
        parsed = datetime.datetime.fromisoformat(str(value).replace("Z", "+00:00"))
    except ValueError:
        return None
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=datetime.timezone.utc)
    return parsed.astimezone(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def extras(pkg):
    return {e["key"]: e["value"] for e in pkg.get("extras") or []}


def sources(pkg):
    """The http addresses of the dataset's `source` extra: a JSON list of {url, title} or of
    addresses, or one address."""
    value = extras(pkg).get("source")
    if not value:
        return []
    try:
        items = json.loads(value)
    except ValueError:
        items = value
    items = items if isinstance(items, list) else [items]
    urls = [i.get("url") if isinstance(i, dict) else i for i in items]
    return [u for u in urls if isinstance(u, str) and u.startswith(("https://", "http://"))]


def nkod_record(iri):
    rows = sparql("PREFIX dct: <http://purl.org/dc/terms/> SELECT ?issued WHERE { <%s> dct:issued ?issued }" % iri)
    issued = sorted(filter(None, (iso(r["issued"]) for r in rows)))
    return (issued[0], f"NKOD {iri} dct:issued") if issued else None


def nkod_by_distribution(url):
    """The NKOD record one of whose distributions is `url`, when exactly one is."""
    escaped = url.replace("\\", "\\\\").replace('"', '\\"')
    rows = sparql(
        "PREFIX dct: <http://purl.org/dc/terms/> PREFIX dcat: <http://www.w3.org/ns/dcat#> "
        'SELECT DISTINCT ?ds ?issued WHERE { ?ds dcat:distribution ?d . '
        '{ ?d dcat:accessURL ?u } UNION { ?d dcat:downloadURL ?u } FILTER(STR(?u) = "%s") '
        "OPTIONAL { ?ds dct:issued ?issued } }" % escaped)
    if len({r["ds"] for r in rows}) != 1:
        return None
    issued = sorted(filter(None, (iso(r.get("issued")) for r in rows)))
    return (issued[0], f"NKOD {rows[0]['ds']} dct:issued") if issued else None


def nkod_by_page(url):
    """data.slovensko.sk/datasety/{id} is the NKOD record https://data.gov.sk/set/{id}."""
    found = re.match(r"https://data\.slovensko\.sk/datasety/([0-9a-f-]+)$", url)
    return nkod_record(f"https://data.gov.sk/set/{found.group(1)}") if found else None


def datacube(url):
    """A ŠÚ SR DATAcube table: NKOD lists each cube once, with its downloads on
    statdata.statistics.sk rather than the API the pipeline reads, so the record is found by the
    cube's code (T-4718). Those records carry no `dct:issued`, only `dct:modified`: the table was
    at the source by then at the latest, so that date is shown, never one earlier."""
    found = re.match(r"https://data\.statistics\.sk/api/v2/dataset/([a-z0-9]+)/", url)
    if not found:
        return None
    rows = sparql(
        "PREFIX dct: <http://purl.org/dc/terms/> PREFIX dcat: <http://www.w3.org/ns/dcat#> "
        "SELECT DISTINCT ?ds ?issued ?modified WHERE { ?ds dcat:distribution ?d . "
        "{ ?d dcat:accessURL ?u } UNION { ?d dcat:downloadURL ?u } "
        'FILTER(CONTAINS(LCASE(STR(?u)), "statistics.sk/") && CONTAINS(LCASE(STR(?u)), "/dataset/%s/")) '
        "OPTIONAL { ?ds dct:issued ?issued } OPTIONAL { ?ds dct:modified ?modified } }" % found.group(1))
    if len({r["ds"] for r in rows}) != 1:
        return None
    for term in ("issued", "modified"):
        dates = sorted(filter(None, (iso(r.get(term)) for r in rows)))
        if dates:
            return dates[0], f"NKOD {rows[0]['ds']} dct:{term} (DATAcube {found.group(1)})"
    return None


def hri(url):
    found = re.match(r"https://hri\.fi/data/[a-zA-Z_]+/dataset/([a-z0-9-]+)$", url)
    if not found:
        return None
    record = get_json("https://hri.fi/data/api/3/action/package_show?id=" + found.group(1))["result"]
    created = iso(record.get("metadata_created"))
    return (created, f"HRI {found.group(1)} metadata_created") if created else None


def arcgis(url):
    found = re.match(r"https://hub\.arcgis\.com/api/v3/datasets/([0-9a-f]{32}_\d+)/", url)
    if not found:
        return None
    item = get_json("https://hub.arcgis.com/api/v3/datasets/" + found.group(1))["data"]["attributes"]
    created = item.get("created")
    if not isinstance(created, (int, float)):
        return None
    stamp = datetime.datetime.fromtimestamp(created / 1000, datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    return stamp, f"ArcGIS Hub {found.group(1)} created"


def source_date(pkg):
    """The source publisher's own date for the dataset, with what it was read from."""
    if pkg["name"] in RECORDS:
        return nkod_record(RECORDS[pkg["name"]])
    for url in sources(pkg):
        for reader in (nkod_by_page, hri, arcgis, nkod_by_distribution, datacube):
            try:
                found = reader(url)
            except (OSError, ValueError, KeyError) as error:
                print(f"  {pkg['name']}: {reader.__name__} {url}: {error}", file=sys.stderr)
                continue
            if found:
                return found
    return None


def first_commit(repo, pkg):
    """The earliest commit of `repo` that names the dataset, as (date, sha): its Endpoint slug, or
    its whole name where it has no slug (a name inside a longer one is not it)."""
    slug = extras(pkg).get("identifier")
    pattern = re.escape(slug) if slug else r"(^|[^a-z0-9-])%s([^a-z0-9-]|$)" % re.escape(pkg["name"])
    out = subprocess.run(["git", "-C", repo, "log", "--reverse", "--format=%H %aI", "-G" + pattern],
                         capture_output=True, text=True, check=True).stdout.split("\n", 1)[0]
    if not out:
        return None
    sha, date = out.split()
    return iso(date), sha[:12]


def choose(pkg, repo):
    """(date, evidence) of the dataset's first publication, or None to leave it as it is."""
    found = source_date(pkg)
    if found:
        return found
    commit = first_commit(repo, pkg)
    if commit:
        return commit[0], f"first commit naming it in joinedcontext-deployment, {commit[1]}"
    return None


def ckan(base, action, payload=None, token=None):
    url = f"{base.rstrip('/')}/api/3/action/{action}"
    data = json.dumps(payload).encode() if payload is not None else None
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = token
    answer = get_json(url, data, headers)
    if not answer.get("success"):
        raise RuntimeError(f"{action}: {answer.get('error')}")
    return answer["result"]


def packages(base):
    found, start = [], 0
    while True:
        page = ckan(base, "package_search", {"rows": 1000, "start": start})
        found += page["results"]
        start += 1000
        if start >= page["count"]:
            return found


def restore(base, repo, write, token=None, not_after=None):
    """One row per dataset: (name, metadata_created, issued or None, evidence, action).

    With `not_after` (T-4733) no dataset keeps an `issued` later than it: an older true date stays,
    a later or missing one becomes `not_after`."""
    rows = []
    for pkg in sorted(packages(base), key=lambda p: p["name"]):
        created = iso(pkg.get("metadata_created"))
        current = iso(extras(pkg).get("issued"))
        if extras(pkg).get("issued") and (not_after is None or (current and current <= not_after)):
            rows.append((pkg["name"], created, extras(pkg)["issued"], "issued already set", "kept"))
            continue
        found = choose(pkg, repo)
        if not_after is not None:
            if not found or found[0] > not_after:
                was = found[0] if found else current or "none"
                found = (not_after, f"--not-after {not_after} (evidence {was})")
        elif not found or found[0] >= created:
            evidence = found[1] if found else "no evidence"
            rows.append((pkg["name"], created, None, evidence, "unchanged"))
            continue
        issued, evidence = found
        if write:
            kept = [e for e in pkg.get("extras") or [] if e["key"] != "issued"]
            ckan(base, "package_patch", {"id": pkg["id"], "extras": kept + [{"key": "issued", "value": issued}]}, token)
        rows.append((pkg["name"], created, issued, evidence, "written" if write else "would write"))
    return rows


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("catalogue", help="the CKAN base address, e.g. https://opendata.dev.joinedcontext.com")
    parser.add_argument("--repo", default=os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                        help="the joinedcontext-deployment clone whose history is the evidence")
    parser.add_argument("--write", action="store_true", help="add the issued extras (default: dry run)")
    parser.add_argument("--not-after", help="no issued later than this date, e.g. 2026-09-28 (T-4733)")
    args = parser.parse_args()
    not_after = iso(args.not_after) if args.not_after else None
    if args.not_after and not not_after:
        print(f"--not-after {args.not_after!r} is no date", file=sys.stderr)
        return 2
    token = os.environ.get("CKAN_API_TOKEN")
    if args.write and not token:
        print("--write needs CKAN_API_TOKEN", file=sys.stderr)
        return 2
    rows = restore(args.catalogue, args.repo, args.write, token, not_after)
    print("| dataset | metadata_created | issued | evidence | action |")
    print("|---|---|---|---|---|")
    for name, created, issued, evidence, action in rows:
        print(f"| {name} | {created} | {issued or '—'} | {evidence} | {action} |")
    return 0


if __name__ == "__main__":
    sys.exit(main())
