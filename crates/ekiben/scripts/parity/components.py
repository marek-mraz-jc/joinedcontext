#!/usr/bin/env python3
"""Component parity of ekiben with Bento 1.22.0 (one-to-one port tasks, T-6835..T-6843).

    components.py record   run every tests/parity/components/<component>/<case>.yaml (stdin from
                           <case>.stdin when present) in Bento twice and in ekiben, and write
                           tests/parity/components.json
    components.py check    rerun the recorded cases in ekiben only

A case's config ends in a mapping that prints body and metadata, so a byte-identical stdout means
identical bodies and metadata. crates/ekiben-server/tests/parity.rs replays components.json.
Needs .bento/bento (scripts/reference-bento.sh) and a debug ekiben in $CARGO_TARGET_DIR.
"""
import json, sys
from pathlib import Path

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parent))
import regen  # noqa: E402

DIR = regen.PARITY / "components"
OUT = regen.PARITY / "components.json"


def collect() -> list[dict]:
    cases = []
    for conf in sorted(DIR.glob("*/*.yaml")):
        stdin = conf.with_suffix(".stdin")
        cases.append({
            "id": f"{conf.parent.name}/{conf.stem}",
            "mode": "run",
            "env": {},
            "config": regen.put_blob(json.dumps(yaml.safe_load(conf.read_text())).encode()),
            "stdin": regen.put_blob(stdin.read_bytes() if stdin.exists() else b""),
            "tests": [f"tests/parity/components/{conf.parent.name}/{conf.name}"],
        })
    return cases


def record():
    cases = collect()
    for c in cases:
        code, out, err = regen.run_case(c, regen.BENTO, True)
        c["bento"] = {"code": code, "stdout": regen.put_blob(out)}
        again = regen.run_case(c, regen.BENTO, True)
        if again[:2] != (code, out):
            c["why"] = "Bento itself answers differently on a second run (clock, random or order)"
        print(f"{c['id']}: bento exit {code}, {len(out)} bytes {err[-200:] if code else ''}")
    regen.check(cases, write=True)
    OUT.write_text(json.dumps({"bento": "1.22.0", "cases": cases}, indent=1, sort_keys=True) + "\n")


def main():
    if sys.argv[1:] == ["record"]:
        record()
    elif sys.argv[1:] == ["check"]:
        sys.exit(1 if regen.check(json.loads(OUT.read_text())["cases"]) else 0)
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
