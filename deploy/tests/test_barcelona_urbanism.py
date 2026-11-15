"""Barcelona open data batch 3, urbanism (T-5264, PL-03, DM-01, DM-61): districts, barris, works,
pedestrian crossings.

Bento runs the committed mappings over recordings of Open Data BCN's files as the DataSources'
URLs answered them on 2026-10-23 (04:34Z), trimmed to a few rows each and without the ETRS89
column the mappings never read: `fixtures/barcelona/districtes.csv` (districts 01, 06, 10 of
BarcelonaCiutat_Districtes.csv), `barris.csv` (barris 01, 02, 28, 29, 68 of
BarcelonaCiutat_Barris.csv), `obres.json` (28 rows, 19 works, of obres_espai_public.json) and
`pas-vianants.csv` (10 rows of Infraestructures_Inventari_Pas_Vianants.csv as of 2026-10-23T06:05Z,
without the ETRS89 columns: two current crossings of each kind, the one pair the file lists twice
within a metre, one removed crossing and one on the city's edge without a barri).
Every entity is checked against the space's generated JSON Schema, which the gateway enforces,
and against its Smart Data Model at the vendored pin. The edge cases are those recordings edited
in the one place each names.

Without a container runtime the Bento cases skip; the seed cases do not.
"""

import csv
import io
import json
import os
import shutil
import tempfile
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-barcelona-urbanism")

ROOT = Path(__file__).resolve().parent.parent
CITY = ROOT / "components/context-gateway/seed/barcelona"
FIXTURES = Path(__file__).resolve().parent / "fixtures/barcelona"
DISTRICTS = CITY / "barcelona-pipeline-districts-bento.yaml"
BARRIS = CITY / "barcelona-pipeline-neighbourhoods-bento.yaml"
WORKS = CITY / "barcelona-pipeline-city-works-bento.yaml"
CROSSINGS = CITY / "barcelona-pipeline-pedestrian-crossings-bento.yaml"
AREA = "urn:ngsi-ld:AdministrativeArea:barcelona.cat:barcelona:"
WORK = "urn:ngsi-ld:CityWork:barcelona.cat:barcelona:"
CROSSING = "urn:ngsi-ld:PedestrianCrossing:barcelona.cat:barcelona:"
AS_OF = {"bcn_now": "2026-10-23T04:30:00Z"}
BBOX = yaml.safe_load((CITY / "barcelona-space.yaml").read_text())["spec"]["quality"][0]["bbox"]
SCHEMA = CITY / "barcelona.v1.schema.json"


def read(name: str) -> str:
    return (FIXTURES / name).read_text()


def run(mapping: Path, document: str, now: dict | None = None) -> list[dict]:
    return open_data.run_all(mapping, document.encode(), "barcelona", "barcelona.cat", now)


def first_step(mapping: Path, document: str, now: dict | None = None) -> dict:
    """What the first step answers before the last keeps only the entities: its `dropped` counts
    and entities, or the error it raised. The trimmed pipeline is a temp file of its own, so
    xdist workers never share it and no seed glob reads it."""
    handle, name = tempfile.mkstemp(suffix=".yaml")
    os.close(handle)
    trimmed = Path(name)
    pipeline = yaml.safe_load(mapping.read_text())
    pipeline["pipeline"]["processors"] = pipeline["pipeline"]["processors"][:1] + [
        {"mapping": 'root = if errored() { { "error": error() } } else { this }'}]
    trimmed.write_text(yaml.safe_dump(pipeline))
    try:
        return run(trimmed, document, now)[0]
    finally:
        trimmed.unlink()


def csv_edited(name: str, edit) -> str:
    rows = list(csv.reader(io.StringIO(read(name))))
    edit(rows)
    out = io.StringIO()
    csv.writer(out, quoting=csv.QUOTE_ALL, lineterminator="\n").writerows(rows)
    return out.getvalue()


