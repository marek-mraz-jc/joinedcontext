#!/usr/bin/env python3
"""What a release changes on production, from two renders of `smartcity` (T-0051).

    scripts/release-diff.py <old-rendered.yaml> <new-rendered.yaml> [--max-bytes N]

Prints Markdown for the approval gate's job summary (.github/workflows/release-gate.yml):
the objects added and removed, then one unified diff per changed object. It needs no cluster
credential: owner 2026-10-24, the release workflow never holds one, so it compares what the
previous release tag renders with what the new tag renders.

Secret values never reach the output: `data` and `stringData` keep their keys and show
`<redacted>`. The secrets chart invents fresh random values on every `helm template` and the
`checksum/*` pod annotations follow them, so both are masked as well; otherwise each render
would show every Secret and its Deployments as changed.
"""

from __future__ import annotations

import argparse
import difflib
import sys

import yaml

REDACTED = "<redacted>"
# GitHub refuses a step summary over 1 MiB; leave room for the header lines.
DEFAULT_MAX_BYTES = 900_000


def _mask_checksums(node: object) -> None:
    if isinstance(node, dict):
        for key, value in node.items():
            if key == "annotations" and isinstance(value, dict):
                for name in value:
                    if str(name).startswith("checksum/"):
                        value[name] = REDACTED
            else:
                _mask_checksums(value)
    elif isinstance(node, list):
        for item in node:
            _mask_checksums(item)


def normalise(doc: dict) -> dict:
    if doc.get("kind") == "Secret":
        for field in ("data", "stringData"):
            if isinstance(doc.get(field), dict):
                doc[field] = {key: REDACTED for key in doc[field]}
    _mask_checksums(doc)
    return doc


def objects(path: str) -> dict[str, str]:
    """`kind namespace/name` -> the object as sorted YAML."""
    with open(path, encoding="utf-8") as fh:
        docs = [d for d in yaml.safe_load_all(fh) if isinstance(d, dict) and d.get("kind")]
    out = {}
    for doc in docs:
        meta = doc.get("metadata") or {}
        key = f"{doc['kind']} {meta.get('namespace') or '-'}/{meta.get('name')}"
        out[key] = yaml.safe_dump(normalise(doc), sort_keys=True, default_flow_style=False)
    return out


def report(old: dict[str, str], new: dict[str, str], max_bytes: int) -> str:
    added = sorted(new.keys() - old.keys())
    removed = sorted(old.keys() - new.keys())
    changed = sorted(k for k in old.keys() & new.keys() if old[k] != new[k])
    lines = [(f"**{len(added)} added, {len(removed)} removed, {len(changed)} changed** "
             f"of {len(new)} objects. Secret values are never shown.")]
    for title, keys in (("Added", added), ("Removed", removed)):
        if keys:
            lines += ["", f"### {title}", "", *(f"- `{k}`" for k in keys)]
    if changed:
        lines += ["", "### Changed"]
    body = "\n".join(lines) + "\n"
    # Room for the closing list, so a diff too big to show never pushes it over the cap.
    budget = max_bytes - sum(len(k) + 4 for k in changed) - 200
    diffs = {key: "".join(difflib.unified_diff(old[key].splitlines(True), new[key].splitlines(True),
                                               f"previous/{key}", f"release/{key}")) for key in changed}
    left_out = []
    # Smallest first: one huge seed ConfigMap must not crowd out fifty image bumps.
    for key in sorted(changed, key=lambda k: (len(diffs[k]), k)):
        diff = diffs[key]
        section = f"\n<details><summary><code>{key}</code></summary>\n\n```diff\n{diff}```\n</details>\n"
        if len((body + section).encode()) > budget:
            left_out.append(key)
        else:
            body += section
    if left_out:
        rest = ", ".join(f"`{k}`" for k in left_out)
        body += f"\nToo big for the summary, the whole diff is in the run's artifact: {rest}\n"
    return body


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("old")
    parser.add_argument("new")
    parser.add_argument("--max-bytes", type=int, default=DEFAULT_MAX_BYTES)
    args = parser.parse_args(argv)
    sys.stdout.write(report(objects(args.old), objects(args.new), args.max_bytes))
    return 0


if __name__ == "__main__":
    sys.exit(main())
