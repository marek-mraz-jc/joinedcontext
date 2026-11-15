"""T-5286: the street parking map, the tourist coach stops and parking places and the junctions with traffic
lights and warning lights of kartta.hel.fi in the Helsinki space (PL-03, DM-61).

Bento runs each committed mapping over the WFS answers the deployer recorded on 2026-10-26 (T-5668,
`fixtures/helsinki/kartta/`), and every entity it writes is checked against the space's generated JSON
Schema and its Smart Data Model. The edge cases are those answers edited in the one place they name.

Without a container runtime the Bento cases skip; the seed cases do not.
"""

import copy
import json
import re
from pathlib import Path

import open_data
import pytest
import yaml
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-helsinki-traffic-layers")

ROOT = Path(__file__).resolve().parent.parent
HELSINKI = ROOT / "components/context-gateway/seed/helsinki"
KARTTA = Path(__file__).resolve().parent / "fixtures/helsinki/kartta"
WFS = "https://kartta.hel.fi/ws/geoserver/avoindata/wfs?"

# pipeline -> (DataSource, recorded layer, type written, Endpoint, entities the recording gives)
FEEDS = {
    # 20 places, the smallest of 8754 (the whole layer gave 7873 on 2026-10-26 without its 881 no-parking areas).
    "parking-places": ("hel-parking-places", "Pysakointipaikat_alue", "OnStreetParking", "helsinki-parking", 20),
    "tourist-stops": ("hel-tourist-stops", "Matkailuliikenne_pysakit_piste", "OnStreetParking", "helsinki-parking", 20),
    "tourist-parking": ("hel-tourist-parking", "Matkailuliikenne_pysakointipaikat_piste", "OnStreetParking", "helsinki-parking", 11),
    "traffic-lights": ("hel-traffic-lights", "Liikennevalot_piste", "TrafficSignalJunction", "helsinki-traffic-signals", 20),
    "warning-lights": ("hel-warning-lights", "Varoitusvalot_piste", "TrafficSignalJunction", "helsinki-traffic-signals", 12),
}
MODEL = {"OnStreetParking": "dataModel.Parking/OnStreetParking", "TrafficSignalJunction": "extensions/TrafficSignalJunction"}


def recording(layer: str) -> dict:
    return json.loads((KARTTA / f"{layer}.json").read_text())


def run(pipeline: str, document: dict) -> list[dict]:
    return open_data.run_all(HELSINKI / f"helsinki-pipeline-{pipeline}-bento.yaml", json.dumps(document).encode(), "helsinki")


def one(layer: str, **properties) -> dict:
    """The layer's first recorded feature with `properties` changed."""
    feature = copy.deepcopy(recording(layer)["features"][0])
    feature["properties"].update(properties)
    return feature


@pytest.fixture(scope="module")
def written():
    return {name: run(name, recording(feed[1])) for name, feed in FEEDS.items()}


@requires_docker
def test_every_feed_writes_its_type_once_per_record_and_the_gateway_accepts_it(written):
    for name, (_, layer, kind, _, count) in FEEDS.items():
        entities = written[name]
        assert len(entities) == count, name
        assert len({e["id"] for e in entities}) == count, name
        for entity in entities:
            assert entity["type"] == kind
            assert re.fullmatch(rf"urn:ngsi-ld:{kind}:hel\.fi:helsinki:[a-z-]+-[0-9]+", entity["id"]), entity["id"]
            assert open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", entity) == [], (entity["id"],)
            assert open_data.sdm_errors(MODEL[kind], entity) == [], entity["id"]
            assert entity["dataProvider"]["value"]
            assert entity["source"]["value"] == f"{WFS}typeNames=avoindata:{layer}", entity["id"]
            assert re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", entity["expiresAt"]), entity["id"]


@requires_docker
def test_a_parking_place_takes_the_categories_of_its_class_and_its_type():
    layer = "Pysakointipaikat_alue"
    cases = {
        # luokka, tyyppi -> categories, allowedVehicleType
        1: (6, "", ["feeCharged", "forResidents"], None),
        2: (1, None, ["free", "shortTerm"], None),
        3: (5, "Inva", ["feeCharged", "shortTerm", "forDisabled"], None),
        4: (0, "Sähköpotkulauta", ["public"], ["other"]),
        5: (None, "Matkailuliikenne", ["public"], ["bus"]),
        6: (11, "CD", ["onlyWithPermit"], None),
        7: (9, "Taxi", ["taxiStop"], None),
        8: (12, "", ["public"], None),
    }
    features = [one(layer, id=n, luokka=c, tyyppi=t) for n, (c, t, _, _) in cases.items()]
    out = {int(e["id"].rsplit("-", 1)[1]): e for e in run("parking-places", {"features": features})}
    assert set(out) == set(cases)
    for n, (_, _, categories, vehicles) in cases.items():
        assert out[n]["category"]["value"] == categories, n
        assert out[n].get("allowedVehicleType", {}).get("value") == vehicles, n


