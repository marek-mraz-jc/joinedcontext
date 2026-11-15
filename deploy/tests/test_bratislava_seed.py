"""The Bratislava seed (T-5271, ADR-N-058, PF-121..PF-123, DM-61).

Hlavné mesto SR Bratislava is an Organization of its own with one project and one space of
data.bratislava.sk, set up as Mesto Žilina is (T-5242). These cases read the seed manifests directly; what the
cluster does with them is `dev-smoke`'s.
"""

import json
from pathlib import Path

import pytest
import yaml
from conftest import demo_id

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "components/context-gateway/seed"
BRATISLAVA = SEED / "bratislava"
# The types the space's model declares; the write and read Policies grant these and no other.
TYPES = {
    "AirQualityObserved", "AdministrativeArea", "PointOfInterest", "KeyPerformanceIndicator",
    "TrafficFlowObserved", "CrowdFlowObserved", "BikeHireDockingStation", "GtfsStop", "GtfsRoute", "GtfsAgency",
    "OnStreetParking",
}
# The districts pipeline checks every vertex of an outline itself; an indicator, a line and its
# agency have no location.
LOCATED = TYPES - {"AdministrativeArea", "KeyPerformanceIndicator", "GtfsRoute", "GtfsAgency"}
WRITES = {"upsertBatch", "createBatch", "queryBatch"}


def manifests(kind: str):
    for path in sorted(BRATISLAVA.glob("*.yaml")):
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
    index = yaml.safe_load((BRATISLAVA / "index.yaml").read_text())
    files = {p.name for p in BRATISLAVA.iterdir() if p.is_file() and p.name not in {"index.yaml", "README.md"}}
    assert set(index) == files
    own = {
        "organizations/bratislava.yaml",
        "users/assignments/bratislava-admins.yaml",
        "users/assignments/bratislava-viewers.yaml",
    }
    assert all(path.startswith("projects/bratislava/") or path in own for path in index.values())
    assert own <= set(index.values())
    assert len(set(index.values())) == len(index), "two files on one repository path"


def test_the_forge_commits_the_seed():
    bootstrap = (ROOT / "components/gitea/values/bootstrap/development-values.yaml.gotmpl").read_text()
    seed = (ROOT / "components/gitea/values/seed/development-values.yaml.gotmpl").read_text()
    assert 'seed/bratislava/index.yaml' in bootstrap
    assert bootstrap.count('"context-gateway/seed/bratislava"') == 1
    assert seed.count('"context-gateway/seed/bratislava"') == 1


def test_the_organization_is_its_own_and_not_the_installations():
    org = by_name("Organization")["bratislava"]
    assert org["metadata"]["namespace"] == "org"
    assert org["metadata"]["title"]["sk"] == "Hlavné mesto SR Bratislava"
    spec = org["spec"]
    assert spec["domain"] == "bratislava.sk"
    assert spec["defaultLocale"] in spec["locales"] == ["sk", "en"]
    assert spec["dcatProfiles"] == ["dcat-ap-sk"]
    assert not spec.get("installation"), "Helsinki stays the installation's Organization"
    assert spec["catalog"]["publisher"]["uri"] == "https://data.gov.sk/id/legal-subject/00603481", "the capital's IČO"
    project = by_name("Project")
    assert list(project) == ["bratislava"]
    assert project["bratislava"]["spec"]["organizationRef"] == "bratislava"
    domain = project["bratislava"]["spec"]["domain"]
    assert domain == spec["domain"] or domain.endswith("." + spec["domain"]), "PF-122"


