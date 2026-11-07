#!/usr/bin/env python3
"""Gate (T-5105): every Antares migration states what its release's rollback does.

crates/antares-sql/migrations.rollback.toml holds one entry per file in
crates/antares-sql/migrations/: rollback = "compatible" or "breaking", and a
note. A breaking migration's file name starts with `breaking_`, the only mark a
previous release can read at boot (store/pg/mod.rs `tolerate_newer`).
Migrations are forward-only: an .up.sql/.down.sql pair is refused.
--self-test plants each violation once.
"""
import re
import sys
import tempfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent / "crates/antares-sql"
NAME = re.compile(r"^(\d{4})_([a-z0-9_]+)\.sql$")


def problems(migrations: Path, manifest: Path) -> list[str]:
    out = []
    files = {}
    for f in sorted(migrations.iterdir()):
        if f.name.endswith((".up.sql", ".down.sql")):
            out.append(f"{f.name}: migrations are forward-only, ship no down step")
            continue
        m = NAME.match(f.name)
        if not m:
            out.append(f"{f.name}: not NNNN_lower_snake.sql")
            continue
        files[m.group(1)] = m.group(2)
    try:
        entries = tomllib.loads(manifest.read_text())
    except (OSError, tomllib.TOMLDecodeError) as e:
        return out + [f"{manifest.name}: {e}"]
    for v, name in files.items():
        e = entries.get(v)
        if not isinstance(e, dict):
            out.append(f"{v}_{name}.sql: no [{v}] entry in {manifest.name}")
            continue
        kind = e.get("rollback")
        if kind not in ("compatible", "breaking"):
            out.append(f"[{v}]: rollback must be \"compatible\" or \"breaking\", not {kind!r}")
        elif (kind == "breaking") != name.startswith("breaking_") and v != "0001":
            out.append(f"[{v}]: rollback = {kind!r} but the file name {'lacks' if kind == 'breaking' else 'has'} the breaking_ prefix")
        if not str(e.get("note", "")).strip():
            out.append(f"[{v}]: note says what the previous release misses (or why it cannot serve the schema)")
    for v in entries.keys() - files.keys():
        out.append(f"[{v}]: no migration file with that version")
    return out


def self_test() -> None:
    cases = {
        "ok": ({"0001_init.sql": "", "0002_breaking_drop.sql": ""},
               '[0001]\nrollback="breaking"\nnote="x"\n[0002]\nrollback="breaking"\nnote="y"\n', 0),
        "missing entry": ({"0002_a.sql": ""}, "", 1),
        "stale entry": ({}, '[0003]\nrollback="compatible"\nnote="x"\n', 1),
        "bad kind": ({"0002_a.sql": ""}, '[0002]\nrollback="maybe"\nnote="x"\n', 1),
        "breaking without prefix": ({"0002_a.sql": ""}, '[0002]\nrollback="breaking"\nnote="x"\n', 1),
        "prefix without breaking": ({"0002_breaking_a.sql": ""}, '[0002]\nrollback="compatible"\nnote="x"\n', 1),
        "empty note": ({"0002_a.sql": ""}, '[0002]\nrollback="compatible"\nnote=" "\n', 1),
        "down step": ({"0002_a.down.sql": ""}, "", 1),
        "bad name": ({"2_A.sql": ""}, "", 1),
    }
    for label, (files, toml, want) in cases.items():
        with tempfile.TemporaryDirectory() as d:
            mig = Path(d) / "migrations"
            mig.mkdir()
            for n, body in files.items():
                (mig / n).write_text(body)
            man = Path(d) / "m.toml"
            man.write_text(toml)
            got = len(problems(mig, man))
            assert got == want, f"self-test {label}: {got} problems, want {want}"
    print(f"check-migrations self-test: OK ({len(cases)} cases)")


if __name__ == "__main__":
    if sys.argv[1:] == ["--self-test"]:
        self_test()
        sys.exit(0)
    found = problems(ROOT / "migrations", ROOT / "migrations.rollback.toml")
    for p in found:
        print(f"MIGRATION ROLLBACK: {p} (docs/src/operations.md)")
    if not found:
        print(f"check-migrations: OK ({len(list((ROOT / 'migrations').iterdir()))} migrations)")
    sys.exit(1 if found else 0)
