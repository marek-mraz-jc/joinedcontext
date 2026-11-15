"""Managing a CKAN instance is the organization administrator's right, handed out per instance
(T-3046, PF-107, PF-56).

Every `CkanInstance` the seed commits has its own `ckan-admin-{project}-{name}` Role, constrained
to that one instance and bound to nobody; the seeded `steward` and `approver`, which name every
project kind, no longer name `CkanInstance`. The seed converges on every apply, so this is also
the migration of an installation seeded before.
"""

from pathlib import Path

import yaml

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"
ORG = SEED / "helsinki"


def manifests():
    for path in sorted(SEED.glob("*/*.yaml")):
        if path.name == "index.yaml" or path.name.endswith(".linkml.yaml"):
            continue
        for doc in yaml.safe_load_all(path.read_text()):
            if isinstance(doc, dict) and "kind" in doc:
                yield path, doc


def instances():
    return [
        (doc["metadata"]["namespace"], doc["metadata"]["name"])
        for _, doc in manifests()
        if doc["kind"] == "CkanInstance"
    ]


def role(name):
    return yaml.safe_load((ORG / f"helsinki-role-{name}.yaml").read_text())


def test_the_seed_has_instances_to_hand_out():
    assert ("helsinki", "hel-fi") in instances()


def test_each_instance_has_its_own_role_constrained_to_it_alone():
    index = yaml.safe_load((ORG / "index.yaml").read_text())
    for project, name in instances():
        role_name = f"ckan-admin-{project}-{name}"
        assert index[f"helsinki-role-{role_name}.yaml"] == f"users/roles/{role_name}.yaml"
        manifest = role(role_name)
        assert manifest["metadata"] == {"name": role_name, "namespace": "org"}
        assert manifest["spec"]["rules"] == [
            {
                "kinds": ["CkanInstance"],
                "verbs": ["propose", "approve", "delete"],
                "constraints": [{"field": "metadata.name", "in": [name]}],
            }
        ], role_name


def test_no_ckan_admin_role_is_bound_by_the_seed():
    bound = [
        path.name
        for path, doc in manifests()
        if doc["kind"] == "RoleBinding" and doc["spec"]["role"].startswith("ckan-admin-")
    ]
    assert bound == []


def test_only_the_administrator_and_the_per_instance_roles_change_an_instance():
    """`steward` and `approver` name every project kind but this one; `janitor` keeps its test
    names and `org-admin` every kind."""
    for name in ("steward", "approver"):
        kinds = {kind for rule in role(name)["spec"]["rules"] for kind in rule["kinds"]}
        assert "Endpoint" in kinds, name
        assert "CkanInstance" not in kinds, name
    writers = {
        doc["metadata"]["name"]
        for _, doc in manifests()
        if doc["kind"] == "Role"
        and any(
            "CkanInstance" in rule["kinds"] and {"propose", "approve", "delete"} & set(rule["verbs"])
            for rule in doc["spec"]["rules"]
        )
    }
    expected = {f"ckan-admin-{project}-{name}" for project, name in instances()} | {"janitor"}
    assert writers - {"org-admin"} == expected
