"""Do the Prague entities on an instance make sense (T-5270, owner 2026-10-22T18:10Z)?

The Helsinki check (tests/helsinki/plausible.py) with Prague's area and same-object rules: every
entity of each type the given public Endpoints serve, read as key-values in Czech, counted for a
point outside the area, at 0,0 or swapped, a value outside its physical range, more available than
capacity, a stamp more than a day ahead, a dangling `ref*`, and two entities of one real object
within 20 m. Entities the broker has not modified for seven days are listed, not judged: an upsert
pipeline never deletes, so they are the leftovers of an earlier mapping or of a record the source
dropped (compare them with the source before deleting).

    python3 tests/praha/plausible.py \\
        https://api.dev.joinedcontext.com/api/endpoint/yw7epl4ai3dujdj4itt4uwvhwcz6tib2 ...

Only reads, anonymously: it sends no token. Exits 1 when a location, value, date, duplicate or
reference check fails.
"""

from __future__ import annotations

import argparse
import importlib.util
import re
import sys
from collections import Counter
from datetime import UTC, datetime, timedelta
from pathlib import Path

_spec = importlib.util.spec_from_file_location("praha_validate_sdm", Path(__file__).resolve().parent / "validate_sdm.py")
assert _spec and _spec.loader
_praha = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(_praha)
types = _praha.types
_checks = _praha.helsinki("plausible")
check, read_all, stamp = _checks.check, _checks.read_all, _checks.stamp

#: Prague (RÚIAN obec 554782) plus about 1 km, as (west, south, east, north) in WGS 84 degrees.
PRAGUE = (14.21, 49.93, 14.72, 50.19)

#: ROPID's ticket points (ids `pid-`) serve the whole PID network: Prague and the Central Bohemian
#: Region (railway stations such as Mělník and Všetaty), plus about 1 km.
PID = (13.38, 49.52, 15.55, 50.74)

#: Two entities of one name within 20 m are one real object only when these also agree: a school
#: register lists a school's kindergarten, after-school club and canteen as rows of one address, a
#: metro station's ticket machines stand at its entrances, a collection point holds one container
#: per kind of waste.
IDENTITY = {
    "PointOfInterest": ("serviceCategory", "facilityType", "address"),
    "WasteContainer": ("containerCode",),
}


def box(entity_id: str) -> tuple[float, float, float, float]:
    return PID if re.search(r":pid-[^:]+$", entity_id) else PRAGUE


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("endpoints", nargs="+", help="the public Endpoints of the praha space")
    parser.add_argument("--cap", type=int, default=50000, help="most entities read per type")
    args = parser.parse_args(argv)
    now = datetime.now(UTC)
    read = {kind: read_all(endpoint, kind, args.cap, "cs") for endpoint in args.endpoints for kind in types(endpoint)}
    known = {kind: {e["id"] for e in rows} for kind, rows in read.items() if rows}
    ok = True
    print("| type | entities | failing checks | stale > 7 d | newest modifiedAt |\n|---|---|---|---|---|")
    for kind, rows in read.items():
        found: Counter = Counter()
        for area in (PRAGUE, PID):
            found += check(kind, [e for e in rows if box(e["id"]) == area], known, now, area, IDENTITY)
        ok &= not found
        stale = sum(1 for e in rows if (stamp(e.get("modifiedAt")) or now) < now - timedelta(days=7))
        newest = max((e.get("modifiedAt", "") for e in rows), default="—")
        detail = "; ".join(f"{c} {d}: {n}" for (c, d), n in sorted(found.items())) or "—"
        print(f"| {kind} | {len(rows)} | {detail} | {stale} | {newest} |")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
