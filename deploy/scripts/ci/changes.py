#!/usr/bin/env python3
"""What ci runs for a commit range (T-6110, T-6152): the tests that read the changed files and the
environments they render, or the full set.

    scripts/ci/changes.py <base> <head>     prints GITHUB_OUTPUT lines:
        pins_only=true|false   every changed file is a components/<c>/images.yaml (pin-images' commits)
        pin_tests=<files>      (pins_only only) the tests that read a moved images.yaml
        full=true|false        the six shards over every test file
        tests=<files>          (full=false) the test files the change selects, empty when none read it
        envs=<environments>    the environments the render job renders and gates
        pins=true|false        an images.yaml moved: verify the pinned signatures
        demo=true|false        render and gate the BBSK demo
        cluster=true|false     cluster/hetzner moved: the cluster-hetzner job runs
        shards=[1,...]         the tests job's matrix: six for the full set, one per SHARD_SECONDS
                               of the scoped set's measured time

The per-push set (owner 2026-10-30: push to dev under 5 minutes) runs the tests that read a changed
file and renders `production` plus the environments a change names. The whole set (every test file, every
environment, the demo) runs in the weekly sweep and in the release gate (.github/workflows/sweep.yml),
and here whenever the range cannot be judged: an unreadable range (a new branch's all-zero base, a
force-pushed base), an empty diff, a file every render or test reads (FULL), or a file no test names
(an unknown path). A missed match costs minutes; a wrong match skips a gate until the sweep.

A test reads a file when its text names the file's path or a directory above it (up to
components/<c>/<dir>, or <top>/<dir> outside components/), in either spelling ("components/x/y" or "components" / "x" / "y"),
or its base name. A component change also selects the test files named after the component and
RENDER_TESTS, which read every rendered object. pin_tests keeps T-6110's rule: a pin reruns the tests
naming its images.yaml, every images.yaml (a glob) or none by path.
"""

from __future__ import annotations

import json
import math
import re
import subprocess
import sys
from pathlib import Path

PIN = re.compile(r"^components/([a-z0-9-]+)/images\.yaml$")
# components/<c>/images.yaml, or "components" / "<c>" / "images.yaml" as pathlib spells it
NAMED = re.compile(r'components/([a-z0-9-]+)/images\.yaml|"components" */ *"([a-z0-9-]+)" */ *"images\.yaml"')
ENVS_FILE = "ci/golden/ENVIRONMENTS"
# Rendered on every push: the hosted public example (the private dev renders on the host only).
BASE_ENV = "production"
# Every render and every test reads these: any change to one runs the whole set.
FULL = ("tests/conftest.py", "tests/pyproject.toml", "tests/uv.lock", "defaults/", "helmfile-root.yaml.gotmpl",
        "helmfile-components.yaml.gotmpl", "component.schema.json", "tools.env", ".github/actions/",
        "ci/example-deployments/helmfile.yaml", "ci/example-deployments/environments/_layers/",
        "scripts/render.sh", "scripts/render-all.sh", ENVS_FILE, "ci/golden/list-objects.py", "policies/")
# Read by nothing a test checks, or gated by the render job alone.
NO_TESTS = re.compile(r"(\.md$|^\ci/test-durations\.json$|^\ci/golden/[^/]+\.txt$|^releases/|^LICENSE$)")
# Every rendered object of every component: networking, memory, schema and layout rules.
RENDER_TESTS = ("tests/test_networkpolicies.py", "tests/test_memory_budget.py", "tests/test_values_schemas.py",
                "tests/test_deployment_layout.py", "tests/test_production_baseline.py")
ENV_PATH = re.compile(r"^(?:\ci/example-deployments/environments|deployment/environments)/([a-z0-9-]+)/|"
                      r"^\ci/golden/([a-z0-9-]+)\.txt$|^gitops/([a-z0-9-]+)\.yaml$")
COMPONENT = re.compile(r"^components/([a-z0-9-]+)/")
# The scoped set gets a shard per this many seconds of ci/test-durations.json (about four minutes of
# pytest wall at scripts/ci/test-shard.py's PARALLEL), at most SHARDS; the full set takes SHARDS.
SHARD_SECONDS = 900
SHARDS = 6


def pins_only(files: list[str]) -> bool:
    return bool(files) and all(PIN.match(f) for f in files)


