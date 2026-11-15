"""T-5259: the plausibility checks of the Helsinki entities (tests/helsinki/plausible.py), offline:
each check counts the defect it names and leaves a sound entity alone."""

import importlib.util
import sys
from datetime import UTC, datetime
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "tests/helsinki"))
spec = importlib.util.spec_from_file_location("plausible", ROOT / "tests/helsinki/plausible.py")
plausible = importlib.util.module_from_spec(spec)
spec.loader.exec_module(plausible)

NOW = datetime(2026, 10, 22, 18, 0, tzinfo=UTC)
RAUTATIENTORI = [24.9414, 60.1710]


def point(lon, lat):
    return {"type": "Point", "coordinates": [lon, lat]}


def test_a_sound_entity_fails_nothing():
    sound = {"id": "urn:ngsi-ld:OffStreetParking:a", "name": "Kamppi", "location": point(*RAUTATIENTORI),
             "availableSpotNumber": 10, "totalSpotNumber": 50, "dateObserved": "2026-10-22T17:55:00Z"}
    assert not plausible.check("OffStreetParking", [sound], {}, NOW)


def test_an_out_of_area_a_zero_and_a_swapped_point_are_told_apart():
    rows = [{"id": "a", "location": point(13.40, 52.52)}, {"id": "b", "location": point(0, 0)},
            {"id": "c", "location": point(60.17, 24.94)},
            {"id": "d", "location": {"type": "LineString", "coordinates": [RAUTATIENTORI, [26.5, 61.0]]}}]
    assert plausible.check("Thing", rows, {}, NOW) == {
        ("location", "outside"): 2, ("location", "zero"): 1, ("location", "swapped"): 1}


def test_an_out_of_range_value_and_more_available_than_capacity_are_counted():
    rows = [{"id": "a", "temperature": 61.0, "relativeHumidity": 85},
            {"id": "b", "availableSpotNumber": 60, "totalSpotNumber": 50},
            {"id": "c", "availableBikeNumber": -1, "temperature": -12.5, "relativeHumidity": 0.85}]
    assert plausible.check("Thing", rows, {}, NOW) == {
        ("range", "temperature"): 1, ("range", "relativeHumidity"): 1,
        ("capacity", "availableSpotNumber > totalSpotNumber"): 1, ("range", "availableBikeNumber"): 1}


def test_a_stamp_ahead_of_tomorrow_is_counted_and_an_event_start_ahead_is_not():
    rows = [{"id": "a", "dateIssued": "2026-11-29T16:00:00Z"}, {"id": "b", "dateObserved": "2026-10-23T10:00:00Z"},
            {"id": "c", "startDate": "2026-12-01T10:00:00Z", "dateModified": "2026-10-22T10:00:00+03:00"}]
    assert plausible.check("Thing", rows, {}, NOW) == {("future", "dateIssued"): 1}


def test_a_duplicate_is_one_name_within_20_m_unless_its_identity_differs():
    near = [24.9416, 60.1710]  # about 11 m east
    twins = [{"id": "a", "name": "Kirjasto", "location": point(*RAUTATIENTORI)},
             {"id": "b", "name": "Kirjasto", "location": point(*near)},
             {"id": "c", "name": "Kirjasto", "location": point(24.95, 60.17)}]
    assert plausible.check("PointOfInterest", twins, {}, NOW) == {("duplicate", "same name within 20 m"): 1}
    platforms = [{"id": "a", "name": "Rautatientori", "stopCode": "H2013", "location": point(*RAUTATIENTORI)},
                 {"id": "b", "name": "Rautatientori", "stopCode": "H2014", "location": point(*near)}]
    assert not plausible.check("GtfsStop", platforms, {}, NOW)
    assert plausible.check("GtfsStop", [platforms[0], dict(platforms[1], stopCode="H2013")], {}, NOW)


def test_a_reference_is_dangling_only_into_a_type_that_was_read():
    stop = "urn:ngsi-ld:GtfsStop:hel.fi:helsinki:1"
    rows = [{"id": "a", "refStop": [stop, "urn:ngsi-ld:GtfsStop:hel.fi:helsinki:2"]},
            {"id": "b", "refDevice": "urn:ngsi-ld:Device:x"}]
    assert plausible.check("Alert", rows, {"GtfsStop": {stop}}, NOW) == {("dangling", "refStop"): 1}


def test_the_region_holds_every_feed_city_and_the_ends_of_the_hsl_lines():
    for lon, lat in ([24.94, 60.17], [24.66, 60.21], [25.04, 60.29], [24.78, 60.735], [25.66, 60.98], [24.03, 60.18]):
        assert plausible.place(lon, lat) is None, (lon, lat)
    assert plausible.place(23.76, 61.50) == "outside"  # Tampere
