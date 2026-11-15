"""T-4830: a seeded model's texts are help a person reads in the Portal's forms.

A title, description or comment of a class, slot or enum never cites a board task (`T-4727`) or
a page of the specification (`Development/10 §5`): neither means anything to the reader, and
both leaked into the Form tab of every Helsinki model. YAML comments (`#`) are for maintainers
and stay free.
"""

import re
from pathlib import Path

import yaml

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"
INTERNAL = re.compile(r"\bT-\d{4}\b|\b(?:Development|Architecture|Requirements|Deployment|Testing)/\d+\b")


def texts(node, where=""):
    if isinstance(node, dict):
        for key, value in node.items():
            yield from texts(value, f"{where}/{key}")
    elif isinstance(node, list):
        for index, value in enumerate(node):
            yield from texts(value, f"{where}[{index}]")
    elif isinstance(node, str):
        yield where, node


def test_seed_model_texts_cite_no_task_or_spec_page():
    found = [
        f"{path.relative_to(SEED)}{where}: {INTERNAL.search(text).group(0)}"
        for path in sorted(SEED.rglob("*.linkml.yaml"))
        for where, text in texts(yaml.safe_load(path.read_text()))
        if INTERNAL.search(text)
    ]
    assert not found, "\n".join(found)


def test_the_check_sees_a_task_id_and_a_spec_page():
    model = {"slots": {"x": {"description": "never a zero (Development/10 §5), see T-4727"}}}
    assert [INTERNAL.search(text).group(0) for _, text in texts(model)] == ["Development/10"]
    assert INTERNAL.findall("see T-4727") == ["T-4727"]
