"""Barcelona open data, urbanism rest (T-5431, PL-03, DM-01, DM-61): speed reducers as
StreetFurniture, drinking and ornamental fountains as PointOfInterest.

Bento runs the committed mappings over recordings of Open Data BCN as it answered on 2026-10-24,
trimmed to a few rows each and without the ETRS89 columns the mappings never read:
`fixtures/barcelona/reductors-velocitat.csv` (one reducer of each model code of
Infrastruc_Mobiliari_Urba_Reductors_Velocitat.csv), `fonts.csv` and `fonts-ornamentals.csv` (four
rows each of fonts_beure_2026.csv and fonts_ornamentals_2026.csv, a drinking fountain with no name
among them), and `package-fonts.json`, `package-fonts-ornamentals.json` (package_show of each, its
first three and its last resource). The fountains' file fetch is the cut: the recorded CSV stands
in for the http step's answer. Every entity is checked against the space's generated JSON Schema,
which the gateway enforces; the fountains against their Smart Data Model too. The edge cases are
those recordings edited in the one place each names. Without a container runtime the Bento cases
skip; the seed cases do not.
"""

import csv
import io
import json
import os
import tempfile
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-barcelona-street-inventory")

ROOT = Path(__file__).resolve().parent.parent
CITY = ROOT / "components/context-gateway/seed/barcelona"
FIXTURES = Path(__file__).resolve().parent / "fixtures/barcelona"
REDUCERS = CITY / "barcelona-pipeline-speed-reducers-bento.yaml"
DRINKING = CITY / "barcelona-pipeline-drinking-fountains-bento.yaml"
ORNAMENTAL = CITY / "barcelona-pipeline-ornamental-fountains-bento.yaml"
FURNITURE = "urn:ngsi-ld:StreetFurniture:barcelona.cat:barcelona:speed-reducer-"
PLACE = "urn:ngsi-ld:PointOfInterest:barcelona.cat:barcelona:"
AS_OF = {"bcn_now": "2026-10-24T06:00:00Z"}
SCHEMA = CITY / "barcelona.v1.schema.json"
BBOX = [2.04, 41.3, 2.24, 41.48]


def read(name: str) -> str:
    return (FIXTURES / name).read_text()


def csv_edited(name: str, edit) -> str:
    rows = list(csv.reader(io.StringIO(read(name).lstrip("﻿"))))
    edit(rows)
    out = io.StringIO()
    csv.writer(out, quoting=csv.QUOTE_ALL, lineterminator="\n").writerows(rows)
    return out.getvalue()


def run(mapping: Path, document: str, answer: str | None = None, steps: int | None = None) -> list[dict]:
    """The mapping over `document` with the run's clock at AS_OF. A fountains mapping fetches its
    file: `answer` stands in for that http step. `steps` keeps only the first ones, then a step
    that answers the error a step raised, so a test reads the counts or the refusal."""
    pipeline = yaml.safe_load(mapping.read_text())
    processors = pipeline["pipeline"]["processors"]
    for i, step in enumerate(processors):
        if "http" in step:
            assert step["http"]["url"] == "https://opendata-ajuntament.barcelona.cat/data/dataset/${! @file }"
            processors[i] = {"mapping": 'root = if @file.or("") == "" { throw("no file picked") } else { %s }' % json.dumps(answer or "")}
    if steps is not None:
        del processors[steps:]
        processors.append({"mapping": 'root = if errored() { { "error": error() } } else { this }'})
    handle, name = tempfile.mkstemp(suffix=".yaml")
    os.close(handle)
    trimmed = Path(name)
    trimmed.write_text(yaml.safe_dump(pipeline))
    try:
        return open_data.run_all(trimmed, document.encode(), "barcelona", "barcelona.cat", AS_OF)
    finally:
        trimmed.unlink()


def counts(mapping: Path, document: str, answer: str | None = None) -> dict:
    """The first mapping step after the fetch: its `dropped` counts and rows, or its error."""
    position = next(i for i, s in enumerate(yaml.safe_load(mapping.read_text())["pipeline"]["processors"])
                    if "log" in s)
    return run(mapping, document, answer, steps=position)[0]


def inside(entity: dict) -> bool:
    lon, lat = entity["location"]["value"]["coordinates"]
    return BBOX[0] <= lon <= BBOX[2] and BBOX[1] <= lat <= BBOX[3]


# Speed reducers.

@pytest.fixture(scope="module")
def reducers():
    return {e["id"]: e for e in run(REDUCERS, read("reductors-velocitat.csv"))}


