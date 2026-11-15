"""seeds/bbsk-events/geocode-venues.py: a venue's point from the address register or a reviewed name match (T-5083,
T-5240); offices.py and towns.py: the municipality's office or centre."""

import csv
import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
EVENTS = ROOT / "seeds/bbsk-events"
spec = importlib.util.spec_from_file_location("geocode_venues", EVENTS / "geocode-venues.py")
geocode_venues = importlib.util.module_from_spec(spec)
spec.loader.exec_module(geocode_venues)


def hit(category: str, kind: str, town: str, lat: str = "48.7395768", lon: str = "19.1376321", **extra) -> dict:
    return {"category": category, "type": kind, "addresstype": extra.pop("addresstype", kind), "lat": lat, "lon": lon,
            "osm_type": "way", "osm_id": 61153499,
            "address": {"road": "Cesta na amfiteáter", "house_number": "2728/1", "city": town, **extra}}


def run(answers: dict, place: str, town: str = "Banská Bystrica", reviewed: frozenset = frozenset(), register: dict | None = None):
    asked = []

    def search(query: str) -> list:
        asked.append(query)
        return answers.get(query, [])

    row, note = geocode_venues.geocode(town, place, search, lambda: None, lambda q: (register or {}).get(q, []), reviewed)
    return row, note, asked


ACCEPTED = frozenset({("banska-bystrica", "cpdv umb cesta na amfiteáter 1 učebňa č. 507", "way/61153499")})


def test_a_room_text_is_placed_by_the_address_it_holds():
    place = "CPDV UMB, Cesta na amfiteáter 1 (nová budova EF UMB), učebňa č. 507"
    answers = {"Cesta na amfiteáter 1, Banská Bystrica": [hit("building", "university", "Banská Bystrica")]}
    row, _note, asked = run(answers, place, reviewed=ACCEPTED)
    assert asked == ["CPDV UMB, Cesta na amfiteáter 1, učebňa č. 507, Banská Bystrica", "Cesta na amfiteáter 1, Banská Bystrica"]
    assert row["match"] == "cpdv umb cesta na amfiteáter 1 učebňa č. 507"
    assert (row["town"], row["lat"], row["lon"], row["osm"]) == ("banska-bystrica", "48.7395768", "19.1376321", "way/61153499")
    assert row["streetAddress"] == "Cesta na amfiteáter 2728/1" and row["checked_by"] == "nominatim"


def test_an_unreviewed_name_match_is_never_written(tmp_path, monkeypatch, capsys):
    # The pilot of 2026-10-22: a third of the name matches were another building ("Hotel Jason" -> "Hotel Bonums").
    place = "CPDV UMB, Cesta na amfiteáter 1 (nová budova EF UMB), učebňa č. 507"
    answers = {"Cesta na amfiteáter 1, Banská Bystrica": [hit("building", "university", "Banská Bystrica")]}
    row, note, _ = run(answers, place)
    assert row is None and note.startswith("unreviewed: banska-bystrica,cpdv umb cesta na amfiteáter 1 učebňa č. 507,way/61153499")
    venues = tmp_path / "venues.csv"
    venues.write_text(",".join(geocode_venues.FIELDS) + "\n", encoding="utf-8")
    (tmp_path / "reviewed.csv").write_text("town,match,osm\n", encoding="utf-8")
    places = tmp_path / "places.csv"
    places.write_text(f'town,place\nBanská Bystrica,"{place}"\n', encoding="utf-8")
    monkeypatch.setattr(geocode_venues, "VENUES", venues)
    monkeypatch.setattr(geocode_venues, "REVIEWED", tmp_path / "reviewed.csv")
    monkeypatch.setattr(geocode_venues, "PAUSE", 0)
    monkeypatch.setattr(geocode_venues, "fetch", lambda q: answers.get(q, []))
    monkeypatch.setattr(geocode_venues, "register", lambda q: [])
    assert geocode_venues.main([str(places), "--write"]) == 0
    assert venues.read_text(encoding="utf-8").count("\n") == 1, "no row without a review"
    assert "unreviewed" in capsys.readouterr().err


