"""The golden object list prints one line per gitea seed release, so a seed edit that renames
or adds parts leaves the golden alone, while any other change still moves it (T-5450)."""

import importlib.util
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "list_objects", Path(__file__).resolve().parents[1] / "ci/golden/list-objects.py"
)
list_objects = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(list_objects)


def cm(name, ns="dev"):
    return {"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": name, "namespace": ns}}


BASE = [{"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": "gitea", "namespace": "dev"}}]


def test_seed_parts_renamed_or_added_give_the_same_golden():
    one = BASE + [cm("gitea-seed-s1-bbsk"), cm("gitea-seed-s1-slovensko--19"), cm("gitea-seed-s2-zilina")]
    two = BASE + [
        cm("gitea-seed-s1-bbsk--2"),
        cm("gitea-seed-s1-slovensko--18"),
        cm("gitea-seed-s1-slovensko--23"),
        cm("gitea-seed-s2-barcelona"),
        cm("gitea-seed-s2-zilina--3"),
    ]
    assert list_objects.lines(one) == list_objects.lines(two)
    assert "v1/ConfigMap dev/gitea-seed-s2-*" in list_objects.lines(one)


def test_a_removed_seed_release_changes_the_golden():
    one = BASE + [cm("gitea-seed-s1-bbsk"), cm("gitea-seed-s2-zilina")]
    assert list_objects.lines(one) != list_objects.lines(BASE + [cm("gitea-seed-s1-bbsk")])


def test_other_objects_are_not_collapsed():
    one = BASE + [cm("gitea-seed-s1-bbsk")]
    assert list_objects.lines(one) != list_objects.lines(one + [cm("gitea-config")])
    assert list_objects.lines(one) != list_objects.lines(one + [cm("gitea-seed-s1-bbsk", ns="other")])
    secret = {"apiVersion": "v1", "kind": "Secret", "metadata": {"name": "gitea-seed-s1-x", "namespace": "dev"}}
    assert "v1/Secret dev/gitea-seed-s1-x" in list_objects.lines(one + [secret])
    assert list_objects.lines(BASE + BASE) == sorted([BASE_LINE, BASE_LINE])


def test_helm_test_hooks_are_left_out():
    hook = cm("gitea-test")
    hook["metadata"]["annotations"] = {"helm.sh/hook": "pre-install,test"}
    assert list_objects.lines(BASE + [hook]) == [BASE_LINE]


BASE_LINE = "apps/v1/Deployment dev/gitea"
