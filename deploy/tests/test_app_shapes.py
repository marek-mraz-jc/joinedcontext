"""Every App the seed and the forge seed carry names its shape by this release's names (AP-124, AP-152).

`static` and `fullstack` still read for one release, and the Portal's Applications page warns about
every App that uses them ("1 app still uses a shape name the next release refuses", T-3183): a
seed that writes the old name puts that warning on dev. The next release refuses them outright.
"""

from pathlib import Path

import yaml

COMPONENTS = Path(__file__).resolve().parent.parent / "components"
SHAPES = {"ui", "wasm", "ui-node", "ui-rust"}  # AP-152; never the retired `static`, `fullstack`


def apps():
    for path in sorted(COMPONENTS.rglob("*.yaml")):
        try:
            documents = list(yaml.safe_load_all(path.read_text()))
        except yaml.YAMLError:
            continue
        for document in documents:
            if isinstance(document, dict) and document.get("kind") == "App" and isinstance(document.get("spec"), dict):
                yield path.relative_to(COMPONENTS), document


def test_the_seed_carries_apps():
    assert len(list(apps())) >= 8


def test_every_app_names_its_shape_by_this_releases_name():
    wrong = [f"{path}: {app['metadata']['name']} is `{app['spec'].get('kind')}`" for path, app in apps() if app["spec"].get("kind") not in SHAPES]
    assert wrong == [], f"write `ui`, `wasm`, `ui-node` or `ui-rust` (AP-124, AP-152): {wrong}"
