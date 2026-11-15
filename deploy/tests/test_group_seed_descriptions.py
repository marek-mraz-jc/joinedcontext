"""T-4210 (UI-44, ui-writing rule 1): a seeded Group's description is shown to people on the
group's page, so it carries no requirement id; the ids belong in comments."""
import pathlib
import re

import yaml

ROOT = pathlib.Path(__file__).resolve().parent.parent
REQUIREMENT_ID = re.compile(r"\((?:[A-Z]{2,4}-N?-?\d+[,; ]*)+\)")


def seeded_groups():
    # Helm templates are not YAML until rendered; a seeded Group is a plain manifest.
    for path in sorted((ROOT / "components").rglob("*.yaml")):
        text = path.read_text()
        if "kind: Group" not in text or "{{" in text:
            continue
        for doc in yaml.safe_load_all(text):
            if isinstance(doc, dict) and doc.get("kind") == "Group":
                yield path, doc


def test_there_are_seeded_groups():
    assert len(list(seeded_groups())) > 10


def test_no_seeded_group_description_cites_a_requirement():
    cited = [
        str(path.relative_to(ROOT))
        for path, doc in seeded_groups()
        if REQUIREMENT_ID.search((doc.get("spec") or {}).get("description") or "")
    ]
    assert cited == []