@requires_docker
def test_every_reducer_is_one_street_furniture_with_its_kind_model_and_address(reducers):
    assert sorted(reducers) == sorted(FURNITURE + i for i in ("1370805", "1319712", "1307524", "1415305"))
    cushion = reducers[FURNITURE + "1370805"]
    assert cushion["furnitureType"]["value"] == "speedCushion"
    assert cushion["furnitureTypeLabel"]["value"] == "Coixí berlinés"
    assert cushion["address"]["value"] == {"streetAddress": "PROVENÇA, C., DE 517", "addressLocality": "Barcelona",
                                           "district": "Eixample", "addressCountry": "ES"}
    assert {reducers[FURNITURE + i]["furnitureType"]["value"] for i in ("1319712", "1307524", "1415305")} == {"speedBump"}
    assert {e["expiresAt"] for e in reducers.values()} == {"2026-11-14T06:00:00Z"}
    for entity in reducers.values():
        assert open_data.schema_errors(SCHEMA, entity) == [], entity["id"]
        assert inside(entity), entity["id"]


@requires_docker
def test_a_removed_an_outside_a_duplicate_and_an_unknown_reducer_are_each_dropped_and_counted():
    def edit(rows):
        header = rows[0]
        lon, lat, gone, code = (header.index(c) for c in ("longitud", "latitud", "data_baixa", "codi"))
        rows.append(list(rows[1]))                            # 1370805 twice
        rows[2][lon], rows[2][lat] = rows[2][lat], rows[2][lon]  # 1319712 with its axes swapped
        rows[3][gone] = "2025-04-01 00:00:00.000"             # 1307524 left the street
        rows[4][code] = "Reductor nou"                        # 1415305 of a model the mapping lacks
    baseline = counts(REDUCERS, read("reductors-velocitat.csv"))["dropped"]
    assert baseline == {"removed": 0, "noIdOrKind": 0, "noPoint": 0, "outsideTheCity": 0, "duplicateId": 0}
    answer = counts(REDUCERS, csv_edited("reductors-velocitat.csv", edit))
    assert answer["dropped"] == {"removed": 1, "noIdOrKind": 1, "noPoint": 0, "outsideTheCity": 1, "duplicateId": 1}
    assert [r["id"] for r in answer["reducers"]] == ["1370805"]


@requires_docker
def test_a_file_without_a_current_reducer_fails_the_run():
    header = read("reductors-velocitat.csv").splitlines()[0]
    assert "no current reducer" in counts(REDUCERS, header + "\n")["error"]


# Fountains.

@pytest.fixture(scope="module")
def fountains():
    drinking = run(DRINKING, read("package-fonts.json"), read("fonts.csv"))
    ornamental = run(ORNAMENTAL, read("package-fonts-ornamentals.json"), read("fonts-ornamentals.csv"))
    return {e["id"]: e for e in drinking + ornamental}


@requires_docker
def test_every_fountain_is_one_place_of_its_category_and_the_two_registers_never_share_an_id(fountains):
    drinking = [PLACE + "drinking-fountain-" + c for c in ("01-004", "01-005", "01-162", "10-001")]
    ornamental = [PLACE + "ornamental-fountain-" + c for c in ("01-001", "01-002", "03-069", "03-115")]
    assert sorted(fountains) == sorted(drinking + ornamental)
    assert fountains[drinking[0]]["name"]["value"] == "Sant Carles"
    assert fountains[drinking[0]]["category"]["value"] == ["drinking-fountain"]
    assert fountains[drinking[0]]["address"]["value"]["streetAddress"] == "C COMTE DE SANTA CLARA 11"
    # 349 of the 1,760 drinking fountains have no name in the 2026 file: the code names them.
    assert fountains[PLACE + "drinking-fountain-01-162"]["name"]["value"] == "Font de beure 01-162"
    # The ornamental file writes the code before most names; it is cut, with or without its dot.
    assert fountains[ornamental[0]]["name"]["value"] == "Font d'Emili Vendrell"
    assert fountains[ornamental[3]]["name"]["value"] == "Font del Carrer Foc - Linial"
    assert fountains[ornamental[0]]["category"]["value"] == ["ornamental-fountain"]
    for entity in fountains.values():
        assert open_data.schema_errors(SCHEMA, entity) == [], entity["id"]
        assert not open_data.sdm_errors("dataModel.PointOfInterest/PointOfInterest", entity), entity["id"]
        assert inside(entity), entity["id"]
        assert entity["expiresAt"] == "2026-11-14T06:00:00Z"


@requires_docker
def test_the_run_reads_the_newest_csv_file_of_the_package():
    first = yaml.safe_load(DRINKING.read_text())["pipeline"]["processors"][:1]
    handle, name = tempfile.mkstemp(suffix=".yaml")
    os.close(handle)
    probe = Path(name)
    probe.write_text(yaml.safe_dump({"pipeline": {"processors": first + [{"mapping": "root.file = @file"}]}}))
    try:
        package = json.loads(read("package-fonts.json"))
        # Listed in any order, the newest CSV wins; the XML and a file on another host are never read.
        package["result"]["resources"].reverse()
        package["result"]["resources"].append({"format": "CSV", "url": "https://example.org/fonts.csv",
                                               "last_modified": "2026-10-01T00:00:00"})
        picked = open_data.run_all(probe, json.dumps(package).encode(), "barcelona", "barcelona.cat", AS_OF)
        # The http step keeps the host and path literal (PL-07): the message names only the rest.
        assert picked == [{"file": "889c583e-6c11-46c3-bc45-7f3ae5e6c621"
                          "/resource/8f63c3cf-399a-4a8d-a73d-eb1fca80d7f0/download/fonts_beure_2026.csv"}]
    finally:
        probe.unlink()


