"""Barcelona statistics as Smart Data Model KeyPerformanceIndicator (T-5266, PL-03, DM-01).

Bento runs the committed processors of `barcelona-pipeline-statistics-bento.yaml` over recorded
answers of Open Data BCN's CKAN API (`fixtures/barcelona/statistics/`, CC BY 4.0, fetched on
2026-10-23 with the calls the pipeline sends): the Padró's code list (pad-dimensions, the
DataSource), one package_show and the DataStore query answers of three tables. The two calls
themselves are the steps left out: each case feeds the message as the branch hands it on. Every
entity is checked against the space's generated JSON Schema and against the vendored Smart Data
Model. The edge cases are those answers edited in the one place they name.

Without a container runtime the Bento cases skip; the seed cases do not.
"""

import copy
import json
import subprocess
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import requires_docker

pytestmark = pytest.mark.xdist_group("docker-barcelona-statistics")

ROOT = Path(__file__).resolve().parent.parent
CITY = ROOT / "components/context-gateway/seed/barcelona"
MAPPING = CITY / "barcelona-pipeline-statistics-bento.yaml"
SCHEMA = CITY / "barcelona.v1.schema.json"
FIXTURES = Path(__file__).resolve().parent / "fixtures/barcelona/statistics"
PREFIX = "urn:ngsi-ld:KeyPerformanceIndicator:barcelona.cat:barcelona:stat-"
AREA = "urn:ngsi-ld:AdministrativeArea:barcelona.cat:barcelona:"
SDM = "dataModel.KeyPerformanceIndicator/KeyPerformanceIndicator"
THEMES = ["population", "housing", "vehicles"]


def steps() -> list:
    return yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"][0]["try"]