def ra(text: str, municipality: str, lat: float = 48.718273, lon: float = 21.217407, ident: str = "36516755") -> dict:
    """A Register adries hit as ra.synchronix.sk answers it (read 2026-10-22)."""
    return {"text": f"{text}, {municipality}", "municipality": municipality, "location": {"lat": lat, "lon": lon},
            "source": f"https://data.gov.sk/id/physical-address/{ident}"}


def test_the_register_places_a_street_and_number_by_its_orientacne_or_supisne_number():
    # Košice's boroughs are municipalities of their own in the register; "Titogradská 17" is orientačné 17.
    register = {"Titogradská 17, Košice": [ra("Titogradská 1398/17", "Košice-Sídlisko KVP")]}
    row, note, asked = run({}, "Výmenník Wuppertálska, Titogradská 17", "Košice", register=register)
    assert asked == [] and note == "Titogradská 17", "the register answers before any name search"
    assert (row["checked_by"], row["osm"], row["streetAddress"]) == ("register-adries", "ra/36516755", "Titogradská 1398/17")
    assert (row["lat"], row["lon"], row["town"]) == ("48.7182730", "21.2174070", "kosice")
    # "Hlavná 59" is orientačné 59 (súpisné 94), never the house whose súpisné number is 59.
    register = {"Hlavná 59, Košice": [ra("Hlavná 59/112", "Košice-Staré Mesto", ident="1"), ra("Hlavná 94/59", "Košice-Staré Mesto", ident="2")]}
    row, _, _ = run({}, "Stará radnica, Hlavná 59", "Košice", register=register)
    assert row["osm"] == "ra/2"
    # A village without streets numbers its houses by súpisné number alone.
    register = {"Bíňovce 134, Bíňovce": [ra("Bíňovce 134", "Bíňovce", 48.504597, 17.480251)]}
    assert run({}, "Obecný úrad, Bíňovce 134", "Bíňovce", register=register)[0]["osm"] == "ra/36516755"


def test_the_register_refuses_another_town_another_street_or_another_number():
    for label, municipality in (("Titogradská 1398/17", "Prešov"), ("Tatranská 1398/17", "Košice-Sídlisko KVP"),
                                ("Titogradská 1398/19", "Košice-Sídlisko KVP"), ("Titogradská 17/1398", "Košice-Sídlisko KVP")):
        register = {"Titogradská 17, Košice": [ra(label, municipality)]}
        row, _, asked = run({}, "Titogradská 17", "Košice", register=register)
        assert row is None and asked, label
    register = {"Titogradská 17, Košice": [ra("Titogradská 1398/17", "Košice-Sídlisko KVP", lat=50.2)]}
    assert run({}, "Titogradská 17", "Košice", register=register)[0] is None, "outside Slovakia"


def test_a_town_a_street_or_a_district_is_no_venue():
    for answer in (hit("place", "town", "Banská Bystrica"), hit("boundary", "administrative", "Banská Bystrica"),
                   hit("highway", "residential", "Banská Bystrica"), hit("place", "suburb", "Banská Bystrica"),
                   hit("amenity", "theatre", "Banská Bystrica", addresstype="city")):
        row, note, _ = run({"Javorina, Banská Bystrica": [answer]}, "Javorina")
        assert row is None and "no venue" in note


def test_the_village_itself_or_a_map_board_named_like_it_is_no_venue():
    # T-5135: Nominatim answered "Ľubietová" with the cadastral community and "Hriňová" with a map board.
    for answer in (hit("place", "cadastral_community", "Ľubietová", name="Ľubietová"),
                   hit("information", "map", "Ľubietová", name="Ľubietová"),
                   hit("tourism", "museum", "Ľubietová", name="Ľubietová")):
        answer["name"] = answer["address"].pop("name")
        row, note, _ = run({"Ľubietová, Ľubietová": [answer]}, "Ľubietová", "Ľubietová")
        assert row is None and "no venue" in note, note


