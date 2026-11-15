"""Do the Prague entities on an instance validate against their Smart Data Models (T-5270)?

The Helsinki check (tests/helsinki/validate_sdm.py) with Prague's types: up to `--limit` entities
of each type every public Endpoint serves, read as key-values in Czech, against the official
`schema.json` of the model `praha.linkml.yaml` names in `class_uri`. Prague has no Endpoint over
the whole space, so it takes several and reads each type from the Endpoint that serves it.

    python3 tests/praha/validate_sdm.py \\
        https://api.dev.joinedcontext.com/api/endpoint/yw7epl4ai3dujdj4itt4uwvhwcz6tib2 ...

Only reads, anonymously: it sends no token. Exits 1 when a type fails.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import sys
from pathlib import Path
from types import ModuleType
from typing import Any


def helsinki(name: str) -> ModuleType:
    """The Helsinki checker of that name, by its path: this directory holds a module of the same name."""
    path = Path(__file__).resolve().parents[1] / "helsinki" / f"{name}.py"
    spec = importlib.util.spec_from_file_location(f"helsinki_{name}", path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


_sdm = helsinki("validate_sdm")
EXTENSIONS, HELSINKI, SDM, failures, fetch, validator = (
    _sdm.EXTENSIONS, _sdm.MODELS, _sdm.SDM, _sdm.failures, _sdm.fetch, _sdm.validator)

#: Prague type -> (SDM subject repository, model), from praha.linkml.yaml `class_uri`.
MODELS = {
    kind: HELSINKI[kind]
    for kind in ("AirQualityObserved", "BikeHireDockingStation", "PointOfInterest", "OffStreetParking", "Vehicle", "AdministrativeArea")
} | {
    "WasteContainer": ("dataModel.WasteManagement", "WasteContainer"),
    "WasteContainerIsle": ("dataModel.WasteManagement", "WasteContainerIsle"),
}

#: Types with no Smart Data Model, and why the Prague model keeps its own.
EXCEPTIONS = {
    "BudgetLine": "the city's budget lines (jc:BudgetLine): SDM has no budget or finance model",
}


def types(endpoint: str) -> list[str]:
    """The types an Endpoint serves, by their short names."""
    answer = fetch(f"{endpoint.rstrip('/')}/ngsi-ld/v1/types")
    return [str(name).rsplit("/", 1)[-1] for name in answer.get("typeList", [])]


def schema(kind: str) -> dict[str, Any]:
    repo, model = MODELS[kind]
    return json.loads((EXTENSIONS / model / "schema.json").read_text()) if repo == "extension" else fetch(SDM.format(repo=repo, model=model))


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("endpoints", nargs="+", help="the public Endpoints of the praha space")
    parser.add_argument("--limit", type=int, default=50)
    args = parser.parse_args(argv)
    ok = True
    print("| Endpoint | type | entities | result | attribute: entities failing (message) |\n|---|---|---|---|---|")
    for endpoint in args.endpoints:
        slug = endpoint.rstrip("/").rsplit("/", 1)[-1]
        for kind in types(endpoint):
            if kind in EXCEPTIONS:
                print(f"| {slug} | {kind} | — | exception | {EXCEPTIONS[kind]} |")
                continue
            if kind not in MODELS:
                ok = False
                print(f"| {slug} | {kind} | — | FAIL | no Smart Data Model named for this type |")
                continue
            query = f"type={kind}&limit={args.limit}&options=keyValues&lang=cs"
            sample = fetch(f"{endpoint.rstrip('/')}/ngsi-ld/v1/entities?{query}")
            found = failures(validator(schema(kind)), sample)
            ok &= not found
            detail = "; ".join(f"{a}: {n} ({m})" for (a, m), n in sorted(found.items())) or "—"
            verdict = "FAIL" if found else "PASS" if sample else "no entities"
            print(f"| {slug} | {kind} | {len(sample)} | {verdict} | {detail.replace('|', '/')} |")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
