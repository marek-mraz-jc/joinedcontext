"""Helsinki service usage as Smart Data Model KeyPerformanceIndicator (T-5258, T-5667, PL-03, DM-01).

Bento runs the committed processors of `helsinki-pipeline-service-usage-bento.yaml` over recorded
answers (`fixtures/helsinki/service-usage/`, fetched on 2026-10-23): the first four sites of the
city's Matomo export for September 2026. The web analytics call itself is left out: each case
feeds the answers the step after it sees.
"""

import json

import pytest
import yaml

import open_data
from open_data import requires_docker
from test_helsinki_statistics import HELSINKI, ROOT, SDM, bento

pytestmark = pytest.mark.xdist_group("docker-helsinki-statistics")

MAPPING = HELSINKI / "helsinki-pipeline-service-usage-bento.yaml"
FIXTURES = ROOT / "tests/fixtures/helsinki/service-usage"
PREFIX = "urn:ngsi-ld:KeyPerformanceIndicator:hel.fi:helsinki:stat-services-"


def steps() -> list:
    return yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"][0]["try"]


def decode(sites: list, month: str = "2026-9") -> dict | None:
    out = bento([steps()[2]], {"registered": 252, "web": {"month": month, "sites": sites}})
    return out[0] if out else None


def sites() -> list:
    return json.loads((FIXTURES / "matomo-export-2026-9-sites.json").read_text())


@pytest.fixture(scope="module")
def usage():
    if open_data.shutil.which("docker") is None:
        pytest.skip("no container runtime")
    return decode(sites())


@requires_docker
def test_each_site_of_the_month_is_two_indicators(usage):
    entities = {e["id"]: e for e in usage["entities"]}
    visits = entities[PREFIX + "web-visits-141-0910000000"]
    assert visits["kpiValue"] == {"type": "Property", "value": 1144538, "unitCode": "C62", "observedAt": "2026-09-30T00:00:00Z"}
    assert visits["calculationPeriod"]["value"] == {"from": "2026-09-01", "to": "2026-09-30"}
    assert visits["calculationFrequency"]["value"] == "monthly"
    assert visits["name"]["languageMap"]["fi"] == "HEL Etusivu: käynnit kuukaudessa"
    assert visits["dimensionKey"]["value"] == "idsite=141"
    assert entities[PREFIX + "web-pageviews-141-0910000000"]["kpiValue"]["value"] == 2228869
    # Two per site (4 sites); the zoo series are gone with its suspended API (T-5667).
    assert len(entities) == 2 * 4 and usage["dropped"] == {} and usage["period"] == "2026-9"
    assert not [i for i in entities if "zoo" in i]


@requires_docker
def test_every_indicator_meets_the_space_schema_and_the_smart_data_model(usage):
    for entity in usage["entities"]:
        assert open_data.schema_errors(HELSINKI / "helsinki.v1.schema.json", entity) == [], entity["id"]
        assert open_data.sdm_errors(SDM, entity) == [], entity["id"]
        assert entity["id"].startswith(PREFIX)


@requires_docker
def test_negative_not_a_number_and_duplicate_are_dropped_with_a_count():
    listed = sites() + [sites()[0], {"idsite": 5, "label": "x", "main_url": "", "nb_visits": -3, "nb_pageviews": "many"}]
    out = decode(listed)
    assert out["dropped"] == {"duplicate id": 2, "negative count": 1, "not a number": 1}
    ids = [e["id"] for e in out["entities"]]
    assert len(ids) == len(set(ids)) == 2 * 4


@requires_docker
def test_no_site_in_the_export_fails_the_run():
    assert decode([]) is None


@requires_docker
def test_the_site_register_is_the_first_read_and_an_answer_without_sites_fails_the_run():
    first = steps()[0]
    assert bento([first], {"141": {"idsite": 141, "name": "HEL Etusivu"}, "57": {"idsite": 57}}) == [{"registered": 2}]
    # The suspended zoo host answered an HTML page; an error page or an empty object fails the same way.
    assert bento([first], {}) == []
    assert bento([first], [{"idsite": 141}]) == []


def test_service_usage_is_one_public_endpoint_whose_policy_reads_only_its_own_indicators():
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    endpoint = yaml.safe_load((HELSINKI / "helsinki-endpoint-statistics-services.yaml").read_text())
    policy = yaml.safe_load((HELSINKI / "helsinki-policy-public-statistics-services.yaml").read_text())
    spec = endpoint["spec"]
    assert spec["policyRef"].endswith(":public-statistics-services")
    assert {"ngsi-ld", "csv", "odata"} <= set(spec["enabledRepresentations"])
    assert spec["catalog"]["license"] == "CC_BY_4_0" and "CC BY 4.0" in spec["catalog"]["attribution"]["en"]
    assert "dimensionKey" in endpoint["metadata"]["description"]["en"]
    assert spec["catalog"]["pipelineRef"] == {"kind": "Pipeline", "name": "service-usage"}
    (selector,) = policy["spec"]["information"][0]["entities"]
    assert selector == {"type": "KeyPerformanceIndicator",
                        "idPattern": "^urn:ngsi-ld:KeyPerformanceIndicator:[a-z0-9.-]+:helsinki:stat-services-[A-Za-z0-9._-]+$"}
    assert not [op for op in policy["spec"]["operations"] if op.startswith(("create", "update", "delete", "upsert", "append"))
                and "Subscription" not in op]
    for name in ["helsinki-datasource-web-analytics-sites.yaml", "helsinki-pipeline-service-usage.yaml",
                 "helsinki-pipeline-service-usage-bento.yaml", "helsinki-endpoint-statistics-services.yaml",
                 "helsinki-policy-public-statistics-services.yaml"]:
        assert name in index


def test_the_pipeline_reads_only_the_two_named_hosts_with_a_timeout():
    source = yaml.safe_load((HELSINKI / "helsinki-datasource-web-analytics-sites.yaml").read_text())
    assert source["spec"]["http"]["url"] == "https://www.hel.fi/static/avoindata/kanslia/ID_lista_Matomo.json"
    pipeline = yaml.safe_load((HELSINKI / "helsinki-pipeline-service-usage.yaml").read_text())
    assert pipeline["spec"]["source"]["dataSourceRef"]["name"] == source["metadata"]["name"]
    assert "onrender.com" not in MAPPING.read_text()
    assert source["spec"]["http"]["timeout"] == "30s"
    calls = next(p for p in steps() if "branch" in p)["branch"]["processors"]
    assert calls[0]["http"]["url"] == (
        "https://datastore.hri.fi/Helsinki/kanslia/kavijaanalytiikka/matomo-export-${! @month }-sites.json")
    assert calls[0]["http"]["timeout"] == "30s"
    assert "4194304" in calls[1]["mapping"] and "4194304" in steps()[0]["mapping"]
