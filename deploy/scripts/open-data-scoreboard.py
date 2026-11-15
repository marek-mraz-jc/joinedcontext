#!/usr/bin/env python3
# SPDX-License-Identifier: EUPL-1.2
"""Count the validated Smart Data Models datasets of each city against its target of 100 (T-5275).

    scripts/open-data-scoreboard.py <domain> <out-dir> [city ...]

Reads only public data, anonymously, through scripts/smoke-catalogue.py's paced client: CKAN's
package_search for each city's organisations, then per dataset 20 NGSI-LD entities of its type
(with sysAttrs), the OData count and every distribution link. A dataset passes when it holds
entities of a Smart Data Models type vendored in scripts/open-data/sdm and all four checks hold:

- sdm: each sampled entity, as keyValues, is valid against the pinned schema.json of its type;
- validation: positions inside the city (+1 km), no 0,0 or swapped lat/lon, values in physical
  ranges, no observation dated over a day ahead, no duplicate (same id, or same name within 20 m),
  every Relationship resolves, the platform holds at least one entity;
- freshness: the newest entity time is younger than 3 periods of the dataset's DCAT frequency;
- formats: every distribution link answers with content (WebSocket 426, MCP a tools/list).

Writes <out-dir>/<date>.md (the table, the per-city count against the target) and
<out-dir>/latest.json (what passed). Exits 1 when a dataset that passed in latest.json fails now.
"""

import importlib.util
import json
import math
import re
import socket
import sys
import urllib.parse
from datetime import UTC, datetime, timedelta
from pathlib import Path

ROOT = Path(__file__).resolve().parent
_spec = importlib.util.spec_from_file_location("smoke_catalogue", ROOT / "smoke-catalogue.py")
smoke = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(smoke)

SDM = ROOT / "open-data/sdm"
TARGET = 100
SAMPLE = 20
# ponytail: CKAN organisations per city and the city's own bounding box (west, south, east, north),
# rounded outward; a city that gets a second organisation adds it here.
CITIES = {
    "helsinki": (["hel-fi"], (24.78, 59.92, 25.27, 60.30)),
    "barcelona": (["barcelona"], (2.05, 41.32, 2.23, 41.47)),
    "praha": (["praha"], (14.22, 49.94, 14.71, 50.18)),
    "bratislava": (["bratislava"], (16.94, 48.00, 17.30, 48.27)),
}
MARGIN_KM = 1.0
# DCAT frequency (publications.europa.eu authority table) -> seconds; a continuous feed counts hourly.
PERIODS = {"CONT": 3600, "UPDATE_CONT": 3600, "1MIN": 60, "5MIN": 300, "10MIN": 600, "15MIN": 900,
           "30MIN": 1800, "HOURLY": 3600, "BIHOURLY": 7200, "DAILY": 86400, "BIDAILY": 2 * 86400,
           "WEEKLY": 7 * 86400, "BIWEEKLY": 14 * 86400, "MONTHLY": 31 * 86400, "BIMONTHLY": 62 * 86400,
           "QUARTERLY": 92 * 86400, "ANNUAL_2": 183 * 86400, "ANNUAL": 366 * 86400, "BIENNIAL": 731 * 86400}
STALE_PERIODS = 3
# Physical ranges by attribute name (owner 2026-10-22T18:10Z): (pattern, low, high).
RANGES = [
    (re.compile(r"(?i)temperature$|^dewPoint$"), -40, 50),
    (re.compile(r"(?i)percent(age)?$"), 0, 100),
    (re.compile(r"(?i)speed$"), 0, 150),
    (re.compile(r"(?i)(count|number|capacity|occupancy|intensity)$"), 0, math.inf),
]
# Pairs whose first may not exceed the second.
BOUNDS = [("occupiedSpotNumber", "totalSpotNumber"), ("availableSpotNumber", "totalSpotNumber"),
          ("occupancy", "capacity")]
# Observation times that may not lie in the future; an event's startDate may.
OBSERVED = {"dateObserved", "dateModified", "dateCreated", "dateLastReported", "dateIssued", "observedAt",
            "modifiedAt", "createdAt"}
SYSTEM = {"@context", "createdAt", "modifiedAt", "deletedAt", "expiresAt"}
COLUMNS = ["sdm", "validation", "freshness", "formats"]
NOT_RUN = "not this run"


