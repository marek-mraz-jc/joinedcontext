"""The heat vulnerability index and the service property energy use of Helsinki as Smart Data Model
KeyPerformanceIndicator (T-5278, PL-03, DM-01).

Bento runs the committed processors of the two mappings over recorded source data
(`fixtures/helsinki/open-data/`, fetched on 2026-10-22, CC BY 4.0): ECOTEN's heat vulnerability zip
whole, as the pipeline reads it, and the Nuuka property list with the monthly answers of three
locations for September 2026, and the space's sub-districts as dev's helsinki-all Endpoint answered
the pipeline's query on 2026-10-25 (`space-subdistricts.json`, T-5667). The calls to the space and to
Nuuka are the steps left out: each case feeds the recorded answer as the branch hands it on. Every entity is checked against the space's
generated JSON Schema and against the vendored Smart Data Model. The edge cases are those answers
edited in the one place they name.

Without a container runtime the Bento cases skip; the seed cases do not.
"""

import copy
import io
import json
import os
import subprocess
import zipfile
from pathlib import Path

import open_data
import pytest
import yaml
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-helsinki-heat-energy")

ROOT = Path(__file__).resolve().parent.parent
HELSINKI = ROOT / "components/context-gateway/seed/helsinki"
FIXTURES = Path(__file__).resolve().parent / "fixtures/helsinki/open-data"
BENTO = "ghcr.io/warpstreamlabs/bento:1.22.0@sha256:ef674bebae45793c982c350d8f37c2c7661133c1dc65a5517fb977b36dc09841"
PREFIX = "urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:helsinki:"
SDM = "dataModel.KeyPerformanceIndicator/KeyPerformanceIndicator"
HEAT = HELSINKI / "helsinki-pipeline-heat-vulnerability-bento.yaml"
ENERGY = HELSINKI / "helsinki-pipeline-service-property-energy-bento.yaml"
# The space's 148 sub-districts, Finnish name (case and spaces aside) to code, as the branch builds it.
SPACE = json.loads((FIXTURES / "space-subdistricts.json").read_text())
REGISTER = {e["name"]["languageMap"]["fi"].strip().lower(): e["districtCode"] for e in SPACE}


def processors(path: Path) -> list:
    return yaml.safe_load(path.read_text())["pipeline"]["processors"]


