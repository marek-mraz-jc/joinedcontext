"""Prague's city tables as KeyPerformanceIndicator series and its road works as CityWork (T-5428, PL-03):
the municipal police's monthly activity reports, city-wide and per police unit, the budget execution,
the transfers to the city districts, and the works and closures of the city's opravujeme.to service,
each its own public dataset of the space praha-mesto.

Bento runs each committed mapping over real records downloaded on 2026-10-25
(`fixtures/praha/city-tables.json`: two reported months and the empty January of the police file, two
units of one month of the per-unit file, four lines each of the two finance files, five road works).
In the road works the responsible people's names, phones and e-mails are replaced with invented ones,
which the test checks are never written. The edge cases are that answer edited in the one place they
name. Each entity is checked against the space's generated JSON Schema, which the gateway enforces,
and the pinned Smart Data Models schema of its type.
"""

import copy
import json
import re
import shutil
import tempfile
from pathlib import Path

import open_data
import pytest
import yaml
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-praha-city-tables")

ROOT = Path(__file__).resolve().parent.parent
PRAHA = ROOT / "components/context-gateway/seed/praha"
RECORDED = json.loads((Path(__file__).resolve().parent / "fixtures/praha/city-tables.json").read_text())
SPACE = "praha-mesto"
KPI = ("KeyPerformanceIndicator", "dataModel.KeyPerformanceIndicator/KeyPerformanceIndicator")
# pipeline -> (type, pinned SDM model, Policy, id prefix)
PIPELINES = {
    "police-activity": (*KPI, "public-cinnost-mestske-policie", "mp-cinnost-"),
    "police-units": (*KPI, "public-cinnost-mestske-policie-utvary", "mp-utvary-"),
    "budget-drawing": (*KPI, "public-cerpani-rozpoctu", "mhmp-cerpani-"),
    "district-transfers": (*KPI, "public-financni-vztahy-mc", "mhmp-mc-vztahy-"),
    "road-works": ("CityWork", "dataModel.Transportation/CityWork", "public-opravy-a-uzavirky", "opravujeme-"),
}


def outcome(name: str, document) -> tuple[list[dict], list[str]]:
    """The mapping of `name` over one answer: the entities written and the error of a message the
    runner refuses (an errored message fails the run, nothing is written)."""
    pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-{name}-bento.yaml").read_text())
    pipeline["pipeline"]["processors"].append({"mapping": 'root = if errored() { {"__error": error()} } else { this }'})
    with tempfile.TemporaryDirectory() as tmp:
        pinned = Path(tmp) / "bento.yaml"
        pinned.write_text(yaml.safe_dump(pipeline, allow_unicode=True))
        body = document if isinstance(document, bytes) else json.dumps(document, ensure_ascii=False).encode()
        answer = open_data.run_all(pinned, body, SPACE, "praha.eu")
    errors = [e["__error"] for e in answer if "__error" in e]
    return ([], errors) if errors else (answer, [])


def run(name: str, document=None) -> dict[str, dict]:
    entities, errors = outcome(name, RECORDED[name] if document is None else document)
    assert not errors, errors
    prefix = PIPELINES[name][3]
    return {e["id"].rsplit(f":{prefix}", 1)[1]: e for e in entities}


def edited(name: str, edit):
    document = copy.deepcopy(RECORDED[name])
    edit(document)
    return document


def leaves(tree: dict) -> int:
    return sum(leaves(v) if isinstance(v, dict) else 1 for v in tree.values())


@pytest.fixture(scope="module")
def written():
    if shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return {name: run(name) for name in PIPELINES}


def by_indicator(entities: dict[str, dict], indicator: str, period: str, unit: str | None = None) -> dict:
    found = [e for e in entities.values() if e["indicator"]["value"] == indicator and e["refPeriod"]["value"] == period
             and e.get("dimensionKey", {}).get("value") == unit]
    assert len(found) == 1, (indicator, period, unit, len(found))
    return found[0]


