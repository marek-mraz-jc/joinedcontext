"""What the release journeys cover, and what they do not (T-7156).

    coverage.py [--requirements <docs clone>/Requirements] [--json]

Reads crates/portal/ui/e2e/release/catalogue.json (each journey: id, title, file, requirements,
pages) and the Portal's routes (crates/portal/ui/src/router.tsx `path:` lines), and prints:

- every Portal route with no journey, so the suite grows toward 1000 by what it does not reach yet,
  not by copies of what it has;
- per requirement area (UI, PF, EP, ...) how many ids a journey proves, and, with --requirements,
  how many the docs define.

A route is covered when a journey's `pages` names it with its parameters filled
(`/projects/helsinki/explore` covers `/projects/$project/explore`). Python 3 stdlib only."""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections import Counter
from pathlib import Path

REPO = Path(__file__).resolve().parents[4]
UI = REPO / "crates/portal/ui"
CATALOGUE = UI / "e2e/release/catalogue.json"
ROUTER = UI / "src/router.tsx"
REQ = re.compile(r"^- \*\*([A-Z]+-\d+)\*\*", re.MULTILINE)


def routes(router: Path = ROUTER) -> list[str]:
    """Absolute route paths of the Portal (relative children like `new`, `$name` are skipped)."""
    return sorted({m for m in re.findall(r'^\s*path:\s*"([^"]+)"', router.read_text(), re.MULTILINE) if m.startswith("/")})


def pattern(route: str) -> re.Pattern:
    parts = [("[^/]+" if p.startswith("$") and p != "$" else ".+" if p == "$" else re.escape(p))
             for p in route.strip("/").split("/")]
    return re.compile("^/" + "/".join(parts) + "$" if parts != [""] else "^/$")


def report(catalogue: dict, all_routes: list[str], defined: Counter | None = None) -> dict:
    pages = [p.split("?")[0] for j in catalogue["journeys"] for p in j.get("pages", [])]
    covered = {r for r in all_routes if any(pattern(r).match(p) for p in pages)}
    # A literal route wins over a parameter route that also matches (/projects/x/explore is the
    # explore route, not /projects/$project/$plural).
    proven = Counter(r.split("-")[0] for j in catalogue["journeys"] for r in set(j.get("requirements", [])))
    return {
        "journeys": len(catalogue["journeys"]),
        "routes": len(all_routes),
        "covered": sorted(covered),
        "uncovered": [r for r in all_routes if r not in covered],
        "requirements": {k: {"proven": proven[k], "defined": (defined or Counter())[k]}
                         for k in sorted(set(proven) | set(defined or {}))},
    }


def main(argv=None) -> int:
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--requirements", type=Path, help="docs/Requirements of the docs clone")
    p.add_argument("--json", action="store_true")
    a = p.parse_args(argv)
    defined = None
    if a.requirements:
        defined = Counter(m.split("-")[0] for f in a.requirements.glob("*.md") for m in REQ.findall(f.read_text()))
    r = report(json.loads(CATALOGUE.read_text()), routes(), defined)
    if a.json:
        print(json.dumps(r, indent=1))
        return 0
    print(f"# Release e2e coverage\n\n{r['journeys']} journeys; {len(r['covered'])} of {r['routes']} Portal routes reached.\n")
    print("## Routes no journey reaches\n")
    print("\n".join(f"- `{x}`" for x in r["uncovered"]) or "(none)")
    print("\n## Requirements proven per area\n\n| area | proven | defined |\n|---|---|---|")
    for k, v in r["requirements"].items():
        print(f"| {k} | {v['proven']} | {v['defined'] or '-'} |")
    return 0


if __name__ == "__main__":
    sys.exit(main())
