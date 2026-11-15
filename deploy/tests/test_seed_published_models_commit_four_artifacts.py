"""Every published seed DataModel commits its four generated artifacts (DM-02, T-4425).

The Portal refuses a published model whose `spec.artifacts` lacks one of `jsonSchema`,
`context`, `docs` and `example` ("a published model commits all four generated artifacts"),
so the seed checks it before the forge does, and that each named file sits beside the model,
at any depth of the seed (the ŠÚ SR models sit one folder deeper, T-4434).
"""

from pathlib import Path

import pytest
from conftest import seed_documents

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"
FOUR = ("jsonSchema", "context", "docs", "example")


def published():
    for path, doc in seed_documents():
        if path.name.endswith(("-bento.yaml", ".linkml.yaml")) or path.name == "index.yaml":
            continue
        if isinstance(doc, dict) and doc.get("kind") == "DataModel" and doc["spec"].get("lifecycle") == "published":
            yield pytest.param(path, doc["spec"], id=str(path.relative_to(SEED)))


@pytest.mark.parametrize(("path", "spec"), list(published()))
def test_a_published_model_lists_and_commits_all_four_artifacts(path, spec):
    artifacts = spec.get("artifacts") or {}
    assert [k for k in FOUR if not artifacts.get(k)] == [], f"{path.name}: spec.artifacts lacks these"
    missing = [artifacts[k] for k in FOUR if not (path.parent / artifacts[k]).is_file()]
    assert missing == [], f"{path.name}: named but not beside it"


def test_there_are_published_models_to_check():
    assert len(list(published())) >= 46