@requires_docker
def test_every_record_is_one_entity_the_gateway_and_the_sdm_accept(written):
    expected = {
        "police-activity": leaves(RECORDED["police-activity"]),
        "police-units": leaves(RECORDED["police-units"]),
        "budget-drawing": len(RECORDED["budget-drawing"]["data"]["row"]),
        "district-transfers": len(RECORDED["district-transfers"]["data"]["row"]),
        "road-works": len(RECORDED["road-works"]["data"]),
    }
    for name, (kind, sdm, _, prefix) in PIPELINES.items():
        assert len(written[name]) == expected[name], name
        for entity in written[name].values():
            assert entity["id"].startswith(f"urn:ngsi-ld:{kind}:praha.eu:{SPACE}:{prefix}")
            assert entity["type"] == kind
            assert not open_data.schema_errors(PRAHA / "praha.v1.schema.json", entity), entity["id"]
            assert not open_data.sdm_errors(sdm, entity), entity["id"]


@requires_docker
def test_a_police_figure_is_a_number_of_its_month_with_its_unit(written):
    police = written["police-activity"]
    tows = by_indicator(police, "Odtahy vozidel / Celkem", "2025-09")
    assert tows["kpiValue"] == {"type": "Property", "value": 517, "unitCode": "C62"}
    assert tows["unitText"]["value"] == "počet"
    assert tows["refArea"]["value"] == "CZ010"
    assert tows["dataSet"]["value"] == "mp-cinnost"
    assert tows["name"]["languageMap"] == {"cs": "Odtahy vozidel / Celkem (2025-09)"}
    assert "dimensionKey" not in tows
    # "151 802" with a space as the thousands separator, "95,0" with a decimal comma.
    assert by_indicator(police, "Počty zjištěných přestupků / Doprava / Celkem", "2025-09")["kpiValue"]["value"] == 151802
    share = by_indicator(police, "Procentické zastoupení přestupků / Doprava", "2025-09")
    assert share["kpiValue"] == {"type": "Property", "value": 95, "unitCode": "P1"} and share["unitText"]["value"] == "%"
    fines = by_indicator(police, "Uložené pokuty v Kč / Výše pokut celkem", "2025-09")
    assert fines["kpiValue"] == {"type": "Property", "value": 5538850} and fines["unitText"]["value"] == "Kč"
    # One figure keeps its id's hash from month to month; the empty January writes nothing.
    august = by_indicator(police, "Odtahy vozidel / Celkem", "2025-08")
    assert august["id"].rsplit("-", 1)[1] == tows["id"].rsplit("-", 1)[1]
    assert {e["refPeriod"]["value"] for e in police.values()} == {"2025-08", "2025-09"}
    units = written["police-units"]
    # A unit's report carries fewer headings than the city's: offences, fines and other solutions.
    unit = by_indicator(units, "Počet uložených pokut / Počet pokut celkem", "2025-09", "OŘ1")
    assert unit["dataSet"]["value"] == "mp-utvary"
    assert unit["kpiValue"]["value"] == int(RECORDED["police-units"]["092025"]["OŘ1"]["Počet uložených pokut"]["Počet pokut celkem"].replace(" ", ""))
    assert {e["dimensionKey"]["value"] for e in units.values()} == {"OŘ1", "Hlídkový útvar"}