def schemas() -> dict:
    """{type: schema.json} of every vendored Smart Data Model; extensions/ are ours, not SDM's."""
    return {p.parent.name: p for p in sorted(SDM.glob("*/*/schema.json")) if p.parts[-3] != "extensions"}


def validator(path: Path):
    import jsonschema
    from referencing import Registry, Resource
    from referencing.jsonschema import DRAFT7

    common = Resource.from_contents(json.loads((SDM / "common-schema.json").read_text()), DRAFT7)
    registry = Registry().with_resources((u, common) for u in (
        "https://smart-data-models.github.io/data-models/common-schema.json",
        "https://raw.githubusercontent.com/smart-data-models/data-models/master/common-schema.json"))
    registry = registry.with_resources(
        (f"https://smart-data-models.github.io/dataModel.{p.parent.name}/{p.name}",
         Resource.from_contents(json.loads(p.read_text()), DRAFT7)) for p in SDM.glob("*/*-schema.json"))
    return jsonschema.Draft7Validator(json.loads(path.read_text()), registry=registry,
                                      format_checker=jsonschema.Draft7Validator.FORMAT_CHECKER)


def key_values(entity: dict) -> dict:
    """The entity as keyValues, the shape SDM schemas describe; a LanguageProperty as its first text,
    a multi-instance attribute as its last instance."""
    out = {}
    for name, attribute in entity.items():
        if name in SYSTEM:
            continue
        if isinstance(attribute, list) and attribute and all(isinstance(i, dict) and "type" in i for i in attribute):
            attribute = attribute[-1]
        if not isinstance(attribute, dict) or "type" not in attribute or name in ("id", "type"):
            out[name] = attribute
        elif attribute["type"] == "Relationship":
            out[name] = attribute.get("object")
        elif attribute["type"] == "LanguageProperty":
            out[name] = next(iter((attribute.get("languageMap") or {}).values()), "")
        elif attribute["type"] == "JsonProperty":
            out[name] = attribute.get("json")
        else:
            out[name] = attribute.get("value")
    return out


def sdm_check(rows: list, kind: str, known: dict) -> str:
    if kind not in known:
        return smoke.ko(f"{kind} is no vendored Smart Data Model (scripts/open-data/sdm, pins.json)")
    check = validator(known[kind])
    problems = []
    for row in rows:
        for error in check.iter_errors(key_values(row)):
            where = "/".join(map(str, error.absolute_path)) or "(entity)"
            problems.append(f"{row.get('id')} {where}: {error.message[:120]}")
    if problems:
        more = f" (+{len(problems) - 3} more)" if len(problems) > 3 else ""
        return smoke.ko("; ".join(problems[:3]) + more)
    return smoke.ok(f"{len(rows)} entities valid against {known[kind].parent.parent.name}/{kind}")


def widened(box: tuple) -> tuple:
    west, south, east, north = box
    dlat = MARGIN_KM / 111.0
    dlon = dlat / math.cos(math.radians((south + north) / 2))
    return west - dlon, south - dlat, east + dlon, north + dlat


def inside(box: tuple, lon: float, lat: float) -> bool:
    west, south, east, north = box
    return west <= lon <= east and south <= lat <= north


def metres(a: list, b: list) -> float:
    lat = math.radians((a[1] + b[1]) / 2)
    return math.hypot((a[0] - b[0]) * math.cos(lat), a[1] - b[1]) * 111_195


def when(text) -> datetime | None:
    if not isinstance(text, str) or not re.match(r"^\d{4}-\d{2}-\d{2}T", text):
        return None
    try:
        moment = datetime.fromisoformat(text)
    except ValueError:
        return None
    return moment if moment.tzinfo else moment.replace(tzinfo=UTC)


def times(row: dict) -> list:
    """(name, time) of the entity's observation times: system times, dates, each observedAt."""
    found = []
    for name, attribute in row.items():
        if name in OBSERVED and (moment := when(attribute)):
            found.append((name, moment))
        for instance in attribute if isinstance(attribute, list) else [attribute]:
            if not isinstance(instance, dict):
                continue
            if name in OBSERVED and (moment := when(instance.get("value"))):
                found.append((name, moment))
            if moment := when(instance.get("observedAt")):
                found.append((f"{name}.observedAt", moment))
    return found


