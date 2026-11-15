#!/usr/bin/env python3
"""Place the Bratislava hub's address-only tables of places from the national address register (T-5444).

The places pipeline (components/context-gateway/seed/bratislava/bratislava-pipeline-places-bento.yaml) maps
tables whose rows carry an address and no coordinates (`"geocoded": true`): the city's contact points,
schools, kindergartens, art schools, leisure centres and cooling places. This script reads each such table
from the hub as the pipeline does, looks every address up in Register adries (ra.synchronix.sk, the address
register of MV SR, no key), and writes the hits into positions.csv beside it and into the mapping's
`let positions = {...}`, keyed as the mapping keys them: the first 16 hex of sha256 of "<street>|<district>"
in lower case. A hit counts only in Bratislava (or one of its boroughs), on the same street with the same
number (geocode-venues.py's same_address); several hits at different points are refused unless one of them
lies in the row's district. An address the register does not place is printed and stays out: the pipeline
drops its row as "no position" and counts it. One request a second.

    python3 seeds/bratislava-places/geocode.py          # print what it would add and what does not resolve
    python3 seeds/bratislava-places/geocode.py --write  # write positions.csv, not-placed.csv and the mapping

The pipelines never call a geocoder.
"""

from __future__ import annotations

import argparse
import csv
import datetime
import hashlib
import importlib.util
import json
import re
import sys
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
MAPPING = ROOT / "components/context-gateway/seed/bratislava/bratislava-pipeline-places-bento.yaml"
POSITIONS = HERE / "positions.csv"
NOT_PLACED = HERE / "not-placed.csv"
HUB = "https://services8.arcgis.com/pRlN1m0su5BYaFAS/arcgis/rest/services/"
FIELDS = ["key", "street", "district", "lat", "lon", "source", "checked_on"]
#: Bratislava and 1 km around it, the box the mapping keeps.
LAT = (47.99, 48.28)
LON = (16.93, 17.3)
BLOCK = re.compile(r"(\n        let positions = )\{.*?\}\n(        let rows = )", re.DOTALL)

_spec = importlib.util.spec_from_file_location("geocode_venues", ROOT / "seeds/bbsk-events/geocode-venues.py")
venues = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(venues)


def text(value) -> str:
    """The mapping's `text`: Bloblang's \\s is ASCII whitespace."""
    return "" if value is None else re.sub(r"[\t\n\f\r ]+", " ", str(value)).strip()


def address_key(street: str, district: str) -> str:
    return hashlib.sha256(f"{street}|{district}".lower().encode()).hexdigest()[:16]


def tables(mapping: str) -> list[dict]:
    start = mapping.index("let tables = [") + len("let tables = ")
    return [t for t in json.loads(mapping[start : mapping.index("\n        ]", start) + 10]) if t.get("geocoded")]


def addresses(table: dict, rows: list[dict]) -> list[tuple[str, str]]:
    found = []
    for row in rows:
        street = " ".join(s for s in (text(row.get(table["street"])), text(row.get(table.get("number", "-")))) if s)
        district = text(row.get(table.get("district", "-")))
        if street:
            found.append((street, district))
    return found


def read_table(table: dict) -> list[dict]:
    url = HUB + table["path"] + f"/FeatureServer/{table.get('layer', 0)}/query?where=1%3D1&outFields=*&resultRecordCount=2000&f=json"
    answer = venues.get(urllib.request.Request(url, headers={"User-Agent": venues.UA, "Accept": "application/json"}))
    if answer.get("error") or answer.get("exceededTransferLimit"):
        raise SystemExit(f"{table['key']}: the hub answered {answer.get('error') or 'more than 2,000 rows'}")
    return [f["attributes"] for f in answer.get("features", [])]


def located(hit: dict) -> dict:
    """Some register hits carry the point only as GeoJSON geometry, not as `location`."""
    at = (hit.get("geometry") or {}).get("coordinates") or []
    if hit.get("location") or len(at) != 2:
        return hit
    return {**hit, "location": {"lat": at[1], "lon": at[0]}}


