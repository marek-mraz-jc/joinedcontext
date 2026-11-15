#!/usr/bin/env python3
"""Geocode the venues the event sources name into venues.csv, at inventory time (T-5083, T-5240).

Input: a CSV with the columns `town,place` and an optional `street`, one row per venue text as a source writes it
and the street address its events give beside it (report.py --places).
Each place not yet in venues.csv for its town is looked up in this order, one request a second per host with the
inventory User-Agent:

1. Register adries (ra.synchronix.sk, the official address register of MV SR) for each part of the text holding
   a street and a number. A hit counts only in the same municipality (or one of its boroughs), on the same street,
   with the same orientačné number, or the same súpisné number where the address has no orientačné one (the
   geocode pilot of 2026-10-22: 17 of 20, none false). Its rows say `register-adries` and name the register's
   address id in `osm` (`ra/<id>`).
2. A name match on OpenStreetMap Nominatim: a third of these were another building in the pilot, so a hit is
   written only when a person has opened it and listed its `town,match,osm` in reviewed.csv. Every other hit is
   printed as a line to review, never written.

A town, a village, a district or a street is no venue: an event at a place neither tier places falls back to its
town's office or centre (towns.csv), labelled approximate.

    python3 seeds/bbsk-events/geocode-venues.py places.csv          # print the rows it would add
    python3 seeds/bbsk-events/geocode-venues.py places.csv --write  # append them to venues.csv

The pipelines never call a geocoder: generate.py reads venues.csv into each source's mapping.
"""

from __future__ import annotations

import argparse
import csv
import datetime
import json
import re
import sys
import time
import urllib.parse
import urllib.request
from collections.abc import Callable
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import generate

VENUES = HERE / "venues.csv"
REVIEWED = HERE / "reviewed.csv"
SEARCH = "https://nominatim.openstreetmap.org/search"
REGISTER = "https://ra.synchronix.sk/geocoder/api/v1/search"
UA = "joinedcontext-platform-events-inventory/1.0 (+https://dev.joinedcontext.com)"
#: An answer bigger than this is no Nominatim answer of one hit.
MAX_BYTES = 1 << 20
#: Nominatim's usage policy: at most one request a second; the register gets the same.
PAUSE = 1.0
#: Areas and lines: a hit of one of these is no venue.
AREAS = {
    "city", "town", "village", "hamlet", "municipality", "suburb", "quarter", "neighbourhood", "borough",
    "city_district", "district", "county", "state", "region", "country", "postcode", "locality", "road",
    "isolated_dwelling", "farm", "allotments", "square",
}
#: A `place` is a settlement or a part of one (cadastral_community is the village itself); `information` a map board.
REFUSED_CATEGORIES = {"boundary", "highway", "landuse", "natural", "waterway", "railway", "route", "place", "information"}
#: Slovakia's bounding box, the one the events' common mapping keeps too.
LAT = (47.7, 49.7)
LON = (16.8, 22.6)
FIELDS = ["town", "match", "regional", "venue", "streetAddress", "addressLocality", "lat", "lon", "osm", "checked_on", "checked_by"]


slug = generate.slug
key = generate.key


def queries(place: str, town: str) -> list[str]:
    """The whole text, then each part that holds a house number, then the first part: each with the town."""
    parts = [p.strip() for p in re.sub(r"\([^)]*\)", " ", place).split(",") if p.strip()]
    numbered = [p for p in parts if re.search(r"\D \d+[a-zA-Z]?(/\d+[a-zA-Z]?)?$", p)]
    out = []
    for q in [", ".join(parts), *numbered, *parts[:1]]:
        q = f"{q}, {town}"
        if q not in out:
            out.append(q)
    return out


def locality(address: dict) -> str | None:
    for k in ("city", "town", "village", "municipality"):
        if address.get(k):
            return address[k]
    return None


def accepted(hit: dict, town: str) -> str | None:
    """None when the hit is a venue in the town, else why not."""
    category, kind = hit.get("category") or hit.get("class"), hit.get("type")
    if category in REFUSED_CATEGORIES or kind in AREAS or hit.get("addresstype") in AREAS:
        return f"{category}/{kind} is an area or a line, no venue"
    if (hit.get("name") or "").casefold() == town.casefold():
        return f"{hit.get('name')} is the town itself, no venue"
    found = locality(hit.get("address") or {})
    if not found or found.casefold() != town.casefold():
        return f"in {found or 'no town'}, not in {town}"
    lat, lon = float(hit["lat"]), float(hit["lon"])
    if not (LAT[0] <= lat <= LAT[1] and LON[0] <= lon <= LON[1]):
        return "outside Slovakia"
    return None


def get(request: urllib.request.Request):
    with urllib.request.urlopen(request, timeout=20) as answer:
        body = answer.read(MAX_BYTES + 1)
    if len(body) > MAX_BYTES:
        raise ValueError(f"answer over {MAX_BYTES} bytes")
    return json.loads(body)


def fetch(query: str) -> list[dict]:
    url = SEARCH + "?" + urllib.parse.urlencode(
        {"q": query, "format": "jsonv2", "addressdetails": 1, "limit": 3, "countrycodes": "sk"}
    )
    return get(urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "application/json"}))


def register(text: str) -> list[dict]:
    """Register adries' address hits for a free text ("Titogradská 17, Košice")."""
    body = json.dumps({"text": text, "categories": ["adresa"], "size": 5}, ensure_ascii=False).encode()
    request = urllib.request.Request(REGISTER, data=body, headers={
        "User-Agent": UA, "Accept": "application/json", "Content-Type": "application/json"})
    return get(request).get("items") or []


