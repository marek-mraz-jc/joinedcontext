"""T-4678: scripts/parity.py names every difference between dev and prod, and only dev's test
objects, objects left out of prod's seed or made at runtime, and live drift pass."""

import copy
import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("parity", ROOT / "scripts/parity.py")
parity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(parity)

ENDPOINT = "joinedcontext/helsinki.git:spaces/helsinki/endpoints/helsinki-bikes.yaml"
EVAL = "joinedcontext/bbsk.git:assistant/deployments/data-eval-gemini.yaml"
APP = "joinedcontext/helsinki.git:apps/air-quality/app.yaml"
SNAPSHOT = {
    "objects": {ENDPOINT: "a1", APP: "b2", EVAL: "c3"},
    "apps": ["helsinki_air-quality.git"],
    "ckan": {"datasets": ["hel-bikes"], "organizations": ["hel-fi"], "groups": [], "resources": 400,
             "models": {"hel-bikes": True}},
    "entities": {"helsinki|BikeHireDockingStation": 1000, "helsinki|Vehicle": 30},
    "pipelines": ["helsinki/bikes"],
}


def prod_of(dev):
    """What prod holds when it is right: dev without its test objects."""
    prod = copy.deepcopy(dev)
    del prod["objects"][EVAL]
    return prod


def test_equal_instances_pass_and_dev_s_test_objects_are_listed_as_allowed():
    fail, allowed = parity.compare(SNAPSHOT, prod_of(SNAPSHOT))
    assert fail == [] and allowed == [f"test object, dev only: {EVAL}"]


def test_a_missing_endpoint_fails_by_name():
    prod = prod_of(SNAPSHOT)
    del prod["objects"][ENDPOINT]
    fail, _ = parity.compare(SNAPSHOT, prod)
    assert fail == [f"missing on prod: {ENDPOINT}"]


def test_a_changed_setting_fails_as_differs():
    prod = prod_of(SNAPSHOT)
    prod["objects"][ENDPOINT] = "zz"
    assert parity.compare(SNAPSHOT, prod)[0] == [f"differs: {ENDPOINT}"]


def test_live_counts_pass_within_tolerance_and_fail_outside():
    prod = prod_of(SNAPSHOT)
    prod["entities"]["helsinki|BikeHireDockingStation"] = 905  # within 10 %
    prod["entities"]["helsinki|Vehicle"] = 75  # within 50
    assert parity.compare(SNAPSHOT, prod)[0] == []
    prod["entities"]["helsinki|BikeHireDockingStation"] = 850
    assert parity.compare(SNAPSHOT, prod)[0] == ["entities helsinki|BikeHireDockingStation: dev 1000, prod 850"]


def test_a_test_object_on_prod_fails():
    fail, _ = parity.compare(SNAPSHOT, copy.deepcopy(SNAPSHOT))
    assert fail == [f"test object on prod: {EVAL}"]


def test_the_seeds_tell_a_left_out_or_runtime_object_from_a_missing_one():
    made = "joinedcontext/helsinki.git:sync/t1553-1944.yaml"
    feed = "joinedcontext/helsinki.git:pipelines/demo-counters/pipeline.yaml"
    dev = copy.deepcopy(SNAPSHOT)
    dev["objects"].update({made: "d4", feed: "e5"})
    dev["pipelines"].append("helsinki/demo-counters")
    dev["apps"].append("helsinki_bikes-full-1454.git")
    seeded = {"dev": [ENDPOINT, APP, EVAL, feed], "smartcity": [ENDPOINT, APP]}
    fail, allowed = parity.compare(dev, prod_of(SNAPSHOT), seeded)
    assert fail == []
    assert f"made on dev: {made}" in allowed and f"left out of prod's seed: {feed}" in allowed
    assert "App without its manifest on prod: helsinki_bikes-full-1454.git" in allowed
    assert "pipeline reporting without its manifest on prod: helsinki/demo-counters" in allowed
    prod = prod_of(SNAPSHOT)
    del prod["objects"][APP]
    assert parity.compare(SNAPSHOT, prod, seeded)[0] == [f"missing on prod: {APP}"]


def test_normalize_undoes_the_seed_rewrites_and_what_each_forge_writes_for_itself():
    dev = """url: https://data.dev.joinedcontext.com/x
api: https://api.dev.joinedcontext.com
owner: demo.steward@joinedcontext.com
- user: demo.analyst@joinedcontext.com
source:
  ref: 785e0d4a9ada113f32e076b11f2bcb6392bdb8d4
status:
  phase: Pending
"""
    prod = """url: https://opendata.smartcity.joinedcontext.com/x
api: https://api.smartcity.joinedcontext.com
owner: owner@joinedcontext.com
source:
  ref: 9a731442bca8b2cb185caefe57800da2a1ceb22e
"""
    assert parity.normalize(dev, "dev.joinedcontext.com") == parity.normalize(prod, "smartcity.joinedcontext.com")
    assert parity.normalize(dev, "dev.joinedcontext.com") != parity.normalize(
        prod.replace("api.smartcity", "smartcity"), "smartcity.joinedcontext.com")


def test_seed_keys_map_to_forge_paths():
    assert parity.seed_path("gitea-seed-s5-helsinki", "projects__helsinki__pipelines__bikes__pipeline.yaml") == \
        "joinedcontext/helsinki.git:pipelines/bikes/pipeline.yaml"
    assert parity.seed_path("gitea-bootstrap-config", "users__assignments__admins.yaml") == \
        "joinedcontext/configuration.git:users/assignments/admins.yaml"
    assert parity.seed_path("gitea-sample-apps-helsinki", "src__main.ts") is None


def test_a_data_model_served_on_dev_and_not_on_prod_fails_by_dataset():
    prod = prod_of(SNAPSHOT)
    prod["ckan"]["models"]["hel-bikes"] = False
    assert parity.compare(SNAPSHOT, prod)[0] == ["data model not served on prod: hel-bikes"]
    dev = copy.deepcopy(SNAPSHOT)
    dev["ckan"]["models"]["hel-bikes"] = False
    assert parity.compare(dev, prod_of(dev))[0] == []


def test_the_endpoint_is_read_from_a_resource_and_an_empty_index_is_no_model():
    dataset = {"resources": [{"url": "https://opendata.x/download.csv"},
                             {"url": "https://api.x/api/endpoint/scsd2eeh42/ngsi-ld/v1/entities?type=A"}]}
    assert parity.endpoint_of(dataset) == "scsd2eeh42"
    assert parity.endpoint_of({"resources": [{"url": None}]}) is None
    assert parity.served({"endpoint": "x", "models": []}) is False
    assert parity.served({"models": [{"name": "helsinki"}]}) is True
