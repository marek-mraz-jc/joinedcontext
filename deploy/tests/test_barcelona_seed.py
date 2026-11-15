"""The Barcelona seed (T-5260, ADR-N-058, PF-121..PF-123, DM-61).

Ajuntament de Barcelona is an Organization of its own with one project and one space of Open Data
BCN, set up as Mesto Žilina is (T-5242). These cases read the seed manifests directly; what the
cluster does with them is `dev-smoke`'s.
"""

import json
import re
from pathlib import Path
from urllib.parse import unquote

import pytest
import yaml
from conftest import demo_id

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "components/context-gateway/seed"
BARCELONA = SEED / "barcelona"
# The types the space's model declares; the write and read Policies grant these and no other.
TYPES = {
    "AirQualityObserved",
    "WeatherObserved",
    "WeatherForecast",
    "PhreaticObserved",
    "TrafficFlowObserved",
    "OnStreetParking",
    "EVChargingStation",
    "Event",
    "PointOfInterest",
    "AdministrativeArea",
    "CityWork",
    "PedestrianCrossing",
    "StreetFurniture",
    "TouristRental",
    "KeyPerformanceIndicator",
    "BikeLane",
    "RoadAccident",
}
WRITES = {"upsertBatch", "createBatch", "queryBatch"}


def manifests(kind: str):
    for path in sorted(BARCELONA.glob("*.yaml")):
        if path.name == "index.yaml" or path.name.endswith(".linkml.yaml"):
            continue
        for doc in yaml.safe_load_all(path.read_text()):
            if isinstance(doc, dict) and doc.get("kind") == kind:
                yield doc


def by_name(kind: str) -> dict:
    return {doc["metadata"]["name"]: doc for doc in manifests(kind)}


def granted_types(policy: dict) -> set:
    return {e["type"] for rule in policy["spec"]["information"] for e in rule["entities"]}


def test_the_index_lists_every_file_and_names_no_missing_one():
    index = yaml.safe_load((BARCELONA / "index.yaml").read_text())
    files = {p.name for p in BARCELONA.iterdir() if p.is_file() and p.name not in {"index.yaml", "README.md"}}
    assert set(index) == files
    own = {
        "organizations/barcelona.yaml",
        "users/assignments/barcelona-admins.yaml",
        "users/assignments/barcelona-viewers.yaml",
    }
    assert all(path.startswith("projects/barcelona/") or path in own for path in index.values())
    assert own <= set(index.values())
    assert len(set(index.values())) == len(index), "two files on one repository path"


def test_the_forge_commits_the_seed():
    bootstrap = (ROOT / "components/gitea/values/bootstrap/development-values.yaml.gotmpl").read_text()
    seed = (ROOT / "components/gitea/values/seed/development-values.yaml.gotmpl").read_text()
    assert 'seed/barcelona/index.yaml' in bootstrap
    assert bootstrap.count('"context-gateway/seed/barcelona"') == 1
    assert seed.count('"context-gateway/seed/barcelona"') == 1


def test_the_organization_is_its_own_and_not_the_installations():
    org = by_name("Organization")["barcelona"]
    assert org["metadata"]["namespace"] == "org"
    assert org["metadata"]["title"]["ca"] == "Ajuntament de Barcelona"
    spec = org["spec"]
    assert spec["domain"] == "barcelona.cat"
    assert spec["defaultLocale"] in spec["locales"] == ["ca", "es", "en"]
    assert not spec.get("installation"), "Helsinki stays the installation's Organization"
    assert spec["catalog"]["publisher"]["uri"].endswith("/L01080193"), "the Ajuntament's DIR3 code"
    project = by_name("Project")
    assert list(project) == ["barcelona"]
    assert project["barcelona"]["spec"]["organizationRef"] == "barcelona"
    domain = project["barcelona"]["spec"]["domain"]
    assert domain == spec["domain"] or domain.endswith("." + spec["domain"]), "PF-122"