def test_one_space_pinning_its_segment_and_its_one_model():
    (space,) = by_name("ContextSpace").values()
    assert space["metadata"]["name"] == "bratislava"
    spec = space["spec"]
    assert spec["urnSegment"] == "bratislava", "the segment the ids carry is the space's name (PF-84)"
    assert spec["dataModelRef"] == {"kind": "DataModel", "name": "bratislava"}
    model = by_name("DataModel")["bratislava"]["spec"]
    assert model["contextSpaceRef"] == "bratislava" and model["lifecycle"] == "published"
    assert set(model["classes"]) == TYPES
    linkml = yaml.safe_load((BRATISLAVA / "bratislava.linkml.yaml").read_text())
    assert {n for n, c in linkml["classes"].items() if c.get("is_a") == "Entity"} == TYPES
    for artifact in model["artifacts"].values():
        assert (BRATISLAVA / artifact.removeprefix("./")).is_file(), artifact
    # Every type keeps a retention row and the city's area check (owner 2026-10-22T18:10Z).
    assert {row["entityType"] for row in spec["retention"]} == TYPES
    areas = {q["entityType"]: q["bbox"] for q in spec["quality"] if q["kind"] == "area"}
    assert set(areas) == LOCATED
    for west, south, east, north in areas.values():
        # The municipality (16.946..17.285 E, 48.007..48.265 N, OSM) with about 1 km to spare,
        # nothing wider.
        assert 16.92 <= west <= 16.946 and 17.285 <= east <= 17.31, "longitude first, not swapped"
        assert 47.98 <= south <= 48.007 and 48.265 <= north <= 48.28


def test_the_example_validates_against_its_model():
    jsonschema = pytest.importorskip("jsonschema")
    schema = json.loads((BRATISLAVA / "bratislava.v1.schema.json").read_text())
    example = json.loads((BRATISLAVA / "bratislava.v1.example.jsonld").read_text())
    example.pop("@context", None)
    validator = jsonschema.Draft7Validator(schema["definitions"][example["type"]])
    assert not sorted(validator.iter_errors(example), key=str)


def test_the_pipelines_account_writes_the_one_space_and_nothing_else():
    account = by_name("ServiceAccount")["pipelines"]["spec"]
    assert account["roles"] == [{"role": "space-writer", "scope": {"contextSpace": "bratislava"}}], (
        "no role in another project's space"
    )
    assert [c["kind"] for c in account["credentials"]] == ["oauth-client"], "a Keycloak client, no static key"


def test_the_write_policy_grants_the_pipelines_the_models_types_only():
    """A write of a type outside the Policy is refused (403), and so is any write by someone the
    Policy does not name, an anonymous caller included (401 at the door, 403 by Policy)."""
    policies = by_name("Policy")
    write = policies["pipelines-write"]["spec"]
    assert write["contextSpaceRef"] == {"kind": "ContextSpace", "name": "bratislava"}
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


def test_the_one_door_is_public_cc_by_and_credits_the_city():
    spec = by_name("Endpoint")["bratislava"]["spec"]
    assert spec["contextSpaceRef"] == "bratislava" and spec["audience"] == "public"
    assert len(spec["slug"]) == 32
    catalog = spec["catalog"]
    assert catalog["harvest"] is False, "the door is not a catalogue dataset; each type gets its own"
    assert catalog["license"] == "CC_BY_4_0"
    assert all("Bratislava" in text and "CC BY 4.0" in text for text in catalog["attribution"].values())
    assert set(catalog["attribution"]) == {"sk", "en"}
    assert "data.bratislava.sk" in catalog["attribution"]["sk"]


def test_the_bindings_end_at_its_organization_pf123():
    bindings = by_name("RoleBinding")
    assert bindings["bratislava-admins"]["spec"] == {
        "subjects": [{"user": demo_id("demo.bratislava")}], "role": "org-admin", "scope": {"organization": "bratislava"},
    }
    assert bindings["bratislava-viewers"]["spec"] == {
        "subjects": [{"group": "bratislava", "source": "provider"}], "role": "viewer",
        "scope": {"organization": "bratislava"},
    }
    people = {demo_id("demo.bratislava"), "bratislava"}
    for path in sorted(SEED.glob("*/*.yaml")):
        for doc in yaml.safe_load_all(path.read_text()):
            if isinstance(doc, dict) and doc.get("kind") == "RoleBinding" and path.parent != BRATISLAVA:
                names = {s.get("user") or s.get("group") for s in doc["spec"]["subjects"]}
                assert not names & people, f"{path.name} names Bratislava's people outside its Organization"