def value_problems(rid: str, kv: dict) -> list:
    problems = []
    for name, value in kv.items():
        if not isinstance(value, (int, float)) or isinstance(value, bool):
            continue
        for pattern, low, high in RANGES:
            if pattern.search(name) and not low <= value <= high:
                problems.append(f"{rid} {name} {value} outside {low}..{high}")
                break
    for small, large in BOUNDS:
        a, b = kv.get(small), kv.get(large)
        if isinstance(a, (int, float)) and isinstance(b, (int, float)) and a > b:
            problems.append(f"{rid} {small} {a} > {large} {b}")
    return problems


def location_problems(rid: str, row: dict, city: str, box: tuple) -> tuple[list, list]:
    """What is wrong with the entity's location, and its first point."""
    attribute = row.get("location")
    if not isinstance(attribute, dict):
        return [], None
    points = smoke.positions(attribute.get("value"))
    if points is None:
        return [f"{rid} location is no GeoJSON geometry"], None
    problems = []
    for lon, lat in points:
        if lon == 0 and lat == 0:
            problems.append(f"{rid} location at 0,0")
        elif not inside(box, lon, lat):
            swapped = inside(box, lat, lon)
            problems.append(f"{rid} location {'has lat/lon swapped' if swapped else f'lies outside {city} +1 km'}")
        if problems:
            break
    return problems, (points[0] if points else None)


def validate(rows: list, kind: str, city: str, box: tuple, now: datetime) -> list:
    problems, ids, named = [], set(), []
    for row in rows:
        rid = str(row.get("id") or "(no id)")
        if rid in ids:
            problems.append(f"{rid} twice")
        ids.add(rid)
        if row.get("type") != kind:
            problems.append(f"{rid} is a {row.get('type')}")
        kv = key_values(row)
        problems += value_problems(rid, kv)
        problems += [f"{rid} {name} {moment:%Y-%m-%dT%H:%MZ} lies over a day ahead"
                     for name, moment in times(row) if moment > now + timedelta(days=1)]
        located, point = location_problems(rid, row, city, box)
        problems += located
        name = kv.get("name")
        if point and isinstance(name, str) and name.strip():
            # An event repeats at one venue: its start tells two showings apart.
            key = (name.strip().casefold(), str(kv.get("startDate") or ""))
            twin = next((other for k, p, other in named if k == key and metres(p, point) <= 20), None)
            if twin:
                problems.append(f"{rid} duplicates {twin} (same name within 20 m)")
            named.append((key, point, rid))
    return problems


def freshness(rows: list, frequency: str, now: datetime) -> str:
    code = frequency.rstrip("/").rsplit("/", 1)[-1]
    period = PERIODS.get(code)
    if period is None:
        return smoke.ok(f"no update rate to hold it to ({code or 'none declared'})")
    newest = max((moment for row in rows for _, moment in times(row) if moment <= now + timedelta(days=1)),
                 default=None)
    if newest is None:
        return smoke.ko("no entity carries a time")
    age = (now - newest).total_seconds()
    limit = STALE_PERIODS * period
    if age > limit:
        return smoke.ko(f"newest {newest:%Y-%m-%dT%H:%MZ}, older than {STALE_PERIODS} x {code}")
    return smoke.ok(f"newest {newest:%Y-%m-%dT%H:%MZ} ({code})")


def link_problem(client, url: str, fmt: str) -> str:
    answer = client.fetch(url, limit=64 << 10)
    if fmt == "WebSocket":
        return "" if answer.status in (101, 426) else f"{fmt} answered {answer.why()}"
    if answer.status != 200:
        return f"{fmt} {url} answered {answer.why()}"
    lines = [line for line in answer.body.decode(errors="replace").splitlines() if line.strip()]
    if not lines:
        return f"{fmt} {url} is empty"
    if fmt == "CSV" and "text/csv" in answer.ctype and len(lines) < 2:
        return f"{fmt} {url} has a header and no row"
    return ""


