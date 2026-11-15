"""A seeded blueprint's choices read as words (UI-16; T-2756).

The flow wizard renders a blueprint's parameter schema as a form. A plain `enum` shows the value
itself ("static", "30m"), so every choice is `oneOf` consts, each with a title.
"""

from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent / "components"


def blueprints():
    for path in sorted(ROOT.glob("**/seed/**/*.yaml")):
        for doc in yaml.safe_load_all(path.read_text()):
            if isinstance(doc, dict) and doc.get("kind") == "Blueprint":
                yield path, doc


def choices(node, at, found):
    if isinstance(node, dict):
        if isinstance(node.get("enum"), list) and all(isinstance(v, str) for v in node["enum"]):
            found.append(f"{at}: plain enum {node['enum']}")
        for option in node.get("oneOf") or []:
            if isinstance(option, dict) and "const" in option and not str(option.get("title", "")).strip():
                found.append(f"{at}: option {option['const']!r} has no title")
        for key, value in node.items():
            choices(value, f"{at}.{key}", found)
    elif isinstance(node, list):
        for item in node:
            choices(item, at, found)
    return found


def test_there_are_seeded_blueprints_to_check():
    assert len(list(blueprints())) >= 3


def test_every_choice_of_a_seeded_blueprint_is_titled():
    found = []
    for path, doc in blueprints():
        choices(doc["spec"].get("parameterSchema", {}), f"{path.relative_to(ROOT)}", found)
    assert found == []


# The widgets the Portal draws (ui/src/components/forms/widgets/index.ts, plus rjsf's own
# `textarea`); an unknown name silently falls back to a text box (Development/05 §2.1, T-2757).
WIDGETS = {"entityPicker", "operations", "resourcePicker", "secretRef", "textarea"}


def test_every_widget_a_seeded_blueprint_names_is_drawn():
    named = []
    for path, doc in blueprints():
        for name, prop in doc["spec"].get("parameterSchema", {}).get("properties", {}).items():
            widget = prop.get("x-jc-widget")
            if widget is not None and widget not in WIDGETS:
                named.append(f"{path.name}: {name} names {widget!r}")
    assert named == []


def test_the_app_builder_picks_its_endpoint_and_space_and_takes_prose():
    # T-2757: the endpoint was a text box to type a name into from memory, the prompt one line.
    (doc,) = [doc for _, doc in blueprints() if doc["metadata"]["name"] == "app-from-prompt"]
    props = doc["spec"]["parameterSchema"]["properties"]
    assert props["prompt"]["x-jc-widget"] == "textarea"
    assert props["endpoint"]["x-jc-widget"] == "resourcePicker"
    assert props["endpoint"]["x-jc-options"] == {"plural": "endpoints"}
    assert props["contextSpace"]["x-jc-widget"] == "resourcePicker"
    assert props["contextSpace"]["x-jc-options"] == {"plural": "spaces"}


def test_the_json_feed_pipeline_writes_the_type_its_card_names():
    """`entityType` reached only the title: the Pipeline had no output type, so the card did not write
    "entities of one type" as it says (F-BNT2-012)."""
    import jinja2

    path = ROOT / "context-gateway/seed/helsinki/helsinki-blueprint-http-json-feed.yaml"
    spec = yaml.safe_load(path.read_text())["spec"]
    defaults = {k: v.get("default") for k, v in spec["parameterSchema"]["properties"].items()}
    parameters = {**defaults, "name": "bus-stops", "url": "https://example.org/stops.json", "entityType": "GtfsStop", "period": "30m"}
    template = next(t["template"] for t in spec["templates"] if t["name"] == "pipeline")
    pipeline = yaml.safe_load(jinja2.Template(template).render(**parameters))
    assert pipeline["spec"]["output"] == {"type": "GtfsStop", "mode": "upsert"}