def test_the_ckan_organisation_is_titled_hlavne_mesto_sr_bratislava():
    """jcctl names the CKAN organisation from the instance title in the space's language (sk)."""
    instance = by_name("CkanInstance")["bratislava"]
    assert instance["metadata"]["title"]["sk"] == "Hlavné mesto SR Bratislava"
    assert instance["spec"]["organizationDefault"] == "bratislava"
    assert set(instance["spec"]["apiTokenRef"]) == {"name", "key", "envVar"}, "the token is a reference"
    (space,) = by_name("ContextSpace").values()
    assert space["spec"]["defaultLocale"] == "sk"


def test_keycloak_makes_the_subtree_root_and_the_administrator():
    environment = (ROOT / "components/keycloak/default-environment.yaml.gotmpl").read_text()
    assert "organizations: [zilina, barcelona, bratislava]" in environment
    users = yaml.safe_load((ROOT / "components/keycloak/demo-users.yaml").read_text())
    assert users["demo.bratislava"]["groups"] == ["bratislava"]
    assert users["demo.bratislava"]["realmRoles"] == ["portal-approver"]


# T-5273: each catalogue dataset is its own door with its own public Policy.
DATASETS = {"bratislava-cyklisti": "public-cyklisti", "bratislava-vozidla": "public-vozidla",
            "bratislava-chodci": "public-chodci", "bratislava-bajk": "public-bajk",
            "bratislava-mhd-zastavky": "public-mhd-zastavky", "bratislava-mhd-linky": "public-mhd-linky"}


def test_each_dataset_door_reads_through_its_own_public_policy_and_publishes_to_the_city():
    endpoints, policies = by_name("Endpoint"), by_name("Policy")
    # T-5274 adds its own doors (tests/test_bratislava_hub.py checks them); these are this batch's.
    assert {"bratislava", *DATASETS} <= set(endpoints)
    slugs = [e["spec"]["slug"] for e in endpoints.values()]
    assert len(set(slugs)) == len(slugs) and all(len(slug) == 32 for slug in slugs)
    for name, policy in DATASETS.items():
        spec = endpoints[name]["spec"]
        assert spec["policyRef"] == f"urn:ngsi-ld:Policy:bratislava.sk:bratislava:{policy}"
        # The MHD stops and lines leave the catalogue while prod holds none of them (T-5788, owner 2026-10-26).
        if spec["catalog"].get("harvest") is False:
            assert "publish" not in spec
        else:
            assert spec["publish"]["ckan"]["instanceRef"] == {"kind": "CkanInstance", "name": "bratislava"}
        read = policies[policy]["spec"]
        assert read["assignee"] == {"kind": "role", "id": "public"}
        assert all(op.startswith(("retrieve", "query", "createSub", "updateSub", "deleteSub")) for op in read["operations"])
        assert "queryBatch" not in read["operations"]
        assert spec["catalog"]["pipelineRef"]["name"] in by_name("Pipeline")
    # The two TrafficFlowObserved datasets split the type by id, so neither reads the other's.
    patterns = [policies[p]["spec"]["information"][0]["entities"][0]["idPattern"] for p in ("public-cyklisti", "public-vozidla")]
    assert ":cycle-" in patterns[0] and ":vehicle-" in patterns[1]
    assert endpoints["bratislava-bajk"]["spec"]["catalog"]["license"] == "CC0", "nextbike's feed is CC0 1.0"


def test_every_pipeline_writes_through_the_door_with_a_type_the_write_policy_grants():
    write = granted_types(by_name("Policy")["pipelines-write"])
    for name, pipeline in by_name("Pipeline").items():
        spec = pipeline["spec"]
        assert spec["targetEndpoint"] == "urn:ngsi-ld:Endpoint:bratislava.sk:bratislava:bratislava", name
        assert spec["output"]["type"] in write and spec["output"]["mode"] == "upsert", name
        assert spec["source"]["dataSourceRef"]["name"] in by_name("DataSource"), name
