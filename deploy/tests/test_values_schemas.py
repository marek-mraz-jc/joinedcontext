"""T-4956: every local chart validates its values, every component documents itself, and the
hosted environments share their identical value files through one layer.

Helm checks values.schema.json on every `helm template`, `helmfile template` and `sync`, so a
typo in a values key fails the render in CI instead of rendering silently.
"""

import json
import re
import subprocess
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
# Our charts, from Git: a chart vendored under another chart's charts/ is upstream's.
CHARTS = sorted(
    (ROOT / p).parent
    for p in subprocess.run(["git", "ls-files", "*Chart.yaml"], cwd=ROOT, capture_output=True, text=True, check=True).stdout.split()
    if Path(p).parts.count("charts") <= 1
)
ENVIRONMENTS = ROOT / "ci/example-deployments/environments"
LAYERS = ENVIRONMENTS / "_layers"


def test_the_local_charts_are_found():
    assert len(CHARTS) >= 29, CHARTS


@pytest.mark.parametrize("chart", CHARTS, ids=lambda c: str(c.relative_to(ROOT)))
def test_each_local_chart_has_a_strict_values_schema(chart):
    schema = json.loads((chart / "values.schema.json").read_text())
    assert schema["type"] == "object" and schema["additionalProperties"] is False
    defaults = yaml.safe_load((chart / "values.yaml").read_text()) or {}
    assert set(defaults) <= set(schema["properties"]), "a default key the schema does not declare"


@pytest.mark.parametrize("chart", CHARTS, ids=lambda c: str(c.relative_to(ROOT)))
def test_a_values_typo_fails_the_render(chart):
    lint = chart / "ci/lint-values.yaml"
    values = ["-f", str(lint)] if lint.is_file() else []
    ok = subprocess.run(["helm", "template", "t", str(chart), *values], capture_output=True, text=True, timeout=120, check=False)
    assert "schema" not in ok.stderr, ok.stderr  # a chart may still `fail` on a required value
    typo = subprocess.run(["helm", "template", "t", str(chart), *values, "--set", "replicaCuont=1"],
                          capture_output=True, text=True, timeout=120, check=False)
    assert typo.returncode != 0
    assert "replicaCuont" in typo.stderr and "additional properties" in typo.stderr, typo.stderr


def test_every_component_has_a_readme():
    missing = [c.name for c in sorted((ROOT / "components").iterdir()) if c.is_dir() and not (c / "README.md").is_file()]
    assert missing == []


def named_layers(env: Path) -> list[str]:
    text = (env / "global.yaml.gotmpl").read_text()
    block = re.search(r"^environmentLayers:\n((?:  - .+\n)+)", text, re.MULTILINE)
    return re.findall(r"  - (\S+)", block[1]) if block else []


def test_the_hosted_clusters_name_the_hosted_layer_and_every_named_layer_exists():
    assert named_layers(ENVIRONMENTS / "dev") == ["hosted"]
    assert named_layers(ENVIRONMENTS / "smartcity") == ["hosted"]
    for env in ENVIRONMENTS.iterdir():
        # secrets/ holds the sops files environments seed (encryptedSeedFiles, T-6982), no environment.
        if env.is_dir() and env not in (LAYERS, ENVIRONMENTS / "secrets"):
            for layer in named_layers(env):
                assert list((LAYERS / layer).glob("*.yaml.gotmpl")), f"{env.name} names layer {layer!r}, which holds no file"


def test_no_environment_file_repeats_a_layer_file():
    # A copy of a layer file in an environment is the drift this layer exists to stop.
    for layer_file in LAYERS.glob("*/*.yaml.gotmpl"):
        for env in ENVIRONMENTS.iterdir():
            copy = env / layer_file.name
            if env != LAYERS and copy.is_file():
                assert copy.read_bytes() != layer_file.read_bytes(), f"{copy.relative_to(ROOT)} repeats {layer_file.relative_to(ROOT)}"
