#!/usr/bin/env python3
"""Every remote chart a helmfile installs names one exact version (docs/analysis/iac-practices.md R1, R2).

    helmfile -f deployment/helmfile.yaml -e dev list --output json | scripts/ci/check-chart-pins.py -
    scripts/ci/check-chart-pins.py --no-lock .

A range (`~1.2.3`, `^1`, `>=1`, `1.2.x`) or a missing version installs whatever the repository
serves on the day of the apply, which is not the chart anybody reviewed. The exact version sits
in the file a reader sees (`components/*/charts.yaml` or the helmfile) and Renovate bumps it;
no `helmfile.lock` keeps a second copy, so `--no-lock` refuses one anywhere under the root.
"""

import json
import re
import sys
from pathlib import Path

EXACT = re.compile(r"^v?\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$")


def remote(chart: str) -> bool:
    """A repository chart (`alias/name`, `oci://…`); a path into this repository is local."""
    if chart.startswith("oci://"):
        return True
    return (
        not chart.startswith((".", "/"))
        and chart.split("/", 1)[0] != "charts"
        and "/" in chart
    )


def unpinned(releases: list[dict]) -> list[str]:
    return [
        f"{r.get('namespace', '?')}/{r.get('name', '?')}: {r['chart']} version {r.get('version') or '(none)'!r}"
        for r in releases
        if remote(r.get("chart", "")) and not EXACT.match(str(r.get("version") or ""))
    ]


def locks(root: Path) -> list[str]:
    return [str(p) for p in root.rglob("helmfile.lock") if ".git" not in p.parts]


def main(argv: list[str]) -> int:
    if argv[:1] == ["--no-lock"] and len(argv) == 2:
        found = locks(Path(argv[1]))
        for path in found:
            print(
                f"::error::{path}: no lock file; pin the exact version in the helmfile or charts.yaml"
            )
        return 1 if found else 0
    if len(argv) != 1:
        print(__doc__, file=sys.stderr)
        return 2
    text = sys.stdin.read() if argv[0] == "-" else Path(argv[0]).read_text()
    bad = unpinned(json.loads(text) or [])
    for line in bad:
        print(f"::error::{line}: pin one exact chart version (X.Y.Z)")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
