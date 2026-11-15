"""T-4244 (T-4159 #6): every seeded Blueprint reads in the page's language.

The Flow wizard shows a blueprint's title, description and parameter texts. JSON Schema keeps
`title` and `description` plain strings, so the other languages sit beside them in `x-jc-title`
and `x-jc-description` (docs Development/05-blueprints.md section 2). A text the form shows names
no request header and no requirement id: the builder's contract lives in the YAML comment.
"""

import re
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
SEEDS = sorted(
    p for p in (ROOT / "components").glob("*/seed/**/*.yaml") if yaml.safe_load(p.read_text()).get("kind") == "Blueprint"
)
# Proper names a translation would only copy.
PROPER = {"CC0 1.0", "CC BY 4.0", "CC BY-SA 4.0", "ODC Attribution", "ODC Open Database Licence", "ODC Public Domain Dedication"}
INTERNAL = re.compile(r"\bX-[A-Z][A-Za-z-]+|\b[A-Z]{2,4}-\d{1,3}\b")


def texts(node, at="spec.parameterSchema"):
    """Every (path, schema node with a title or description) of a parameter schema."""
    if isinstance(node, dict):
        if isinstance(node.get("title"), str) or isinstance(node.get("description"), str):
            yield at, node
        for key, value in node.items():
            if key not in ("title", "description", "x-jc-title", "x-jc-description", "default", "const", "enum"):
                yield from texts(value, f"{at}.{key}")
    elif isinstance(node, list):
        for i, value in enumerate(node):
            yield from texts(value, f"{at}[{i}]")


def test_the_seed_has_blueprints():
    assert len(SEEDS) >= 4, SEEDS


def test_every_blueprint_has_a_slovak_czech_and_german_title_and_description():
    for path in SEEDS:
        metadata = yaml.safe_load(path.read_text())["metadata"]
        for field in ("title", "description"):
            value = metadata.get(field)
            assert isinstance(value, dict), f"{path.name}: metadata.{field} is a map of languages"
            for lang in ("en", "sk", "cs", "de"):
                assert str(value.get(lang, "")).strip(), f"{path.name}: metadata.{field}.{lang}"


def test_every_parameter_text_has_its_slovak_czech_and_german_beside_it():
    for path in SEEDS:
        schema = yaml.safe_load(path.read_text())["spec"]["parameterSchema"]
        for at, node in texts(schema):
            for field in ("title", "description"):
                text = node.get(field)
                if not isinstance(text, str) or text in PROPER:
                    continue
                other = node.get(f"x-jc-{field}")
                assert isinstance(other, dict), f"{path.name}: {at}.{field} {text!r} has no x-jc-{field}"
                for lang in ("sk", "cs", "de"):
                    assert str(other.get(lang, "")).strip(), f"{path.name}: {at}.x-jc-{field}.{lang}"


def test_no_text_the_form_shows_names_a_header_or_a_requirement_id():
    for path in SEEDS:
        manifest = yaml.safe_load(path.read_text())
        shown = [str(v) for v in (manifest["metadata"].get("description") or {}).values()]
        for _, node in texts(manifest["spec"]["parameterSchema"]):
            for field in ("title", "description"):
                if isinstance(node.get(field), str):
                    shown.append(node[field])
                shown.extend(str(v) for v in (node.get(f"x-jc-{field}") or {}).values())
        leaked = [t for t in shown if INTERNAL.search(t)]
        assert not leaked, f"{path.name}: {leaked}"
