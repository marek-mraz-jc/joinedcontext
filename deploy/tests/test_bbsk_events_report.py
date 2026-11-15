"""seeds/bbsk-events/report.py: events per source and the share with a GPS point, by where the point came from (T-5135)."""

import csv
import importlib.util
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EVENTS = ROOT / "seeds/bbsk-events"
spec = importlib.util.spec_from_file_location("events_report", EVENTS / "report.py")
report = importlib.util.module_from_spec(spec)
spec.loader.exec_module(report)


def point(lon: float, lat: float) -> dict:
    return {"type": "Point", "coordinates": [lon, lat]}


def test_the_geocoded_share_counts_each_points_origin_per_source():
    venues = {(v["town"], v["match"]): v for v in csv.DictReader((EVENTS / "venues.csv").open(encoding="utf-8"))}
    hand = venues[("detva", "dom kultúry a. sládkoviča v detve")]
    home = venues[("banska-bystrica", "štátna vedecká knižnica")]
    nominatim = venues[("banska-stiavnica", "kino akademik")]
    events = [
        {"source": "a", "location": point(19.1462, 48.7363)},
        {"source": "a", "location": point(float(hand["lon"]), float(hand["lat"]))},
        {"source": "a", "venue": "Neznáma sála", "areaServed": "Zvolen", "refPointOfInterest": "urn:ngsi-ld:PointOfInterest:x:osm-way-1"},
        {"source": "b", "location": point(float(home["lon"]), float(home["lat"]))},
        {"source": "b", "location": point(float(nominatim["lon"]), float(nominatim["lat"]))},
        {"source": "b"},
    ]
    out = report.summarise(events, report.points())
    a, b = out["sources"]["a"], out["sources"]["b"]
    assert (a["events"], a["gps"], a["feed"], a["hand"]) == (3, 2, 1, 1)
    assert (b["events"], b["gps"], b["home"], b["nominatim"]) == (3, 2, 1, 1)
    assert (out["total"], out["gps"], round(out["share"], 3)) == (6, 4, 0.667)
    assert dict(out["unplaced"]) == {("a", "Neznáma sála"): 1}, "an event without a venue gives nothing to geocode"
    assert (out["municipality"], out["venue_link"]) == (1, 1)  # DM-141


def test_the_report_prints_the_share_and_writes_the_unplaced_venues_for_geocoding(tmp_path, capsys):
    url = "https://www.zvolen.sk/aktualne-podujatia.html"
    (tmp_path / "e.json").write_text(json.dumps([
        {"source": url, "venue": "podľa plagátu"},
        {"source": url, "location": point(19.12, 48.57)},
    ]))
    assert report.main(["--file", str(tmp_path / "e.json"), "--places", str(tmp_path / "p.csv")]) == 0
    assert capsys.readouterr().out.splitlines()[-2:] == [
        "1 sources, 2 events, 1 with GPS (50%)",
        "precision: source 0 (0%), building 0 (0%), office 0 (0%), centre 0 (0%), unlabelled 1 (50%)"]
    assert (tmp_path / "p.csv").read_text().splitlines() == ["town,place,street", "Zvolen,podľa plagátu,"]


def test_only_an_https_store_is_read():
    try:
        report.fetch("http://example.org/ngsi-ld/v1")
    except ValueError as e:
        assert "https" in str(e)
    else:
        raise AssertionError("an http store was read")


def test_the_next_page_link_keeps_the_commas_of_its_target():
    # Recorded 2026-10-22 from the dev Endpoint: the old split on "," cut the target at attrs=source, and 404ed.
    headers = [
        '<https://uri.etsi.org/ngsi-ld/v1/ngsi-ld-core-context-v1.9.jsonld>; rel="http://www.w3.org/ns/json-ld#context"',
        '</api/endpoint/x/ngsi-ld/v1/entities?attrs=source,location,venue&limit=100&cursor=ab.cd>; rel="next";type="application/json"',
    ]
    assert report.next_links(headers) == ["/api/endpoint/x/ngsi-ld/v1/entities?attrs=source,location,venue&limit=100&cursor=ab.cd"]
    assert report.next_links([headers[0]]) == []


def test_the_report_counts_each_precision_and_writes_the_towns_the_events_name(tmp_path, capsys):
    # T-5240: the shares per precision go into the task; the towns feed towns.py.
    url = "https://www.zvolen.sk/aktualne-podujatia.html"
    events = [{"source": url, "location": point(19.12, 48.57), "locationPrecision": p, "scope": ["/SK/BB/zvolen"],
               "address": {"addressLocality": t}} for p, t in (("building", "Zvolen"), ("office", "Zvolen"), ("centre", "Sliač"))]
    events.append({"source": url, "scope": "/SK/KE/kosice", "address": {"addressLocality": "Košice"}})
    (tmp_path / "e.json").write_text(json.dumps(events))
    assert report.main(["--file", str(tmp_path / "e.json"), "--localities", str(tmp_path / "l.csv")]) == 0
    assert capsys.readouterr().out.splitlines()[-1] == (
        "precision: source 0 (0%), building 1 (25%), office 1 (25%), centre 1 (25%), unlabelled 0 (0%)")
    assert (tmp_path / "l.csv").read_text().splitlines() == ["town,region", "Košice,KE", "Sliač,BBSK", "Zvolen,BBSK"]