def works_edited(edit) -> str:
    rows = json.loads(read("obres.json"))
    edit(rows)
    return json.dumps(rows, ensure_ascii=False)


def points(geometry: dict) -> list:
    rings = geometry["coordinates"] if geometry["type"] == "Polygon" else [r for p in geometry["coordinates"] for r in p]
    return [p for ring in rings for p in ring]


def needs_docker():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")


@pytest.fixture(scope="module")
def areas():
    needs_docker()
    return {e["id"]: e for e in run(DISTRICTS, read("districtes.csv")) + run(BARRIS, read("barris.csv"))}


@pytest.fixture(scope="module")
def works():
    needs_docker()
    return {e["id"]: e for e in run(WORKS, read("obres.json"), AS_OF)}


# Districts and barris.

@requires_docker
def test_every_district_and_barri_of_the_file_is_one_area_with_its_outline(areas):
    assert sorted(areas) == sorted([AREA + f"district-{c}" for c in ("01", "06", "10")]
                                   + [AREA + f"neighbourhood-{c}" for c in ("01", "02", "28", "29", "68")])
    ciutat_vella = areas[AREA + "district-01"]
    assert ciutat_vella["name"] == {"type": "Property", "value": "Ciutat Vella"}
    assert ciutat_vella["divisionLevel"]["value"] == "district"
    assert ciutat_vella["location"]["value"]["type"] == "Polygon"
    raval = areas[AREA + "neighbourhood-01"]
    assert raval["name"]["value"] == "el Raval" and raval["divisionLevel"]["value"] == "neighbourhood"
    # The barri points at its district, which the districts pipeline writes.
    assert raval["refParentDistrict"] == {"type": "Relationship", "object": AREA + "district-01"}
    gracia = areas[AREA + "neighbourhood-28"]
    assert gracia["refParentDistrict"]["object"] in areas
    # The WKT's first vertex, read as GeoJSON with longitude first.
    row = next(r for r in csv.DictReader(io.StringIO(read("barris.csv"))) if r["codi_barri"] == "01")
    lon, lat = row["geometria_wgs84"].split("((")[1].split(",")[0].split()
    assert raval["location"]["value"]["coordinates"][0][0] == [float(lon), float(lat)]


@requires_docker
def test_every_area_is_one_the_gateway_accepts_and_valid_in_its_smart_data_model(areas):
    for entity in areas.values():
        assert open_data.schema_errors(SCHEMA, entity) == [], entity["id"]
        assert open_data.sdm_errors("dataModel.extensions/AdministrativeArea", entity) == [], entity["id"]


@requires_docker
def test_every_vertex_of_every_area_stands_inside_the_city(areas):
    for entity in areas.values():
        for lon, lat in points(entity["location"]["value"]):
            assert BBOX[0] <= lon <= BBOX[2] and BBOX[1] <= lat <= BBOX[3], entity["id"]


@requires_docker
def test_an_outside_vertex_a_bad_code_and_a_duplicate_area_are_each_dropped_and_counted():
    def edit(rows):
        rows.append(list(rows[1]))                                      # Ciutat Vella twice
        rows[2][2] = rows[2][2].replace("POLYGON ((", "POLYGON ((0 0, ", 1)  # a vertex at 0,0
        rows.append(["1", "Nowhere", "POLYGON ((2.1 41.4, 2.11 41.4, 2.1 41.41, 2.1 41.4))"])
    answer = first_step(DISTRICTS, csv_edited("districtes.csv", edit))
    assert answer["dropped"] == {"noCodeOrName": 1, "noPolygon": 0, "outsideTheCity": 1, "duplicateCode": 1}
    assert [e["districtCode"]["value"] for e in answer["entities"]] == ["01", "10"]