def words(text: str) -> list[str]:
    return [w for w in slug(text).split("-") if w and not w.isdigit()]


def numbers(text: str) -> tuple[str, str] | None:
    """The house number at the end of an address part: (súpisné, orientačné), either one possibly empty."""
    found = re.search(r"(\d+[a-zA-Z]?)(?:/(\d+[a-zA-Z]?))?\s*$", text)
    if not found:
        return None
    return (found.group(1), found.group(2)) if found.group(2) else ("", found.group(1))


def same_address(item: dict, address: str, town: str) -> bool:
    """The register's hit is the address: the town or one of its boroughs ("Košice-Sídlisko KVP"), every word of the
    street ("Nám." for "Námestie"), and the number. A lone number matches the orientačné number, or the súpisné one
    where the register has no orientačné (a village's "Bíňovce 134"); "Hlavná 59" is never "Hlavná 59/112"."""
    municipality = slug(item.get("municipality") or "")
    if not (municipality == slug(town) or municipality.startswith(slug(town) + "-")):
        return False
    label = (item.get("text") or "").split(",")[0]
    ours, theirs = numbers(address), numbers(label)
    if not ours or not theirs or not item.get("location"):
        return False
    if (ours != theirs) if ours[0] else (ours[1] != theirs[1]):
        return False
    named = words(label)
    return all(any(n.startswith(w) for n in named) for w in words(re.sub(r"\d.*$", "", address)))


def located(item: dict) -> tuple[float, float] | None:
    at = item.get("location") or {}
    lat, lon = at.get("lat"), at.get("lon")
    if lat is None or lon is None or not (LAT[0] <= lat <= LAT[1] and LON[0] <= lon <= LON[1]):
        return None
    return lat, lon


def row(town: str, place: str, street: str, lat: float, lon: float, ref: str, by: str) -> dict:
    return {
        "town": slug(town), "match": key(place), "regional": "n", "venue": place.strip(), "streetAddress": street,
        "addressLocality": town, "lat": f"{lat:.7f}", "lon": f"{lon:.7f}", "osm": ref,
        "checked_on": datetime.date.today().isoformat(), "checked_by": by,
    }


def geocode(town: str, place: str, search: Callable[[str], list[dict]], pause: Callable[[], None],
            lookup: Callable[[str], list[dict]] = lambda _q: [], reviewed: frozenset = frozenset(),
            street: str = "") -> tuple[dict | None, str]:
    """The venues.csv row for the place, or None and why none: the register by address first (the place's own
    parts, or the street the event wrote beside it), then a name hit a person accepted in reviewed.csv."""
    why = "no hit"
    parts = [p.strip() for p in re.sub(r"\([^)]*\)", " ", f"{place}, {street}").split(",") if p.strip()]
    for address in [p for p in parts if re.search(r"\D \d+[a-zA-Z]?(/\d+[a-zA-Z]?)?$", p)]:
        pause()
        for item in lookup(f"{address}, {town}"):
            point = located(item)
            if point and same_address(item, address, town):
                ident = (item.get("source") or "").rstrip("/").rsplit("/", 1)[-1]
                return row(town, place, (item.get("text") or "").split(",")[0], *point, f"ra/{ident}", "register-adries"), address
    for query in queries(place, town):
        pause()
        for hit in search(query):
            why = accepted(hit, town) or ""
            if why:
                continue
            ref = f"{hit['osm_type']}/{hit['osm_id']}"
            if (slug(town), key(place), ref) not in reviewed:
                why = f"unreviewed: {slug(town)},{key(place)},{ref} ({hit.get('name') or hit.get('display_name', '')})"
                continue
            address = hit.get("address") or {}
            street = " ".join(v for v in (address.get("road"), address.get("house_number")) if v)
            return row(town, place, street, float(hit["lat"]), float(hit["lon"]), ref, "nominatim"), query
    return None, why
def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("places", type=Path, help="CSV with the columns town,place")
    parser.add_argument("--write", action="store_true", help="append the rows to venues.csv")
    args = parser.parse_args(argv)
    known = {(v["town"], v["match"]) for v in csv.DictReader(VENUES.open(encoding="utf-8"))}
    reviewed = frozenset((r["town"], r["match"], r["osm"]) for r in csv.DictReader(REVIEWED.open(encoding="utf-8")))
    rows, last = [], [0.0]

    def pause() -> None:
        wait = last[0] + PAUSE - time.monotonic()
        if wait > 0:
            time.sleep(wait)
        last[0] = time.monotonic()

    for item in csv.DictReader(args.places.open(encoding="utf-8")):
        town, place = item["town"].strip(), item["place"].strip()
        if not town or not place or (slug(town), key(place)) in known:
            continue
        row, note = geocode(town, place, fetch, pause, register, reviewed, (item.get("street") or "").strip())
        print(f"{'+' if row else '-'} {town} | {place} | {note}", file=sys.stderr)
        if row:
            known.add((row["town"], row["match"]))
            rows.append(row)
    if args.write and rows:
        with VENUES.open("a", encoding="utf-8", newline="") as out:
            csv.DictWriter(out, fieldnames=FIELDS, lineterminator="\n").writerows(rows)
    else:
        csv.DictWriter(sys.stdout, fieldnames=FIELDS, lineterminator="\n").writerows(rows)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
