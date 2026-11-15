"""T-2958: the indicator space and the hub of the Helsinki seed publish their models (DM-02, DM-22,
DM-26, DM-61, PF-54).

A draft model has no artifacts, so the gateway serves a schema derived from the grants that names
no attribute, and nobody reading `helsinki-kpi` or `helsinki-hub` can see what an entity is. Both
are published with the four artifacts; the indicator model is the regions' contract under the
`hel.fi` id, and the hub's slots are its members' own, so the hub cannot drift from the spaces it
registers.
"""

import json
from pathlib import Path

import pytest
import yaml


ROOT = Path(__file__).resolve().parents[1]
SEED = ROOT / "components/context-gateway/seed"
HELSINKI = SEED / "helsinki"


def load(path: Path) -> dict:
    return yaml.safe_load(path.read_text())


@pytest.mark.parametrize("model", ["helsinki-kpi", "helsinki-hub"])
def test_the_model_is_published_with_its_four_artifacts_in_the_seed_index(model):
    """DM-02, DM-26: a published model names its four artifacts, each in the seed and in its index."""
    manifest = load(HELSINKI / f"{model}-datamodel.yaml")["spec"]
    assert manifest["lifecycle"] == "published"
    assert manifest["version"].startswith("1.")
    index = load(HELSINKI / "index.yaml")
    folder = Path(index[f"{model}-datamodel.yaml"]).parent
    for artifact in manifest["artifacts"].values():
        name = artifact.removeprefix("./")
        assert (HELSINKI / name).is_file(), name
        assert Path(index[name]).parent == folder, name


def test_the_hubs_example_validates_against_its_schema():
    """DM-21: the example entity is one the hub's schema accepts (generated in keyValues form)."""
    jsonschema = pytest.importorskip("jsonschema")
    schema = json.loads((HELSINKI / "helsinki-hub.v1.schema.json").read_text())
    example = json.loads((HELSINKI / "helsinki-hub.v1.example.jsonld").read_text())
    example.pop("@context", None)
    validator = jsonschema.Draft7Validator(
        {**schema["definitions"][example["type"]], "definitions": schema["definitions"]}
    )
    assert [error.message for error in validator.iter_errors(example)] == []


def test_the_indicator_model_is_the_regions_contract():
    """PF-54: Helsinki's indicators are the same contract bbsk and Banská Bystrica publish."""
    own = load(HELSINKI / "helsinki-kpi.linkml.yaml")
    contract = load(SEED / "bbsk/key-performance-indicator.linkml.yaml")
    for key in ("classes", "slots", "prefixes", "imports"):
        assert own[key] == contract[key], key
    schema = json.loads((HELSINKI / "helsinki-kpi.v1.schema.json").read_text())
    reference = json.loads((SEED / "bbsk/key-performance-indicator.v1.schema.json").read_text())
    assert schema["definitions"] == reference["definitions"]


def test_the_hubs_classes_carry_their_members_slots_unchanged():
    """DM-61: the hub reads the terms of the spaces it registers, slot for slot."""
    hub = load(HELSINKI / "helsinki-hub.linkml.yaml")
    members = {
        "Vehicle": load(HELSINKI / "helsinki.linkml.yaml"),
        "KeyPerformanceIndicator": load(HELSINKI / "helsinki-kpi.linkml.yaml"),
    }
    entities = {name for name, body in hub["classes"].items() if body.get("is_a") == "Entity"}
    assert entities == set(members)
    # A value class (the shape of one slot's value, T-4011) is the indicator model's own.
    for name in set(hub["classes"]) - entities:
        assert hub["classes"][name] == members["KeyPerformanceIndicator"]["classes"][name], name
        for slot in hub["classes"][name].get("slots", []):
            assert hub["slots"][slot] == members["KeyPerformanceIndicator"]["slots"][slot], f"{name}.{slot}"
    for name, member in members.items():
        source = member["classes"][name]
        assert hub["classes"][name]["class_uri"] == source["class_uri"], name
        assert hub["classes"][name]["slots"] == source["slots"], name
        for slot in source["slots"]:
            assert hub["slots"][slot] == member["slots"][slot], f"{name}.{slot}"


KPI_MODELS = ["helsinki/helsinki-kpi", "zilina/key-performance-indicator", "bbsk/key-performance-indicator",
              "banskabystrica/key-performance-indicator"]


def key_values(entity: dict) -> dict:
    """An NGSI-LD normalized entity in keyValues form, as the model's schema describes it."""
    member = {"Relationship": "object", "LanguageProperty": "languageMap"}
    out = {}
    for name, attribute in entity.items():
        if isinstance(attribute, dict) and "type" in attribute and name != "@context":
            value = attribute.get(member.get(attribute["type"], "value"))
            out[name] = value.get("@value", value) if isinstance(value, dict) else value
        elif name != "@context":
            out[name] = attribute
    return out


@pytest.mark.parametrize("model", KPI_MODELS)
def test_an_indicator_is_a_number_or_not_measured_over_a_start_end_window(model):
    """T-4011: the gateway holds every write to the model's schema, so the schema says what the
    pipelines write: `currentValue` a number or `not measured` (Development/10 §5), and
    `calculationPeriod` an object of two instants. Both were `string` by default_range, and
    the gateway refused 866 Helsinki indicator writes in 30 min on dev."""
    jsonschema = pytest.importorskip("jsonschema")
    schema = json.loads((SEED / f"{model}.v1.schema.json").read_text())
    validator = jsonschema.Draft7Validator(
        {**schema["definitions"]["KeyPerformanceIndicator"], "definitions": schema["definitions"]}
    )
    example = key_values(json.loads((SEED / f"{model}.v1.example.jsonld").read_text()))
    assert isinstance(example["currentValue"], (int, float))
    assert [error.message for error in validator.iter_errors(example)] == []
    assert list(validator.iter_errors({**example, "currentValue": "not measured"})) == []
    window = example["calculationPeriod"]
    for wrong in ("2026-09-18T05:31:05Z/2026-09-18T05:46:05Z", {"start": window["start"]}):
        assert list(validator.iter_errors({**example, "calculationPeriod": wrong})), wrong


@pytest.mark.parametrize("model", KPI_MODELS + ["helsinki/helsinki-hub"])
def test_an_indicator_names_one_source_endpoint_or_the_list_of_them(model):
    """PL-104, T-6593: `derivedFrom` is the URN of the Endpoint the sources were read through, or
    the list of them when a KPI step reads through several, as jc-core's kpi step writes it. The
    Portal refused every KPI editor proposal (`records_failed`) while the schema took one string."""
    jsonschema = pytest.importorskip("jsonschema")
    schema = json.loads((SEED / f"{model}.v1.schema.json").read_text())
    slot = {**schema["definitions"]["KeyPerformanceIndicator"]["properties"]["derivedFrom"]}
    validator = jsonschema.Draft7Validator(slot)
    one = "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-transport"
    for good in (one, [one], [one, "urn:ngsi-ld:Endpoint:hel.fi:helsinki:helsinki-air-quality"]):
        assert list(validator.iter_errors(good)) == [], good
    # Each wrong value fails on its type, the constraint the gateway refuses a write for (DM-86).
    for wrong in (17, [], [17], {"object": one}):
        errors = list(validator.iter_errors(wrong))
        assert errors and {e.validator for e in errors} <= {"type", "minItems"}, wrong
