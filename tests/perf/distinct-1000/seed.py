#!/usr/bin/env python3
"""The entities of dev/perf/k6-surfaces.js's seed in the store's internal form, for dev/bulk-load.sh.

    python3 tests/perf/distinct-1000/seed.py --entities 581000 > fifo   # run.sh pipes it to bulk-load.sh

The same documents k6-surfaces.js upserts through the API (`entity()`, `countOf()`): ten types,
Event takes EVENTS and the other nine share the rest, ids `urn:ngsi-ld:<Type>:load:<n>`, a Point
in lon 19 + (n % 1000) / 1000, lat 48.5 + (n // 1000 % 500) / 1000, `level` n % 100, `category`
c(n % 8), `lane` on TrafficFlowObserved, start and end dates on Event. The API seed writes about
220 entities a second on the bench's 2-core PostgreSQL (45 min for 581 000); COPY takes a minute.
Synthetic only: no real place, person or device.

    --self-test      check the shape against a document the broker stored and exit
"""

import argparse
import datetime
import json
import sys

DC = "https://uri.etsi.org/ngsi-ld/default-context/"
LOC = "https://uri.etsi.org/ngsi-ld/location"
TYPES = [
    "AirQualityObserved", "Vehicle", "Building", "ParkingSpot", "Streetlight",
    "WeatherObserved", "WaterQualityObserved", "TrafficFlowObserved", "Device", "Event",
]
LANES = 20
HISTORY_START = datetime.datetime(2026, 9, 1, tzinfo=datetime.timezone.utc)


def count_of(t, entities, events):
    if t == "Event":
        return events
    rest, others = entities - events, len(TYPES) - 1
    return rest // others + (1 if TYPES.index(t) < rest % others else 0)


def iso(d):
    return d.strftime("%Y-%m-%dT%H:%M:%S.") + f"{d.microsecond // 1000:03d}Z"


def entity(t, n, now):
    def prop(v, kind="Property"):
        return [{"type": kind, "value": v, "createdAt": now, "modifiedAt": now}]

    doc = {
        "id": f"urn:ngsi-ld:{t}:load:{n}",
        "type": [DC + t],
        "createdAt": now,
        "modifiedAt": now,
        LOC: prop({"type": "Point", "coordinates": [19 + (n % 1000) / 1000, 48.5 + (n // 1000 % 500) / 1000]}, "GeoProperty"),
        DC + "name": prop(f"{t} {n}"),
        DC + "level": prop(n % 100),
        DC + "category": prop(f"c{n % 8}"),
    }
    if t == "TrafficFlowObserved":
        doc[DC + "lane"] = prop(n % LANES)
    if t == "Event":
        start = HISTORY_START + datetime.timedelta(days=n % 60, hours=n % 24)
        doc[DC + "startDate"] = prop({"@type": "DateTime", "@value": iso(start)})
        doc[DC + "endDate"] = prop({"@type": "DateTime", "@value": iso(start + datetime.timedelta(hours=2))})
    return doc


def stream(entities, events, now):
    for t in TYPES:
        for n in range(count_of(t, entities, events)):
            yield entity(t, n, now)


def self_test():
    now = "2026-10-21T20:07:43.267Z"
    # a Vehicle the broker stored from k6-surfaces.js's upsert, members in its order
    stored = json.loads(
        '{"id": "urn:ngsi-ld:Vehicle:load:3", "type": ["https://uri.etsi.org/ngsi-ld/default-context/Vehicle"],'
        ' "createdAt": "2026-10-21T20:07:43.267Z", "modifiedAt": "2026-10-21T20:07:43.267Z",'
        ' "https://uri.etsi.org/ngsi-ld/location": [{"type": "GeoProperty", "value": {"type": "Point",'
        ' "coordinates": [19.003, 48.5]}, "createdAt": "2026-10-21T20:07:43.267Z", "modifiedAt": "2026-10-21T20:07:43.267Z"}],'
        ' "https://uri.etsi.org/ngsi-ld/default-context/name": [{"type": "Property", "value": "Vehicle 3",'
        ' "createdAt": "2026-10-21T20:07:43.267Z", "modifiedAt": "2026-10-21T20:07:43.267Z"}],'
        ' "https://uri.etsi.org/ngsi-ld/default-context/level": [{"type": "Property", "value": 3,'
        ' "createdAt": "2026-10-21T20:07:43.267Z", "modifiedAt": "2026-10-21T20:07:43.267Z"}],'
        ' "https://uri.etsi.org/ngsi-ld/default-context/category": [{"type": "Property", "value": "c3",'
        ' "createdAt": "2026-10-21T20:07:43.267Z", "modifiedAt": "2026-10-21T20:07:43.267Z"}]}'
    )
    assert entity("Vehicle", 3, now) == stored, entity("Vehicle", 3, now)
    assert sum(count_of(t, 581000, 5000) for t in TYPES) == 581000
    assert [count_of(t, 100, 10) for t in TYPES] == [10] * 10
    ids = [d["id"] for d in stream(1000, 100, now)]
    assert len(ids) == len(set(ids)) == 1000
    print("seed.py self-test ok")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--entities", type=int, default=581000)
    ap.add_argument("--events", type=int, default=5000)
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        self_test()
        return
    now = iso(datetime.datetime.now(datetime.timezone.utc))
    out = sys.stdout
    for doc in stream(a.entities, min(a.events, a.entities), now):
        out.write(json.dumps(doc, separators=(",", ":")))
        out.write("\n")


if __name__ == "__main__":
    main()
