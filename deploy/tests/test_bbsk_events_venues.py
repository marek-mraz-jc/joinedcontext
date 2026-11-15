"""One PointOfInterest per real venue of the events' venue table (T-5364, DM-141): rows of one OpenStreetMap
object or of one name within 50 m are one venue, every event the table places links it, and each store's
miesta-podujati pipeline writes the venues its events can link."""

import csv
import json
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
EVENTS = ROOT / "seeds/bbsk-events"
SEED = ROOT / "components/context-gateway/seed"
sys.path.insert(0, str(EVENTS))
import generate

ROWS = list(csv.DictReader((EVENTS / "venues.csv").open(encoding="utf-8")))


def row(venue: str, lat: float, lon: float, osm: str = "", town: str = "detva") -> dict:
    return {"town": town, "match": venue.lower(), "venue": venue, "streetAddress": "", "addressLocality": "Detva",
            "lat": str(lat), "lon": str(lon), "osm": osm}


def test_one_name_within_50_m_is_one_venue_and_200_m_apart_two():
    """DM-141: rows of one name merge within 50 m and stay apart at 200 m."""
    # 0.0003° of latitude is about 33 m, 0.0018° about 200 m.
    groups = generate.venue_groups([
        row("Kultúrny dom", 48.5, 19.4),
        row("KD", 48.5003, 19.4),            # the same name spelled short, 33 m away: one venue
        row("Kultúrny dom", 48.5018, 19.4),  # the same name 200 m away: another venue
        row("Amfiteáter", 48.5001, 19.4),    # another name 11 m away: another venue
    ])
    assert [[r["match"] for r in g] for g in groups] == [["kultúrny dom", "kd"], ["kultúrny dom"], ["amfiteáter"]]


def test_one_openstreetmap_object_is_one_venue_whatever_its_rows_call_it():
    groups = generate.venue_groups([row("FEE", 48.5, 19.4, "way/1"), row("FEE E214", 48.51, 19.4, "way/1")])
    assert len(groups) == 1 and generate.poi_id(groups[0]) == "osm-way-1"


def test_a_venue_without_an_openstreetmap_object_takes_a_stable_hash():
    a = generate.poi_id([row("Námestie", 48.5, 19.4)])
    assert a == generate.poi_id([row("námestie", 48.5, 19.4)]) and a.startswith("h-") and len(a) == 18
    assert a != generate.poi_id([row("Námestie", 48.5018, 19.4)])


def test_the_venue_table_counts_its_real_venues_and_two_galleries_of_kosice_stay_two():
    groups = generate.venue_groups(ROWS)
    ids = [generate.poi_id(g) for g in groups]
    assert len(ids) == len(set(ids))
    # Before 617 rows, after one PointOfInterest per venue (2026-10-22: 617 rows, 306 venues).
    print(f"venues.csv: {len(ROWS)} rows, {len(groups)} venues")
    assert len(groups) < len(ROWS)
    galleries = {generate.poi_id(g) for g in groups for r in g if r["town"] == "kosice" and r["venue"] == "Východoslovenská galéria"}
    assert len(galleries) == 2  # Hlavná 27 and Hlavná 40, 290 m apart


def test_every_venue_an_event_can_link_is_written_by_its_stores_venue_pipeline():
    """DM-141: each store writes exactly the venues its events link, from the table, with no outbound call."""
    for store in generate.STORES:
        ns = store["ns"]
        bento = yaml.safe_load((SEED / ns / f"{ns}-pipeline-{generate.VENUES}-bento.yaml").read_text())
        table = json.loads(bento["pipeline"]["processors"][0]["mapping"].removeprefix("root = "))
        written = {v[0] for v in table}
        assert len(written) == len(table)
        linked = {generate.poi_ids()[(v["town"], v["match"])] for r in generate.store_rows(store)
                  for v in generate.venues_for(r["town_slug"])}
        assert linked == written, ns
        pipeline = yaml.safe_load((SEED / ns / f"{ns}-pipeline-{generate.VENUES}.yaml").read_text())
        assert pipeline["spec"]["output"] == {"type": "PointOfInterest", "mode": "upsert"}
        assert pipeline["spec"]["targetEndpoint"] == store["target"]
        datasource = yaml.safe_load((SEED / ns / f"{ns}-datasource-{generate.VENUES}.yaml").read_text())
        assert datasource["spec"]["type"] == "generate"  # no outbound call: the table rides in the mapping


def test_every_venue_lies_near_its_municipality():
    """A venue's point within 10 km of its municipality's office or centre (towns.csv): a geocoder hit in a
    namesake village elsewhere fails here (T-5252's rule, with the municipality's point for its area)."""
    towns = {t["town"]: t for t in csv.DictReader((EVENTS / "towns.csv").open(encoding="utf-8"))}
    far = [(r["town"], r["venue"], round(generate.metres(r, towns[r["town"]]) / 1000, 1)) for r in ROWS
           if generate.metres(r, towns[r["town"]]) > 10000]
    assert far == []
