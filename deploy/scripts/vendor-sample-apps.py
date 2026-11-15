#!/usr/bin/env python3
"""Copies the sample applications from a joinedcontext-portal commit into the forge seed (T-2599).

Each application has its own repository on the forge (AP-75), and the forge bootstrap Job
pushes it there from `components/gitea/apps/<name>/`. The source of truth is the portal
repository's `apps/<name>/`, where CI tests the same tree; this copies it at one commit and
records which, so the two cannot drift silently:

    scripts/vendor-sample-apps.py ../joinedcontext-portal <commit> helsinki-bikes helsinki-events helsinki-alerts

It reads the commit with `git show`, never the working tree, so an uncommitted edit in the
portal clone is not vendored. `index.yaml` lists every file of every app, which is what the
bootstrap values read (helmfile has no directory walk).
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

DEST = Path(__file__).resolve().parent.parent / "components/gitea/apps"


def git(portal: Path, *args: str) -> bytes:
    return subprocess.run(["git", "-C", str(portal), *args], check=True, capture_output=True).stdout


def main(argv: list[str]) -> int:
    if len(argv) < 3:
        print(__doc__, file=sys.stderr)
        return 2
    portal, commit, names = Path(argv[0]), argv[1], argv[2:]
    full = git(portal, "rev-parse", "--verify", f"{commit}^{{commit}}").decode().strip()
    index = [
        "# Written by scripts/vendor-sample-apps.py; do not edit. The files of each sample application,",
        "# relative to its folder, as the forge bootstrap pushes them to its repository (T-2599).",
        f"source: {full}",
        "apps:",
    ]
    for name in names:
        prefix = f"apps/{name}/"
        files = git(portal, "ls-tree", "-r", "--name-only", full, prefix).decode().split()
        if not files:
            print(f"{prefix} is empty at {full}", file=sys.stderr)
            return 1
        target = DEST / name
        shutil.rmtree(target, ignore_errors=True)
        index.append(f"  {name}:")
        for path in sorted(files):
            relative = path[len(prefix):]
            out = target / relative
            out.parent.mkdir(parents=True, exist_ok=True)
            out.write_bytes(git(portal, "show", f"{full}:{path}"))
            index.append(f"    - {relative}")
    (DEST / "index.yaml").write_text("\n".join(index) + "\n", encoding="utf-8")
    print(f"vendored {', '.join(names)} from {full}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