def test_a_venue_of_the_same_name_in_another_town_or_outside_slovakia_is_refused():
    row, note, _ = run({"Kino, Zvolen": [hit("amenity", "cinema", "Detva")]}, "Kino", "Zvolen")
    assert row is None and note == "in Detva, not in Zvolen"
    row, note, _ = run({"Kino, Zvolen": [hit("amenity", "cinema", "Zvolen", lat="50.1")]}, "Kino", "Zvolen")
    assert row is None and note == "outside Slovakia"


def test_the_match_key_is_the_one_the_pipelines_compute():
    # pipelines/common.yaml: lower case, parentheses dropped, commas and semicolons as spaces.
    assert geocode_venues.key("Veľká zasadačka a veľký študijný hub, Tajovského 40 (SĎ 1)") == "veľká zasadačka a veľký študijný hub tajovského 40"
    assert 'replace_all("\\\\([^)]*\\\\)", " ").re_replace_all("[,;]", " ")' in (EVENTS / "pipelines/common.yaml").read_text()


def test_every_venue_row_names_its_osm_object_and_who_checked_it():
    rows = list(csv.DictReader((EVENTS / "venues.csv").open(encoding="utf-8")))
    assert rows and all(r["checked_by"] in ("hand", "nominatim", "overpass", "register-adries") for r in rows)
    assert all(r["osm"].split("/")[0] in ("node", "way", "relation", "ra") and r["osm"].split("/")[1].isalnum() for r in rows)
    assert all((r["checked_by"] == "register-adries") == r["osm"].startswith("ra/") for r in rows)
    keys = [(r["town"], r["match"]) for r in rows]
    assert len(keys) == len(set(keys))
    for r in rows:
        assert 47.7 <= float(r["lat"]) <= 49.7 and 16.8 <= float(r["lon"]) <= 22.6, r


def test_a_venue_already_in_the_table_is_never_looked_up_again(tmp_path, monkeypatch, capsys):
    known = next(csv.DictReader((EVENTS / "venues.csv").open(encoding="utf-8")))
    places = tmp_path / "places.csv"
    places.write_text(f"town,place\n{known['addressLocality']},{known['match']}\n", encoding="utf-8")
    asked = []
    monkeypatch.setattr(geocode_venues, "fetch", lambda q: asked.append(q) or [])
    monkeypatch.setattr(geocode_venues, "register", lambda q: asked.append(q) or [])
    assert geocode_venues.main([str(places)]) == 0
    assert asked == [], "a cached venue costs no request"


# --- offices.py (T-5215): the municipality's office as its home, its one culture house as "Kultúrny dom" -------

ospec = importlib.util.spec_from_file_location("offices", EVENTS / "offices.py")
offices = importlib.util.module_from_spec(ospec)
ospec.loader.exec_module(offices)


def element(kind: str, amenity: str, name: str | None, lat: float = 48.3, lon: float = 17.6, n: int = 1) -> dict:
    tags = {"amenity": amenity, "addr:street": "Hlavná", "addr:housenumber": "1"}
    if name:
        tags["name"] = name
    return {"type": kind, "id": n, "lat": lat, "lon": lon, "tags": tags} if kind == "node" else \
        {"type": kind, "id": n, "center": {"lat": lat, "lon": lon}, "tags": tags}


def answer(*munis) -> dict:
    """An Overpass answer: per municipality its boundary (name, maybe web site), then its elements."""
    out = []
    for name, website, items in munis:
        tags = {"name": name, **({"website": website} if website else {})}
        out += [{"type": "relation", "id": 2, "tags": tags}, *items]
    return {"elements": out}


