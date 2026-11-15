"""Every seed space names exactly one data model (T-2700, DM-61, ADR-N-033).

A space's model decides which types the gateway lets into it, so each seed space names its one
model in `spec.dataModelRef`, that model belongs to the same project and space, and no space
holds a second one. The forge seed writes each file where `index.yaml` says, so a model the
index leaves out would reach the gateway's flat checkout and never the forge.
"""

from pathlib import Path

import pytest
import yaml

SEED = Path(__file__).resolve().parent.parent / "components/context-gateway/seed"
FOLDERS = sorted(path for path in SEED.iterdir() if path.is_dir())


def manifests(folder: Path):
    for path in sorted(folder.glob("*.yaml")):
        if path.name.endswith(("-bento.yaml", ".linkml.yaml")) or path.name == "index.yaml":
            continue
        for doc in yaml.safe_load_all(path.read_text()):
            if isinstance(doc, dict) and "kind" in doc:
                yield path, doc


def spaces_and_models():
    spaces, models = {}, {}
    for folder in FOLDERS:
        for path, doc in manifests(folder):
            meta = doc["metadata"]
            key = (meta.get("namespace"), meta["name"])
            if doc["kind"] == "ContextSpace":
                spaces[key] = (path, doc["spec"])
            elif doc["kind"] == "DataModel" and doc["spec"].get("lifecycle") != "mirrored":
                models.setdefault(
                    (meta.get("namespace"), doc["spec"]["contextSpaceRef"]), []
                ).append((path, meta["name"], doc["spec"]))
    return spaces, models


SPACES, MODELS = spaces_and_models()


def test_the_seed_has_spaces_to_check():
    assert len(SPACES) >= 9, sorted(SPACES)


@pytest.mark.parametrize("key", sorted(SPACES), ids=lambda key: f"{key[0]}/{key[1]}")
def test_every_space_names_exactly_its_one_model(key):
    path, spec = SPACES[key]
    named = spec.get("dataModelRef")
    assert named, f"{path.name}: the space names no data model (DM-61)"
    assert named.get("kind") == "DataModel" and "namespace" not in named, (
        f"{path.name}: a space names a model of its own project (DM-61)"
    )
    held = MODELS.get(key, [])
    assert len(held) == 1, f"{path.name}: the space holds {[name for _, name, _ in held]} (DM-61)"
    assert held[0][1] == named["name"], f"{path.name}: names {named['name']}, holds {held[0][1]}"


@pytest.mark.parametrize(
    "held", [held[0] for held in MODELS.values()], ids=lambda held: held[1]
)
def test_every_model_source_sits_beside_its_manifest_and_in_the_index(held):
    path, name, spec = held
    source = path.parent / spec["linkml"]
    assert source.is_file(), f"{path.name}: {spec['linkml']} is not beside the manifest"
    parsed = yaml.safe_load(source.read_text())
    assert parsed["name"] == name, f"{source.name} is the source of another model"
    declared = set((parsed.get("classes") or {}).keys())
    assert set(spec.get("classes") or []) <= declared, (
        f"{path.name}: classes the source does not define: "
        f"{set(spec.get('classes') or []) - declared}"
    )
    index = yaml.safe_load((path.parent / "index.yaml").read_text())
    assert path.name in index and source.name in index, f"{path.parent.name}/index.yaml"
    folder = index[path.name].rsplit("/", 1)[0]
    assert index[source.name] == f"{folder}/{source.name}", (
        "the forge layout keeps the source beside its manifest"
    )
