#!/usr/bin/env python3
"""Validate the GBFS files the surface serves against MobilityData's GBFS 3.0 JSON schemas (T-4488).

    python3 tests/gbfs/validate.py               the golden files of `cargo test -p antares-gbfs`
    python3 tests/gbfs/validate.py DIR           the files a live feed wrote into DIR

The schemas in tests/gbfs/schemas are MobilityData/gbfs-json-schema v3.0 at commit
2e974fd51ae1db9c8006c97375d53e0b882cfedf, unchanged. A file that does not validate prints each
error with its JSON path and the script exits 1.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import jsonschema

HERE = Path(__file__).resolve().parent
FILES = ("gbfs", "system_information", "station_information", "station_status")


def errors(folder: Path) -> list[str]:
    found = []
    for name in FILES:
        schema = json.loads((HERE / "schemas" / f"{name}.json").read_text())
        document = json.loads((folder / f"{name}.json").read_text())
        validator = jsonschema.Draft7Validator(schema, format_checker=jsonschema.FormatChecker())
        found += [f"{name}.json {'/'.join(map(str, e.absolute_path)) or '(root)'}: {e.message}"
                  for e in validator.iter_errors(document)]
    return found


def main() -> int:
    folder = Path(sys.argv[1]) if len(sys.argv) > 1 else HERE / "golden"
    found = errors(folder)
    for line in found:
        print(line)
    print(f"gbfs: {len(FILES)} files in {folder}, {'red' if found else 'valid against GBFS 3.0'}")
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
