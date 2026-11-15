"""T-3889: the Smart Data Models validator of the Helsinki types (tests/helsinki/validate_sdm.py),
offline: it groups failures by rule, names the member a required rule misses, and every class of
the Helsinki model is either held to an SDM schema or carries a written exception."""

import importlib.util
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("validate_sdm", ROOT / "tests/helsinki/validate_sdm.py")
sdm = importlib.util.module_from_spec(spec)
spec.loader.exec_module(sdm)

COMMON = "https://example.org/common-schema.json"
SCHEMAS = {
    COMMON: {"definitions": {"address": {"type": "object", "properties": {"streetAddress": {"type": "string"}}}}},
}
ALERT = {
    "$schema": "http://json-schema.org/draft-07/schema#",
    "required": ["id", "type", "alertSource"],
    "properties": {
        "address": {"$ref": COMMON + "#/definitions/address"},
        "subCategory": {"anyOf": [{"enum": ["roadWorks", "roadClosed"]}]},
    },
}


def test_failures_are_grouped_by_rule_and_a_missing_member_is_named():
    check = sdm.validator(ALERT, retrieve=SCHEMAS.__getitem__)
    entities = [
        {"id": "urn:ngsi-ld:Alert:1", "type": "Alert", "address": "Espoo", "subCategory": "ROAD_WORK"},
        {"id": "urn:ngsi-ld:Alert:2", "type": "Alert", "address": "Vantaa", "subCategory": "roadWorks"},
        {"id": "urn:ngsi-ld:Alert:3", "type": "Alert", "alertSource": "Fintraffic", "address": {"streetAddress": "x"}},
    ]
    found = sdm.failures(check, entities)
    assert found == {
        ("address", "the value is not of type 'object'"): 2,
        ("alertSource", "'alertSource' is a required property"): 2,
        ("subCategory", "the value is not valid under any of the given schemas"): 1,
    }


def test_a_valid_entity_has_no_failure():
    check = sdm.validator(ALERT, retrieve=SCHEMAS.__getitem__)
    assert not sdm.failures(check, [{"id": "a", "type": "Alert", "alertSource": "x", "subCategory": "roadClosed"}])


def test_every_helsinki_class_is_validated_or_excepted_and_every_exception_names_a_known_type():
    seed = ROOT / "components/context-gateway/seed/helsinki"
    classes = set()
    for name in ("helsinki.linkml.yaml", "helsinki-hub.linkml.yaml", "helsinki-kpi.linkml.yaml"):
        # An entity type is `is_a: Entity`; a value class such as PostalAddress (T-3949) is the
        # shape of one attribute and is validated inside the type that holds it.
        declared = yaml.safe_load((seed / name).read_text()).get("classes") or {}
        classes |= {cls for cls, body in declared.items() if (body or {}).get("is_a") == "Entity"}
    assert not set(sdm.MODELS) & set(sdm.EXCEPTIONS)
    assert classes <= set(sdm.MODELS) | set(sdm.EXCEPTIONS), classes - set(sdm.MODELS) - set(sdm.EXCEPTIONS)
    for kind, attribute in sdm.ACCEPTED:
        assert kind in sdm.MODELS, kind
    # A written reason, not a placeholder.
    assert all(len(reason) > 20 for reason in list(sdm.EXCEPTIONS.values()) + list(sdm.ACCEPTED.values()))