class Board:
    def __init__(self, client, domain: str, now: datetime, known: dict | None = None):
        self.client, self.domain, self.now = client, domain, now
        self.known = schemas() if known is None else known
        self.run = smoke.Run(client, domain, {}, "", now.strftime("%Y%m%dT%H%M%SZ"))

    def packages(self, orgs: list) -> list:
        """The datasets of the organisations, one query each: CKAN answers an OR of two fq with every dataset."""
        found = []
        for org in orgs:
            query = urllib.parse.urlencode({"rows": 1000, "fq": f'organization:"{org}"'})
            answer = self.client.fetch(f"https://opendata.{self.domain}/api/3/action/package_search?{query}",
                                       limit=smoke.MAX_INVENTORY)
            result = (answer.json() or {}).get("result") if answer.status == 200 else None
            if not isinstance(result, dict):
                raise SystemExit(f"  FAIL  scoreboard: package_search answered {answer.why()}")
            found += result.get("results") or []
        return found

    def sample(self, data) -> list | str:
        query = smoke.sample_query(data, SAMPLE) | {"options": "sysAttrs"}
        answer = self.client.fetch(self.run.base(data) + "ngsi-ld/v1/entities?" + urllib.parse.urlencode(query),
                                   limit=smoke.MAX_INVENTORY)
        body = answer.json() if answer.status == 200 else None
        if not isinstance(body, list):
            return f"entities answered {answer.why()}"
        return [r for r in body if isinstance(r, dict)] or f"no {data.type} entity"

    def formats(self, package: dict, data) -> str:
        problems, seen = [], set()
        for resource in package.get("resources") or []:
            url, fmt = resource.get("url") or "", resource.get("format") or ""
            if not url.startswith("https://") or url in seen:
                continue
            seen.add(url)
            if fmt == "MCP":
                cell = self.run.mcp(data)
                problem = "" if smoke.verdict(cell) == "ok" else f"MCP {cell[4:]}"
            else:
                problem = link_problem(self.client, url, fmt)
            if problem:
                problems.append(problem)
        if not seen:
            return smoke.ko("no distribution link")
        return smoke.ko("; ".join(problems[:3])) if problems else smoke.ok(f"{len(seen)} links")

    def row(self, city: str, box: tuple, package: dict, data) -> dict:
        extras = {e.get("key"): e.get("value") or "" for e in package.get("extras") or []}
        row = {"city": city, "dataset": data.name, "pipeline": extras.get("was_generated_by", "").removeprefix("pipeline "),
               "type": "", "entities": ""}
        if not data.slug:
            return row | {c: smoke.ko("the dataset lists no Endpoint") for c in COLUMNS}
        if not self.run.entity_type(data):
            return row | {c: smoke.ko(f"no entity type: {data.why}") for c in COLUMNS}
        row["type"] = data.type
        rows = self.sample(data)
        if isinstance(rows, str):
            row |= {c: smoke.ko(rows) for c in COLUMNS[:3]}
        else:
            row["sdm"] = sdm_check(rows, data.type, self.known)
            count = self.run.odata(data)
            row["entities"] = str(self.run.counts.get(data.name, ("", ""))[0])
            problems = validate(rows, data.type, city, widened(box), self.now) + self.run.dangling(data, rows)
            if smoke.verdict(count) == "ko":
                problems.append(f"count: {count[4:]}")
            more = f" (+{len(problems) - 3} more)" if len(problems) > 3 else ""
            row["validation"] = (smoke.ko("; ".join(problems[:3]) + more) if problems
                                 else smoke.ok(f"{len(rows)} entities checked"))
            row["freshness"] = freshness(rows, extras.get("frequency", ""), self.now)
        row["formats"] = self.formats(package, data)
        return row

    def table(self, cities: list) -> list:
        rows, pending = [], []
        for city in cities:
            orgs, _ = CITIES[city]
            for package in self.packages(orgs):
                pending.append((city, package))
        try:
            # Every dataset's type first: a Relationship is looked up in the datasets of its type.
            # ponytail: a dataset over several types (helsinki-transport) is judged by its first one.
            described = [(city, package, smoke.describe(package, self.domain)) for city, package in pending]
            # run.holders() derives (publisher, slug) per type from these (T-5402).
            self.run.datasets = [data for _, _, data in described]
            self.run.public = {data.slug for data in self.run.datasets if data.slug}
            for city, package, data in described:
                rows.append(self.row(city, CITIES[city][1], package, data))
                verdicts = " ".join(f"{c} {smoke.verdict(rows[-1][c])}" for c in COLUMNS)
                print(f"  {city} {rows[-1]['dataset']}: {verdicts} ({self.client.count} requests)", flush=True)
        except smoke.OutOfBudget:
            # The budget is spent or the edge said 429: what is left is not this run, never a ko.
            rows += [{"city": city, "dataset": package.get("name", ""), "pipeline": "", "type": "", "entities": "",
                      **dict.fromkeys(COLUMNS, NOT_RUN)} for city, package in pending[len(rows):]]
        return rows


