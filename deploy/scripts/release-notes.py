#!/usr/bin/env python3
"""Write the notes of a release, `releases/<version>.md` (T-3794).

    scripts/release-notes.py <version> [--since <tag>]

Nothing in them is typed. The images are every pin of `components/*/images.yaml` as committed,
`repository:tag@digest`; the changes are the subjects of the commits since the previous release
tag (or `--since`); the version-specific steps are the ones queued in `releases/next.yaml`,
`{name, run}` each, which this script moves into the release and leaves empty. Commit the file,
then tag the commit `<version>`: `scripts/upgrade.py` reads the notes from the tag.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
VERSION = re.compile(r"^v\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$")


def images(components: Path) -> list[str]:
    """`repository:tag@digest` of every pin, sorted. A pin is a mapping with a `repository`, at
    any depth (apisix nests one per part); one without a sha256 digest is refused."""
    pins = set()

    def walk(node, where):
        if not isinstance(node, dict):
            return
        if "repository" in node:
            if not str(node.get("digest", "")).startswith("sha256:"):
                raise SystemExit(f"{where} is not pinned by digest")
            pins.add(f"{node['repository']}:{node.get('tag', '')}@{node['digest']}")
            return
        for key, child in node.items():
            walk(child, f"{where}.{key}")

    for path in sorted(components.glob("*/images.yaml")):
        walk(yaml.safe_load(path.read_text()), str(path.relative_to(components)))
    return sorted(pins)


def previous_tag(version: str) -> str | None:
    tags = subprocess.run(["git", "tag", "--list", "v*", "--sort=-v:refname", "--merged", "HEAD"],
                          cwd=ROOT, capture_output=True, text=True, check=True).stdout.split()
    return next((tag for tag in tags if tag != version), None)


def changes(since: str | None) -> list[str]:
    span = [f"{since}..HEAD"] if since else ["HEAD"]
    return subprocess.run(["git", "log", "--no-merges", "--format=%s", *span],
                          cwd=ROOT, capture_output=True, text=True, check=True).stdout.splitlines()


def notes(version: str, since: str | None, pins: list[str], steps: list[dict], subjects: list[str],
          date: str | None = None) -> str:
    meta = {"version": version, **({"date": date} if date else {}), "previous": since, "images": pins,
            "steps": steps}
    body = [f"# Release {version}", "", f"Changes since {since or 'the first commit'}:", ""]
    body += [f"- {subject}" for subject in subjects] or ["- none"]
    body += ["", "## Steps after the apply", ""]
    body += [f"1. {step['name']}: `{step['run']}`" for step in steps] or ["None."]
    return "---\n" + yaml.safe_dump(meta, sort_keys=False) + "---\n\n" + "\n".join(body) + "\n"


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("version")
    p.add_argument("--since", default=None)
    args = p.parse_args()
    if not VERSION.match(args.version):
        raise SystemExit(f"{args.version!r} is not v<major>.<minor>.<patch>")
    queue = ROOT / "releases/next.yaml"
    steps = (yaml.safe_load(queue.read_text()) or {}).get("steps") or [] if queue.exists() else []
    for step in steps:
        if set(step) != {"name", "run"} or not (ROOT / step["run"]).is_file():
            raise SystemExit(f"releases/next.yaml: {step!r} is not {{name, run}} naming a script")
    since = args.since or previous_tag(args.version)
    out = ROOT / f"releases/{args.version}.md"
    out.write_text(notes(args.version, since, images(ROOT / "components"), steps, changes(since)))
    queue.write_text("---\n# Steps the next release runs after its apply, {name, run} each.\nsteps: []\n")
    print(out.relative_to(ROOT))
    return 0


if __name__ == "__main__":
    sys.exit(main())
