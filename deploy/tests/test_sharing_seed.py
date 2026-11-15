"""T-4911: the App sharing matrix's test project, seeded on dev and nowhere else.

Five copies of the sharing-probe bundle, one per visibility, in the project `sharing`; the test
people's bindings and the roles App's default group; the people themselves in demo-users.yaml,
out of `platform-readers`. The conformance journey 39 opens the Apps as those people.
"""

from pathlib import Path

import pytest
import yaml
from conftest import seed_configmap

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "components/context-gateway/seed/sharing"
PEOPLE = yaml.safe_load((ROOT / "components/keycloak/demo-users.yaml").read_text())
VISIBILITIES = ["private", "project", "organization", "roles", "public"]


def docs():
    for path in sorted(SEED.glob("*.yaml")):
        if path.name == "index.yaml" or path.name.endswith(".linkml.yaml"):
            continue
        yield path, yaml.safe_load(path.read_text())


def test_the_five_apps_differ_only_in_visibility_and_the_roles_block():
    apps = {doc["metadata"]["name"]: doc for _, doc in docs() if doc["kind"] == "App"}
    assert sorted(apps) == sorted(f"sharing-{v}" for v in VISIBILITIES)
    shapes = set()
    for visibility in VISIBILITIES:
        app = apps[f"sharing-{visibility}"]
        spec = dict(app["spec"])
        assert spec.pop("visibility") == visibility
        assert spec.pop("source") == {"path": f"./apps/sharing-{visibility}"}
        assert app["metadata"]["namespace"] == "sharing"
        # The bundle is the Portal image's (portal Dockerfile, apps/sharing-probe).
        annotations = dict(app["metadata"]["annotations"])
        if visibility == "private":
            annotations.pop("joinedcontext.com/owner")
        assert annotations == {"joinedcontext.com/shipped-with": "portal"}
        if visibility == "roles":
            assert spec.pop("access") == [{"role": "viewer", "subjects": [{"group": "sharing-roles-viewer"}]}]
            assert [role["name"] for role in spec.pop("roles")] == ["viewer"]
        shapes.add(yaml.safe_dump(spec, sort_keys=True))
    assert len(shapes) == 1, "the five copies read the same data the same way"
    (need,) = apps["sharing-public"]["spec"]["dataNeeds"]
    assert need["operations"] == ["queryEntity", "retrieveEntity"], "the probe only reads"


def test_every_private_app_names_its_owner_by_keycloak_id():
    """F-BNT3-009: the Portal opens a private App without an owner to the project's administrators
    alone, so each seeded one names its owner: demo.author's fixed id (T-5712)."""
    owners = {
        doc["metadata"]["name"]: doc["metadata"]["annotations"].get("joinedcontext.com/owner")
        for _, doc in docs()
        if doc["kind"] == "App" and doc["spec"]["visibility"] == "private"
    }
    assert owners == {"sharing-private": PEOPLE["demo.author"]["id"]}


def test_the_space_reads_only_the_public_city_bikes():
    (reference,) = [doc for _, doc in docs() if doc["kind"] == "SharedSpaceReference"]
    assert reference["spec"]["endpointRef"] == {"project": "helsinki", "name": "helsinki-bikes"}
    assert not [doc for _, doc in docs() if doc["kind"] in {"Pipeline", "DataSource", "Endpoint", "Policy"}]


def test_each_test_person_holds_exactly_their_one_kind_of_access():
    bindings = {doc["metadata"]["name"]: doc["spec"] for _, doc in docs() if doc["kind"] == "RoleBinding"}
    held = {}
    for spec in bindings.values():
        for subject in spec["subjects"]:
            held.setdefault(subject["user"], set()).add((spec["role"], spec["scope"]["project"]))
    assert held == {
        PEOPLE["demo.member"]["id"]: {("viewer", "sharing")},
        PEOPLE["demo.author"]["id"]: {("viewer", "sharing"), ("app-editor", "sharing")},
        PEOPLE["demo.neighbour"]["id"]: {("viewer", "sharing-other")},
    }
    (group,) = [doc for _, doc in docs() if doc["kind"] == "Group"]
    assert group["spec"]["members"] == [{"user": PEOPLE["demo.bearer"]["id"]}]
    for name in ["outsider", "neighbour", "member", "author", "bearer"]:
        person = PEOPLE[f"demo.{name}"]
        assert person["platformReaders"] is False, name
        assert person["realmRoles"] == [], name
        assert set(person["groups"]) <= {"sharing", "sharing-other"}, name


def test_the_index_lists_every_file_once():
    index = yaml.safe_load((SEED / "index.yaml").read_text())
    files = {p.name for p in SEED.glob("*.yaml") if p.name != "index.yaml"}
    assert set(index) == files
    assert len(set(index.values())) == len(index)


@pytest.mark.parametrize("env, seeded", [("dev", True), ("smartcity", False)])
def test_only_the_development_profile_seeds_it(rendered, env, seeded):
    data = seed_configmap(rendered(env))["data"]
    assert ("projects__sharing__apps__sharing-private__app.yaml" in data) is seeded
    assert ("users__assignments__sharing-members.yaml" in data) is seeded
