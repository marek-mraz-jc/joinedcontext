#!/usr/bin/env python3
"""Make a bundled OpenAPI JSON renderable by ReDoc: `dev/redoc-spec.py <in.json> <out.json>`.

The vendored ETSI document gives Entity, FeatureProperties and each of the eight
Attribute schemas (Property, Relationship, GeoProperty, ...) the same inline
`additionalProperties: {oneOf: [16 alternatives]}` block, each naming the
Attribute schemas again. ReDoc stops a cycle only when one $ref repeats on the
current path, so it expands every ordering of those eight schemas and the
static render runs out of heap at any size (4 GB on the runner, 6 GB tried).

This writes a render-only copy in which that block is one component,
`AnyAttribute`, referenced from every place that held it verbatim: the same JSON
Schema, but every cycle closes after one hop. The vendored file is never
touched (docs/openapi/README.md). A document without a `Property` schema whose
`additionalProperties` is an object (antares-admin.yaml) is copied unchanged.

`dev/redoc-spec.py --self-test` checks the hoist on a three-schema document.
"""

import json
import sys

NAME = "AnyAttribute"


def hoist(doc):
    """Replace every verbatim copy of Property's additionalProperties with a $ref.

    Returns how many copies it replaced; 0 leaves the document unchanged.
    """
    schemas = doc.get("components", {}).get("schemas", {})
    block = schemas.get("Property", {}).get("additionalProperties")
    if not isinstance(block, dict):
        return 0
    if NAME in schemas:
        sys.exit(
            f"redoc-spec: components.schemas.{NAME} exists already; pick another name"
        )
    block = json.loads(json.dumps(block))
    ref = {"$ref": f"#/components/schemas/{NAME}"}
    count = 0

    def walk(node):
        nonlocal count
        if isinstance(node, dict):
            if node.get("additionalProperties") == block:
                node["additionalProperties"] = dict(ref)
                count += 1
            for value in node.values():
                walk(value)
        elif isinstance(node, list):
            for value in node:
                walk(value)

    walk(doc)
    schemas[NAME] = {
        "description": "Any NGSI-LD Attribute, or an array of its instances (one per datasetId).",
        **block,
    }
    return count


def self_test():
    alternatives = {"oneOf": [{"$ref": "#/components/schemas/Property"}]}
    doc = {
        "components": {
            "schemas": {
                "Property": {
                    "type": "object",
                    "additionalProperties": json.loads(json.dumps(alternatives)),
                },
                "Entity": {
                    "type": "object",
                    "additionalProperties": json.loads(json.dumps(alternatives)),
                },
                "Other": {"type": "object", "additionalProperties": {"type": "string"}},
            }
        }
    }
    assert hoist(doc) == 2, doc
    schemas = doc["components"]["schemas"]
    assert schemas["Entity"]["additionalProperties"] == {
        "$ref": f"#/components/schemas/{NAME}"
    }
    assert schemas["Other"]["additionalProperties"] == {"type": "string"}
    assert schemas[NAME]["oneOf"] == alternatives["oneOf"]
    assert hoist({"components": {"schemas": {"Other": {}}}}) == 0
    print("redoc-spec: self-test passed")


def main(argv):
    if argv == ["--self-test"]:
        self_test()
        return
    if len(argv) != 2:
        sys.exit(__doc__.splitlines()[0])
    with open(argv[0], encoding="utf-8") as f:
        doc = json.load(f)
    count = hoist(doc)
    with open(argv[1], "w", encoding="utf-8") as f:
        json.dump(doc, f)
    print(f"redoc-spec: {argv[0]}: {count} attribute blocks now reference {NAME}")


if __name__ == "__main__":
    main(sys.argv[1:])
