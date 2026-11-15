"""Prague's city places from Golemio (T-5276, PL-03): Municipal Police stations, Municipal Library
branches, municipal offices, gardens, playgrounds and waste collection yards, one PointOfInterest each.

Bento runs each committed mapping over real records recorded from api.golemio.cz on 2026-10-22 at
about 20:00Z (`fixtures/praha/golemio-places.json`, two or three features per endpoint; the
libraries include Barrandov twice, under its old numeric id 2 and its slug). The edge cases are that
answer edited in the one place they name. Each entity is checked against the space's generated JSON
Schema, which the gateway enforces, and the pinned Smart Data Models PointOfInterest schema.
"""

import copy
import re
import json
import shutil
import tempfile
from pathlib import Path

import open_data
import pytest
import yaml
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-praha-golemio-places")

ROOT = Path(__file__).resolve().parent.parent
PRAHA = ROOT / "components/context-gateway/seed/praha"
RECORDED = json.loads((Path(__file__).resolve().parent / "fixtures/praha/golemio-places.json").read_text())
SPACE = "praha-mesto"
NOW = "2026-10-22T20:00:00Z"
SETS = {
    "police": ("/v2/municipalpolicestations", "police"),
    "libraries": ("/v2/municipallibraries", "library"),
    "authorities": ("/v2/municipalauthorities", "municipalAuthority"),
    "gardens": ("/v2/gardens", "garden"),
    "playgrounds": ("/v2/playgrounds", "playground"),
    "yards": ("/v2/wastecollectionyards", "wasteCollectionYard"),
}


def outcome(name: str, document: dict, now: str = NOW) -> tuple[list[dict], list[str]]:
    """The mapping of `golemio-<name>` over one answer, with now() pinned: the entities written and
    the error of a message the runner refuses (an errored message fails the run, nothing is written)."""
    mapping = PRAHA / f"praha-pipeline-golemio-{name}-bento.yaml"
    text = mapping.read_text().replace("now()", f'"{now}".ts_parse("2006-01-02T15:04:05Z07:00")')
    pipeline = yaml.safe_load(text)
    pipeline["pipeline"]["processors"].append({"mapping": 'root = if errored() { {"__error": error()} } else { this }'})
    with tempfile.TemporaryDirectory() as tmp:
        pinned = Path(tmp) / "bento.yaml"
        pinned.write_text(yaml.safe_dump(pipeline, allow_unicode=True))
        answer = open_data.run(pinned, json.dumps(document).encode(), SPACE, "praha.eu")
    if isinstance(answer, dict) and "__error" in answer:
        return [], [answer["__error"]]
    return answer, []


def run(name: str, document: dict | None = None, now: str = NOW) -> dict[str, dict]:
    entities, errors = outcome(name, RECORDED[name] if document is None else document, now)
    assert not errors, errors
    return {e["id"].rsplit(f":golemio-{name}-", 1)[1]: e for e in entities}


def edited(name: str, edit) -> dict:
    document = copy.deepcopy(RECORDED[name])
    edit(document["features"])
    return document


@pytest.fixture(scope="module")
def written():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {name: run(name) for name in SETS}


@requires_docker
def test_every_place_is_one_point_of_interest_the_gateway_and_the_sdm_accept(written):
    assert {name: sorted(places) for name, places in written.items()} == {
        "police": ["106", "107", "108"],
        "libraries": ["69", "barrandov"],
        "authorities": ["urad-mestske-casti-praha-6", sorted(written["authorities"])[1]],
        "gardens": sorted(written["gardens"]),
        "playgrounds": sorted(written["playgrounds"]),
        "yards": sorted(written["yards"]),
    }
    for name, (path, category) in SETS.items():
        assert len(written[name]) >= 2, name
        for entity in written[name].values():
            assert entity["id"].startswith(f"urn:ngsi-ld:PointOfInterest:praha.eu:{SPACE}:golemio-{name}-")
            assert entity["serviceCategory"]["value"] == category
            assert entity["source"]["value"] == "https://api.golemio.cz" + path
            assert entity["dataProvider"]["value"] == "Golemio, Operátor ICT, a.s."
            assert entity["dateModified"]["value"].endswith("Z")
            assert not open_data.schema_errors(PRAHA / "praha.v1.schema.json", entity), entity["id"]
            assert not open_data.sdm_errors("dataModel.PointOfInterest/PointOfInterest", entity), entity["id"]


@requires_docker
def test_a_police_desk_is_named_by_its_unit_and_street(written):
    desk = written["police"]["106"]
    assert desk["name"]["languageMap"] == {"cs": "Městská policie Praha: OŘ MP Praha 1, Opletalova 19"}
    assert desk["facilityType"]["value"] == "OŘ MP Praha 1"
    assert desk["address"]["value"] == {"streetAddress": "Opletalova 19", "addressLocality": "Praha", "addressCountry": "CZ"}


@requires_docker
def test_library_hours_are_the_staffed_week_grouped_by_day(written):
    barrandov = written["libraries"]["barrandov"]
    # Self-service hours (st, čt 9-13) and the closure of 28 October stay out.
    assert barrandov["openingHours"]["languageMap"]["cs"] == "po 13:00-19:00\nút 09:00-17:00\nst–čt 09:00-19:00\npá 09:00-17:00"
    assert barrandov["url"]["value"] == "https://www.mlp.cz/cz/pobocky/barrandov"


@requires_docker
def test_a_web_address_without_a_scheme_gets_https_and_free_text_hours_are_kept(written):
    office = written["authorities"]["urad-mestske-casti-praha-6"]
    assert office["url"]["value"] == "https://www.praha6.cz"
    assert office["facilityType"]["value"] == "Obecní úřad"
    yard = next(iter(written["yards"].values()))
    assert yard["openingHours"]["languageMap"]["cs"].startswith("Po - So 8:30 - 18:00")


