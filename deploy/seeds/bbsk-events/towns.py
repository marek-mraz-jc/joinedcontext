#!/usr/bin/env python3
"""Place each municipality the events name into towns.csv: its office, else its centre (T-5240).

An event that names no known venue takes the point of its own town (its `addressLocality`, else the source's
town), labelled approximate in `locationPrecision`: `office` for the municipality's `amenity=townhall`, `centre`
for its `place` node when OpenStreetMap maps no office (owner 2026-10-21: "GPS should be there, at least 90 %").
Košice and Bratislava are pinned by hand to their city halls: the townhall rule picks a borough office there
(geocode pilot 2026-10-22, all Košice office hits false).

The towns are the kept sources' towns and, with --localities, the `town,region` rows of a file (report.py
--localities). Overpass finds each municipality boundary of those names in its kraj; a name held by
several municipalities of one kraj is told apart only for a source's own town, by the source's web site, and is then
used for that source alone (`unique` n). One query per kraj (the source's region; a locality's is its source's). One request with the inventory User-Agent; an answer over 8 MB is refused.

    python3 seeds/bbsk-events/towns.py                              # print the rows it would add, with each OSM name
    python3 seeds/bbsk-events/towns.py --localities l.csv --write   # append them to towns.csv

Rows it adds say `overpass` in `checked_by`; read each printed name before --write.
"""

from __future__ import annotations

import argparse
import csv
import datetime
import json
import re
import sys
import time
from collections.abc import Callable
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import generate
import offices

TOWNS = HERE / "towns.csv"
FIELDS = ["town", "name", "region", "unique", "lat", "lon", "precision", "osm", "checked_on", "checked_by"]
MUNICIPAL = {"town", "municipality", "village"}
#: Seconds between two Overpass queries (one per kraj), as the public instance asks.
PAUSE = 2.0
#: Cities whose townhall rule picks a borough office: their city hall, read from Register adries 2026-10-22.
PINNED = {"Košice", "Bratislava"}
#: A name the inventory writes for a region or a district is no municipality (geocode pilot: "okres Myjava").
NOT_A_TOWN = re.compile(r"(?i)^okres\b|\bkraj$|^(sever|juh|západ|východ|stred)$")
OFFICE_NAME = re.compile(r"(?i)(obecn|mestsk|miestn)\w* úrad|^obec\b|^mesto\b|^úrad\b|radnica")


def query(names: list[str], region: str) -> str:
    """Each municipality boundary of those names in the region (a kraj), then its townhalls and its own place node."""
    pattern = json.dumps("^(" + "|".join(re.escape(n) for n in sorted(set(names))) + ")$", ensure_ascii=False)
    kraj = json.dumps(region, ensure_ascii=False)
    return (
        "[out:json][timeout:600];"
        f'area["boundary"="administrative"]["admin_level"="4"]["name"={kraj}]->.k;'
        f'rel(area.k)["boundary"="administrative"]["admin_level"="8"]["name"~{pattern}]->.m;'
        "foreach.m->.one(.one out ids tags;.one map_to_area->.a;"
        '(nwr(area.a)["amenity"="townhall"];node(area.a)["place"~"^(city|town|village)$"];);out center tags;);'
    )


def pick(elements: list[dict], name: str) -> tuple[dict | None, str]:
    """The municipality's point: its single (or single office-named) townhall, else its place node of the same
    name. Returns the element and its precision."""
    placed = [e for e in elements if offices.point(e)]
    halls = [e for e in placed if e["tags"].get("amenity") == "townhall"]
    named = [e for e in halls if OFFICE_NAME.search(e["tags"].get("name") or "")]
    office = named[0] if len(named) == 1 else halls[0] if len(halls) == 1 else None
    if office:
        return office, "office"
    nodes = [e for e in placed if e["tags"].get("place") and e["tags"].get("name") == name]
    return (nodes[0], "centre") if len(nodes) == 1 else (None, "")


def plan(wanted: list[dict], known: set[str], search: Callable[[str], dict]) -> list[dict]:
    """towns.csv rows for each wanted {name, region, site} not yet known; `site` set only for a source's own town."""
    todo: dict[str, dict] = {}
    for w in wanted:
        if w["name"] in PINNED or NOT_A_TOWN.search(w["name"]) or generate.slug(w["name"]) in known:
            continue
        todo.setdefault(w["name"], w)
    inside: dict[tuple[str, str], list] = {}
    for region in sorted({w["region"] for w in todo.values()}):
        names = [n for n, w in todo.items() if w["region"] == region]
        for name, found in offices.grouped(search(query(names, generate.REGIONS[region][1]))).items():
            inside[(region, name)] = found
    rows = []
    for name, w in sorted(todo.items()):
        candidates = inside.get((w["region"], name), [])
        unique = len(candidates) == 1
        if not unique and w.get("site"):
            candidates = [c for c in candidates if offices.host(c[0].get("website", "")) == offices.host(w["site"])]
        if len(candidates) != 1:
            print(f"- {name}: {len(inside.get((w['region'], name), []))} municipalities of that name in {w['region']}, none told apart",
                  file=sys.stderr)
            continue
        element, precision = pick(candidates[0][1], name)
        print(f"{'+' if element else '-'} {name}: {precision or 'nothing'} "
              f"{(element or {}).get('tags', {}).get('name', '')}", file=sys.stderr)
        if element:
            lat, lon = offices.point(element)
            rows.append({
                "town": generate.slug(name), "name": name, "region": w["region"], "unique": "y" if unique else "n",
                "lat": f"{lat:.7f}", "lon": f"{lon:.7f}", "precision": precision,
                "osm": f"{element['type']}/{element['id']}",
                "checked_on": datetime.date.today().isoformat(), "checked_by": "overpass",
            })
    return rows


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--write", action="store_true", help="append the rows to towns.csv")
    parser.add_argument("--localities", type=Path, help="town,region rows the events wrote (report.py --localities)")
    args = parser.parse_args(argv)
    wanted = [
        {"name": r["town"], "region": r["region"], "site": r["events_url"] if r["org_type"] in MUNICIPAL else ""}
        for r in generate.kept_sources() if r["town_slug"]
    ]
    if args.localities:
        wanted += [{"name": r["town"].strip(), "region": r["region"].strip(), "site": ""}
                   for r in csv.DictReader(args.localities.open(encoding="utf-8")) if r["town"].strip()]
    known = {t["town"] for t in csv.DictReader(TOWNS.open(encoding="utf-8"))}
    def polite(text: str) -> dict:
        time.sleep(PAUSE)
        return offices.fetch(text)

    rows = plan(wanted, known, polite)
    if not args.write:
        csv.DictWriter(sys.stdout, fieldnames=FIELDS, lineterminator="\n").writerows(rows)
        return 0
    with TOWNS.open("a", encoding="utf-8", newline="") as out:
        csv.DictWriter(out, fieldnames=FIELDS, lineterminator="\n").writerows(rows)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