def pin_tests(files: list[str], tests: dict[str, str]) -> list[str]:
    """The test files (name -> text) a pin of `files` reruns."""
    moved = {m.group(1) for f in files if (m := PIN.match(f))}
    keep = []
    for name, text in sorted(tests.items()):
        if "images.yaml" not in text:
            continue
        named = {a or b for a, b in NAMED.findall(text)}
        if "*/images.yaml" in text or not named or named & moved:
            keep.append(name)
    return keep


def _spelled(path: str) -> re.Pattern:
    """`path` as a test names it: a/b/c or "a" / "b" / "c"."""
    return re.compile(r'(?:/|" */ *")'.join(re.escape(p) for p in path.split("/")))


def readers(path: str, tests: dict[str, str]) -> set[str]:
    """The test files that name `path`, a directory above it, or its base name."""
    parts = path.split("/")
    floor = 3 if parts[0] == "components" else 2
    needles = [_spelled("/".join(parts[:depth])) for depth in range(len(parts), min(floor, len(parts)) - 1, -1)]
    found = {n for n, t in tests.items() if any(r.search(t) for r in needles) or (len(parts[-1]) > 8 and parts[-1] in t)}
    if m := COMPONENT.match(path):
        stem = m.group(1).replace("-", "_")
        found |= {n for n in tests if re.search(rf"test_{stem}(_|\.py$)", n)}
        found |= {n for n in RENDER_TESTS if n in tests}
    return found


def scope(files: list[str] | None, tests: dict[str, str], environments: list[str]) -> dict:
    """The per-push set for `files`, or the full one."""
    whole = {"full": True, "tests": [], "envs": environments, "pins": True, "demo": True}
    if not files or any(f.startswith(FULL) or f == p for f in files for p in FULL):
        return whole
    chosen, envs = set(), {BASE_ENV}
    for f in files:
        if f.startswith("tests/") and f in tests:
            chosen.add(f)
            continue
        env = ENV_PATH.match(f)
        if env and (name := next(g for g in env.groups() if g)) in environments:
            envs.add(name)
        found = readers(f, tests)
        # an environment's own file is gated by its render; any other file no test names is unknown
        if not found and not env and not NO_TESTS.search(f):
            return whole
        chosen |= found
    return {"full": False, "tests": sorted(chosen), "envs": [e for e in environments if e in envs],
            "pins": any(PIN.match(f) for f in files), "demo": any(f.startswith(("demos/", "ci/golden/demo-bbsk-")) for f in files)}


def shards(s: dict, weights: dict[str, float]) -> int:
    if s["full"]:
        return SHARDS
    weight = sum(weights.get(Path(t).name, 0.0) for t in s["tests"])
    return max(1, min(SHARDS, math.ceil(weight / SHARD_SECONDS)))


def changed(base: str, head: str) -> list[str] | None:
    if not re.fullmatch(r"[0-9a-f]{7,40}", base or "") or set(base) == {"0"}:
        return None
    # --relative: the paths under the working directory (deploy/ of jc), relative to it
    done = subprocess.run(["git", "diff", "--name-only", "--relative", "--no-renames", base, head],
                          capture_output=True, text=True, check=False)
    return done.stdout.split() if done.returncode == 0 else None


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(__doc__.strip().splitlines()[3].strip(), file=sys.stderr)
        return 2
    files = changed(argv[0], argv[1])
    short = files is not None and pins_only(files)
    tests = {str(p): p.read_text(encoding="utf-8") for p in sorted(Path("tests").glob("test_*.py"))}
    print(f"pins_only={'true' if short else 'false'}")
    if short:
        print(f"pin_tests={' '.join(pin_tests(files, tests))}")
    environments = Path(ENVS_FILE).read_text(encoding="utf-8").split() if Path(ENVS_FILE).exists() else [BASE_ENV]
    s = scope(files, tests, environments)
    if short:
        s["tests"] = pin_tests(files, tests)
    print(f"full={'true' if s['full'] else 'false'}")
    print(f"tests={' '.join(s['tests'])}")
    print(f"envs={' '.join(s['envs'])}")
    print(f"pins={'true' if s['pins'] else 'false'}")
    print(f"demo={'true' if s['demo'] else 'false'}")
    print(f"cluster={'true' if files is None or any(f.startswith('cluster/hetzner/') for f in files) else 'false'}")
    table = Path("ci/test-durations.json")
    weights = json.loads(table.read_text(encoding="utf-8")) if table.exists() else {}
    print(f"shards={json.dumps(list(range(1, shards(s, weights) + 1)), separators=(',', ':'))}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
