#!/usr/bin/env python3
"""Place each municipal source's own office and its one culture house into venues.csv and homes.csv (T-5215).

For every kept inventory row of a town, municipality or village with no homes.csv row, one Overpass query finds
the municipality's boundary by name within Slovakia (several of one name: the one whose OSM `website` is the
source's host) and asks OpenStreetMap for the `amenity=townhall` and
`amenity=community_centre` objects inside it. The office becomes the source's home, the point of an event
that names no place or only the municipality (owner 2026-10-22: "or the institution's address"; the village
office counts, T-5135). A community centre becomes the row for "Kultúrny dom" and its usual spellings only when
it is the municipality's single one. Nothing is a town's centre: a municipality without an office in OSM gets no
row. One request with the inventory User-Agent; an answer over 4 MB is refused.

    python3 seeds/bbsk-events/offices.py           # print what it would add, and every OSM name it read
    python3 seeds/bbsk-events/offices.py --write   # append the rows to venues.csv and homes.csv
    python3 seeds/bbsk-events/offices.py --places places.csv --write   # also each hall text the events wrote

Rows it adds say `overpass` in `checked_by`; read each printed OSM name before --write.
"""

from __future__ import annotations

import argparse
import csv
import datetime
import json
import re
import sys
import urllib.parse
import urllib.request
from collections.abc import Callable
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import generate

OVERPASS = "https://overpass-api.de/api/interpreter"
UA = "joinedcontext-platform-events-inventory/1.0 (+https://dev.joinedcontext.com)"
MAX_BYTES = 4 << 20
MUNICIPAL = {"town", "municipality", "village"}
FIELDS = ["town", "match", "regional", "venue", "streetAddress", "addressLocality", "lat", "lon", "osm", "checked_on", "checked_by"]
#: How a village's calendar names its office or the village itself, beside "Obec X" (generate.py SELF).
OFFICE = re.compile(r"(?i)(obecn|mestsk|miestn)\w* úrad|^obec\b|^mesto\b|^úrad\b|\b(ocú|ocu|oú|msú|mú)\b")
#: A culture house by its name: a church centre, a retreat or a spa hall is none (read 2026-10-22: Marianka's
#: "Exercičný dom", Špania Dolina's "Diecézne centrum mládeže", Nimnica's "Kursalon").
HALL = re.compile(r"(?i)^(kult[uú]rny dom|spolo[cč]ensk[ýy] dom|dom kult[uú]ry|(mestsk|miestn)\w* kult[uú]rn\w* stredisko|ms?ks|mskc)\b(?P<rest>.*)$")
#: How a calendar names the culture house; matched only when the municipality has exactly one.
HALL_TEXTS = ("kultúrny dom", "kultúrny dom {town}", "kultúrny dom v {town}", "obecný kultúrny dom",
              "dom kultúry", "spoločenský dom", "sála kultúrneho domu", "kultúrne stredisko", "miestne kultúrne stredisko")


def query(towns: list[str]) -> str:
    """One Overpass query: each municipality boundary of those names in Slovakia's box, then the offices and culture
    houses inside it."""
    names = "|".join(re.escape(t) for t in sorted(set(towns)))
    return (
        "[out:json][timeout:600];"
        f'rel["boundary"="administrative"]["admin_level"="8"]["name"~{json.dumps("^(" + names + ")$", ensure_ascii=False)}]'
        "(47.7,16.8,49.7,22.6)->.m;"
        'foreach.m->.one(.one out ids tags;.one map_to_area->.a;'
        '(nwr(area.a)["amenity"~"^(townhall|community_centre)$"];'
        'nwr(area.a)["name"~"^(Kult[uú]rny dom|Dom kult[uú]ry|Spolo[cč]ensk[ýy] dom)",i];);out center tags;);'
    )


def get(request: urllib.request.Request, timeout: int):
    with urllib.request.urlopen(request, timeout=timeout) as answer:
        body = answer.read(MAX_BYTES + 1)
    if len(body) > MAX_BYTES:
        raise ValueError(f"answer over {MAX_BYTES} bytes")
    return json.loads(body)


def fetch(text: str) -> dict:
    data = urllib.parse.urlencode({"data": text}).encode()
    return get(urllib.request.Request(OVERPASS, data=data, headers={"User-Agent": UA, "Accept": "application/json"}), 960)


def grouped(answer: dict) -> dict[str, list[tuple[dict, list[dict]]]]:
    """Per municipality name each boundary of that name (its tags) with the elements inside it."""
    out: dict[str, list[tuple[dict, list[dict]]]] = {}
    current = None
    for element in answer.get("elements") or []:
        if element["type"] == "relation":
            tags = element.get("tags", {})
            current = []
            out.setdefault(tags.get("name", ""), []).append((tags, current))
        elif current is not None:
            current.append(element)
    return out


def host(url: str) -> str:
    return (urllib.parse.urlsplit(url).hostname or "").removeprefix("www.")


def point(element: dict) -> tuple[float, float] | None:
    at = element.get("center") or element
    lat, lon = at.get("lat"), at.get("lon")
    if lat is None or lon is None or not (47.7 <= lat <= 49.7 and 16.8 <= lon <= 22.6):
        return None
    return lat, lon