@requires_docker
def test_a_place_listed_twice_keeps_the_newer_record(written):
    # Golemio lists Barrandov as id 2 (2026-01-09) and as barrandov (2026-10-16), at one point.
    assert "2" not in written["libraries"]
    assert written["libraries"]["barrandov"]["dateModified"]["value"].startswith("2026-10-16")


@requires_docker
def test_out_of_area_zero_and_swapped_positions_are_dropped():
    def edit(features):
        features[0]["geometry"]["coordinates"] = [0, 0]
        features[1]["geometry"]["coordinates"] = list(reversed(features[1]["geometry"]["coordinates"]))
        features[2]["geometry"]["coordinates"] = [16.6, 49.2]
    assert run("police", edited("police", edit)) == {}


@requires_docker
def test_a_record_without_a_name_or_a_bad_web_address_is_handled():
    def edit(features):
        features[0]["properties"]["name"] = "  "
        features[1]["properties"]["url"] = "javascript:alert(1)"
    playgrounds = run("playgrounds", edited("playgrounds", edit))
    first, second = (str(f["properties"]["id"]) for f in RECORDED["playgrounds"]["features"][:2])
    assert first not in playgrounds
    assert "url" not in playgrounds[second]
    assert not open_data.schema_errors(PRAHA / "praha.v1.schema.json", playgrounds[second])


@requires_docker
@pytest.mark.parametrize("name", [n for n in SETS if n != "police"])
def test_only_a_police_desk_falls_back_to_the_city_police_name(name):
    """F-R2BNT3-006: the six feeds share one template, and a nameless record with a note was named
    "Městská policie Praha: <note>" in every one of them, a library or a playground too."""
    def edit(features):
        features[0]["properties"]["name"] = ""
        features[0]["properties"]["note"] = "vchod z ulice"
    places = run(name, edited(name, edit))
    assert str(RECORDED[name]["features"][0]["properties"]["id"]) not in places
    assert not any("Městská policie" in json.dumps(e, ensure_ascii=False) for e in places.values())


@requires_docker
@pytest.mark.parametrize("document,reason", [
    ({"type": "FeatureCollection", "features": []}, "listed no place"),
    ({"error_message": "Forbidden", "error_status": 403}, "not a feature list"),
])
def test_an_empty_or_broken_answer_fails_the_run_instead_of_emptying_the_places(document, reason):
    entities, errors = outcome("yards", document)
    assert entities == [] and len(errors) == 1 and reason in errors[0], errors


@requires_docker
def test_a_stale_register_fails_the_run():
    entities, errors = outcome("police", RECORDED["police"], now="2027-02-01T00:00:00Z")
    assert entities == [] and len(errors) == 1 and "older than 100 days" in errors[0], errors


def test_each_pipeline_reads_its_golemio_endpoint_by_the_secret_into_the_city_space():
    for name, (path, _) in SETS.items():
        source = yaml.safe_load((PRAHA / f"praha-datasource-golemio-{name}.yaml").read_text())
        http = source["spec"]["http"]
        assert http["url"] == f"https://api.golemio.cz{path}?limit=10000"
        assert http["authorization"] == {"header": "X-Access-Token", "headerRef": {"name": "golemio", "key": "token"}}
        pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-golemio-{name}.yaml").read_text())
        assert pipeline["spec"]["source"]["dataSourceRef"]["name"] == f"golemio-{name}"
        assert pipeline["spec"]["period"] in ("24h", "168h")
        assert pipeline["spec"]["output"] == {"type": "PointOfInterest", "mode": "upsert"}
        assert pipeline["spec"]["targetEndpoint"].endswith(":praha-mesto:praha-mesto")


def test_the_cc_by_and_the_golemio_places_are_separate_datasets():
    mista = yaml.safe_load((PRAHA / "praha-policy-public-mista.yaml").read_text())
    sluzby = yaml.safe_load((PRAHA / "praha-policy-public-sluzby.yaml").read_text())
    (mista_selector,) = mista["spec"]["information"][0]["entities"]
    (sluzby_selector,) = sluzby["spec"]["information"][0]["entities"]

    golemio = f"urn:ngsi-ld:PointOfInterest:praha.eu:{SPACE}:golemio-libraries-barrandov"
    ipr = f"urn:ngsi-ld:PointOfInterest:praha.eu:{SPACE}:ipr-0b1c2d3e-0000-4000-8000-000000000000"
    assert re.fullmatch(sluzby_selector["idPattern"], golemio) and not re.fullmatch(sluzby_selector["idPattern"], ipr)
    assert re.fullmatch(mista_selector["idPattern"], ipr) and not re.fullmatch(mista_selector["idPattern"], golemio)
    # The ticket-points mapping keeps a PID id of [0-9A-Za-z_-]+, lowercased: an underscore stays in mista.
    assert re.fullmatch(mista_selector["idPattern"], f"urn:ngsi-ld:PointOfInterest:praha.eu:{SPACE}:pid-dp_12")
    endpoint = yaml.safe_load((PRAHA / "praha-endpoint-sluzby.yaml").read_text())
    assert endpoint["spec"]["policyRef"].endswith(":public-sluzby")
    assert "license" not in endpoint["spec"]["catalog"]
    # Off the catalogue while prod holds none of it (T-5788, owner 2026-10-26): no publish block, harvest false.
    assert "publish" not in endpoint["spec"] and endpoint["spec"]["catalog"]["harvest"] is False