def passes(row: dict) -> bool:
    return all(smoke.verdict(row.get(c, "")) == "ok" for c in COLUMNS)


def cell(text: str) -> str:
    return str(text).replace("|", "\\|").replace("\n", " ")


def markdown(rows: list, cities: list, now: datetime, domain: str, requests: int) -> str:
    lines = [f"# Open data scoreboard {now:%Y-%m-%d}", "",
             (f"Run {now:%Y-%m-%dT%H:%MZ} against {domain} by scripts/open-data-scoreboard.py (T-5275), "
             f"{requests} requests. A dataset counts when sdm, validation, freshness and formats are all ok."), "",
             "| City | Passing | Target | To go | Listed |", "|---|---:|---:|---:|---:|"]
    for city in cities:
        mine = [r for r in rows if r["city"] == city]
        good = sum(passes(r) for r in mine)
        lines.append(f"| {city} | {good} | {TARGET} | {max(TARGET - good, 0)} | {len(mine)} |")
    for city in cities:
        lines += ["", f"## {city}", ""]
        mine = [r for r in rows if r["city"] == city]
        if not mine:
            lines.append(f"No CKAN dataset of {', '.join(CITIES[city][0])} yet.")
            continue
        lines += ["| Dataset | Pipeline | Type | Entities | Pass | SDM | Validation | Freshness | Formats |",
                  "|---|---|---|---:|---|---|---|---|---|"]
        for r in sorted(mine, key=lambda r: (not passes(r), r["dataset"])):
            lines.append("| " + " | ".join(cell(x) for x in (
                r["dataset"], r["pipeline"], r["type"], r["entities"], "yes" if passes(r) else "no",
                *(r.get(c, "") for c in COLUMNS))) + " |")
    return "\n".join(lines) + "\n"


def run(domain: str, out: Path, cities: list, client=None, now: datetime | None = None,
        known: dict | None = None) -> int:
    client = client or smoke.Client(budget=6000, deadline=3600.0, interval=0.6)
    now = now or datetime.now(UTC)
    rows = Board(client, domain, now, known).table(cities)
    out.mkdir(parents=True, exist_ok=True)
    state = out / "latest.json"
    before = json.loads(state.read_text()) if state.is_file() else {}
    passed_before = {(c, d) for c, names in (before.get("passing") or {}).items() for d in names}
    listed = {(r["city"], r["dataset"]) for r in rows}
    regressed = [r for r in rows if (r["city"], r["dataset"]) in passed_before
                 and not passes(r) and NOT_RUN not in r.values()]
    # A passing dataset gone from the catalogue is a loss too.
    regressed += [{"city": c, "dataset": d, **dict.fromkeys(COLUMNS, smoke.ko("no longer in the catalogue"))}
                  for c, d in sorted(passed_before - listed) if c in cities]
    (out / f"{now:%Y-%m-%d}.md").write_text(markdown(rows, cities, now, domain, client.count))
    passing = {c: sorted(r["dataset"] for r in rows if r["city"] == c and passes(r)) for c in cities}
    # A dataset this run did not reach, or a city it left out, keeps its earlier verdict.
    for city, names in (before.get("passing") or {}).items():
        if city not in cities:
            passing[city] = names
            continue
        unreached = {r["dataset"] for r in rows if r["city"] == city and NOT_RUN in r.values()}
        passing[city] = sorted(set(passing.get(city, [])) | (set(names) & unreached))
    state.write_text(json.dumps({"at": now.strftime("%Y-%m-%dT%H:%M:%SZ"), "passing": passing}, indent=2) + "\n")
    for city in cities:
        print(f"  {city}: {len(passing.get(city, []))} of {TARGET} passing")
    for r in regressed:
        bad = "; ".join(f"{c} {r[c][4:]}" for c in COLUMNS if smoke.verdict(r[c]) == "ko")
        print(f"  FAIL  {r['city']} {r['dataset']} passed before and fails now: {bad}")
    return 1 if regressed else 0


def main(argv: list[str]) -> int:
    if len(argv) < 3 or any(c not in CITIES for c in argv[3:]):
        print(__doc__.strip().splitlines()[2].strip(), file=sys.stderr)
        return 2
    socket.getaddrinfo = smoke.functools.lru_cache(maxsize=64)(socket.getaddrinfo)
    return run(argv[1], Path(argv[2]), argv[3:] or list(CITIES))


if __name__ == "__main__":
    sys.exit(main(sys.argv))