@requires_docker
def test_a_budget_drawing_and_a_district_transfer_carry_their_codes_and_amounts(written):
    row = RECORDED["budget-drawing"]["data"]["row"][0]
    key = f"{row['rok']}-{row['oblast']}-{row['odpa']}-{row['pol']}-{row['uz']}"
    drawing = written["budget-drawing"][key]
    assert drawing["kpiValue"]["value"] == float(row["cerpani"])
    assert drawing["adjustedAmount"]["value"] == float(row["rozpocet_aktualni"])
    assert drawing["unitText"]["value"] == "tis. Kč"
    assert drawing["refPeriod"]["value"] == row["rok"]
    assert drawing["dimensionKey"]["value"] == f"{row['oblast']} {row['nazev_oblast']}"
    assert drawing["indicator"]["value"].startswith(f"{row['odpa']} {row['nazev_odpa']} / {row['pol']} ")
    # The same five codes as the BudgetLine of the budget pipeline, so a reader sets them side by side.
    assert key == "2026-12-003529-5331-000000091"
    row = RECORDED["district-transfers"]["data"]["row"][0]
    transfer = written["district-transfers"][f"{row['rok']}-{row['mc_ic']}-{row['odpa']}-{row['pol']}-{row['uz']}"]
    assert transfer["refArea"]["value"] == "00063410 HMP-MČ PRAHA 1"
    assert transfer["dimensionKey"]["value"] == row["nazev_zdroj"]
    assert transfer["approvedAmount"]["value"] == float(row["rozpocet_schvaleny"])
    assert transfer["adjustedAmount"]["value"] == float(row["rozpocet_upraveny"])
    assert transfer["kpiValue"]["value"] == float(row["cerpani"])
    # "HMP-MČ PRAHA  3" style double spaces are folded.
    assert all("  " not in e["refArea"]["value"] for e in written["district-transfers"].values())


@requires_docker
def test_a_road_work_has_its_state_dates_and_point_and_no_person(written):
    works = written["road-works"]
    assert sorted(works) == sorted(str(r["id"]) for r in RECORDED["road-works"]["data"])
    active = works["2621"]
    assert active["workState"]["value"] == "open"
    assert active["trafficImpact"]["value"] == "severe"
    assert active["workCategory"]["value"] == "construction"
    assert active["contractingAuthority"]["value"] == "Dopravní podnik hl. m. Prahy, a.s."
    assert active["location"]["value"]["type"] == "Point"
    assert works["2684"]["workState"]["value"] == "planned"
    assert works["2111"]["workState"]["value"] == "completed" and works["2111"]["workCategory"]["value"] == "cultural"
    # Prague wall time gets Prague's offset: summer +02:00, winter +01:00.
    assert active["startDate"]["value"] == "2026-09-14T00:00:00+02:00"
    assert works["2352"]["startDate"]["value"] == "2026-02-16T00:00:00+01:00"
    # A private investor is a person: left out. No name, phone or e-mail of the responsible people,
    # and no free text, is written anywhere.
    assert "contractingAuthority" not in works["2314"]
    text = json.dumps(list(works.values()), ensure_ascii=False)
    assert "Testovací" not in text and "600 100 200" not in text and "example.cz" not in text
    assert all("description" not in w for w in works.values())


@requires_docker
def test_an_impossible_value_a_future_month_a_duplicate_and_a_point_outside_are_each_dropped():
    def police(d):
        d["092025"]["Procentické zastoupení přestupků"]["Doprava"] = "101,0"
        d["092025"]["Odtahy vozidel"]["Celkem"] = "-5"
        d["092025"]["Odtahy vozidel"]["Úplné"] = "n/a"
        d["122099"] = {"Odtahy vozidel": {"Celkem": "1"}}
    entities = run("police-activity", edited("police-activity", police))
    assert len(entities) == leaves(RECORDED["police-activity"]) - 3
    assert not [e for e in entities.values() if e["refPeriod"]["value"] == "2099-12"]

    def budget(d):
        rows = d["data"]["row"]
        rows.append(dict(rows[0]))
        rows[1] = dict(rows[1], cerpani="99999999999")
    drawn = run("budget-drawing", edited("budget-drawing", budget))
    assert len(drawn) == len(RECORDED["budget-drawing"]["data"]["row"]) - 1

    def works(d):
        d["data"][0]["lat"], d["data"][0]["lon"] = 0, 0
        d["data"][1]["end"] = "2020-01-01T00:00:00"
        d["data"].append(dict(d["data"][2]))
        d["data"].append(dict(d["data"][3], id=9999999, hidden=True))
    kept = run("road-works", edited("road-works", works))
    assert sorted(kept) == ["2111", "2314", "2352"]