@requires_docker
def test_a_stale_or_empty_package_and_a_refused_one_fail_the_run():
    package = json.loads(read("package-fonts.json"))
    old = dict(package, result={"resources": [r for r in package["result"]["resources"] if "2026" not in r["url"]]})
    assert "more than 13 months old" in run(DRINKING, json.dumps(old), steps=1)[0]["error"]
    none = dict(package, result={"resources": [{"format": "XML", "url": "https://opendata-ajuntament.barcelona.cat/x.xml"}]})
    assert "lists no CSV file" in run(DRINKING, json.dumps(none), steps=1)[0]["error"]
    assert "refused the fonts package" in run(DRINKING, json.dumps({"success": False}), steps=1)[0]["error"]


@requires_docker
def test_an_outside_a_duplicate_and_an_uncoded_fountain_are_each_dropped_and_counted():
    def edit(rows):
        header = rows[0]
        lat, code = header.index("LATITUD"), header.index("CODI")
        rows.append(list(rows[1]))   # 01-004 twice
        rows[2][lat] = "40.0"        # 01-005 far south of the city
        rows[3][code] = "sense codi"  # 01-162 with no code
    package = read("package-fonts.json")
    baseline = counts(DRINKING, package, read("fonts.csv"))["dropped"]
    assert baseline == {"noCode": 0, "noPoint": 0, "outsideTheCity": 0, "duplicateCode": 0}
    answer = counts(DRINKING, package, csv_edited("fonts.csv", edit))
    assert answer["dropped"] == {"noCode": 1, "noPoint": 0, "outsideTheCity": 1, "duplicateCode": 1}
    assert [f["id"] for f in answer["fountains"]] == ["01-004", "10-001"]
    assert "holds no fountain" in counts(DRINKING, package, read("fonts.csv").splitlines()[0] + "\n")["error"]


# The seed.

def seed() -> dict:
    docs = {}
    for path in CITY.glob("*.yaml"):
        if path.name == "index.yaml" or path.name.endswith((".linkml.yaml", "-bento.yaml")):
            continue
        doc = yaml.safe_load(path.read_text())
        docs[(doc["kind"], doc["metadata"]["name"])] = doc["spec"]
    return docs


@pytest.mark.parametrize("pipeline,kind,url", [
    ("speed-reducers", "StreetFurniture", "https://opendata-ajuntament.barcelona.cat/resources/bcn/Mobiliari_Urba/"),
    ("drinking-fountains", "PointOfInterest", "https://opendata-ajuntament.barcelona.cat/data/api/3/action/package_show?id=fonts"),
    ("ornamental-fountains", "PointOfInterest", "https://opendata-ajuntament.barcelona.cat/data/api/3/action/package_show?id=fonts-ornamentals"),
])
def test_each_pipeline_upserts_its_type_from_the_city_host_by_its_schedule_alone(pipeline, kind, url):
    docs = seed()
    spec = docs[("Pipeline", pipeline)]
    assert spec["output"] == {"type": kind, "mode": "upsert"}, "upsert: an empty file never deletes"
    assert "period" not in spec and len(spec["schedule"].split()) == 5
    source = docs[("DataSource", spec["source"]["dataSourceRef"]["name"])]["http"]["url"]
    assert source.startswith(url), "the allow-listed host"
    index = yaml.safe_load((CITY / "index.yaml").read_text())
    assert index[f"barcelona-pipeline-{pipeline}-bento.yaml"] == f"projects/barcelona/pipelines/{pipeline}/bento.yaml"


def test_the_street_furniture_dataset_publishes_its_type_alone_to_the_public():
    docs = seed()
    spec = docs[("Endpoint", "street-furniture")]
    policy = docs[("Policy", spec["policyRef"].rsplit(":", 1)[1])]
    assert policy["assignee"] == {"kind": "role", "id": "public"}
    assert [e["type"] for rule in policy["information"] for e in rule["entities"]] == ["StreetFurniture"]
    assert all(op.startswith(("retrieve", "createSub", "updateSub", "deleteSub")) for op in policy["operations"])
    assert spec["publish"]["ckan"]["instanceRef"] == {"kind": "CkanInstance", "name": "barcelona"}
    assert spec["catalog"]["pipelineRef"] == {"kind": "Pipeline", "name": "speed-reducers"}
    space = docs[("ContextSpace", "barcelona")]
    assert any(r["entityType"] == "StreetFurniture" and r["name"] == "in-area-street-furniture" for r in space["quality"])
    write = docs[("Policy", "pipelines-write")]
    assert "StreetFurniture" in [e["type"] for rule in write["information"] for e in rule["entities"]]
