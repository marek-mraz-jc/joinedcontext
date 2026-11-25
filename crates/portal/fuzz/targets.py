"""Print the fuzz targets of a cargo-fuzz manifest as `list=<JSON array>` for $GITHUB_OUTPUT.

portal-fuzz.yml builds its matrix from this, so the manifest stays the one list of targets.
Fails on an empty list or a name outside [a-z0-9_]+, before any name reaches a shell.
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
print("list=" + json.dumps(names))
