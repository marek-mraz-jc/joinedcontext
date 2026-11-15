"""Harvests one publisher's datasets from the national catalogue (NKOD) into a CKAN organization (T-4151).

Runs as `python3 -c` in the catalogue's image, from the `harvest` CronJob. One run:

1. reads the publisher's datasets and distributions from the NKOD SPARQL endpoint (DCAT-AP), with
   the licence from `legislation:termsOfUse`, where NKOD keeps it instead of `dct:license`;
2. fetches every distribution once and corrects what the source gets wrong (docs
   `Research/banska-bystrica-open-data-sources.md` sections 7 and 8): a title the configuration
   names as mislinked or personal is not ingested, an HTML or PDF answer where JSON is declared is
   not ingested, a non-ASCII URL is percent-encoded, string values are trimmed, and the rows the
   configuration says must carry an identifier (an IČO) are dropped without one;
3. writes each ingested dataset whose rows were converted to NGSI-LD into CKAN with its licence,
   the original publisher and its `modified`, the platform's distributions and data model
   (od-standards.py, EP-118) in place of the source's file links, and every JSON table of 50 MB
   or less as a DataStore table, which CKAN also serves as CSV; a dataset whose files hold no
   such table gets the Endpoint's own rows as its table instead (T-4742);
4. deletes a dataset an earlier run harvested that the publisher no longer lists or whose rows
   are not converted, unless no dataset is converted at all (a broken run), checks every
   ingested dataset (title, licence, a format on each resource, every link answered 200, every
   table holds its rows) and prints the count per state with each skipped title and its reason.

Fetched text is data: it is stored, never interpreted. TLS verification stays on, and a
distribution is fetched only over https from a host the configuration allows, redirects
included. The API token comes from the environment and is never printed.
"""
import hashlib
import json
import os
import re
import sys
import time
import unicodedata
import urllib.error
import urllib.parse
import urllib.request

CATALOGUE = os.environ.get("CATALOGUE", "")
TOKEN = os.environ.get("CKAN_API_TOKEN", "")
CONFIG = json.loads(os.environ.get("HARVEST_CONFIG", "{}"))

MAX_BYTES = 64 * 1024 * 1024
AGENT = "joinedcontext-harvest/1 (+https://www.joinedcontext.com)"
EU_LICENCE = "http://publications.europa.eu/resource/authority/licence/"
# The EU licence vocabulary to CKAN's own licence ids, in order of how much each one binds a
# reuser: a dataset whose distributions differ takes the stricter one.
LICENCES = {"CC0": "cc-zero", "CC_BY_4_0": "cc-by", "CC_BYSA_4_0": "cc-by-sa"}
STRICTNESS = ["cc-zero", "cc-by", "cc-by-sa"]

QUERY = """
PREFIX dcat: <http://www.w3.org/ns/dcat#>
PREFIX dct: <http://purl.org/dc/terms/>
PREFIX l: <https://data.gov.sk/def/ontology/legislation/>
SELECT ?ds ?title ?description ?modified ?issued ?landing ?keyword
       ?dist ?distTitle ?url ?format ?work ?database ?special WHERE {
  ?ds a dcat:Dataset ; dct:publisher <%s> ; dct:title ?title .
  OPTIONAL { ?ds dct:description ?description }
  OPTIONAL { ?ds dct:modified ?modified }
  OPTIONAL { ?ds dct:issued ?issued }
  OPTIONAL { ?ds dcat:landingPage ?landing }
  OPTIONAL { ?ds dcat:keyword ?keyword }
  OPTIONAL {
    ?ds dcat:distribution ?dist .
    OPTIONAL { ?dist dct:title ?distTitle }
    OPTIONAL { ?dist dcat:downloadURL ?url }
    OPTIONAL { ?dist dct:format ?format }
    OPTIONAL { ?dist l:termsOfUse ?terms .
               OPTIONAL { ?terms l:authorsWorkType ?work }
               OPTIONAL { ?terms l:originalDatabaseType ?database }
               OPTIONAL { ?terms l:databaseProtectedBySpecialRightsType ?special } }
  }
}
"""


