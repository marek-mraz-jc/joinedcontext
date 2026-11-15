"""T-3185 (AP-04, AP-14, EP-77): the mobility project's city bike stations App.

`helsinki-mobility/bike-stations` reads the helsinki project's docking stations through the
project's SharedSpaceReference `city-bikes` and writes nothing. Its own Endpoint and Policy are the
ones the Portal's door would commit beside it: the project's members only, read operations only.
"""

from pathlib import Path

import yaml

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed/helsinki"
READS = {"queryEntity", "retrieveEntity"}


def one(kind: str, name: str, namespace: str) -> dict:
    for path in sorted(SEED.glob("*.yaml")):
        for doc in yaml.safe_load_all(path.read_text()):
            if (
                isinstance(doc, dict)
                and doc.get("kind") == kind
                and doc["metadata"]["name"] == name
                and doc["metadata"].get("namespace") == namespace
            ):
                return doc
    raise AssertionError(f"{kind} {namespace}/{name} is not seeded")


def test_the_app_is_a_published_ui_bundle_for_the_project_that_only_reads():
    app = one("App", "bike-stations", "helsinki-mobility")
    spec = app["spec"]
    assert spec["kind"] == "ui"
    assert spec["visibility"] == "project"
    assert spec["lifecycle"] == "published"
    assert app["metadata"]["annotations"]["joinedcontext.com/shipped-with"] == "portal"
    assert spec["source"]["path"] == "./apps/bike-stations"
    for need in spec["dataNeeds"]:
        assert set(need["operations"]) <= READS, need
        assert need["contextSpaceRef"]["name"] == "mobility"


def test_its_endpoint_and_policy_admit_the_project_alone_and_grant_reads_alone():
    endpoint = one("Endpoint", "app-bike-stations", "helsinki-mobility")["spec"]
    assert endpoint["audience"] == "project-list"
    assert endpoint["allowedProjects"] == ["helsinki-mobility"]
    assert endpoint["callerRole"] is True
    assert endpoint["contextSpaceRef"]["name"] == "mobility"
    policy = one("Policy", "app-bike-stations-1", "helsinki-mobility")["spec"]
    assert policy["assignee"] == {"kind": "role", "id": "endpoint:helsinki-mobility/app-bike-stations"}
    assert set(policy["operations"]) <= READS
    assert policy["assigner"] == "did:web:{orgDomain}"


def test_the_stations_come_through_the_shared_reference_to_a_public_endpoint():
    reference = one("SharedSpaceReference", "city-bikes", "helsinki-mobility")["spec"]
    assert reference["endpointRef"] == {"project": "helsinki", "name": "helsinki-bikes"}
    source = one("Endpoint", "helsinki-bikes", "helsinki")["spec"]
    # A reference never widens access (EP-77): the App reads what the public endpoint serves.
    assert source["audience"] == "public"
    assert source["contextSpaceRef"] == "helsinki"


def test_the_three_manifests_are_seeded_at_their_kinds_paths():
    index = yaml.safe_load((SEED / "index.yaml").read_text())
    assert index["helsinki-mobility-app-bike-stations.yaml"] == "projects/helsinki-mobility/apps/bike-stations/app.yaml"
    assert (
        index["helsinki-mobility-app-bike-stations-endpoint.yaml"]
        == "projects/helsinki-mobility/spaces/mobility/endpoints/app-bike-stations.yaml"
    )
    assert (
        index["helsinki-mobility-app-bike-stations-policy.yaml"]
        == "projects/helsinki-mobility/spaces/mobility/policies/app-bike-stations-1.yaml"
    )
