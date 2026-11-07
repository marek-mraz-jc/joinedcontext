#!/usr/bin/env python3
"""`odata_url_abnf_testcases`: odata-url against the OASIS ABNF test cases (ADR-0024 decision 3,
T-3503).

    dev/odata-url-abnf.py                fetch, check, convert, run (CI step of the same name)
    dev/odata-url-abnf.py --seeds <dir>  write each case's URL into <dir>, the fuzz corpus of
                                         odata_url_parse, and run nothing

The test cases are fetched at the pinned commit and checked against their SHA-256 before use;
they are never vendored (their licence is the OASIS IPR policy). The YAML is read without type
inference (every scalar a string, as the OASIS runner reads its inputs) and handed to
`cargo run -p odata-url --example abnf` as JSON; the example prints the count per rule and
outcome and fails on any case that fails.

Needs: cargo, python3 with PyYAML.
"""

from __future__ import annotations

import hashlib
import json
import pathlib
import subprocess
import sys
import tempfile
import urllib.request

import yaml

ROOT = pathlib.Path(__file__).resolve().parent.parent
URL = (
    "https://raw.githubusercontent.com/oasis-tcs/odata-abnf/"
    "a31c8db229a0d8e957bee86512b01efe7ce74173/abnf/odata-abnf-testcases.yaml"
)
SHA256 = "feded192570e4c7e64c60b47d9631ae20da7cd1c974d54ae58111cd84d12c921"
# One case (`$orderby=Name<TAB>asc`) has a raw tab in a plain scalar, which PyYAML refuses;
# it is swapped for a private-use character before loading and back after.
TAB = "\uE000"


def restore(node):
    if isinstance(node, str):
        return node.replace(TAB, "\t")
    if isinstance(node, list):
        return [restore(item) for item in node]
    if isinstance(node, dict):
        return {key: restore(value) for key, value in node.items()}
    return node


def prefix(rule: str) -> str:
    """The URL around a case, as examples/abnf.rs wraps it."""
    if rule in ("odataRelativeUri", "resourcePath", "entitySetName", "odataIdentifier"):
        return ""
    if rule in ("queryOptions", "systemQueryOption", "customQueryOption", "filter", "select", "expand",
                "orderby", "orderBy", "skiptoken", "deltatoken", "compute", "search"):
        return "Customers?"
    if rule == "searchExpr":
        return "Customers?$search="
    return "Customers?$filter=Items/" if rule == "anyExpr" else "Customers?$filter="


def seeds(cases: dict, directory: pathlib.Path) -> int:
    directory.mkdir(parents=True, exist_ok=True)
    for case in cases["TestCases"]:
        url = (prefix(case["Rule"]) + case["Input"]).encode("utf-8")
        (directory / hashlib.sha256(url).hexdigest()[:16]).write_bytes(url)
    print(f"{len(cases['TestCases'])} seeds in {directory}")
    return 0


def main() -> int:
    with urllib.request.urlopen(URL, timeout=60) as response:
        raw = response.read()
    digest = hashlib.sha256(raw).hexdigest()
    if digest != SHA256:
        print(f"{URL}: sha256 {digest}, pinned {SHA256}", file=sys.stderr)
        return 1
    cases = restore(yaml.load(raw.decode("utf-8").replace("\t", TAB), Loader=yaml.BaseLoader))
    if sys.argv[1:2] == ["--seeds"] and len(sys.argv) == 3:
        return seeds(cases, pathlib.Path(sys.argv[2]))
    with tempfile.TemporaryDirectory() as tmp:
        path = pathlib.Path(tmp) / "cases.json"
        path.write_text(json.dumps(cases), encoding="utf-8")
        run = ["cargo", "run", "--quiet", "-p", "odata-url", "--example", "abnf", "--", str(path)]
        return subprocess.run(run, cwd=ROOT, check=False).returncode


if __name__ == "__main__":
    sys.exit(main())