@requires_docker
def test_a_parking_place_keeps_its_permit_letter_estimate_and_rules_and_drops_noise():
    layer = "Pysakointipaikat_alue"
    place = one(layer, id=41, luokka=8, luokka_nimi="Ilman asukas-/yritystunnusta ilmainen lyhytaikainen pysäköinti.",
                tyyppi="", asukaspysakointitunnus="F", paikat_ala=18, voimassaolo="9-21, (9-18)", kesto="4 h",
                kausi="(1.4.-31.10.)", paikan_asento="vinottain", lisatieto="7639-2")
    bare = one(layer, id=42, asukaspysakointitunnus="0", paikat_ala=0, kausi="0", lisatieto="0", luokka_nimi=None,
               voimassaolo="", kesto="None", paikan_asento=None, tyyppi=None)
    ban = one(layer, id=43, tyyppi="Pysäköintikielto")
    line = one(layer, id=44)
    line["geometry"] = {"type": "LineString", "coordinates": [[24.9, 60.2], [24.91, 60.21]]}
    out = {int(e["id"].rsplit("-", 1)[1]): e for e in run("parking-places", {"features": [place, bare, ban, line]})}
    assert set(out) == {41, 42}, "a no-parking area and a line are no parking place"
    assert out[41]["zoneCode"]["value"] == "F" and out[41]["totalSpotNumber"]["value"] == 18
    assert out[41]["description"]["languageMap"] == {"fi": "\n".join([
        "Ilman asukas-/yritystunnusta ilmainen lyhytaikainen pysäköinti.", "Voimassa: 9-21, (9-18)",
        "Enimmäisaika: 4 h", "Kausi: 1.4.-31.10.", "Paikan asento: vinottain"])}, "a bare sign number is no text"
    for attribute in ("zoneCode", "totalSpotNumber", "description", "allowedVehicleType", "name"):
        assert attribute not in out[42], attribute
    assert out[42]["location"]["value"] == bare["geometry"]


@requires_docker
def test_a_tourist_place_is_for_buses_named_in_three_languages_and_short_term_only_with_a_time_limit():
    stop = one("Matkailuliikenne_pysakit_piste", id=7, p_numero="15", p_nimi="Senaatintori", p_nimi_en="Senate Square",
               p_nimi_ven="Сенатская площадь", aika="30 min", kausi="(1.1.-31.12)", lisatieto="", lisati_eng="")
    place = one("Matkailuliikenne_pysakointipaikat_piste", id=8, p_kirjain="K", p_nimi="Hernesaari", p_nimi_en="Hernesaari",
                p_nimi_ven="None", aika="ei aikarajoitusta", aika_en="no time limit", kausi="(1.4 - 30.9)",
                lisatieto="2 paikkaa", lisati_en="2 places")
    unnamed = one("Matkailuliikenne_pysakit_piste", id=9, p_nimi="None")
    (s,) = run("tourist-stops", {"features": [stop, unnamed]})
    (p,) = run("tourist-parking", {"features": [place]})
    assert s["id"].endswith(":tourist-stop-7") and p["id"].endswith(":tourist-parking-8")
    assert s["allowedVehicleType"]["value"] == ["bus"] == p["allowedVehicleType"]["value"]
    assert s["category"]["value"] == ["shortTerm"] and p["category"]["value"] == ["public"]
    assert s["zoneCode"]["value"] == "15" and p["zoneCode"]["value"] == "K"
    assert s["name"]["languageMap"] == {"fi": "Senaatintori", "en": "Senate Square", "ru": "Сенатская площадь"}
    assert p["name"]["languageMap"] == {"fi": "Hernesaari", "en": "Hernesaari"}
    assert s["description"]["languageMap"] == {
        "fi": "Matkailuliikenteen pysäkki.\nAika: 30 min\nKausi: 1.1.-31.12",
        "en": "Tourist coach stop.\nTime: 30 min\nSeason: 1.1.-31.12"}
    assert p["description"]["languageMap"] == {
        "fi": "Matkailuliikenteen pysäköintipaikka.\nAika: ei aikarajoitusta\nKausi: 1.4 - 30.9\n2 paikkaa",
        "en": "Tourist coach parking place.\nTime: no time limit\nSeason: 1.4 - 30.9\n2 places"}