@requires_docker
@pytest.mark.parametrize("name, answer", [
    ("police-activity", {"012025": {}}),
    ("police-activity", []),
    ("budget-drawing", {"data": {"row": []}}),
    ("district-transfers", {"rows": []}),
    ("road-works", {"count": 0, "data": []}),
    ("road-works", {"detail": "Not found"}),
])
def test_an_empty_or_foreign_answer_fails_the_run(name, answer):
    entities, errors = outcome(name, answer)
    assert not entities and errors


@requires_docker
def test_a_run_where_fewer_than_half_survive_fails():
    def broken(d):
        for r in d["data"][:3]:
            r["state"] = "UNKNOWN"
    entities, errors = outcome("road-works", edited("road-works", broken))
    assert not entities and "only 2 of 5" in errors[0]


def test_each_policy_names_exactly_its_pipeline_ids():
    patterns = {}
    for name, (kind, _, policy, prefix) in PIPELINES.items():
        doc = yaml.safe_load((PRAHA / f"praha-policy-{policy}.yaml").read_text())
        [selector] = doc["spec"]["information"][0]["entities"]
        assert selector["type"] == kind
        patterns[name] = re.compile(selector["idPattern"])
    samples = {
        "police-activity": "urn:ngsi-ld:KeyPerformanceIndicator:praha.eu:praha-mesto:mp-cinnost-2025-09-212cd4cb3391",
        "police-units": "urn:ngsi-ld:KeyPerformanceIndicator:praha.eu:praha-mesto:mp-utvary-2025-09-b3916731783d",
        "budget-drawing": "urn:ngsi-ld:KeyPerformanceIndicator:praha.eu:praha-mesto:mhmp-cerpani-2026-12-003529-5331-000000091",
        "district-transfers": "urn:ngsi-ld:KeyPerformanceIndicator:praha.eu:praha-mesto:mhmp-mc-vztahy-2026-00063410-006330-4137-000000000",
        "road-works": "urn:ngsi-ld:CityWork:praha.eu:praha-mesto:opravujeme-2621",
    }
    for name, pattern in patterns.items():
        assert [other for other, sample in samples.items() if pattern.match(sample)] == [name]


def test_each_dataset_is_wired_from_source_to_catalogue():
    index = yaml.safe_load((PRAHA / "index.yaml").read_text())
    for name, (kind, _, policy, _) in PIPELINES.items():
        pipeline = yaml.safe_load((PRAHA / f"praha-pipeline-{name}.yaml").read_text())
        assert pipeline["spec"]["output"] == {"type": kind, "mode": "upsert"}
        source = yaml.safe_load((PRAHA / f"praha-datasource-{name}.yaml").read_text())
        assert source["metadata"]["name"] == pipeline["spec"]["source"]["dataSourceRef"]["name"]
        endpoint = yaml.safe_load((PRAHA / f"praha-endpoint-{policy.removeprefix('public-')}.yaml").read_text())
        assert endpoint["spec"]["policyRef"] == f"urn:ngsi-ld:Policy:praha.eu:praha-mesto:{policy}"
        assert endpoint["spec"]["catalog"]["pipelineRef"] == {"kind": "Pipeline", "name": name}
        assert endpoint["spec"]["catalog"]["source"][0]["url"] == source["spec"]["http"]["url"]
        for path in (f"praha-datasource-{name}.yaml", f"praha-pipeline-{name}.yaml", f"praha-pipeline-{name}-bento.yaml",
                     f"praha-endpoint-{policy.removeprefix('public-')}.yaml", f"praha-policy-{policy}.yaml"):
            assert path in index, path
