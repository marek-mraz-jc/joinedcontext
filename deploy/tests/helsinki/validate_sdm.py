"""Do the Helsinki entities on an instance validate against their Smart Data Models (T-3889)?

For every type the Helsinki model maps to a Smart Data Model (`class_uri: sdm:...`), read up to
`--limit` entities from a public Endpoint as key-values in one language (`options=keyValues&lang=fi`,
the shape an SDM `schema.json` describes) and validate each against the model's official schema,
its `$ref`s into the SDM common schema resolved over HTTP and cached for the run. A type the SDM
catalogue has no model for is listed with the reason the Helsinki model keeps its own
(`EXCEPTIONS`). Prints one row per type and every failing attribute with how many entities fail
it, and exits 1 when a type fails.

    python3 tests/helsinki/validate_sdm.py \\
        https://api.dev.joinedcontext.com/api/endpoint/si6epqkx364lprho5uaigutk274r5grb

Only reads, anonymously: it sends no token.
"""

from __future__ import annotations

import argparse
import json
import sys
import urllib.parse
import urllib.request
from collections import Counter
from functools import cache
from pathlib import Path
from typing import Any, Callable

from jsonschema import Draft202012Validator, Draft7Validator
from referencing import Registry, Resource
from referencing.jsonschema import DRAFT7

SDM = "https://raw.githubusercontent.com/smart-data-models/{repo}/master/{model}/schema.json"

#: Helsinki type -> (SDM subject repository, model), from helsinki.linkml.yaml `class_uri` and
#: its `upstream_source`.
MODELS = {
    "AirQualityObserved": ("dataModel.Environment", "AirQualityObserved"),
    "WaterQualityObserved": ("dataModel.WaterQuality", "WaterQualityObserved"),
    "WeatherObserved": ("dataModel.Weather", "WeatherObserved"),
    "Alert": ("dataModel.Alert", "Alert"),
    "BikeHireDockingStation": ("dataModel.Transportation", "BikeHireDockingStation"),
    "Vehicle": ("dataModel.Transportation", "Vehicle"),
    "PointOfInterest": ("dataModel.PointOfInterest", "PointOfInterest"),
    "GtfsStop": ("dataModel.UrbanMobility", "GtfsStop"),
    "GtfsRoute": ("dataModel.UrbanMobility", "GtfsRoute"),
    "GtfsStation": ("dataModel.UrbanMobility", "GtfsStation"),
    "GtfsAgency": ("dataModel.UrbanMobility", "GtfsAgency"),
    "GtfsShape": ("dataModel.UrbanMobility", "GtfsShape"),
    "Event": ("dataModel.TourismDestinations", "Event"),
    "ServiceRequest": ("dataModel.IssueTracking", "service_requests"),
    "OffStreetParking": ("dataModel.Parking", "OffStreetParking"),
    "OnStreetParking": ("dataModel.Parking", "OnStreetParking"),
    "CityWork": ("dataModel.Transportation", "CityWork"),
    "PublicTransportRoute": ("dataModel.UrbanMobility", "PublicTransportRoute"),
    "KeyPerformanceIndicator": ("dataModel.KeyPerformanceIndicator", "KeyPerformanceIndicator"),
    "TrafficFlowObserved": ("dataModel.Transportation", "TrafficFlowObserved"),
    # The platform's extensions of an official model (T-4336, T-4338), read from the vendored copy.
    "Article": ("extension", "Article"),
    "AdministrativeArea": ("extension", "AdministrativeArea"),
    "HeritageArea": ("extension", "HeritageArea"),
    "TrafficSignalJunction": ("extension", "TrafficSignalJunction"),
    "NoiseZone": ("extension", "NoiseZone"),
}

EXTENSIONS = Path(__file__).resolve().parents[2] / "scripts/open-data/sdm/extensions"

#: Types with no Smart Data Model, and why the Helsinki model keeps its own.
EXCEPTIONS: dict[str, str] = {
    "Tree": "Smart Data Models has no tree model; GreenspaceRecord records soil and weather readings of a green area (T-5378)",
    "GtfsDailyDepartures": "Smart Data Models has no count of a day's timetable; a GTFS type describes one stop, line or trip (T-6467)",
    "NatureSite": "Smart Data Models has no protected-area model; GreenspaceRecord records soil and weather readings of a green area (T-5823)",
}