class Refused(Exception):
    """A fetch the rules do not allow: plain http, or a host outside the allowlist."""


class Redirects(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        allowed(newurl)
        return super().redirect_request(req, fp, code, msg, headers, newurl)


OPENER = urllib.request.build_opener(Redirects())


def allowed(url):
    parts = urllib.parse.urlsplit(url)
    if parts.scheme != "https" or parts.hostname not in CONFIG.get("allowedHosts", []):
        raise Refused(f"{parts.scheme}://{parts.hostname} is not an allowed source")


def corrected_url(url):
    """The publisher's URL as a client can send it: a host the configuration moves (the bare
    host that 301s to `www.`) is moved, and a non-ASCII path or query is percent-encoded."""
    parts = urllib.parse.urlsplit(url.strip())
    host = CONFIG.get("rewriteHosts", {}).get(parts.netloc, parts.netloc)
    path = urllib.parse.quote(parts.path, safe="/%:@!$&'()*+,;=-._~")
    query = urllib.parse.quote(parts.query, safe="/%:@!$&'()*+,;=-._~?")
    return urllib.parse.urlunsplit((parts.scheme, host, path, query, ""))


def fetch(url, method="GET"):
    """(status, content type, body) of an allowed https URL, the body capped at MAX_BYTES."""
    allowed(url)
    request = urllib.request.Request(url, method=method, headers={"User-Agent": AGENT})
    try:
        with OPENER.open(request, timeout=60) as response:
            body = response.read(MAX_BYTES + 1) if method == "GET" else b""
            if len(body) > MAX_BYTES:
                raise Refused(f"answered more than {MAX_BYTES} bytes")
            kind = (response.headers.get("Content-Type") or "").split(";")[0].strip().lower()
            return response.status, kind, body
    except urllib.error.HTTPError as error:
        return error.code, "", b""


def sparql(query):
    data = urllib.parse.urlencode({"query": query}).encode()
    request = urllib.request.Request(CONFIG["sparql"], data=data, headers={
        "Accept": "application/sparql-results+json", "User-Agent": AGENT})
    with urllib.request.urlopen(request, timeout=120) as response:
        return json.load(response)["results"]["bindings"]


def ckan(action, payload):
    """One Action API call on the catalogue's Service. Returns `result`, or None on a 404."""
    host, _, port = CATALOGUE.partition(":")
    request = urllib.request.Request(
        f"http://{host}:{port or 5000}/api/3/action/{action}",
        data=json.dumps(payload).encode(),
        headers={"Content-Type": "application/json", "Authorization": TOKEN})
    # A new pod is refused for its first seconds, until the node's network-policy controller
    # admits its address (prod demo-bbsk, 2026-10-19: refused at 0 s, open at 5 s); the first
    # call comes sooner, so a refused connection is tried again, and nothing else is.
    for attempt in range(6):
        try:
            with urllib.request.urlopen(request, timeout=300) as response:
                return json.load(response)["result"]
        except urllib.error.HTTPError as error:
            if error.code == 404:
                return None
            detail = error.read(2000).decode("utf-8", "replace")
            raise RuntimeError(f"{action} answered {error.code}: {detail}") from None
        except urllib.error.URLError as error:
            if not isinstance(error.reason, ConnectionRefusedError) or attempt == 5:
                raise
            time.sleep(3)


# --- 1. what the catalogue lists ---------------------------------------------------------------

def datasets(bindings):
    """The SPARQL rows folded into one dict per dataset, keyed by its IRI."""
    out = {}
    for row in bindings:
        value = {key: cell["value"] for key, cell in row.items()}
        ds = out.setdefault(value["ds"], {
            "iri": value["ds"], "title": value["title"].strip(),
            "description": (value.get("description") or "").strip(),
            "modified": value.get("modified", ""), "issued": value.get("issued", ""),
            "landing": value.get("landing", ""), "keywords": set(), "distributions": {}})
        if value.get("keyword"):
            ds["keywords"].add(value["keyword"].strip())
        if value.get("dist") and value.get("url"):
            dist = ds["distributions"].setdefault(value["dist"], {
                "title": (value.get("distTitle") or ds["title"]).strip(), "url": value["url"],
                "format": value.get("format", "").rsplit("/", 1)[-1].upper(), "licences": set()})
            for key in ("work", "database", "special"):
                if value.get(key):
                    dist["licences"].add(value[key])
    return out


def licence(distributions):
    """The CKAN licence id of a dataset, or None when a distribution carries no open licence."""
    ids = set()
    for dist in distributions:
        codes = {code[len(EU_LICENCE):] if code.startswith(EU_LICENCE) else code for code in dist["licences"]}
        if not codes or not codes <= LICENCES.keys():
            return None
        ids |= {LICENCES[code] for code in codes}
    return max(ids, key=STRICTNESS.index) if ids else None


def slug(ds):
    """A readable CKAN name from the title, `Počet občanov podľa veku` -> `bb-pocet-obcanov-podla-veku`.
    NKOD's own landing-page names are often a mangled download URL, so they are not used."""
    ascii_title = unicodedata.normalize("NFKD", ds["title"]).encode("ascii", "ignore").decode()
    base = re.sub(r"[^a-z0-9]+", "-", ascii_title.lower()).strip("-") or ds["iri"].rsplit("/", 1)[-1]
    return (CONFIG["namePrefix"] + base)[:100]


def names(found):
    """CKAN names per dataset IRI; two titles that fold to one name both get their NKOD id."""
    counts = {}
    for ds in found.values():
        counts[slug(ds)] = counts.get(slug(ds), 0) + 1
    return {iri: slug(ds) if counts[slug(ds)] == 1 else
            (slug(ds)[:91] + "-" + iri.rsplit("/", 1)[-1][:8]) for iri, ds in found.items()}


# --- 2. what the files hold, corrected -----------------------------------------------------------

def table(body, title):
    """The JSON array of flat records a city file holds, trimmed, with the rows the
    configuration says must carry an identifier dropped when they do not. None if it is not one."""
    try:
        rows = json.loads(body.decode("utf-8-sig"))
    except (UnicodeDecodeError, ValueError):
        return None
    if not isinstance(rows, list) or not rows or not all(isinstance(r, dict) for r in rows):
        return None
    if any(isinstance(v, (dict, list)) for r in rows for v in r.values()):
        return None
    rows = [{str(k).strip(): (v.strip() if isinstance(v, str) else v) for k, v in r.items()} for r in rows]
    column = CONFIG.get("requireColumn", {}).get(title)
    dropped = 0
    if column:
        kept = [r for r in rows if str(r.get(column) or "").strip()]
        dropped, rows = len(rows) - len(kept), kept
    return rows, dropped


# States a later run can clear by itself: a distribution host that did not answer or answered an
# error, or served an HTML page where its file was.
TRANSIENT = {"dead", "html"}


def assess(ds):
    """Decides one dataset: (state, reason, resources, tables). Only `ingested` is written."""
    skip = CONFIG.get("skip", {}).get(ds["title"])
    if skip:
        return skip["state"], skip["reason"], [], []
    dists = list(ds["distributions"].values())
    if not dists:
        return "empty", "no distribution with a download URL", [], []
    licence_id = licence(dists)
    if not licence_id:
        return "licence", "no open licence in legislation:termsOfUse", [], []
    resources, tables, refusals, seen = [], [], [], set()
    filtered = ds["title"] in CONFIG.get("requireColumn", {})
    for dist in dists:
        url = corrected_url(dist["url"])
        if url in seen:
            continue
        seen.add(url)
        try:
            status, kind, body = fetch(url, "GET" if dist["format"] == "JSON" else "HEAD")
            if status in (405, 501):
                status, kind, body = fetch(url)
        except Refused as error:
            refusals.append(("refused", str(error)))
            continue
        except (urllib.error.URLError, OSError) as error:
            refusals.append(("dead", f"{url} did not answer: {error}"))
            continue
        if status != 200:
            refusals.append(("dead", f"{url} answered {status}"))
            continue
        if kind == "text/html":
            refusals.append(("html", f"declared {dist['format']}, {url} serves an HTML page"))
            continue
        if kind == "application/pdf":
            refusals.append(("pdf", f"declared {dist['format']}, {url} serves a PDF"))
            continue
        if dist["format"] == "JSON":
            parsed = table(body, ds["title"])
            if parsed is not None:
                rows, dropped = parsed
                tables.append({"name": f"{dist['title']} (tabuľka)", "rows": rows, "dropped": dropped})
        # A file whose rows were filtered is published only as the filtered table: linking the
        # original would publish the rows that were dropped.
        if not (filtered and dist["format"] == "JSON"):
            resources.append({"name": dist["title"], "url": url, "format": dist["format"] or kind,
                              "mimetype": kind or None})
    if not resources and not tables:
        state, reason = refusals[0] if refusals else ("empty", "nothing to publish")
        return state, reason, [], []
    return "ingested", licence_id, resources, tables


# --- 3. what CKAN holds --------------------------------------------------------------------------

def package(ds, name, licence_id, distributions, tables):
    dropped = sum(t["dropped"] for t in tables)
    notes = [ds["description"] or ds["title"],
             f"Zdroj: Národný katalóg otvorených dát, {ds['landing'] or ds['iri']}."]
    if dropped:
        notes.append(f"Z tabuľky sú vynechané riadky bez údaja {CONFIG['requireColumn'][ds['title']]} "
                     f"({dropped}), ktoré môžu označovať fyzickú osobu.")
    tags = [{"name": t[:100]} for t in sorted(ds["keywords"]) if re.fullmatch(r"[\w .-]{2,100}", t)]
    extras = {"harvest_source": CONFIG["source"], "source_uri": ds["iri"],
              "publisher_name": CONFIG["publisherName"], "publisher_uri": CONFIG["publisher"],
              "spatial_uri": CONFIG.get("spatialUri"),
              "issued": ds["issued"], "modified": ds["modified"]}
    return {"name": name, "title": ds["title"], "notes": "\n\n".join(notes),
            "owner_org": CONFIG["organization"], "license_id": licence_id, "tags": tags,
            "url": ds["landing"] or ds["iri"],
            "extras": [{"key": k, "value": v} for k, v in extras.items() if v] + od_extras(name),
            # The platform's own distributions of the converted rows and their data model, never the
            # source's file links (EP-118, T-4324); the DataStore tables follow in `upsert`.
            "resources": distributions}


def upsert(desired, tables):
    """Creates the dataset or updates it in place, never delete and create; a run whose source
    changed nothing writes nothing, so `metadata_modified` moves only with the data (T-4644)."""
    digest = hashlib.sha256(json.dumps([desired, [t["rows"] for t in tables]], sort_keys=True,
                                       ensure_ascii=False).encode()).hexdigest()
    desired["extras"].append({"key": "harvest_hash", "value": digest})
    live = ckan("package_show", {"id": desired["name"]})
    if live is not None and live.get("state") == "active" and {"key": "harvest_hash", "value": digest} in live.get("extras", []):
        existing = {r["name"]: r["id"] for r in live.get("resources", []) if r.get("url_type") == "datastore"}
        if all(t["name"] in existing for t in tables):
            for t in tables:
                t["resource_id"] = existing[t["name"]]
            return
    if live is None:
        live = ckan("package_create", desired)
    else:
        payload = dict(desired, id=live["id"], state="active")
        payload["resources"] = desired["resources"] + od_datastore_resources(live, tables)
        live = ckan("package_update", payload)
    od_write_tables(ckan, live, tables)


def harvested_names():
    names, start = set(), 0
    while True:
        page = ckan("package_search", {
            "fq": f'organization:{CONFIG["organization"]} AND extras_harvest_source:"{CONFIG["source"]}"',
            "rows": 1000, "start": start, "include_private": True, "fl": "name"})
        names |= {p["name"] for p in page["results"]}
        start += 1000
        if start >= page["count"]:
            return names


def verify(desired, tables):
    """The defects a reader would hit: missing title or licence, a resource without a format, a
    table that lost rows. Every link resource was fetched with 200 in `assess` already."""
    live = ckan("package_show", {"id": desired["name"]})
    problems = []
    if not live.get("title") or not live.get("license_id"):
        problems.append("title or licence missing")
    problems += [f"resource {r['name']} has no format" for r in live["resources"] if not r.get("format")]
    for t in tables:
        total = ckan("datastore_search", {"resource_id": t["resource_id"], "limit": 0})["total"]
        if total != len(t["rows"]):
            problems.append(f"table {t['name']} holds {total} rows, {len(t['rows'])} expected")
    return problems


def main():
    if not CATALOGUE or not TOKEN or not CONFIG.get("organization"):
        print("CATALOGUE, CKAN_API_TOKEN and HARVEST_CONFIG.organization are required", file=sys.stderr)
        return 2
    found = datasets(sparql(QUERY % CONFIG["publisher"]))
    print(f"{CONFIG['source']}: {len(found)} datasets in the catalogue")
    if not found:
        print("the catalogue listed nothing; nothing is deleted on an empty answer", file=sys.stderr)
        return 1
    if ckan("organization_show", {"id": CONFIG["organization"]}) is None:
        # The harvest user edits organizations and creates none (F-DEP1-002).
        print(f"organization {CONFIG['organization']} does not exist; the catalogue's api-token Job "
              "creates it on the next apply", file=sys.stderr)
        return 1
    states, problems, kept, named = {}, [], set(), names(found)
    converted = {ds["iri"]: od_standards(named[ds["iri"]]) for ds in found.values()}
    if not any(converted.values()):
        # No dataset converted at all is a missing gateway or mapping, not a catalogue to empty.
        print(f"none of {len(found)} datasets lists the platform's distributions; nothing is withdrawn",
              file=sys.stderr)
        return 1
    for ds in sorted(found.values(), key=lambda d: d["title"]):
        if not converted[ds["iri"]]:
            states.setdefault("not-converted", []).append((ds["title"], "not converted to NGSI-LD, so not listed (EP-118)"))
            continue
        state, reason, _, tables = assess(ds)
        states.setdefault(state, []).append((ds["title"], reason))
        if state in TRANSIENT:
            # A host down or a maintenance page tonight is no withdrawal: the dataset published
            # before stays until NKOD stops listing it or it is no longer converted (F-DEP1-013).
            kept.add(named[ds["iri"]])
        if state != "ingested":
            continue
        tables = [t for t in tables if len(json.dumps(t["rows"], ensure_ascii=False).encode()) <= OD_MAX_SHEET_BYTES]
        if not tables:
            # Every public dataset has an active DataStore (T-4742): the Endpoint's rows.
            sheet = od_sheet(named[ds["iri"]])
            tables = [sheet] if sheet else []
        desired = package(ds, named[ds["iri"]], reason, converted[ds["iri"]], tables)
        upsert(desired, tables)
        od_table_views(ckan, tables)
        kept.add(desired["name"])
        problems += [f"{ds['title']}: {p}" for p in verify(desired, tables)]
    for name in sorted(harvested_names() - kept):
        ckan("package_delete", {"id": name})
        states.setdefault("withdrawn", []).append((name, "no longer ingested from the catalogue"))
    for state in sorted(states):
        print(f"{state}: {len(states[state])}")
        if state != "ingested":
            for title, reason in states[state]:
                print(f"  - {title}: {reason}")
    for problem in problems:
        print(f"check failed: {problem}", file=sys.stderr)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