@requires_docker
def test_a_file_without_an_area_fails_the_run_instead_of_writing_nothing():
    assert "holds no district" in first_step(DISTRICTS, read("districtes.csv").splitlines()[0] + "\n")["error"]
    assert "holds no neighbourhood" in first_step(BARRIS, read("barris.csv").splitlines()[0] + "\n")["error"]


# Works on the public space.

@requires_docker
def test_the_current_works_are_one_entity_each_and_the_old_ones_stay_in_the_source(works):
    rows = json.loads(read("obres.json"))
    current = {r["codi"] for r in rows if r["estat"] != "Finalitzada" or r["data_fi"] >= "2026-09-23"}
    # 02-2023G0020 ends before it starts, 11-2026G0001 has no outline ("None"): both dropped.
    assert sorted(works) == sorted(WORK + c for c in current - {"02-2023G0020", "11-2026G0001"})
    assert WORK + "08-2025G0006" not in works, "finished 2025-12-09, more than 30 days ago"


@requires_docker
def test_a_work_of_several_rows_is_one_entity_naming_every_district(works):
    rows = [r for r in json.loads(read("obres.json")) if r["codi"] == "04-2007S0009"]
    assert len(rows) == 4
    work = works[WORK + "04-2007S0009"]
    names = sorted({r["nom_districte"] for r in rows})
    assert work["territorialArea"]["value"] == "; ".join(names)
    if len({r["codi_districte"] for r in rows}) > 1:
        assert "refDistrict" not in work, "a work in several districts points at none of them"
    single = works[WORK + "02-2025G0068"]
    assert single["refDistrict"] == {"type": "Relationship", "object": AREA + "district-02"}


@requires_docker
def test_a_work_carries_the_source_fields_by_their_smart_data_model_names(works):
    row = next(r for r in json.loads(read("obres.json")) if r["codi"] == "02-2025G0068")
    work = works[WORK + "02-2025G0068"]
    assert work["workNumber"]["value"] == "02-2025G0068"
    assert work["workState"]["value"] == "open" and work["workStateLabel"]["value"] == "En execució"
    assert work["name"]["value"] == row["titol"]
    assert work["workCategory"]["value"] == row["tipusobra"]
    assert work["contractingAuthority"]["value"] == row["promotor"]
    assert "constructor" not in json.dumps(work) and row["constructor"] not in json.dumps(work, ensure_ascii=False)
    # 2025-12-15 local midnight (CET, +01:00) and 2026-10-16 (CEST, +02:00), as UTC.
    assert work["startDate"]["value"] == "2025-12-14T23:00:00Z"
    assert work["endDate"]["value"] == "2026-10-15T22:00:00Z"
    # In execution: it leaves every read three days after the file last listed it.
    assert work["expiresAt"] == "2026-10-26T04:30:00Z"
    stopped = works[WORK + "05-2025G0071"]
    assert stopped["workState"]["value"] == "other" and stopped["workStateLabel"]["value"] == "Aturada"
    finished = works[WORK + "05-2025G0070"]
    # Finished 2026-10-02: its 30 days run to 2026-11-01, after the three-day lease, so the lease stands.
    assert finished["workState"]["value"] == "completed"
    assert finished["expiresAt"] == "2026-10-26T04:30:00Z"


@requires_docker
def test_every_work_is_one_the_gateway_accepts_and_valid_in_its_smart_data_model(works):
    assert works
    for entity in works.values():
        assert open_data.schema_errors(SCHEMA, entity) == [], entity["id"]
        assert open_data.sdm_errors("dataModel.Transportation/CityWork", entity) == [], entity["id"]


@requires_docker
def test_every_work_stands_inside_the_city_with_plausible_dates_and_budgets(works):
    for entity in works.values():
        for lon, lat in points(entity["location"]["value"]):
            assert BBOX[0] <= lon <= BBOX[2] and BBOX[1] <= lat <= BBOX[3], entity["id"]
        assert entity["startDate"]["value"] <= entity["endDate"]["value"], entity["id"]
        for name in ("tenderAmount", "awardAmount"):
            if name in entity:
                assert 0 <= entity[name]["value"] <= 1_000_000_000, (entity["id"], name)