def test_one_space_pinning_its_segment_and_its_one_model():
    (space,) = by_name("ContextSpace").values()
    assert space["metadata"]["name"] == "barcelona"
    spec = space["spec"]
    assert spec["urnSegment"] == "barcelona", "the segment the ids carry is the space's name (PF-84)"
    assert spec["dataModelRef"] == {"kind": "DataModel", "name": "barcelona"}
    model = by_name("DataModel")["barcelona"]["spec"]
    assert model["contextSpaceRef"] == "barcelona" and model["lifecycle"] == "published"
    assert set(model["classes"]) == TYPES
    linkml = yaml.safe_load((BARCELONA / "barcelona.linkml.yaml").read_text())
    # The entity classes; PostalAddress and ContactPoint are values inside an attribute.
    assert {n for n, c in linkml["classes"].items() if c.get("is_a") == "Entity"} == TYPES
    for artifact in model["artifacts"].values():
        assert (BARCELONA / artifact.removeprefix("./")).is_file(), artifact
    # Every type keeps a retention row and the city's area check (owner 2026-10-22T18:10Z).
    assert {row["entityType"] for row in spec["retention"]} == TYPES
    areas = {q["entityType"]: q["bbox"] for q in spec["quality"] if q["kind"] == "area"}
    # An indicator has no geometry; its refArea names a district or barri the register holds (T-5266).
    assert set(areas) == TYPES - {"KeyPerformanceIndicator"}
    for west, south, east, north in areas.values():
        # The municipality (2.05..2.23 E, 41.32..41.47 N) with about 1 km to spare, nothing wider.
        assert 2.03 <= west <= 2.05 and 2.23 <= east <= 2.25, "longitude first, not swapped"
        assert 41.29 <= south <= 41.32 and 41.47 <= north <= 41.49


def test_the_example_validates_against_its_model():
    jsonschema = pytest.importorskip("jsonschema")
    schema = json.loads((BARCELONA / "barcelona.v1.schema.json").read_text())
    example = json.loads((BARCELONA / "barcelona.v1.example.jsonld").read_text())
    example.pop("@context", None)
    validator = jsonschema.Draft7Validator({**schema["definitions"][example["type"]], "definitions": schema["definitions"]})
    assert not sorted(validator.iter_errors(example), key=str)


def test_a_scheduled_pipeline_names_no_period():
    # The runner reads `period` before `schedule` (Portal run_queue Clock::of): a pipeline with
    # both runs on the period's clock and the schedule's timing after the source's rewrite is lost.
    for name, pipeline in by_name("Pipeline").items():
        if pipeline["spec"].get("schedule"):
            assert "period" not in pipeline["spec"], name


def test_the_pipelines_account_writes_the_one_space_and_nothing_else():
    account = by_name("ServiceAccount")["pipelines"]["spec"]
    assert account["roles"] == [{"role": "space-writer", "scope": {"contextSpace": "barcelona"}}], (
        "no role in another project's space"
    )
    assert [c["kind"] for c in account["credentials"]] == ["oauth-client"], "a Keycloak client, no static key"


def test_the_write_policy_grants_the_pipelines_the_models_types_only():
    """A write of a type outside the Policy is refused (403), and so is any write by someone the
    Policy does not name, an anonymous caller included (401 at the door, 403 by Policy)."""
    policies = by_name("Policy")
    write = policies["pipelines-write"]["spec"]
    assert write["contextSpaceRef"] == {"kind": "ContextSpace", "name": "barcelona"}
    assert write["assignee"] == {"kind": "serviceAccount", "id": "pipelines"}
    assert set(write["operations"]) == WRITES, "no delete: an empty source never wipes the space"
    assert granted_types(policies["pipelines-write"]) == TYPES
    for name, policy in policies.items():
        operations = set(policy["spec"]["operations"])
        if policy["spec"]["assignee"] != {"kind": "serviceAccount", "id": "pipelines"}:
            assert not operations & (WRITES - {"queryBatch"}), f"{name} grants a write to someone else"
        assert granted_types(policy) <= TYPES, f"{name} grants a type the model does not declare"


def test_the_public_reads_and_never_writes():
    read = by_name("Policy")["public-read"]["spec"]
    assert read["assignee"] == {"kind": "role", "id": "public"}
    assert all(op.startswith(("retrieve", "query", "createSub", "updateSub", "deleteSub")) for op in read["operations"])
    assert not {"queryBatch"} & set(read["operations"])


