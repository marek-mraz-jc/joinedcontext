"""The data workspace journey's own Alert grant (T-5092, ui/e2e/live/data-workspace.spec.ts).

The steward adds one Alert through the "Add row" form and deletes it again. The grant reaches
only rows a person marked `journey` and left without `source`; every Helsinki pipeline writes
Alerts with category `traffic` and a `source`, so no pipeline row is deletable through it.
"""

import json
import re
from pathlib import Path

import yaml
from conftest import demo_id

HELSINKI = Path(__file__).resolve().parent.parent / "components/context-gateway/seed/helsinki"
POLICY = HELSINKI / "helsinki-policy-steward-row-journey.yaml"


def test_the_journey_grant_is_seeded_for_the_steward_alone_and_only_creates_and_deletes_journey_alerts():
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    assert index[POLICY.name] == "projects/helsinki/spaces/helsinki/policies/steward-row-journey.yaml"
    spec = yaml.safe_load(POLICY.read_text())["spec"]
    assert spec["assignee"] == {"kind": "user", "id": demo_id("demo.steward")}
    assert spec["operations"] == ["createEntity", "deleteEntity"]
    assert spec["information"] == [{"entities": [{"type": "Alert"}]}]
    assert spec.get("effect", "permission") == "permission"
    assert "validity" not in spec
    assert spec["q"].split(";") == ['category=="journey"', "!source"]


def test_every_pipeline_alert_carries_traffic_and_a_source_so_the_journey_grant_reaches_none():
    writers = []
    for mapping in sorted(HELSINKI.glob("helsinki-pipeline-*-bento.yaml")):
        text = mapping.read_text()
        if not re.search(r'"type":\s*"Alert"', text):
            continue
        writers.append(mapping.name)
        categories = re.findall(r'"category":\s*(\{[^}]*\})', text)
        assert categories, f"{mapping.name} writes an Alert without a category"
        assert all(json.loads(c)["value"] == "traffic" for c in categories), mapping.name
        assert re.search(r'"source":', text), f"{mapping.name} writes an Alert without source"
    assert writers, "no Helsinki pipeline writes Alert: the grant's premise changed"


def test_the_history_journey_grant_reaches_only_the_journeys_own_stations():
    """T-5326 (ui/e2e/live/attribute-history.spec.ts): the steward creates, corrects and removes
    one `t5326-` station; a real station keeps its pipeline as its only writer."""
    policy = HELSINKI / "helsinki-policy-steward-history-journey.yaml"
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    assert index[policy.name] == "projects/helsinki/spaces/helsinki/policies/steward-history-journey.yaml"
    spec = yaml.safe_load(policy.read_text())["spec"]
    assert spec["assignee"] == {"kind": "user", "id": demo_id("demo.steward")}
    assert spec["operations"] == ["createEntity", "updateAttrs", "deleteEntity"]
    assert "validity" not in spec and "q" not in spec, "a condition would hide the Correct values tab"
    [entities] = [info["entities"] for info in spec["information"]]
    [selector] = entities
    assert selector["type"] == "AirQualityObserved"
    journey = re.compile(selector["idPattern"])
    assert journey.fullmatch("urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:t5326-2104")
    assert not journey.fullmatch("urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:kallio-2")
    assert not journey.fullmatch("urn:ngsi-ld:AirQualityObserved:hel.fi:other:t5326-2104")
    mapping = (HELSINKI / "helsinki-pipeline-air-quality-bento.yaml").read_text()
    assert "t5326" not in mapping, "a station of the pipeline would fall under the grant"


SEED = HELSINKI.parent
DEV_GITEA = HELSINKI.parents[3] / "ci/example-deployments/environments/dev/gitea.yaml.gotmpl"
PREVIEW_STEWARD = "168bb3a1-5b7a-4d2c-a812-8b5f4ebb2586"


def _seeded_policies():
    """Every Policy the seed and dev's bootstrap commit, with where it came from."""
    for path in sorted(SEED.glob("*/*.yaml")):
        for doc in yaml.safe_load_all(path.read_text()):
            if isinstance(doc, dict) and doc.get("kind") == "Policy":
                yield path.name, doc
    files = yaml.safe_load(DEV_GITEA.read_text())["gitea"]["bootstrap"]["rawValues"]["seed"]
    for key, text in files.items():
        doc = yaml.safe_load(text)
        if doc.get("kind") == "Policy":
            yield key, doc


def test_the_steward_reads_versions_of_their_space_and_the_viewer_never_does():
    """T-6348 (ADR-N-054 2.4, DM-114): the steward approves changes, so they read who changed a value
    and why (`versions=all`); the viewer and the public do not. `steward` never reaches a token, so
    the grant names the person; a role, group or DID assignee would grant it to nobody or to more."""
    spec = yaml.safe_load((HELSINKI / "helsinki-policy-steward-versions.yaml").read_text())["spec"]
    index = yaml.safe_load((HELSINKI / "index.yaml").read_text())
    assert index["helsinki-policy-steward-versions.yaml"] == "projects/helsinki/spaces/helsinki/policies/steward-versions.yaml"
    assert spec["contextSpaceRef"] == {"kind": "ContextSpace", "name": "helsinki"}
    assert spec["assignee"] == {"kind": "user", "id": demo_id("demo.steward")}
    assert spec["actions"] == ["readVersions"]
    assert set(spec["operations"]) == {"queryTemporal", "retrieveTemporal"}
    stewards = {demo_id("demo.steward"), PREVIEW_STEWARD}
    granted = {}
    for name, policy in _seeded_policies():
        if "readVersions" in policy["spec"].get("actions", []):
            granted[name] = policy["spec"]
    preview = {s["contextSpaceRef"]["name"] for n, s in granted.items() if n.startswith("projects__")}
    assert preview == {"helsinki", "banskabystrica-mesto", "praha-mesto"}, sorted(granted)
    for name, granted_spec in granted.items():
        assert granted_spec["assignee"]["kind"] == "user", name
        assert granted_spec["assignee"]["id"] in stewards, name
        assert granted_spec["assignee"]["id"] != demo_id("demo.viewer"), name
        assert granted_spec.get("effect", "permission") == "permission", name