@requires_docker
def test_an_outside_outline_a_bad_budget_a_duplicate_and_a_person_are_each_dropped_and_counted():
    def edit(rows):
        open_rows = [r for r in rows if r["codi"] == "02-2025G0068"]
        rows.append(dict(open_rows[0]))                                       # the same row twice
        outside = next(r for r in rows if r["codi"] == "10-2025G0120")
        outside["geometria_wgs84"] = "POLYGON ((2.5 41.4, 2.51 41.4, 2.5 41.41, 2.5 41.4))"
        budget = next(r for r in rows if r["codi"] == "06-2025G0001")
        budget["pressupost_licitacio"], budget["pressupost_adjudicacio"] = "-5", "5000000000"
        person = next(r for r in rows if r["codi"] == "09-2026G0005")
        person["promotor"] = "Joan Exemple Prova"
        missing = next(r for r in rows if r["codi"] == "08-2025G0053")
        missing["geometria_wgs84"] = "None"
    answer = first_step(WORKS, works_edited(edit), AS_OF)
    baseline = first_step(WORKS, read("obres.json"), AS_OF)["dropped"]
    assert baseline == {"noCode": 0, "unknownState": 0, "noDate": 0, "endBeforeStart": 1, "finishedOver30DaysAgo": 8,
                        "noOutline": 1, "outsideTheCity": 0, "rowPerAreaMerged": 3, "promoterLeftOut": 0,
                        "budgetOutOfRange": 0}
    # Each edit moves its own count by one (two for the two budgets), and nothing else.
    assert answer["dropped"] == {**baseline, "outsideTheCity": 1, "noOutline": 2, "rowPerAreaMerged": 4,
                                 "budgetOutOfRange": 2, "promoterLeftOut": 1}
    by_id = {e["id"]: e for e in answer["entities"]}
    assert WORK + "10-2025G0120" not in by_id and WORK + "08-2025G0053" not in by_id
    assert "tenderAmount" not in by_id[WORK + "06-2025G0001"] and "awardAmount" not in by_id[WORK + "06-2025G0001"]
    assert "contractingAuthority" not in by_id[WORK + "09-2026G0005"]
    assert "Joan Exemple" not in json.dumps(answer["entities"])


@requires_docker
def test_a_file_without_a_work_in_execution_fails_the_run_instead_of_emptying_the_space():
    assert "no work in execution" in first_step(WORKS, "[]", AS_OF)["error"]
    finished = works_edited(lambda rows: [r.update(estat="Finalitzada") for r in rows])
    assert "no work in execution" in first_step(WORKS, finished, AS_OF)["error"]


@requires_docker
def test_the_file_s_dates_read_alike_with_and_without_milliseconds(works):
    # The file wrote "2025-12-15 00:00:00.000" until 2026-10-23 05:31Z and "2026-03-02 00:00:00" since.
    plain = works_edited(lambda rows: [r.update(data_inici=r["data_inici"][:19], data_fi=r["data_fi"][:19]) for r in rows])
    assert "00:00:00.000" not in plain
    assert {e["id"]: e for e in run(WORKS, plain, AS_OF)} == works


@requires_docker
def test_a_promoter_stands_only_as_a_known_body_or_a_company():
    names = {"02-2025G0068": "FIRA BARCELONA", "06-2025G0001": "Exemple Obres, S.L.",
             "05-2025G0071": "ACME", "09-2026G0005": "Port Exemple"}
    answer = first_step(WORKS, works_edited(lambda rows: [r.update(promotor=names[r["codi"]]) for r in rows
                                                         if r["codi"] in names]), AS_OF)
    kept = {e["workNumber"]["value"]: e.get("contractingAuthority", {}).get("value") for e in answer["entities"]}
    assert kept["02-2025G0068"] == "FIRA BARCELONA" and kept["06-2025G0001"] == "Exemple Obres, S.L."
    # One unknown word or a name without a legal form may be a person's: left out and counted.
    assert kept["05-2025G0071"] is None and kept["09-2026G0005"] is None
    assert answer["dropped"]["promoterLeftOut"] == 2