#: Attributes of a validated type that knowingly differ from the SDM schema, and why. A failure on
#: one is reported as excepted and does not fail the run.
ACCEPTED = {
    ("Event", "eventStatus"): "one schema.org status as the Helsinki class declares it and the event apps read it; "
                              "SDM's only Event (dataModel.TourismDestinations) makes it a list",
    ("Event", "refPointOfInterest"): "one venue per event, the Linked Events place as its Service Map unit; "
                                     "SDM makes it a list (T-5257)",
    ("ServiceRequest", "type"): "SDM names the type service_requests; the space types it ServiceRequest like its other "
                                "types and keeps the model's attribute names (T-5257)",
    ("OffStreetParking", "status"): "one value, as the space's one status slot holds it for bike stations too "
                                    "(BikeHireDockingStation's SDM status is a single value); SDM makes it a list",
    ("GtfsStation", "hasStop"): "the inverse of each platform's hasParentStation, which the Helsinki model "
                                "computes and never stores (one stored end per relationship, DM-67)",
    ("WaterQualityObserved", "dateObserved"): "a beach sensor that has not reported this season is kept with its "
                                              "position and name and no observation, rather than a date nobody "
                                              "measured (test_a_beach_sensor_without_a_reading...)",
}


@cache
def fetch(url: str) -> Any:
    with urllib.request.urlopen(urllib.request.Request(url, headers={"Accept": "application/json"}), timeout=60) as answer:
        return json.load(answer)


def validator(schema: dict[str, Any], retrieve: Callable[[str], Any] = fetch) -> Draft7Validator:
    """The schema's validator, every remote `$ref` read through `retrieve`."""

    def resource(uri: str) -> Resource:
        return Resource.from_contents(retrieve(uri.split("#", 1)[0]), default_specification=DRAFT7)

    registry = Registry(retrieve=resource)
    draft = Draft202012Validator if "2020-12" in schema.get("$schema", "") else Draft7Validator
    return draft(schema, registry=registry)


def failures(check: Draft7Validator, entities: list[dict[str, Any]]) -> Counter:
    """(attribute, message) -> how many entities fail it. The attribute is the top-level member
    an error sits under, the member a `required` rule misses, or `(entity)` for another rule on
    the whole entity."""
    found: Counter = Counter()
    for entity in entities:
        seen = set()
        for error in check.iter_errors(entity):
            if error.absolute_path:
                attribute = str(error.absolute_path[0])
            elif error.validator == "required":
                attribute = error.message.split("'")[1]
            else:
                attribute = "(entity)"
            # The rule, not the value: one row for every address that is a string.
            key = (attribute, error.message.replace(repr(error.instance), "the value")[:160])
            if key not in seen:
                seen.add(key)
                found[key] += 1
    return found


def entities(endpoint: str, kind: str, limit: int, lang: str = "fi") -> list[dict[str, Any]]:
    query = urllib.parse.urlencode({"type": kind, "limit": limit, "options": "keyValues", "lang": lang})
    return fetch(f"{endpoint.rstrip('/')}/ngsi-ld/v1/entities?{query}")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("endpoint", help="a public Endpoint that serves the Helsinki types")
    parser.add_argument("--limit", type=int, default=50)
    parser.add_argument("--type", action="append", help="only these types")
    args = parser.parse_args(argv)
    ok = True
    print("| type | entities | result | attribute: entities failing (message) |\n|---|---|---|---|")
    for kind, (repo, model) in MODELS.items():
        if args.type and kind not in args.type:
            continue
        sample = entities(args.endpoint, kind, args.limit)
        schema = json.loads((EXTENSIONS / model / "schema.json").read_text()) if repo == "extension" else fetch(SDM.format(repo=repo, model=model))
        check = validator(schema)
        found = failures(check, sample)
        failing = sum(1 for (attribute, _), _ in found.items() if (kind, attribute) not in ACCEPTED)
        ok &= failing == 0
        detail = "; ".join(
            f"{a}: {n} ({m})" + (f" EXCEPTED: {ACCEPTED[(kind, a)]}" if (kind, a) in ACCEPTED else "")
            for (a, m), n in sorted(found.items())
        ) or "—"
        # An empty sample proves nothing either way: a live feed that is quiet now (no vehicle at night).
        verdict = "FAIL" if failing else "excepted" if found else "PASS" if sample else "no entities"
        print(f"| {kind} | {len(sample)} | {verdict} | {detail.replace('|', '/')} |")
    for kind, reason in EXCEPTIONS.items():
        print(f"| {kind} | — | exception | {reason} |")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