def source(town: str, region: str = "Trnavský kraj", org_type: str = "municipality") -> dict:
    return {"town": town, "region_name": region, "org_type": org_type, "events_url": f"https://{offices.generate.slug(town)}.sk/"}


def test_the_office_becomes_the_home_and_the_one_culture_house_the_kulturny_dom():
    asked = []
    found = answer(("Bíňovce", None, [element("node", "townhall", "Obecný úrad", n=10),
                                                 element("way", "community_centre", "Kultúrny dom", 48.31, 17.61, n=11)]))
    venues, homes = offices.plan([source("Bíňovce")], set(), set(), lambda q: asked.append(q) or found)
    assert len(asked) == 1 and "Bíňovce" in asked[0], "one Overpass request for every source"
    assert homes == [{"events_url": "https://binovce.sk/", "town": "binovce", "match": "obecný úrad bíňovce"}]
    office = next(v for v in venues if v["match"] == "obecný úrad bíňovce")
    assert (office["osm"], office["streetAddress"], office["checked_by"]) == ("node/10", "Hlavná 1", "overpass")
    hall = {v["match"]: v for v in venues if v["osm"] == "way/11"}
    assert {"kultúrny dom", "kultúrny dom bíňovce"} <= set(hall) and hall["kultúrny dom"]["lat"] == "48.3100000"


def test_no_office_two_culture_houses_or_two_villages_of_one_name_give_no_point_unless_the_web_site_tells():
    found = answer(
        ("Hubice", None, [element("node", "community_centre", "KD", n=1), element("node", "community_centre", "Sála", n=2)]),
        ("Lehota", "https://www.lehota.sk", [element("node", "townhall", "Obecný úrad", n=3)]),
        ("Lehota", None, [element("node", "townhall", "Obecný úrad", n=4)]),
        ("Kostolná", None, [element("node", "townhall", "Obecný úrad", n=5)]),
        ("Kostolná", None, [element("node", "townhall", "Obecný úrad", n=9)]),
        ("Častá", None, [element("node", "townhall", "Obecný úrad", lat=40.0, n=6)]),
    )
    venues, homes = offices.plan([source("Hubice"), source("Lehota"), source("Kostolná"), source("Častá")], set(), set(),
                                 lambda q: found)
    # Hubice: no office and no single culture house; Kostolná: two of that name, no web site tells them apart;
    # Častá: its only office lies outside Slovakia's box. Never a village centre instead. Lehota: its web site
    # names the one.
    assert [h["town"] for h in homes] == ["lehota"] and [v["osm"] for v in venues] == ["node/3"]


def test_a_named_office_wins_over_another_townhall_and_a_source_with_a_home_is_skipped():
    found = answer(("Galanta", None, [element("node", "townhall", "Mestský úrad Galanta", n=7),
                                                 element("node", "townhall", "Hasičská zbrojnica", n=8)]))
    venues, homes = offices.plan([source("Galanta", org_type="town")], set(), set(), lambda q: found)
    assert homes[0]["match"] == "mestský úrad galanta" and venues[0]["osm"] == "node/7"
    known = {(v["town"], v["match"]) for v in venues}
    assert offices.plan([source("Galanta", org_type="town")], known, {"https://galanta.sk/"}, lambda q: found) == ([], [])


def test_a_building_named_as_the_towns_culture_house_counts_and_so_does_the_text_the_events_wrote():
    found = answer(("Žarnovica", None, [element("node", "townhall", "Mestský úrad", n=1),
                                        element("way", "", "Kultúrny dom Žarnovica", n=2),
                                        element("node", "community_centre", "Exercičný dom", n=3)]))
    found["elements"][2]["tags"].pop("amenity")
    venues, _ = offices.plan([source("Žarnovica", org_type="town")], set(), set(), lambda q: found,
                             {"Žarnovica": {"Kultúrny dom v Žarnovici", "Kino Baník"}})
    hall = {v["match"] for v in venues if v["osm"] == "way/2"}
    assert {"kultúrny dom", "kultúrny dom v žarnovici"} <= hall and "kino baník" not in hall