# Pedestrian crossings.

CROSSINGS_AS_OF = {"bcn_now": "2026-10-23T06:00:00Z"}


@pytest.fixture(scope="module")
def crossings():
    needs_docker()
    return {e["id"]: e for e in run(CROSSINGS, read("pas-vianants.csv"), CROSSINGS_AS_OF)}


@requires_docker
def test_every_current_crossing_is_one_point_with_its_kind_area_and_barri(crossings, areas):
    rows = list(csv.DictReader(io.StringIO(read("pas-vianants.csv"))))
    # 9690076 left the street on 2024-07-18; 9290683 is the older record of 9450901's crossing.
    assert sorted(crossings) == sorted(CROSSING + r["ID_Pas"] for r in rows if r["ID_Pas"] not in ("9690076", "9290683"))
    raval = crossings[CROSSING + "10815385"]
    assert raval["crossingType"]["value"] == "stripes" and raval["crossingTypeLabel"]["value"] == "Pas de pastilles"
    assert raval["markedArea"] == {"type": "Property", "value": 13.48, "unitCode": "MTK"}
    assert raval["location"]["value"] == {"type": "Point", "coordinates": [2.1733816, 41.3782448]}
    assert raval["refNeighbourhood"] == {"type": "Relationship", "object": AREA + "neighbourhood-01"}
    assert raval["refNeighbourhood"]["object"] in areas
    assert crossings[CROSSING + "8788772"]["refNeighbourhood"]["object"] == AREA + "neighbourhood-68"
    assert crossings[CROSSING + "10951039"]["crossingType"]["value"] == "blocks"
    assert crossings[CROSSING + "9450901"]["crossingType"]["value"] == "cycle"
    assert "refNeighbourhood" not in crossings[CROSSING + "8002288"], "on the city's edge, in no barri"
    # Three weekly periods after the run.
    assert {e["expiresAt"] for e in crossings.values()} == {"2026-11-13T06:00:00Z"}


@requires_docker
def test_every_crossing_is_one_the_gateway_accepts_inside_the_city(crossings):
    assert crossings
    for entity in crossings.values():
        assert open_data.schema_errors(SCHEMA, entity) == [], entity["id"]
        lon, lat = entity["location"]["value"]["coordinates"]
        assert BBOX[0] <= lon <= BBOX[2] and BBOX[1] <= lat <= BBOX[3], entity["id"]
        assert 0 < entity["markedArea"]["value"] <= 500, entity["id"]


@requires_docker
def test_an_outside_point_a_bad_area_a_duplicate_and_an_unknown_kind_are_each_dropped_and_counted():
    def edit(rows):
        rows.append(list(rows[1]))                       # 10815385 twice
        rows[2][7], rows[2][8] = rows[2][8], rows[2][7]  # 10815386 with latitude and longitude swapped
        rows[3][2] = "900"                               # 10951039 with 900 m² painted
        rows[4][1] = "000255000_Desconegut"              # 10874075 of a kind the typology lacks
        rows[5][7] = "0"                                 # 8788772 at longitude 0
    baseline = first_step(CROSSINGS, read("pas-vianants.csv"), CROSSINGS_AS_OF)["dropped"]
    assert baseline == {"removed": 1, "noIdOrKind": 0, "noPoint": 0, "outsideTheCity": 0, "duplicateId": 0,
                        "sameKindWithin1m": 1, "areaOutOfRange": 0}
    answer = first_step(CROSSINGS, csv_edited("pas-vianants.csv", edit), CROSSINGS_AS_OF)
    assert answer["dropped"] == {**baseline, "noIdOrKind": 1, "outsideTheCity": 2, "duplicateId": 1, "areaOutOfRange": 1}
    ids = [c["id"] for c in answer["crossings"]]
    assert "10815386" not in ids and "8788772" not in ids and "10874075" not in ids and ids.count("10815385") == 1
    entities = {e["id"]: e for e in run(CROSSINGS, csv_edited("pas-vianants.csv", edit), CROSSINGS_AS_OF)}
    assert "markedArea" not in entities[CROSSING + "10951039"], "an implausible area is left out, the crossing stays"