def street(tags: dict) -> str:
    road = tags.get("addr:street") or tags.get("addr:place") or ""
    number = tags.get("addr:housenumber") or tags.get("addr:conscriptionnumber") or ""
    return f"{road} {number}".strip()


def chosen(elements: list[dict], town: str) -> tuple[dict | None, dict | None, str]:
    """The office and the single culture house among one municipality's elements, and every name read."""
    placed = [e for e in elements if point(e)]
    # A community centre, or any object named as a culture house of this town (a building OSM tags only by its name).
    halls = list({e["id"]: e for e in placed if e["tags"].get("amenity") != "townhall" and own_hall(e, town)}.values())
    offices = [e for e in placed if e["tags"].get("amenity") == "townhall"]
    named = [e for e in offices if OFFICE.search(e["tags"].get("name") or "")]
    office = named[0] if len(named) == 1 else offices[0] if len(offices) == 1 else None
    names = "; ".join(f"{e['tags'].get('amenity')}={e['tags'].get('name', '(no name)')}" for e in placed) or "nothing"
    return office, halls[0] if len(halls) == 1 else None, names


def own_hall(element: dict, town: str) -> bool:
    """A culture house whose name says so and names, if anything more, its own town: "Kultúrny dom Ponická
    Lehôtka" is a village part's, not Poniky's."""
    named = HALL.match(element["tags"].get("name") or "")
    if not named:
        return False
    rest = generate.slug(named.group("rest"))
    return not rest.strip("-") or generate.slug(town)[:-1] in rest


def row(element: dict, town: str, match: str, venue: str) -> dict:
    lat, lon = point(element)
    return {
        "town": generate.slug(town), "match": match, "regional": "n", "venue": venue,
        "streetAddress": street(element["tags"]), "addressLocality": town,
        "lat": f"{lat:.7f}", "lon": f"{lon:.7f}", "osm": f"{element['type']}/{element['id']}",
        "checked_on": datetime.date.today().isoformat(), "checked_by": "overpass",
    }


def plan(rows: list[dict], known: set, homes: set, search: Callable[[str], dict],
         places: dict[str, set[str]] | None = None) -> tuple[list, list]:
    """The venues.csv rows of each municipal source's office and culture house, and the homes.csv rows of those
    without a home. `places` holds, per town, the place texts its events wrote (report.py --places): each one that
    names the town's culture house gets a row too ("Kultúrny dom v Žarnovici")."""
    wanted = [r for r in rows if r["org_type"] in MUNICIPAL]
    inside = grouped(search(query([r["town"] for r in wanted]))) if wanted else {}
    venues, new_homes = [], []
    for source in wanted:
        town, slug = source["town"], generate.slug(source["town"])
        # Slovakia has several villages of one name: then only the one whose OSM web site is the source's own counts.
        candidates = inside.get(town, [])
        if len(candidates) > 1:
            candidates = [c for c in candidates if host(c[0].get("website", "")) == host(source["events_url"])]
        if len(candidates) != 1:
            print(f"- {town}: {len(inside.get(town, []))} municipalities of that name, none told apart", file=sys.stderr)
            continue
        office, hall, names = chosen(candidates[0][1], town)
        print(f"{'+' if office else '-'} {town}: {names}", file=sys.stderr)
        if office:
            match = generate.key(f"Mestský úrad {town}" if source["org_type"] == "town" else f"Obecný úrad {town}")
            if (slug, match) not in known:
                venues.append(row(office, town, match, office["tags"].get("name") or match.capitalize()))
                known.add((slug, match))
            if source["events_url"] not in homes:
                new_homes.append({"events_url": source["events_url"], "town": slug, "match": match})
        if hall:
            written = [t for t in (places or {}).get(town, ()) if own_hall({"tags": {"name": t}}, town)]
            for text in [*HALL_TEXTS, *written]:
                match = generate.key(text.format(town=town))
                if (slug, match) not in known:
                    venues.append(row(hall, town, match, hall["tags"].get("name") or "Kultúrny dom"))
                    known.add((slug, match))
    return venues, new_homes


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--write", action="store_true", help="append the rows to venues.csv and homes.csv")
    parser.add_argument("--places", type=Path, help="report.py --places output: town,place rows the events wrote")
    args = parser.parse_args(argv)
    places: dict[str, set[str]] = {}
    if args.places:
        for item in csv.DictReader(args.places.open(encoding="utf-8")):
            places.setdefault(item["town"].strip(), set()).add(item["place"].strip())
    known = {(v["town"], v["match"]) for v in csv.DictReader((HERE / "venues.csv").open(encoding="utf-8"))}
    homes = {h["events_url"] for h in csv.DictReader((HERE / "homes.csv").open(encoding="utf-8"))}
    venues, new_homes = plan(generate.kept_sources(), known, homes, fetch, places)
    if not args.write:
        csv.DictWriter(sys.stdout, fieldnames=FIELDS, lineterminator="\n").writerows(venues)
        return 0
    with (HERE / "venues.csv").open("a", encoding="utf-8", newline="") as out:
        csv.DictWriter(out, fieldnames=FIELDS, lineterminator="\n").writerows(venues)
    with (HERE / "homes.csv").open("a", encoding="utf-8", newline="") as out:
        csv.DictWriter(out, fieldnames=["events_url", "town", "match"], lineterminator="\n").writerows(new_homes)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