def test_a_culture_house_is_one_by_its_name_and_of_its_own_town():
    # Read on Overpass 2026-10-22: the single community_centre of these villages.
    cases = [("Kultúrny dom", "Babín", True), ("Kulturny dom", "Chocholná-Velčice", True),
             ("Mestské kultúrne stredisko v Revúcej", "Revúca", True), ("MSKS Šamorín - VMK Somorja", "Šamorín", True),
             ("Kultúrny dom Ponická Lehôtka", "Poniky", False), ("Exercičný dom", "Marianka", False),
             ("Kursalon", "Nimnica", False), ("Diecézne centrum mládeže Maják", "Špania Dolina", False)]
    for name, town, own in cases:
        assert offices.own_hall({"tags": {"name": name}}, town) is own, name
    assert offices.OFFICE.search("OcÚ Trstice") and not offices.OFFICE.search("Hasičská zbrojnica")


# --- towns.py (T-5240): the municipality's office, else its centre, for an event at a place nobody placed -----

tspec = importlib.util.spec_from_file_location("towns", EVENTS / "towns.py")
towns = importlib.util.module_from_spec(tspec)
tspec.loader.exec_module(towns)


def place_node(name: str, lat: float = 48.35, lon: float = 17.65, n: int = 20) -> dict:
    return {"type": "node", "id": n, "lat": lat, "lon": lon, "tags": {"place": "village", "name": name}}


def test_a_town_takes_its_office_else_its_centre_and_never_a_district_or_a_pinned_city():
    found = answer(("Bíňovce", None, [element("node", "townhall", "Obecný úrad", n=10), place_node("Bíňovce")]),
                   ("Žitavany", None, [place_node("Žitavany", n=21)]),
                   ("Hubice", None, [place_node("Iná obec", n=22)]))
    asked = []
    wanted = [{"name": n, "region": "TT", "site": ""} for n in ("Bíňovce", "Žitavany", "Hubice", "okres Myjava",
                                                                "Banskobystrický kraj", "Košice", "Bratislava")]
    rows = {r["town"]: r for r in towns.plan(wanted, set(), lambda q: asked.append(q) or found)}
    assert len(asked) == 1 and '"name"="Trnavský kraj"' in asked[0], "one query per kraj"
    assert "Košice" not in asked[0] and "okres" not in asked[0]
    assert (rows["binovce"]["precision"], rows["binovce"]["osm"]) == ("office", "node/10")
    assert (rows["zitavany"]["precision"], rows["zitavany"]["osm"]) == ("centre", "node/21")
    # Hubice: no office and no place node of its own name; a district, a kraj and the pinned cities are never asked.
    assert set(rows) == {"binovce", "zitavany"} and all(r["unique"] == "y" for r in rows.values())


def test_kosice_and_bratislava_are_their_city_halls_never_a_borough_office():
    # Geocode pilot 2026-10-22: the townhall rule picked "Miestny úrad MČ Západ" for every Košice venue.
    rows = {r["town"]: r for r in csv.DictReader((EVENTS / "towns.csv").open(encoding="utf-8"))}
    assert (rows["kosice"]["osm"], rows["kosice"]["checked_by"]) == ("way/48911086", "hand")  # Stará radnica, Hlavná 59
    assert (rows["bratislava"]["osm"], rows["bratislava"]["checked_by"]) == ("way/38539536", "hand")  # Primaciálne nám. 1
    found = answer(("Košice", None, [element("node", "townhall", "Miestny úrad MČ Košice - Západ", n=30)]))
    assert towns.plan([{"name": "Košice", "region": "KE", "site": ""}], set(), lambda q: found) == []