@requires_docker
def test_a_file_without_a_current_crossing_fails_the_run_instead_of_writing_nothing():
    lines = read("pas-vianants.csv").splitlines()
    assert "no current crossing" in first_step(CROSSINGS, "\n".join([lines[0], lines[9]]) + "\n")["error"]
    assert "no current crossing" in first_step(CROSSINGS, lines[0] + "\n")["error"]
    # A reordered file whose last column is no longer Data_Baixa: refused, never read as all current.
    assert "Data_Baixa column last" in first_step(CROSSINGS, read("pas-vianants.csv").replace(",Data_Baixa", ",Data_Baixa,Extra"))["error"]


# The seed.

def test_each_dataset_has_its_pipeline_endpoint_and_a_policy_of_its_own_type():
    docs = {}
    for path in CITY.glob("*.yaml"):
        if path.name == "index.yaml" or path.name.endswith((".linkml.yaml", "-bento.yaml")):
            continue
        doc = yaml.safe_load(path.read_text())
        docs[(doc["kind"], doc["metadata"]["name"])] = doc["spec"]
    for pipeline, kind in (("districts", "AdministrativeArea"), ("neighbourhoods", "AdministrativeArea"),
                           ("city-works", "CityWork"), ("pedestrian-crossings", "PedestrianCrossing")):
        spec = docs[("Pipeline", pipeline)]
        assert spec["output"] == {"type": kind, "mode": "upsert"}, "upsert: an empty file never deletes"
        assert ("DataSource", spec["source"]["dataSourceRef"]["name"]) in docs
        url = docs[("DataSource", spec["source"]["dataSourceRef"]["name"])]["http"]["url"]
        assert url.startswith("https://opendata-ajuntament.barcelona.cat/resources/bcn/"), "the allow-listed host"
    for endpoint, kind in (("areas", "AdministrativeArea"), ("city-works", "CityWork"),
                           ("pedestrian-crossings", "PedestrianCrossing")):
        spec = docs[("Endpoint", endpoint)]
        policy = docs[("Policy", spec["policyRef"].rsplit(":", 1)[1])]
        assert policy["assignee"] == {"kind": "role", "id": "public"}
        assert [e["type"] for rule in policy["information"] for e in rule["entities"]] == [kind]
        assert all(op.startswith(("retrieve", "createSub", "updateSub", "deleteSub")) for op in policy["operations"])
        # City works leave the catalogue while prod holds none of them (T-5788, owner 2026-10-26).
        if spec["catalog"].get("harvest") is False:
            assert "publish" not in spec
        else:
            assert spec["publish"]["ckan"]["instanceRef"] == {"kind": "CkanInstance", "name": "barcelona"}
        assert {"ngsi-ld", "csv", "ogc-features", "odata"} <= set(spec["enabledRepresentations"])


@pytest.mark.parametrize("name", ["districts", "neighbourhoods", "city-works", "pedestrian-crossings"])
def test_each_pipeline_runs_by_its_schedule_alone(name):
    # The runner reads a period before a schedule, so a period beside it would drop the time chosen
    # after the source's rewrite.
    spec = yaml.safe_load((CITY / f"barcelona-pipeline-{name}.yaml").read_text())["spec"]
    assert "period" not in spec and len(spec["schedule"].split()) == 5
