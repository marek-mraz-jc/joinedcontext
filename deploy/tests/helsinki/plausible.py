"""Do the Helsinki entities on an instance make sense (T-5259, owner 2026-10-22T18:10Z)?

The Smart Data Models check (validate_sdm.py) proves the shape; this one reads every entity of each
type from a public Endpoint (`options=keyValues,sysAttrs&lang=fi`, paged) and counts what a shape
cannot catch:

- location: a point or vertex outside the Helsinki region box (HSL's area plus 1 km), at 0,0, or
  inside the box only with longitude and latitude swapped;
- values: a number outside the physical range of its property (RANGES), a count below zero, an
  available number above its capacity (CAPACITY);
- dates: an observation or modification stamp more than a day in the future;
- identity: two entities of one type with the same name (and IDENTITY fields) within 20 m, and a `ref*` Relationship
  whose target type was read but holds no entity of that id;
- freshness: entities the broker has not modified for seven days, and the newest modifiedAt.

    python3 tests/helsinki/plausible.py \\
        https://api.dev.joinedcontext.com/api/endpoint/si6epqkx364lprho5uaigutk274r5grb

Only reads, anonymously: it sends no token. Prints one Markdown row per type and exits 1 when a
location, value, date, duplicate or reference check fails; staleness is reported, not judged,
because a beach sensor out of season and a weekly register are stale by design.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
import urllib.parse
from collections import Counter
from collections.abc import Iterator
from datetime import UTC, datetime, timedelta
from typing import Any

sys.path.insert(0, __file__.rsplit("/", 1)[0])
from validate_sdm import MODELS, fetch

#: What the capital-region feeds cover, plus 1 km: HSL's area (Helsinki, Espoo, Vantaa, Kauniainen,
#: Kerava, Kirkkonummi, Sipoo, Siuntio, Tuusula) and the ends of the trains and regional buses its
#: GTFS lists (Riihimäki, Lahti, Porvoo), as (west, south, east, north) in WGS 84 degrees. Helsinki's
#: own registers sit well inside it; 0,0, a swapped pair and another city do not.
REGION = (23.90, 59.85, 25.90, 61.05)

#: Two entities of one name within 20 m are one real object only when these also agree: a stop's
#: platforms share a name and a kerb, a venue's events a name and a door, a line's variants a name.
IDENTITY = {
    "GtfsStop": ("stopCode",),
    "Event": ("startDate",),
    "PublicTransportRoute": ("headsign", "directionId"),
}

#: Property -> (lowest, highest) as SDM names its unit: degrees Celsius, metres a second, km/h,
#: µg/m³, millimetres, a 0..1 ratio for relative humidity.
RANGES = {
    "temperature": (-40, 50),
    "roadSurfaceTemperature": (-40, 50),
    "dewPoint": (-50, 40),
    "relativeHumidity": (0, 1),
    "windSpeed": (0, 75),
    "windDirection": (-180, 360),
    "precipitation": (0, 500),
    "speed": (0, 150),
    "pm10": (0, 1000),
    "pm25": (0, 1000),
    "airQualityIndex": (0, 10),
    "availableSpotNumber": (0, math.inf),
    "totalSpotNumber": (0, math.inf),
    "occupiedSpotNumber": (0, math.inf),
    "availableBikeNumber": (0, math.inf),
    "freeSlotNumber": (0, math.inf),
    "totalSlotNumber": (0, math.inf),
    "capacity": (0, math.inf),
}

#: (available, capacity): the first may never exceed the second.
CAPACITY = [
    ("availableSpotNumber", "totalSpotNumber"),
    ("occupiedSpotNumber", "totalSpotNumber"),
    ("availableBikeNumber", "totalSlotNumber"),
    ("freeSlotNumber", "totalSlotNumber"),
]

#: Stamps of something that already happened; an Event's startDate is meant to lie ahead.
PAST = ("dateObserved", "dateModified", "dateIssued", "dateCreated", "observedAt", "modifiedAt", "createdAt")


def points(geometry: Any) -> Iterator[tuple[float, float]]:
    """Every (longitude, latitude) of a GeoJSON geometry, at any nesting."""
    if not isinstance(geometry, dict):
        return
    stack = [geometry.get("coordinates")]
    while stack:
        item = stack.pop()
        if isinstance(item, list) and len(item) >= 2 and all(isinstance(c, (int, float)) for c in item[:2]):
            yield float(item[0]), float(item[1])
        elif isinstance(item, list):
            stack.extend(item)


def place(lon: float, lat: float, box: tuple[float, float, float, float] = REGION) -> str | None:
    """None inside the box, else why not: `zero`, `swapped` or `outside`."""
    west, south, east, north = box
    if lon == 0 and lat == 0:
        return "zero"
    if west <= lon <= east and south <= lat <= north:
        return None
    return "swapped" if west <= lat <= east and south <= lon <= north else "outside"


def metres(a: tuple[float, float], b: tuple[float, float]) -> float:
    """Haversine distance of two (lon, lat) points."""
    lon1, lat1, lon2, lat2 = map(math.radians, (*a, *b))
    h = math.sin((lat2 - lat1) / 2) ** 2 + math.cos(lat1) * math.cos(lat2) * math.sin((lon2 - lon1) / 2) ** 2
    return 2 * 6_371_000 * math.asin(math.sqrt(h))


def stamp(value: Any) -> datetime | None:
    if not isinstance(value, str) or len(value) < 10:
        return None
    try:
        moment = datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    return moment if moment.tzinfo else moment.replace(tzinfo=UTC)


def check(kind: str, entities: list[dict[str, Any]], known: dict[str, set[str]], now: datetime,
          box: tuple[float, float, float, float] = REGION, identity: dict[str, tuple[str, ...]] = IDENTITY) -> Counter:
    """(check, detail) -> how many entities of one type fail it. `known` maps each type read to its
    ids, so a reference into a type nobody read is not counted as dangling; `box` and `identity` are
    another city's area and same-object fields (tests/praha/plausible.py)."""
    found: Counter = Counter()
    named: dict[tuple, list[tuple[float, float]]] = {}
    for entity in entities:
        for lon, lat in points(entity.get("location")):
            reason = place(lon, lat, box)
            if reason:
                found[("location", reason)] += 1
                break
        for attribute, (low, high) in RANGES.items():
            value = entity.get(attribute)
            if isinstance(value, (int, float)) and not isinstance(value, bool) and not low <= value <= high:
                found[("range", attribute)] += 1
        for part, whole in CAPACITY:
            a, b = entity.get(part), entity.get(whole)
            if isinstance(a, (int, float)) and isinstance(b, (int, float)) and a > b:
                found[("capacity", f"{part} > {whole}")] += 1
        for attribute in PAST:
            moment = stamp(entity.get(attribute))
            if moment and moment > now + timedelta(days=1):
                found[("future", attribute)] += 1
        for attribute, value in entity.items():
            if not attribute.startswith("ref"):
                continue
            for target in value if isinstance(value, list) else [value]:
                if not isinstance(target, str) or not target.startswith("urn:ngsi-ld:"):
                    continue
                into = target.split(":")[2]
                if into in known and target not in known[into]:
                    found[("dangling", attribute)] += 1
        name = entity.get("name")
        first = next(points(entity.get("location")), None)
        if isinstance(name, str) and first and (entity.get("location") or {}).get("type") == "Point":
            key = (name, *(json.dumps(entity.get(a), sort_keys=True) for a in identity.get(kind, ())))
            if any(metres(first, other) <= 20 for other in named.get(key, [])):
                found[("duplicate", "same name within 20 m")] += 1
            named.setdefault(key, []).append(first)
    return found


