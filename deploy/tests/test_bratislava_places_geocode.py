"""seeds/bratislava-places/geocode.py: an address-only place of the Bratislava hub gets the register's one point
in Bratislava, keyed as the places mapping keys it (T-5444)."""

import csv
import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("geocode", ROOT / "seeds/bratislava-places/geocode.py")
geocode = importlib.util.module_from_spec(spec)
spec.loader.exec_module(geocode)


def hit(text: str, municipality: str, lat: float = 48.213898, lon: float = 17.159346) -> dict:
    return {"text": text, "municipality": municipality, "location": {"lat": lat, "lon": lon},
            "source": "https://data.gov.sk/id/physical-address/35261963"}


def register(*hits):
    asked = []

    def search(query: str) -> list:
        asked.append(query)
        return list(hits)

    return search, asked


def test_the_same_street_and_number_in_bratislava_is_the_point():
    search, asked = register(hit("Tbiliská 1767/2, Bratislava-Rača", "Bratislava-Rača"))
    found, why = geocode.place("Tbiliská 2", "Bratislava-Rača", search)
    assert (found, why, asked) == (
        {"lat": "48.213898", "lon": "17.159346", "source": "https://data.gov.sk/id/physical-address/35261963"}, "",
        ["Tbiliská 2, Bratislava"])


def test_a_full_address_is_looked_up_by_its_street_part():
    search, asked = register(hit("Primaciálne námestie 429/1, Bratislava-Staré Mesto", "Bratislava-Staré Mesto"))
    found, _ = geocode.place("Primaciálne námestie 429/1, Bratislava-Staré Mesto, 81101", "", search)
    assert found and asked == ["Primaciálne námestie 429/1, Bratislava"]


def test_a_letter_after_a_slash_the_word_ulica_and_a_point_given_as_geometry_still_match():
    point = hit("Slovnaftská 12738/5, Bratislava-Podunajské Biskupice", "Bratislava-Podunajské Biskupice")
    point["geometry"] = {"type": "Point", "coordinates": [17.159346, 48.213898]}
    del point["location"]
    search, asked = register(point)
    assert geocode.place("Slovnaftská ulica 5", "", search)[0]["lon"] == "17.159346"
    search, asked = register(hit("Pod brehmi 6102/6A, Bratislava-Karlova Ves", "Bratislava-Karlova Ves"))
    assert geocode.place("Pod brehmi 6/A", "Bratislava-Dúbravka", search)[0] and asked == ["Pod brehmi 6A, Bratislava"]


def test_another_number_another_town_or_a_point_outside_the_city_is_refused():
    assert geocode.place("Tbiliská 2", "", register(hit("Tbiliská 1767/4, Bratislava-Rača", "Bratislava-Rača"))[0]) == (
        None, "no such address in Bratislava")
    assert geocode.place("Hlavná 2", "", register(hit("Hlavná 2, Senec", "Senec"))[0])[0] is None
    pointless = hit("Šancová 3132/112, Bratislava-Nové Mesto", "Bratislava-Nové Mesto")
    del pointless["location"]
    assert geocode.place("Šancová 112", "", register(pointless)[0]) == (None, "in the register without a point")
    assert geocode.place("Tbiliská 2", "", register(hit("Tbiliská 2, Bratislava-Rača", "Bratislava-Rača", lat=48.7))[0]) == (
        None, "outside Bratislava")


def test_two_boroughs_with_the_street_take_the_row_s_district_or_none():
    search, _ = register(hit("Hlavná 5, Bratislava-Rusovce", "Bratislava-Rusovce", 48.05, 17.15),
                         hit("Hlavná 5, Bratislava-Jarovce", "Bratislava-Jarovce", 48.06, 17.10))
    assert geocode.place("Hlavná 5", "Rusovce", search)[0]["lat"] == "48.050000"
    found, why = geocode.place("Hlavná 5", "", search)
    assert found is None and why.startswith("2 points in")


def test_the_key_is_the_one_the_mapping_computes_and_the_block_is_rewritten_in_place(tmp_path):
    # sha256("tbiliská 2|bratislava-rača")[:16], as the mapping's hash("sha256").encode("hex").slice(0, 16).
    assert geocode.address_key("Tbiliská 2", "Bratislava-Rača") == geocode.address_key("tbiliská 2", "bratislava-rača")
    mapping = geocode.MAPPING.read_text()
    rows = [{"key": "00aa", "lat": "48.1", "lon": "17.1"}, {"key": "00ab", "lat": "48.2", "lon": "17.2"}]
    once = geocode.write_mapping(mapping, rows)
    assert '"00aa": { "lat": 48.1, "lon": 17.1 },\n          "00ab": { "lat": 48.2, "lon": 17.2 }\n        }' in once
    assert geocode.write_mapping(once, rows) == once
    assert geocode.write_mapping(once, []).count("let positions = {}\n") == 1


def test_every_committed_position_is_in_the_mapping_and_inside_the_city():
    rows = list(csv.DictReader((ROOT / "seeds/bratislava-places/positions.csv").open()))
    assert rows and geocode.write_mapping(geocode.MAPPING.read_text(), rows) == geocode.MAPPING.read_text()
    for row in rows:
        assert row["key"] == geocode.address_key(row["street"], row["district"]), row
        assert geocode.LAT[0] <= float(row["lat"]) <= geocode.LAT[1] and geocode.LON[0] <= float(row["lon"]) <= geocode.LON[1]
        assert row["source"].startswith("https://data.gov.sk/id/physical-address/"), row