def test_the_one_door_is_public_cc_by_and_credits_the_ajuntament():
    # The door the pipelines write through; each dataset's own narrowed Endpoint sits beside it.
    spec = by_name("Endpoint")["barcelona"]["spec"]
    assert "policyRef" not in spec
    assert spec["contextSpaceRef"] == "barcelona" and spec["audience"] == "public"
    assert len(spec["slug"]) == 32
    catalog = spec["catalog"]
    assert catalog["harvest"] is False, "the door is not a catalogue dataset; each type gets its own"
    assert catalog["license"] == "CC_BY_4_0"
    assert all("Barcelona" in text and "CC BY 4.0" in text for text in catalog["attribution"].values())
    assert set(catalog["attribution"]) == {"ca", "es", "en"}


def test_the_bindings_end_at_its_organization_pf123():
    bindings = by_name("RoleBinding")
    assert bindings["barcelona-admins"]["spec"] == {
        "subjects": [{"user": demo_id("demo.barcelona")}], "role": "org-admin", "scope": {"organization": "barcelona"},
    }
    assert bindings["barcelona-viewers"]["spec"] == {
        "subjects": [{"group": "barcelona", "source": "provider"}], "role": "viewer",
        "scope": {"organization": "barcelona"},
    }
    people = {demo_id("demo.barcelona"), "barcelona"}
    for path in sorted(SEED.glob("*/*.yaml")):
        for doc in yaml.safe_load_all(path.read_text()):
            if isinstance(doc, dict) and doc.get("kind") == "RoleBinding" and path.parent != BARCELONA:
                names = {s.get("user") or s.get("group") for s in doc["spec"]["subjects"]}
                assert not names & people, f"{path.name} names Barcelona's people outside its Organization"


def test_the_ckan_organisation_is_titled_ajuntament_de_barcelona():
    """jcctl names the CKAN organisation from the instance title in the space's language (ca)."""
    instance = by_name("CkanInstance")["barcelona"]
    assert instance["metadata"]["title"]["ca"] == "Ajuntament de Barcelona"
    assert instance["spec"]["organizationDefault"] == "barcelona"
    assert set(instance["spec"]["apiTokenRef"]) == {"name", "key", "envVar"}, "the token is a reference"
    (space,) = by_name("ContextSpace").values()
    assert space["spec"]["defaultLocale"] == "ca"


def test_keycloak_makes_the_subtree_root_and_the_administrator():
    environment = (ROOT / "components/keycloak/default-environment.yaml.gotmpl").read_text()
    assert "organizations: [zilina, barcelona, bratislava]" in environment
    users = yaml.safe_load((ROOT / "components/keycloak/demo-users.yaml").read_text())
    assert users["demo.barcelona"]["groups"] == ["barcelona"]
    assert users["demo.barcelona"]["realmRoles"] == ["portal-approver"]


def test_every_open_data_bcn_query_with_a_limit_fails_the_run_when_its_answer_reaches_it():
    """F-R2BNT1-004 (T-6215): a DataStore SQL query ends in LIMIT N, so an answer of N rows may be
    cut short. Its mapping refuses such an answer, or the rows past N vanish without a word."""
    unguarded, limits = [], 0
    for pipeline in sorted(BARCELONA.glob("barcelona-pipeline-*.yaml")):
        if pipeline.name.endswith("-bento.yaml"):
            continue
        bento = pipeline.with_name(pipeline.stem + "-bento.yaml").read_text()
        sql = bento
        ref = (yaml.safe_load(pipeline.read_text())["spec"].get("source") or {}).get("dataSourceRef")
        for source in BARCELONA.glob("barcelona-datasource-*.yaml"):
            doc = yaml.safe_load(source.read_text())
            if ref and doc["metadata"]["name"] == ref["name"]:
                sql += unquote(doc["spec"].get("http", {}).get("url", ""))
        for n in sorted(set(re.findall(r"\bLIMIT (\d+)", sql))):
            limits += 1
            if f"length() >= {n} " not in bento:
                unguarded.append(f"{pipeline.name}: LIMIT {n}")
    assert not unguarded, unguarded
    assert limits >= 5, "the agenda, equipaments, traffic counters, accidents and trees queries are checked"
