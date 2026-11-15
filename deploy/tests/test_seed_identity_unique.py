"""No two seed files of one project declare the same resource.

The gitea bootstrap Job refuses a seed folder where two manifests resolve to one
kind/namespace/name, before it writes anything, and with it the whole apply stops (apply 85,
2026-10-16: two temporary Policies per project shared one name). This is the same rule, read the
way the Job reads it, so the clash fails here instead of on dev.
"""

from collections import defaultdict
from pathlib import Path

import yaml

SEED = Path(__file__).resolve().parents[1] / "components" / "context-gateway" / "seed"


def identities(folder: Path) -> dict[str, list[str]]:
    seen = defaultdict(list)
    for f in sorted(folder.iterdir()):
        if f.suffix not in (".yaml", ".yml") or f.name == "index.yaml":
            continue
        doc = yaml.safe_load(f.read_text())
        if not isinstance(doc, dict) or "kind" not in doc:
            continue
        meta = doc.get("metadata") or {}
        seen[f"{doc['kind']}/{meta.get('namespace') or 'org'}/{meta.get('name')}"].append(f.name)
    return seen


def test_no_two_seed_files_of_a_project_declare_one_resource():
    clashes = {
        f"{folder.name}: {identity}": files
        for folder in sorted(p for p in SEED.iterdir() if p.is_dir())
        for identity, files in identities(folder).items()
        if len(files) > 1
    }
    assert not clashes, clashes


def test_a_clash_is_found(tmp_path):
    for name in ("a.yaml", "b.yaml"):
        (tmp_path / name).write_text("kind: Policy\nmetadata: { name: same, namespace: p }\n")
    assert identities(tmp_path)["Policy/p/same"] == ["a.yaml", "b.yaml"]