def bento(steps: list, data: bytes) -> list:
    """The processors over one message; one output line per message, errored ones dropped."""
    config = {
        "input": {"stdin": {"scanner": {"to_the_end": {}}}},
        "pipeline": {"processors": steps + [{"catch": [{"mapping": "root = deleted()"}]}]},
        "output": {"stdout": {"codec": "lines"}},
        "logger": {"level": "error"},
    }
    result = subprocess.run(
        ["docker", "run", "--rm", "-i", "-e", "JC_ORG_DOMAIN=hel.fi", "-e", "JC_SPACE=helsinki",
         "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", BENTO, "-c",
         'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        input=data, capture_output=True, timeout=120, check=False,
    )
    assert result.returncode == 0, result.stderr.decode()
    return [json.loads(line) for line in result.stdout.decode().splitlines() if line.strip()]


# --- heat vulnerability -------------------------------------------------------------------------

def heat_rows() -> list:
    """The zip through the unarchive, the CSV filter and the parse step: the rows the call gets."""
    steps = processors(HEAT)
    out = bento(steps[:2] + steps[2]["try"][:1], (FIXTURES / "heat-vulnerability-csv.zip").read_bytes())
    assert len(out) == 1, "only the CSV of the zip goes on"
    return out[0]["rows"]


def heat(rows: list, register: dict = REGISTER) -> dict | None:
    """The step after the layer's call, as the runner's log and writer see it."""
    decide = processors(HEAT)[2]["try"][2]
    out = bento([decide], json.dumps({"rows": rows, "register": register}).encode())
    return out[0] if out else None


@pytest.fixture(scope="module")
def recorded_rows():
    if open_data.shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return heat_rows()


@requires_docker
def test_a_zip_over_one_mebibyte_fails_before_it_is_unpacked():
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", zipfile.ZIP_STORED) as archive:
        archive.writestr("data.csv", b"ID,Name\n1,A\n")
        archive.writestr("padding.bin", os.urandom(1048576))
    steps = processors(HEAT)
    assert bento(steps[:2] + steps[2]["try"][:1], buffer.getvalue()) == [], "the CSV inside the zip never goes on"


@requires_docker
def test_a_space_answer_over_four_mebibytes_fails_the_call():
    cap = processors(HEAT)[2]["try"][1]["branch"]["processors"][1]
    assert bento([cap], b'[]') == [[]]
    assert bento([cap], b'{"pad": "' + b"x" * 4194304 + b'"}') == []


@requires_docker
def test_the_space_answer_becomes_a_name_to_code_register():
    branch = processors(HEAT)[2]["try"][1]["branch"]
    out = bento([{"mapping": branch["result_map"]}], json.dumps(SPACE).encode())
    assert len(out[0]["register"]) == 148 and out[0]["register"] == REGISTER
    # An entity of another kind of area or without a Finnish name is no sub-district to link.
    other = [{"id": "urn:ngsi-ld:AdministrativeArea:hel.fi:helsinki:district-091101", "name": {"languageMap": {"fi": "Vironniemi"}}},
             {"id": SPACE[0]["id"], "name": {"languageMap": {"sv": "Kronohagen"}}}]
    assert bento([{"mapping": branch["result_map"]}], json.dumps(other).encode())[0]["register"] == {}


@requires_docker
def test_each_district_gives_six_indicators_linked_to_its_sub_district(recorded_rows):
    assert len(recorded_rows) == 145
    out = heat(recorded_rows)
    entities = {e["id"]: e for e in out["entities"]}
    assert out["read"] == 145 and len(entities) == 145 * 6
    uhvi = entities[PREFIX + "heat-uhvi-113006"]
    assert uhvi["area"]["value"] == "Sompasaari"
    assert uhvi["refArea"] == {"type": "Relationship",
                               "object": "urn:ngsi-ld:AdministrativeArea:hel.fi:helsinki:subdistrict-" + REGISTER["sompasaari"]}
    assert uhvi["districtCode"]["value"] == REGISTER["sompasaari"] and uhvi["divisionLevel"]["value"] == "subDistrict"
    assert "unitCode" not in uhvi["kpiValue"] and 0 <= uhvi["kpiValue"]["value"] <= 1
    assert "observedAt" not in uhvi["kpiValue"], "one period: the space records it on change"
    assert uhvi["calculationPeriod"]["value"] == {"from": "2018-06-01", "to": "2021-08-31"}
    assert uhvi["dimensionKey"]["value"] == "district=113006;index=Avg-UHVI"
    # Eira's weighted heat vulnerable population exceeds its residents in the study itself: kept.
    eira = next(e for e in entities.values() if e["area"]["value"] == "Eira" and e["id"].startswith(PREFIX + "heat-hvp-"))
    assert round(eira["kpiValue"]["value"]) == 1838
    # Each of the study's 145 names is one sub-district of the space: every indicator is linked.
    assert out["dropped"] == {}
    assert all("refArea" in e for e in entities.values())
    # A name the space does not hold is written without its link, and counted.
    partial = heat(recorded_rows, register={"sompasaari": REGISTER["sompasaari"]})
    assert partial["dropped"] == {"district name not among the space's sub-districts (written without refArea)": 144}
    assert sum("refArea" in e for e in partial["entities"]) == 6


@requires_docker
def test_every_heat_indicator_meets_the_space_schema_and_the_smart_data_model(recorded_rows):
    for entity in heat(recorded_rows)["entities"]:
        assert open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", entity) == [], entity["id"]
        assert open_data.sdm_errors(SDM, entity) == [], entity["id"]


@requires_docker
def test_an_out_of_range_index_a_duplicate_and_a_row_without_id_are_dropped_with_a_count(recorded_rows):
    rows = copy.deepcopy(recorded_rows)
    rows[0]["Avg-UHVI"] = "1.7"
    rows[1]["HVP"] = "-3"
    rows[2]["ID"] = ""
    rows.append(dict(rows[3]))
    out = heat(rows, register={})
    assert out is None, "a space with no sub-district fails the run"
    out = heat(rows)
    assert out["dropped"]["index outside 0..1"] == 1
    assert out["dropped"]["heat vulnerable population below 0"] == 1
    assert out["dropped"]["no district id or name"] == 1
    assert out["dropped"]["duplicate district"] == 1
    assert len(out["entities"]) == (145 - 3) * 6


@requires_docker
def test_a_truncated_file_fails_the_run_instead_of_writing_a_smaller_city(recorded_rows):
    assert heat(recorded_rows[:99]) is None


# --- service property energy ---------------------------------------------------------------------

def energy_steps() -> list:
    return processors(ENERGY)[0]["try"]


def monthly(name: str):
    return json.loads((FIXTURES / "nuuka-monthly-2026-09.json").read_text())[name]


def energy(location: str, group: str, answer, code: str = "091-011-9902-0101") -> list:
    """The step after the call and the drop switch, for September 2026."""
    key = location.lower().replace("ä", "a").replace("ö", "o")
    key = "-".join(part for part in "".join(c if c.isalnum() else " " for c in key).split())
    message = {"location": location, "key": key, "code": code, "group": group,
               "from": "2026-09-01", "to": "2026-09-30", "answer": answer}
    steps = energy_steps()
    return bento([steps[4], steps[5]], json.dumps(message).encode())


@requires_docker
def test_the_list_becomes_three_calls_per_distinct_enabled_location():
    steps = energy_steps()
    out = bento(steps[:1], (FIXTURES / "nuuka-properties.json").read_bytes())[0]
    locations = sorted({m["location"] for m in out})
    # Hakaniemi's three rows are one location; the "(disabled)" location is left out; "New property 7"
    # is a location without a property code. Both Hoplaxskolan spellings are asked (Nuuka holds the
    # readings under one), each under its own id.
    assert locations == ["1000 Hakaniemen kauppahalli", "1037 Vuotalo", "1716 Töölön virastotalo",
                         "4337 Hoplaxskolan (Solnantie 18-20)", "4337 Hoplaxskolan/Solnantie 18-20", "New property 7"]
    assert len(out) == 6 * 3 and {m["group"] for m in out} == {"Electricity", "Heat", "Water"}
    hoplax = {m["key"] for m in out if m["location"].startswith("4337 ")}
    assert len(hoplax) == 2 and all(k.startswith("4337-hoplaxskolan-solnantie-18-20-") for k in hoplax)
    first = out[0]
    assert first["to"].endswith(("-28", "-29", "-30", "-31")) and first["from"] == first["to"][:8] + "01"
    assert next(m for m in out if m["location"] == "1037 Vuotalo")["key"] == "1037-vuotalo"
    assert bento(steps[:1], b"[]") == [], "an empty list fails the run"


@requires_docker
def test_a_month_of_use_is_one_indicator_and_no_data_writes_nothing():
    (heat_kpi,) = energy("1000 Hakaniemen kauppahalli", "Heat", monthly("1000-Heat"))
    assert heat_kpi["id"] == PREFIX + "energy-heat-1000-hakaniemen-kauppahalli"
    assert heat_kpi["kpiValue"] == {"type": "Property", "value": 28797.0, "unitCode": "KWH",
                                    "observedAt": "2026-09-30T00:00:00Z"}
    assert heat_kpi["calculationPeriod"]["value"] == {"from": "2026-09-01", "to": "2026-09-30"}
    assert heat_kpi["calculationFrequency"]["value"] == "monthly"
    assert heat_kpi["dimensionKey"]["value"] == \
        "locationName=1000 Hakaniemen kauppahalli;propertyCode=091-011-9902-0101;reportingGroup=Heat"
    assert heat_kpi["name"]["languageMap"]["en"] == "1000 Hakaniemen kauppahalli: heat, monthly use"
    assert heat_kpi["source"]["value"].startswith(
        "https://helsinki-openapi.nuuka.cloud/api/v1.0/EnergyData/Monthly/ListByProperty?Record=LocationName&SearchString=1000+Hakaniemen")
    (water,) = energy("1037 Vuotalo", "Water", monthly("1037-Water"))
    assert water["kpiValue"]["unitCode"] == "MTQ"
    assert energy("1000 Hakaniemen kauppahalli", "Water", monthly("1000-Water")) == []


@requires_docker
def test_every_energy_indicator_meets_the_space_schema_and_the_smart_data_model():
    names = {"1000": "1000 Hakaniemen kauppahalli", "1037": "1037 Vuotalo", "1716": "1716 Töölön virastotalo"}
    answers = json.loads((FIXTURES / "nuuka-monthly-2026-09.json").read_text())
    written = []
    for key, answer in answers.items():
        number, group = key.split("-")
        written += energy(names[number], group, answer)
    assert len(written) == 8
    for entity in written:
        assert open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", entity) == [], entity["id"]
        assert open_data.sdm_errors(SDM, entity) == [], entity["id"]


@requires_docker
def test_out_of_range_unknown_unit_and_another_month_are_dropped():
    def edited(**change):
        answer = copy.deepcopy(monthly("1716-Electricity"))
        answer[0].update(change)
        return energy("1716 Töölön virastotalo", "Electricity", answer)

    assert edited(value=-5) == []
    assert edited(value=7.2e7) == []
    assert edited(value="n/a") == []
    assert edited(unit="MWh") == []
    assert edited(timestamp="2026-08-31T00:00:00") == []
    # Nuuka stamps a month with its last day of data: one that stops on the 22nd is incomplete.
    assert edited(timestamp="2026-09-22T00:00:00") == []
    # Nuuka writes the unit of water as M3 and as m3.
    assert edited(unit="kwh")[0]["kpiValue"]["unitCode"] == "KWH"
    # An answer for another location (a partial match) is never taken for the one asked.
    assert edited(locationName="1716 Töölön virastotalo B") == []
    assert len(edited(value=0)) == 1


# --- seed ------------------------------------------------------------------------------------------

def test_each_dataset_is_one_public_endpoint_whose_policy_reads_only_its_own_indicators():
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    for name, prefix, pipeline in [("heat-vulnerability", "heat", "heat-vulnerability"),
                                   ("service-property-energy", "energy", "service-property-energy")]:
        endpoint = yaml.safe_load((HELSINKI / f"helsinki-endpoint-{name}.yaml").read_text())
        policy = yaml.safe_load((HELSINKI / f"helsinki-policy-public-{name}.yaml").read_text())
        spec = endpoint["spec"]
        assert spec["policyRef"].endswith(f":public-{name}")
        assert {"ngsi-ld", "csv", "odata", "websocket", "ogc-features"} <= set(spec["enabledRepresentations"])
        assert spec["catalog"]["license"] == "CC_BY_4_0" and "CC BY 4.0" in spec["catalog"]["attribution"]["en"]
        assert spec["catalog"]["pipelineRef"] == {"kind": "Pipeline", "name": pipeline}
        assert "dimensionKey" in endpoint["metadata"]["description"]["en"]
        (selector,) = policy["spec"]["information"][0]["entities"]
        assert selector == {"type": "KeyPerformanceIndicator",
                            "idPattern": f"^urn:ngsi-ld:KeyPerformanceIndicator:[a-z0-9.-]+:helsinki:{prefix}-[a-z0-9-]+$"}
        assert not [op for op in policy["spec"]["operations"] if op.startswith(("create", "update", "delete", "upsert", "append"))
                    and "Subscription" not in op]
        for kind in ("endpoint", "policy-public", "pipeline", "datasource"):
            assert any(key.startswith(f"helsinki-{kind}-") and name in key for key in index), (kind, name)


def test_the_pipelines_read_only_the_named_hosts_with_timeouts_and_a_rate_limit():
    heat_text = HEAT.read_text()
    # The register comes from the space, through its public Endpoint, never from kartta.hel.fi:
    # the cluster stopped reaching that host in October 2026 and every run wrote nothing (T-5667).
    assert "https://kartta.hel.fi" not in heat_text
    held = processors(HEAT)[2]["try"][1]["branch"]["processors"][0]["http"]
    assert held["url"] == ("${JC_GATEWAY_URL}/api/endpoint/si6epqkx364lprho5uaigutk274r5grb"
                           "/ngsi-ld/v1/entityOperations/query?limit=1000&options=keyValues")
    assert held["timeout"] == "20s" and held["rate_limit"] == "pipeline_egress" and "oauth2" not in held
    call = next(p for p in energy_steps() if "branch" in p)["branch"]["processors"][0]["http"]
    assert call["url"].startswith("https://helsinki-openapi.nuuka.cloud/api/v1.0/EnergyData/Monthly/ListByProperty?")
    assert call["timeout"] == "30s" and call["rate_limit"] == "hel_nuuka" and call["successful_on"] == [404]
    rates = {r["label"]: r["local"] for r in yaml.safe_load(
        (ROOT / "components/pipeline-runner/streams/resources.yaml").read_text())["rate_limit_resources"]}
    assert rates["hel_nuuka"] == {"count": 10, "interval": "1s"}
    for name, host in [("heat-vulnerability", "https://www.hel.fi/hel2/Tietokeskus/"),
                       ("service-property-energy", "https://helsinki-openapi.nuuka.cloud/")]:
        source = yaml.safe_load((HELSINKI / f"helsinki-datasource-{name}.yaml").read_text())
        assert source["spec"]["http"]["url"].startswith(host)
