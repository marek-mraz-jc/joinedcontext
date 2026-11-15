"""The city's register of facilities becomes SDM PointOfInterest, checked and counted (T-5265).

Bento runs the committed mapping over the `equipament-*` package list and three facilities of
four families, recorded from Open Data BCN on 2026-10-22 (`fixtures/barcelona/equipaments/`).
The administration rows are the register's own defect: telephone lines with the point (0,0) of
its projection, which land near 0,0 and outside Barcelona. The test adds a facility listed twice
under two ids, one with no name and one listed in a second family. The calls to the portal are the
cut: each family's answer comes from the recording. Without a container runtime the Bento cases
skip.
"""

import json
import subprocess
from datetime import datetime, timezone
from pathlib import Path

import pytest
import yaml

import open_data
from open_data import BENTO, requires_docker

pytestmark = pytest.mark.xdist_group("docker-barcelona-open-data")

ROOT = Path(__file__).resolve().parent.parent
BARCELONA = ROOT / "components/context-gateway/seed/barcelona"
MAPPING = BARCELONA / "barcelona-pipeline-equipaments-bento.yaml"
FIXTURES = Path(__file__).resolve().parent / "fixtures/barcelona/equipaments"
URN = "urn:ngsi-ld:PointOfInterest:barcelona.cat:barcelona:{}"
SPORTS = "6409e71a-6c79-4d21-9c14-373dbd01f26d"
EDUCATION = "29d9ff10-6892-4f16-9012-d5c4997857e7"


def families(modified: str | None = None) -> dict:
    """The recorded answers, every change moved to `modified` (the newest to now by default), and the
    test's own rows: the pool twice, a row with no name, a school in the sports family too."""
    answers = json.loads((FIXTURES / "families.json").read_text())
    sports = answers[SPORTS]["result"]["records"]
    pool = next(r for r in sports if r["name"] == "Piscina Coberta")
    school = next(r for r in answers[EDUCATION]["result"]["records"] if r["name"] == "Tecniber-5")
    sports += [dict(pool, id="90000000001"), dict(pool, id="90000000002", name=" "), dict(school, kind="Altres esports", kinds=["Altres esports"])]
    if modified:
        for record in (r for answer in answers.values() for r in answer["result"]["records"]):
            record["modified"] = modified
    else:
        sports[0]["modified"] = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S")
    return answers


def run(answers: dict, packages: dict | None = None) -> tuple[list[dict], str]:
    steps = yaml.safe_load(MAPPING.read_text())["pipeline"]["processors"]
    for step in steps:
        if "branch" in step:
            http = step["branch"]["processors"][-1]["http"]
            assert http["url"].startswith("https://opendata-ajuntament.barcelona.cat/data/api/3/action/datastore_search_sql?sql=")
            # Each family's query names its resource once, quoted; the recording answers it.
            step["branch"]["processors"] = [{"mapping": "root = %s.get(@sql.re_find_all_submatch(\"%%22([0-9a-f-]{36})%%22\").index(0).index(1)).or(throw(\"unasked\"))" % json.dumps(answers)}]
    config = {"input": {"stdin": {"scanner": {"to_the_end": {}}}}, "pipeline": {"processors": steps},
              "output": {"stdout": {"codec": "lines"}}, "logger": {"level": "info", "format": "logfmt"}}
    document = json.dumps(packages or json.loads((FIXTURES / "packages.json").read_text())).encode()
    result = subprocess.run(
        ["docker", "run", "--rm", "-i", "-e", "JC_ORG_DOMAIN=barcelona.cat", "-e", "JC_SPACE=barcelona",
         "-e", f"CONFIG={json.dumps(config)}", "--entrypoint", "sh", BENTO,
         "-c", 'printf "%s" "$CONFIG" > /tmp/c.json && exec /bento -c /tmp/c.json'],
        input=document, capture_output=True, timeout=120)
    lines = [line for line in result.stdout.decode().splitlines() if line.startswith("[")]
    return [e for line in lines for e in json.loads(line)], result.stderr.decode() + result.stdout.decode()


