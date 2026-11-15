"""Every seed LinkML model passes linkml-lint's recommended rules without an error, and bbsk-registre also
without a canonical_prefixes warning: its `schema:` CURIEs name a declared prefix and none of its prefixes
stands for another namespace than the one the prefix registries give it (T-6747)."""

import shutil
from pathlib import Path

import pytest
from linkml.linter.linter import Linter

ROOT = Path(__file__).resolve().parents[1]
SEED = ROOT / "components/context-gateway/seed"
CORE = Path(__file__).resolve().parent / "fixtures/linkml/ngsi-ld-core.yaml"
MODELS = sorted(SEED.rglob("*.linkml.yaml"))


def problems(model: Path, tmp_path: Path) -> list:
    """linkml-lint's findings on `model`, its sibling models and ngsi-ld-core beside it as its imports."""
    for sibling in model.parent.glob("*.linkml.yaml"):
        shutil.copy(sibling, tmp_path / sibling.name)
    shutil.copy(CORE, tmp_path / "ngsi-ld-core.yaml")
    return list(Linter({"extends": "recommended"}).lint(str(tmp_path / model.name)))


def test_every_seed_model_is_found():
    assert len(MODELS) > 50, MODELS


@pytest.mark.parametrize("model", MODELS, ids=lambda m: str(m.relative_to(SEED)))
def test_a_seed_model_has_no_lint_error(model, tmp_path):
    errors = [f"{p.rule_name}: {p.message}" for p in problems(model, tmp_path) if str(p.level) == "error"]
    assert errors == []


def test_bbsk_registre_uses_the_canonical_prefixes(tmp_path):
    found = problems(SEED / "bbsk/bbsk-registre.linkml.yaml", tmp_path)
    assert [p.message for p in found if p.rule_name == "canonical_prefixes"] == []
    assert not [p for p in found if str(p.level) == "error"]