def read_all(endpoint: str, kind: str, cap: int, lang: str = "fi") -> list[dict[str, Any]]:
    out: list[dict[str, Any]] = []
    while len(out) < cap:
        query = urllib.parse.urlencode(
            {"type": kind, "limit": 1000, "offset": len(out), "options": "keyValues,sysAttrs", "lang": lang})
        page = fetch(f"{endpoint.rstrip('/')}/ngsi-ld/v1/entities?{query}")
        out += page
        if len(page) < 1000:
            break
    return out


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("endpoint", help="a public Endpoint that serves the Helsinki types")
    parser.add_argument("--cap", type=int, default=20000, help="most entities read per type")
    parser.add_argument("--type", action="append", help="only these types")
    args = parser.parse_args(argv)
    now = datetime.now(UTC)
    kinds = [k for k in MODELS if not args.type or k in args.type]
    read = {kind: read_all(args.endpoint, kind, args.cap) for kind in kinds}
    known = {kind: {e["id"] for e in rows} for kind, rows in read.items() if rows}
    ok = True
    print("| type | entities | failing checks | stale > 7 d | newest modifiedAt |\n|---|---|---|---|---|")
    for kind, rows in read.items():
        found = check(kind, rows, known, now)
        ok &= not found
        stale = sum(1 for e in rows if (stamp(e.get("modifiedAt")) or now) < now - timedelta(days=7))
        newest = max((e.get("modifiedAt", "") for e in rows), default="—")
        detail = "; ".join(f"{c} {d}: {n}" for (c, d), n in sorted(found.items())) or "—"
        print(f"| {kind} | {len(rows)} | {detail} | {stale} | {newest} |")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