def test_two_villages_of_one_name_in_a_kraj_count_only_for_the_source_whose_web_site_names_one():
    found = answer(("Lehota", "https://www.lehota.sk", [element("node", "townhall", "Obecný úrad", n=3)]),
                   ("Lehota", None, [element("node", "townhall", "Obecný úrad", n=4)]))
    assert towns.plan([{"name": "Lehota", "region": "NR", "site": ""}], set(), lambda q: found) == []
    (row,) = towns.plan([{"name": "Lehota", "region": "NR", "site": "https://lehota.sk/podujatia/"}], set(), lambda q: found)
    assert (row["osm"], row["unique"]) == ("node/3", "n")


# lat and lon bounds of each country a region lies in (generate.COUNTRIES, Slovakia by default)
COUNTRY_BOX = {"SK": (47.7, 49.7, 16.8, 22.6), "CZ": (48.5, 51.1, 12.0, 18.9)}


def test_every_town_row_is_one_municipality_with_a_labelled_point_in_its_country():
    rows = list(csv.DictReader((EVENTS / "towns.csv").open(encoding="utf-8")))
    assert rows and len({r["town"] for r in rows}) == len(rows)
    for r in rows:
        assert r["town"] == offices.generate.slug(r["name"]) and r["region"] in offices.generate.REGIONS, r
        assert r["precision"] in ("office", "centre") and r["checked_by"] in ("overpass", "hand", "home"), r
        assert not towns.NOT_A_TOWN.search(r["name"]), r
        south, north, west, east = COUNTRY_BOX[offices.generate.COUNTRIES.get(r["region"], "SK")]
        assert south <= float(r["lat"]) <= north and west <= float(r["lon"]) <= east, r


def test_a_source_falls_back_to_its_own_town_and_an_aggregator_to_the_unique_towns_of_its_region():
    kept = {r["events_url"]: r for r in offices.generate.kept_sources()}
    village = next(r for r in kept.values() if r["town"] == "Bíňovce")
    assert [t[1] for t in offices.generate.towns_for(village)] == ["binovce"]
    spis = next(r for r in kept.values() if "Visit Spiš" in r["org_type"])
    near = offices.generate.towns_for(spis)
    assert "spisska-nova-ves" in {t[1] for t in near} and "binovce" not in {t[1] for t in near}


def test_calendar_abbreviations_are_spelled_out_in_the_match_key():
    key = offices.generate.key
    assert key("KD Medzibrod") == "kultúrny dom medzibrod" and key("Sála KD") == "sála kultúrny dom"
    assert key("OcÚ Trstice") == "obecný úrad trstice" and key("MsÚ, zasadačka") == "mestský úrad zasadačka"
    assert key("DK Cífer") == "dom kultúry cífer" and key("MsKS Hriňová") == "mestské kultúrne stredisko hriňová"
    assert key("KDE") == "kde" and key(key("KD (sála)")) == key("KD (sála)") == "kultúrny dom"
    common = (EVENTS / "pipelines/common.yaml").read_text(encoding="utf-8")
    for short, long in offices.generate.ABBREVIATIONS:
        assert f'.replace_all(" {short} ", " {long} ")' in common, short


def test_the_street_an_event_writes_beside_its_venue_is_looked_up_and_the_venue_stays_the_match():
    # K13 Košice writes "Výmenník Wuppertálska" as the venue and "Titogradská 1398/17, 040 23 Košice" as the street.
    register = {"Titogradská 1398/17, Košice": [ra("Titogradská 1398/17", "Košice-Sídlisko KVP")]}
    row, _ = geocode_venues.geocode("Košice", "Výmenník Wuppertálska", lambda q: [], lambda: None,
                                       lambda q: register.get(q, []), frozenset(), "Titogradská 1398/17, 040 23 Košice")
    assert (row["match"], row["venue"], row["osm"]) == ("výmenník wuppertálska", "Výmenník Wuppertálska", "ra/36516755")