def place(street: str, district: str, register) -> tuple[dict | None, str]:
    """The register's one point for the address, or None and why not."""
    # "6/A" is the orientačné number 6A; "Slovnaftská ulica 5" the register's "Slovnaftská 5".
    part = re.sub(r"(\d)/([a-zA-Z])$", r"\1\2", street.split(",")[0].strip())
    part = re.sub(r"\s+ulica\b", "", part, flags=re.IGNORECASE)
    answer = [located(h) for h in register(f"{part}, Bratislava")]
    hits = [h for h in answer if venues.same_address(h, part, "Bratislava")]
    pointless = [h for h in answer if venues.same_address({**h, "location": {"lat": 0, "lon": 0}}, part, "Bratislava")]
    points = {}
    for hit in hits:
        at = hit.get("location") or {}
        lat, lon = at.get("lat"), at.get("lon")
        if lat is None or lon is None or not (LAT[0] <= lat <= LAT[1] and LON[0] <= lon <= LON[1]):
            continue
        points.setdefault((round(lat, 6), round(lon, 6)), hit)
    if len(points) > 1 and district:
        borough = venues.slug(district.removeprefix("Bratislava-"))
        points = {p: h for p, h in points.items() if venues.slug(h.get("municipality") or "").endswith(borough)}
    if not points:
        if not hits:
            return None, "in the register without a point" if pointless else "no such address in Bratislava"
        return None, "outside Bratislava"
    if len(points) > 1:
        return None, f"{len(points)} points in {sorted({h.get('municipality') for h in points.values()})}"
    (lat, lon), hit = next(iter(points.items()))
    return {"lat": f"{lat:.6f}", "lon": f"{lon:.6f}", "source": hit.get("source") or ""}, ""


def block(rows: list[dict]) -> str:
    lines = [f'          "{r["key"]}": {{ "lat": {r["lat"]}, "lon": {r["lon"]} }}' for r in sorted(rows, key=lambda r: r["key"])]
    return "{\n" + ",\n".join(lines) + "\n        }" if lines else "{}"


def write_mapping(mapping: str, rows: list[dict]) -> str:
    replaced, count = BLOCK.subn(lambda m: m.group(1) + block(rows) + "\n" + m.group(2), mapping)
    if count != 1:
        raise SystemExit("the mapping holds no `let positions = {...}` before `let rows`")
    return replaced


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--write", action="store_true", help="write positions.csv, not-placed.csv and the mapping")
    args = parser.parse_args(argv)
    known = {r["key"]: r for r in csv.DictReader(POSITIONS.open())} if POSITIONS.exists() else {}
    mapping = MAPPING.read_text()
    wanted, missing = {}, []
    today = datetime.date.today().isoformat()

    def register(query: str) -> list[dict]:
        time.sleep(venues.PAUSE)
        return venues.register(query)

    for table in tables(mapping):
        for street, district in addresses(table, read_table(table)):
            key = address_key(street, district)
            if key in wanted:
                continue
            if key in known:
                wanted[key] = known[key]
                continue
            hit, why = place(street, district, register)
            if hit is None:
                missing.append((table["key"], street, district, why))
                continue
            wanted[key] = {"key": key, "street": street, "district": district, **hit, "checked_on": today}
            print(f"{table['key']}\t{street}\t{district}\t{hit['lat']} {hit['lon']}")
    for row in missing:
        print("not placed\t" + "\t".join(row))
    print(f"{len(wanted)} addresses placed, {len(missing)} not", file=sys.stderr)
    if args.write:
        rows = sorted(wanted.values(), key=lambda r: r["key"])
        with POSITIONS.open("w", newline="") as out:
            writer = csv.DictWriter(out, fieldnames=FIELDS, lineterminator="\n")
            writer.writeheader()
            writer.writerows({f: r[f] for f in FIELDS} for r in rows)
        with NOT_PLACED.open("w", newline="") as out:
            writer = csv.writer(out, lineterminator="\n")
            writer.writerow(["table", "street", "district", "reason"])
            writer.writerows(sorted(set(missing)))
        MAPPING.write_text(write_mapping(mapping, rows))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