def bento(processors: list, message: dict) -> list:
    """The processors over one message; one output line per message, errored ones dropped."""
    config = {
        "input": {"stdin": {"scanner": {"to_the_end": {}}}},
        # A failed step leaves the message flagged, which the runner reports as a failed run; here
        # it is dropped so the case sees nothing written.
        "pipeline": {"processors": processors + [{"catch": [{"mapping": "root = deleted()"}]}]},
        "output": {"stdout": {"codec": "lines"}},
        "logger": {"level": "error"},
    }
    result = subprocess.run(
        ["docker", "run", "--rm", "-i", "-e", "JC_ORG_DOMAIN=barcelona.cat", "-e", "JC_SPACE=barcelona",
         "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", open_data.BENTO, "-c",
         'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        input=json.dumps(message), capture_output=True, text=True, timeout=120, check=False,
    )
    assert result.returncode == 0, result.stderr
    return [json.loads(line) for line in result.stdout.splitlines() if line.strip()]


def fixture(name: str):
    return json.loads((FIXTURES / f"{name}.json").read_text())


def with_answer(branch: dict, answer) -> dict:
    """The branch with its call replaced by the recorded answer, its request and result maps kept."""
    out = copy.deepcopy(branch)
    out["branch"]["processors"] = [{"mapping": f"root = {json.dumps(answer)}"}]
    return out


def decode(message: dict) -> dict | None:
    """The step after the two calls, as the runner's log and writer see it: the table's summary
    with its entities, or None when the step fails the message."""
    out = bento([steps()[4]], message)
    return out[0] if out else None


def recorded(table_id: str) -> dict:
    """The message the decode step gets for a recorded table."""
    return fixture(f"answer-{table_id}")


@pytest.fixture(scope="module")
def tables():
    if open_data.shutil.which("docker") is None:
        pytest.skip("no container runtime")
    out = bento(steps()[:2], fixture("pad-dimensions"))
    return {m["table"]["id"]: m for m in out}


@pytest.fixture(scope="module")
def sexe(tables):
    return decode(recorded("pad_mdbas_sexe"))


@pytest.fixture(scope="module")
def sales(tables):
    return decode(recorded("notaris_compravendes_trans_tip_us"))


@pytest.fixture(scope="module")
def propulsion(tables):
    return decode(recorded("est_vehicles_propulsio"))


@requires_docker
def test_the_code_list_becomes_one_message_per_table_and_an_empty_list_fails_the_run(tables):
    assert len(tables) == 44
    assert {m["table"]["theme"] for m in tables.values()} == set(THEMES)
    sexe = tables["pad_mdbas_sexe"]
    assert len(sexe["register"]) == 73 and sexe["register"][0] == "01" and sexe["register"][-1] == "73"
    # pad-dimensions labels the coded breakdowns: SEXE 1 is Dona, as the city's total says (more women).
    assert sexe["labels"] == {"SEXE": {"1": "Dona", "2": "Home"}}
    assert tables["est_vehicles_propulsio"]["labels"] == {}
    assert bento(steps()[:2], {"result": {"records": []}}) == []


@requires_docker
def test_the_newest_yearly_resource_in_the_datastore_is_the_one_queried(tables):
    out = bento([with_answer(steps()[2], fixture("package-pad_mdbas_sexe"))], tables["pad_mdbas_sexe"])
    assert out[0]["resource"] == {"id": "3057b89d-9713-4001-8f07-a6fb122a152b", "name": "2026_pad_mdbas_sexe.csv"}
    assert out[0]["title"] == "Població per sexe"
    # A package whose CSV resources are none of them in the DataStore fails its table.
    package = fixture("package-pad_mdbas_sexe")
    for resource in package["result"]["resources"]:
        resource["datastore_active"] = False
    assert bento([with_answer(steps()[2], package)], tables["pad_mdbas_sexe"]) == []


@requires_docker
def test_the_query_sums_the_sections_to_barri_district_and_city_from_seed_names_only(tables):
    message = dict(tables["pad_mdbas_sexe"], resource={"id": "3057b89d-9713-4001-8f07-a6fb122a152b", "name": "x"})
    sql = bento([with_answer(steps()[3], {"result": {"records": []}})], message)
    assert sql[0]["answer"] == []
    request = steps()[3]["branch"]["request_map"]
    out = bento([{"mapping": request}], message)[0]["sql"]
    assert out.startswith('SELECT "Data_Referencia" AS p0, "SEXE" AS k0, "Codi_Districte" AS d, "Codi_Barri" AS b')
    assert out.endswith('FROM "3057b89d-9713-4001-8f07-a6fb122a152b" GROUP BY GROUPING SETS (("Data_Referencia", "SEXE", '
                        '"Codi_Districte", "Codi_Barri"), ("Data_Referencia", "SEXE", "Codi_Districte"), ("Data_Referencia", "SEXE"))')
    # A column or resource id outside the allowed alphabet never reaches the query.
    bad = copy.deepcopy(message)
    bad["table"]["dims"] = ['SEXE" FROM x; --']
    assert bento([{"mapping": request}], bad) == []
    assert bento([{"mapping": request}], dict(message, resource={"id": "x'; DROP", "name": "x"})) == []


@requires_docker
def test_every_sum_is_one_indicator_per_area_with_its_period_and_unit(sexe):
    assert sexe["table"] == "pad_mdbas_sexe" and sexe["period"] == "2026-01-01" and sexe["dropped"] == {}
    entities = {e["id"]: e for e in sexe["entities"]}
    # 2 sexes x (73 barris + 10 districts + the city).
    assert len(entities) == 2 * 84
    city = entities[PREFIX + "population-pad_mdbas_sexe-1-city"]
    assert city["name"]["value"] == "Població per sexe: Dona – Barcelona"
    assert city["kpiValue"] == {"type": "Property", "value": 898038, "unitCode": "C62", "observedAt": "2026-01-01T00:00:00Z"}
    assert city["calculationPeriod"]["value"] == {"from": "2026-01-01", "to": "2026-01-01"}
    assert city["divisionLevel"]["value"] == "city" and "refArea" not in city and "districtCode" not in city
    raval = entities[PREFIX + "population-pad_mdbas_sexe-2-neighbourhood-01"]
    assert raval["area"]["value"] == "el Raval" and raval["districtCode"]["value"] == "01"
    assert raval["refArea"] == {"type": "Relationship", "object": AREA + "neighbourhood-01"}
    assert raval["dimensionKey"]["value"] == "SEXE=2"
    district = entities[PREFIX + "population-pad_mdbas_sexe-2-district-01"]
    assert district["area"]["value"] == "Ciutat Vella" and district["refArea"]["object"] == AREA + "district-01"
    # The sums add up: the barris of a district are the district, the districts the city.
    for sex in ("1", "2"):
        barris = sum(e["kpiValue"]["value"] for e in entities.values() if e["divisionLevel"]["value"] == "neighbourhood" and e["dimensionKey"]["value"] == f"SEXE={sex}")
        districts = sum(e["kpiValue"]["value"] for e in entities.values() if e["divisionLevel"]["value"] == "district" and e["dimensionKey"]["value"] == f"SEXE={sex}")
        assert barris == districts == entities[PREFIX + f"population-pad_mdbas_sexe-{sex}-city"]["kpiValue"]["value"]
    assert entities[PREFIX + "population-pad_mdbas_sexe-1-city"]["kpiValue"]["value"] > entities[PREFIX + "population-pad_mdbas_sexe-2-city"]["kpiValue"]["value"]


@requires_docker
def test_every_indicator_is_one_the_gateway_accepts_and_valid_in_its_smart_data_model(sexe, sales, propulsion):
    for entity in sexe["entities"] + sales["entities"] + propulsion["entities"]:
        assert open_data.schema_errors(SCHEMA, entity) == [], entity["id"]
        assert open_data.sdm_errors(SDM, entity) == [], entity["id"]


@requires_docker
def test_a_monthly_table_reads_its_newest_month_and_drops_each_hidden_cell(sales):
    assert sales["period"] == "2026-02"
    entities = {e["id"]: e for e in sales["entities"]}
    some = next(iter(entities.values()))
    assert some["calculationPeriod"]["value"] == {"from": "2026-02-01", "to": "2026-02-28"}
    assert some["kpiValue"]["observedAt"] == "2026-02-28T00:00:00Z" and some["calculationFrequency"]["value"] == "monthly"
    # The source hides fewer than 5 as "..": that barri value is dropped, and so is every sum over it.
    assert sales["dropped"]["hidden by the source (fewer than 5)"] > 0
    assert sales["dropped"]["a sum over a hidden cell"] > 0
    assert all(e["kpiValue"]["value"] >= 5 for e in entities.values() if e["divisionLevel"]["value"] == "neighbourhood")


@requires_docker
def test_a_text_breakdown_keys_the_id_and_names_the_indicator(propulsion):
    entities = {e["id"]: e for e in propulsion["entities"]}
    petrol = entities[PREFIX + "vehicles-est_vehicles_propulsio-Combusti_Interna_Gasolina-city"]
    assert petrol["name"]["value"].endswith(": Combustió Interna-Gasolina – Barcelona")
    assert petrol["dimensionKey"]["value"] == "Tipus_Propulsio=Combustió Interna-Gasolina"
    assert petrol["kpiValue"]["observedAt"] == "2025-12-31T00:00:00Z"


@requires_docker
def test_an_outside_area_a_negative_value_an_unknown_code_and_a_duplicate_are_each_dropped_and_counted():
    message = recorded("pad_mdbas_sexe")
    rows = message["answer"]
    raval = next(r for r in rows if r["b"] == "1" and r["k0"] == "1" and r["gb"] == 0)
    rows.append(dict(raval, d="11", b="99"))                       # a district and a barri the city has not
    rows.append(dict(raval, b="74"))                               # a barri outside the register
    rows.append(dict(raval, k0="3"))                               # a sex pad-dimensions does not list
    rows.append(dict(raval, k0="2", b="2", v="-4"))                # a negative count
    rows.append(dict(raval))                                       # the same barri twice
    rows.append(dict(raval, d="1", b="3", v=None, s="1"))          # a hidden cell
    out = decode(message)
    assert out["dropped"] == {"not a barri of the register": 2, "a code pad-dimensions does not list": 1,
                              "negative count or amount": 1, "duplicate id": 1, "hidden by the source (fewer than 5)": 1}
    assert len(out["entities"]) == 2 * 84


@requires_docker
def test_a_stale_an_empty_or_a_future_table_fails_the_run_instead_of_writing():
    for edit in (
        lambda rows: [r.update(p0="2019-01-01T00:00:00") for r in rows],   # older than three years
        lambda rows: [r.update(p0="2099-01-01T00:00:00") for r in rows],   # in the future
        lambda rows: rows.clear(),                                         # no row
        lambda rows: [r.update(v=None, s="1") for r in rows],               # every value hidden
    ):
        message = recorded("pad_mdbas_sexe")
        edit(message["answer"])
        assert decode(message) is None


# The seed.

def docs() -> dict:
    out = {}
    for path in CITY.glob("*.yaml"):
        if path.name == "index.yaml" or path.name.endswith((".linkml.yaml", "-bento.yaml")):
            continue
        doc = yaml.safe_load(path.read_text())
        out[(doc["kind"], doc["metadata"]["name"])] = doc["spec"]
    return out


def test_one_pipeline_writes_the_indicators_and_one_public_dataset_reads_each_theme():
    seed = docs()
    pipeline = seed[("Pipeline", "statistics")]
    assert pipeline["output"] == {"type": "KeyPerformanceIndicator", "mode": "upsert"}
    assert "period" not in pipeline and len(pipeline["schedule"].split()) == 5
    url = seed[("DataSource", pipeline["source"]["dataSourceRef"]["name"])]["http"]["url"]
    assert url.startswith("https://opendata-ajuntament.barcelona.cat/data/api/3/action/datastore_search?")
    # Every call of the mapping goes to the one allow-listed host.
    text = MAPPING.read_text()
    assert {line.split("url:")[1].strip().split("/data/")[0] for line in text.splitlines() if "url:" in line} == {
        "https://opendata-ajuntament.barcelona.cat"}
    writes = seed[("Policy", "pipelines-write")]
    assert "KeyPerformanceIndicator" in [e["type"] for rule in writes["information"] for e in rule["entities"]]
    for theme in THEMES:
        spec = seed[("Endpoint", f"statistics-{theme}")]
        policy = seed[("Policy", spec["policyRef"].rsplit(":", 1)[1])]
        assert policy["assignee"] == {"kind": "role", "id": "public"}
        [entry] = [e for rule in policy["information"] for e in rule["entities"]]
        assert entry["type"] == "KeyPerformanceIndicator" and f":barcelona:stat-{theme}-" in entry["idPattern"]
        assert all(op.startswith(("retrieve", "query", "createSub", "updateSub", "deleteSub")) for op in policy["operations"])
        assert spec["publish"]["ckan"]["instanceRef"] == {"kind": "CkanInstance", "name": "barcelona"}
        assert {"ngsi-ld", "csv", "odata", "websocket", "ogc-features"} <= set(spec["enabledRepresentations"])
        assert spec["catalog"]["pipelineRef"] == {"kind": "Pipeline", "name": "statistics"}
    space = yaml.safe_load((CITY / "barcelona-space.yaml").read_text())["spec"]
    [kpi] = [r for r in space["retention"] if r["entityType"] == "KeyPerformanceIndicator"]
    assert kpi == {"entityType": "KeyPerformanceIndicator", "history": "P3650D", "record": "onChange",
                   "heartbeat": "P30D"}


def test_each_table_of_the_mapping_is_listed_in_a_theme_dataset_s_sources():
    text = MAPPING.read_text()
    seed = docs()
    listed = {s["url"].rsplit("/", 1)[1] for theme in THEMES for s in seed[("Endpoint", f"statistics-{theme}")]["catalog"]["source"]}
    ids = {line.split('"id": "')[1].split('"')[0] for line in text.splitlines() if '"id": "' in line and ("assign(" in line or '"theme": ' in line)}
    assert len(ids) == 44 and ids == listed
