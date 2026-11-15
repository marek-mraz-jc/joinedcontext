"""T-6536: stage and prod (smartcity) carry every ContextSpace, DataSource, Pipeline and Endpoint
the dev seed carries, each with dev's text once the instance's own domain is put back.

An object only dev renders passes only with an entry in DEV_ONLY naming the gate that keeps it off
and why it must stay there. An entry for an object stage or prod now carries fails too, so the
list cannot outlive its reason. evidence/T-6536/parity.md holds the counts and the reasoning.
"""

import json

import pytest
from conftest import load_yaml_all

KINDS = {"ContextSpace", "DataSource", "Pipeline", "Endpoint"}
DOMAINS = {"dev": "dev.joinedcontext.com", "stage": "stage.joinedcontext.com",
           "smartcity": "smartcity.joinedcontext.com"}

# Seed filters of components/gitea/values/seed/development-values.yaml.gotmpl.
TEST_OBJECT = "test object: the production profile leaves the seed's test objects out (T-4678, F-BNT1-002)"
SHARING = "the App sharing matrix's test project, seeded under the development profile only (T-4911)"
DEMO_FEEDS = ("reads the dev-only demo-feeds component (its README: never deployed to production); "
              "stage and prod get HSL's trams from the helsinki hfp source itself")
DEV_ONLY = {
    "projects/banskabystrica/spaces/conformance/space.yaml": TEST_OBJECT,
    "projects/banskabystrica/spaces/conformance/endpoints/conformance.yaml": TEST_OBJECT,
    "projects/sharing/spaces/sharing-bikes/space.yaml": SHARING,
    "projects/helsinki/datasources/demo-feed-counters.yaml": DEMO_FEEDS,
    "projects/helsinki/pipelines/demo-counters/pipeline.yaml": DEMO_FEEDS,
    "projects/helsinki/datasources/mqtt-hsl-bridge.yaml": DEMO_FEEDS,
    "projects/helsinki/pipelines/mqtt-hsl-bridge/pipeline.yaml": DEMO_FEEDS,
    "projects/helsinki/spaces/hsl-mqtt-test/space.yaml": DEMO_FEEDS,
    "projects/helsinki/spaces/hsl-mqtt-test/endpoints/hsl-mqtt-test.yaml": DEMO_FEEDS,
}


def seeded(docs, env: str) -> dict[str, str]:
    """Forge path -> manifest text with the domain neutral, for each object of KINDS the
    environment's gitea seed ConfigMaps write."""
    out = {}
    for d in docs:
        if d.get("kind") != "ConfigMap" or not d["metadata"]["name"].startswith("gitea-seed-"):
            continue
        for key, text in (d.get("data") or {}).items():
            if not key.endswith(".yaml"):
                continue
            for m in load_yaml_all(text):
                if isinstance(m, dict) and m.get("kind") in KINDS:
                    out[key.replace("__", "/")] = json.dumps(m, sort_keys=True).replace(DOMAINS[env], "{domain}")
    return out


@pytest.fixture(scope="module")
def dev(rendered):
    objects = seeded(rendered("dev"), "dev")
    assert len(objects) > 1000, "the dev seed rendered almost nothing"
    return objects


@pytest.mark.parametrize("env", ["stage", "smartcity"])
def test_every_dev_seed_object_is_on_the_environment_or_named_why_not(rendered, dev, env):
    there = seeded(rendered(env), env)
    missing = sorted(p for p in dev if p not in there and p not in DEV_ONLY)
    assert not missing, f"{env} lacks these dev seed objects; render them or add a DEV_ONLY reason: {missing}"
    stale = sorted(p for p in DEV_ONLY if p in there or p not in dev)
    assert not stale, f"DEV_ONLY entries no longer dev-only on {env}; delete them: {stale}"
    differs = sorted(p for p in dev.keys() & there.keys() if dev[p] != there[p])
    assert not differs, f"these objects render differently on {env} than on dev: {differs}"