@requires_docker
def test_a_junction_is_keyed_by_its_row_and_tells_its_kind_status_and_streets():
    lights = "Liikennevalot_piste"
    first = one(lights, id=1, numero=812, tyyppi="Liikennevalot", risteys="Vihdintie/Ruosilantie", lisatiedot="JK-valot")
    # The city repeats an installation number: both junctions stay.
    twin = one(lights, id=2, numero=812, tyyppi="Rakenteilla", risteys=None, lisatiedot=None)
    twin["id"] = f"{lights}.2"
    unnumbered = one(lights, id=3, numero=None, risteys="", tyyppi=None)
    unnumbered["id"] = f"{lights}.3"
    other_layer = one(lights, id=4)
    other_layer["id"] = "Pysakointipaikat_alue.4"
    out = {e["id"].rsplit(":", 1)[1]: e for e in run("traffic-lights", {"features": [first, twin, unnumbered, other_layer]})}
    assert set(out) == {"traffic-lights-1", "traffic-lights-2", "traffic-lights-3"}, "a record of another layer is not written"
    assert out["traffic-lights-1"]["name"]["languageMap"] == {"fi": "Vihdintie/Ruosilantie"}
    assert out["traffic-lights-1"]["description"]["languageMap"] == {"fi": "JK-valot"}
    assert [out[k]["status"]["value"] for k in sorted(out)] == ["inService", "underConstruction", "inService"]
    assert out["traffic-lights-2"]["name"]["languageMap"] == {"fi": "Liikennevalot 812", "en": "Traffic lights 812"}
    assert out["traffic-lights-2"]["junctionNumber"]["value"] == "812"
    assert "name" not in out["traffic-lights-3"] and "junctionNumber" not in out["traffic-lights-3"]
    assert {e["signalKind"]["value"] for e in out.values()} == {"trafficLights"}
    warning = one("Varoitusvalot_piste", id=19, numero=425, tyyppi="Rakenteilla", risteys=None)
    (w,) = run("warning-lights", {"features": [warning]})
    assert w["id"].endswith(":warning-lights-19") and w["signalKind"]["value"] == "warningLights"
    assert w["status"]["value"] == "underConstruction"
    assert w["name"]["languageMap"] == {"fi": "Varoitusvalot 425", "en": "Warning lights 425"}


def test_each_pair_of_pipelines_over_one_dataset_runs_one_mapping_text():
    text = {name: (HELSINKI / f"helsinki-pipeline-{name}-bento.yaml").read_text() for name in FEEDS}
    assert text["tourist-stops"] == text["tourist-parking"]
    assert text["traffic-lights"] == text["warning-lights"]


def test_each_source_is_one_kartta_layer_read_weekly_over_https_with_a_timeout():
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    for name, (source, layer, kind, endpoint, _) in FEEDS.items():
        datasource = yaml.safe_load((HELSINKI / f"helsinki-datasource-{name}.yaml").read_text())
        assert datasource["metadata"]["name"] == source
        http = datasource["spec"]["http"]
        url = f"{WFS}service=WFS&version=2.0.0&request=GetFeature&outputFormat=application/json&srsName=EPSG:4326&typeNames=avoindata:{layer}"
        assert http["url"] == url
        assert http["verb"] == "GET" and http["timeout"] in ("30s", "60s") and "auth" not in json.dumps(http).lower()
        pipeline = yaml.safe_load((HELSINKI / f"helsinki-pipeline-{name}.yaml").read_text())["spec"]
        assert pipeline["source"]["dataSourceRef"]["name"] == source
        assert pipeline["output"] == {"type": kind, "mode": "upsert"} and pipeline["period"] == "168h"
        # Only the mapping itself: no further call to any host.
        assert "http" not in yaml.safe_load((HELSINKI / f"helsinki-pipeline-{name}-bento.yaml").read_text())["pipeline"]["processors"][0]
        catalog = yaml.safe_load((HELSINKI / f"helsinki-endpoint-{endpoint.removeprefix('helsinki-')}.yaml").read_text())["spec"]["catalog"]
        assert url in [s["url"] for s in catalog["source"]], (endpoint, layer)
        for file in (f"helsinki-datasource-{name}.yaml", f"helsinki-pipeline-{name}.yaml", f"helsinki-pipeline-{name}-bento.yaml"):
            assert file in index, file


def test_the_signals_endpoint_reads_only_junctions_and_the_model_declares_them():
    endpoint = yaml.safe_load((HELSINKI / "helsinki-endpoint-traffic-signals.yaml").read_text())["spec"]
    assert endpoint["audience"] == "public"
    assert endpoint["policyRef"] == "urn:ngsi-ld:Policy:hel.fi:helsinki:public-traffic-signals"
    assert re.fullmatch(r"[a-z2-7]{32}", endpoint["slug"])
    policy = yaml.safe_load((HELSINKI / "helsinki-policy-public-traffic-signals.yaml").read_text())["spec"]
    assert policy["information"] == [{"entities": [{"type": "TrafficSignalJunction"}]}]
    assert not any(op.startswith(("create", "update", "delete", "append", "merge", "replace", "upsert"))
                   for op in policy["operations"] if "Subscription" not in op)
    model = yaml.safe_load((HELSINKI / "helsinki-datamodel.yaml").read_text())["spec"]
    assert "TrafficSignalJunction" in model["classes"]
    schema = json.loads((HELSINKI / "helsinki.v1.schema.json").read_text())["definitions"]
    linkml = yaml.safe_load((HELSINKI / "helsinki.linkml.yaml").read_text())
    for enum in ("StreetParkingCategory", "TrafficSignalKind", "TrafficSignalStatus"):
        assert schema[enum]["enum"] == list(linkml["enums"][enum]["permissible_values"]), enum
    assert set(schema["TrafficSignalJunction"]["properties"]) - {"id", "type", "location", "observedAt"} == set(
        linkml["classes"]["TrafficSignalJunction"]["slots"])
