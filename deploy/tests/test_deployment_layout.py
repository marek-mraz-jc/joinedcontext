"""The layout OPS-01 and OPS-02 fix: one directory per component, described by a validated
`component.yaml` with its pinned charts and images, and an environment's own values only in the
operator's `deployment/` checkout, layered over defaults nobody edits for one environment."""

import json
import re
import subprocess
from pathlib import Path

import jsonschema
import yaml

ROOT = Path(__file__).resolve().parent.parent
COMPONENTS = sorted(p for p in (ROOT / "components").iterdir() if p.is_dir())

# The example environments that are an operator's own, as opposed to the `development` and
# `production` value profiles every component ships (`values/*/production-values.yaml.gotmpl`).
OPERATOR_ONLY = {"dev", "staging", "smoke-test", "recovery-test"}


def tracked(prefix: str) -> list[str]:
    out = subprocess.run(
        ["git", "ls-files", prefix], cwd=ROOT, capture_output=True, text=True, check=True
    )
    return [line for line in out.stdout.splitlines() if line]


def test_every_component_yaml_validates_against_the_schema():
    """OPS-01: `components/<c>/component.yaml` is validated against `component.schema.json`."""
    schema = json.loads((ROOT / "component.schema.json").read_text())
    validator = jsonschema.Draft7Validator(schema)
    problems = []
    for component in COMPONENTS:
        manifest = component / "component.yaml"
        assert manifest.is_file(), f"{component.name} has no component.yaml"
        document = yaml.safe_load(manifest.read_text())
        problems += [
            f"{component.name}: {'/'.join(map(str, e.path)) or '(root)'}: {e.message}"
            for e in validator.iter_errors(document)
        ]
    assert problems == []


def test_the_schema_refuses_a_nameless_component_and_a_nameless_part():
    """OPS-01: the check has teeth. `parts` may be empty (a component that writes its releases in
    its own helmfile), but a component needs a name and so does every part it declares."""
    validator = jsonschema.Draft7Validator(json.loads((ROOT / "component.schema.json").read_text()))
    assert list(validator.iter_errors({"parts": []})), "a component without a name validated"
    assert list(validator.iter_errors({"component": "x", "parts": [{}]})), "a nameless part validated"
    assert not list(validator.iter_errors({"component": "x", "parts": []}))


def test_every_image_a_component_pins_names_its_digest():
    """OPS-01: `images.yaml` pins tags and digests; a tag alone is not a pin."""
    unpinned = []
    for images in ROOT.glob("components/*/images.yaml"):
        for key, entry in (yaml.safe_load(images.read_text()) or {}).items():
            if isinstance(entry, dict) and "repository" in entry and "digest" not in entry:
                unpinned.append(f"{images.parent.name}/{key}")
    assert unpinned == []


def test_no_environment_of_an_operator_is_committed_here():
    """OPS-02: an environment's values live in the operator's `deployment/` checkout, never in
    this repository; `defaults/deployment/` is the skeleton that checkout starts from."""
    assert tracked("deployment") == [], "deployment/ is the operator's checkout, not ours"
    offenders = [
        path
        for path in tracked("components") + tracked("defaults")
        if not path.startswith("defaults/deployment/")
        and OPERATOR_ONLY & set(re.split(r"[/]", path))
    ]
    assert offenders == []


def test_an_environment_is_layered_over_the_defaults_and_never_replaces_them():
    """OPS-02: every environment loads `defaults/environment/global.yaml` first and its own
    `deployment/environments/<env>/` values after, so an override never needs a default edited."""
    root = (ROOT / "helmfile-root.yaml.gotmpl").read_text()
    loop = root[root.index("{{- range .Values.environments }}") :]
    defaults = loop.index("defaults/environment/global.yaml")
    own = loop.index("deployment/environments/{{ . }}/global.yaml.gotmpl")
    assert defaults < own
