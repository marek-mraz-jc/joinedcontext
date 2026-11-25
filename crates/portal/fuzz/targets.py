"""Print the fuzz targets of a cargo-fuzz manifest as `list=<JSON array>` for $GITHUB_OUTPUT.

portal-fuzz.yml builds its matrix from this, so the manifest stays the one list of targets.
Fails on an empty list or a name outside [a-z0-9_]+, before any name reaches a shell. A second,
non-empty argument (the workflow's `target` input) narrows the list to that one manifest target.
"""

import json
import re
import sys
import tomllib

with open(sys.argv[1], "rb") as f:
    names = [b.get("name", "") for b in tomllib.load(f).get("bin", [])]
bad = [n for n in names if not re.fullmatch(r"[a-z0-9_]+", n)]
if not names or bad:
    sys.exit(
        f"fuzz targets: {'no bin entries' if not names else f'invalid names {bad}'} in {sys.argv[1]}"
    )
only = sys.argv[2] if len(sys.argv) > 2 else ""
if only:
    if only not in names:
        sys.exit(f"fuzz targets: {only!r} is not a bin of {sys.argv[1]}")
    names = [only]
print("list=" + json.dumps(names))