@requires_docker
def test_the_recorded_register_maps_to_valid_places_and_each_bad_row_is_dropped_by_reason():
    places, log = run(families())
    by_id = {e["id"]: e for e in places}
    # Health is not read (stale since 2024, an older layout), nor the associations (home addresses).
    assert sorted(by_id) == sorted(URN.format(i) for i in (
        "1029210328", "1057133310", "1108125041", "1003125537", "1033142110", "1080145330",
        "1031123107", "1047164651", "1066143504", "1138134103"))
    assert ('dropped by reason {\\"duplicate\\":1,\\"listed in a second family\\":1,\\"no name\\":1,'
            '\\"outside Barcelona\\":3}') in log, log
    for place in places:
        assert not open_data.schema_errors(BARCELONA / "barcelona.v1.schema.json", place), place
        assert not open_data.sdm_errors("dataModel.PointOfInterest/PointOfInterest", place), place
    pool = by_id[URN.format("1057133310")]
    # Every kind the register gives a facility, sorted after its family (T-5692).
    assert pool["category"]["value"] == ["sports", "Altres esports", "Instal·lacions esportives", "Natació"]
    assert pool["contactPoint"]["value"] == {"telephone": "934234350"}
    assert pool["source"]["value"] == "https://opendata-ajuntament.barcelona.cat/data/dataset/equipament-esports"
    # The first family that lists a facility comes first; the second listing is counted, not
    # written twice, and adds its family and kinds. The family list's order decides: sports
    # comes before education.
    assert by_id[URN.format("1047164651")]["category"]["value"] == [
        "sports", "education", "Altres esports", "Idiomes", "Informàtica"]
    # A municipal library stays one: a library, a municipal library and a study room (T-5692:
    # one kind a facility served 1 of the register's 52 municipal libraries).
    assert by_id[URN.format("1138134103")]["category"]["value"] == [
        "culture-leisure", "Biblioteques", "Biblioteques municipals", "Sales d'estudi"]
    garden = by_id[URN.format("1003125537")]
    assert garden["category"]["value"] == ["culture-leisure", "Parcs i jardins"]
    assert "contactPoint" not in garden


@requires_docker
def test_a_stale_register_or_one_with_no_family_fails_the_run():
    _, log = run(families(modified="2026-01-01T00:00:00"))
    assert "the Barcelona equipaments are stale" in log, log
    _, log = run(families(), packages={"success": True, "result": {"results": [{"name": "equipament-associacions", "resources": []}]}})
    assert "Open Data BCN lists none of the equipaments families" in log, log


@requires_docker
def test_a_family_the_portal_refuses_is_logged_and_the_others_still_write():
    answers = families()
    answers[EDUCATION] = {"success": False, "error": {"message": "timeout"}}
    places, log = run(answers)
    assert "equipaments: equipament-educacio not read" in log, log
    assert URN.format("1057133310") in {e["id"] for e in places}
    assert not {URN.format("1066143504")} & {e["id"] for e in places}


def test_the_query_takes_only_a_uuid_resource_and_reads_no_association():
    mapping = MAPPING.read_text()
    assert '"equipament-associacions"' not in mapping.split("let families = [")[1].split("\n        ]")[0]
    assert 're_match("^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")' in mapping
    pipeline = yaml.safe_load((BARCELONA / "barcelona-pipeline-equipaments.yaml").read_text())["spec"]
    assert pipeline["output"] == {"type": "PointOfInterest", "mode": "upsert"} and pipeline["schedule"] == "47 6 * * *"
    policy = yaml.safe_load((BARCELONA / "barcelona-policy-public-places.yaml").read_text())["spec"]
    assert [e["type"] for rule in policy["information"] for e in rule["entities"]] == ["PointOfInterest"]
    endpoint = yaml.safe_load((BARCELONA / "barcelona-endpoint-places.yaml").read_text())["spec"]
    assert endpoint["policyRef"] == "urn:ngsi-ld:Policy:barcelona.cat:barcelona:public-places"


@requires_docker
def test_an_extract_without_the_kind_column_writes_its_own_category_only():
    shelters = "7ecae024-6cb2-427d-b2d0-e170500e2a38"
    answers = families()
    garden = next(r for r in answers["f3721b17-bf9e-4bdd-853c-cb6200e1b442"]["result"]["records"]
                  if r["id"] == "1003125537")
    answers[shelters] = {"success": True, "result": {"records": [
        dict(garden, id="90000000003", name="Refugi climàtic", kind=None, kinds=[], num="9999"),
        dict(garden, kind=None, kinds=[])]}}
    packages = json.loads((FIXTURES / "packages.json").read_text())
    packages["result"]["results"].append({"name": "xarxa-refugis-climatics", "resources": [
        {"id": shelters, "format": "CSV", "datastore_active": True}]})
    places, log = run(answers, packages)
    by_id = {e["id"]: e for e in places}
    shelter = by_id[URN.format("90000000003")]
    assert shelter["category"]["value"] == ["climate-shelters"], log
    # 9999 is the register's "no street number", never written as one.
    assert "9999" not in shelter["address"]["value"]["streetAddress"]
    # A garden that is also a shelter is written once, the garden's family first.
    assert by_id[URN.format("1003125537")]["category"]["value"] == ["culture-leisure", "climate-shelters", "Parcs i jardins"]
