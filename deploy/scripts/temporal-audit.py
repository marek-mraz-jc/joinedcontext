#!/usr/bin/env python3
"""How each seed Pipeline creates history rows, per pipeline x entity type x attribute (T-4844).

For every attribute a Pipeline's mapping writes: the record mode its space's retention rule gives
it (DM-108), where its `observedAt` comes from (DM-104, DM-107), its `unitCode` and `datasetId`,
the type's history and the pipeline's lifetime, and a verdict:

- ok: rows match measurements.
- loses-history: a constant `observedAt` makes every write the same measurement (DM-104, DM-105),
  or `onChange` thins a count per interval (DM-110).
- unbounded-growth: `history: forever` keeps every row.
- duplicates-rows: a write without `observedAt` under `record: every` stores an unchanged value as
  a new row each time (DM-107 stamps it with its receive time, so DM-104 never sees a duplicate).
- stamped-on-receive: a measurement (a slot with a unit) without `observedAt`: its rows carry the
  time the broker received them, not the time measured.

The Portal draws the same table per space and pipeline from the same rules
(joinedcontext-portal src/temporal_audit.rs).

ponytail: the mapping is read as text, not run: an attribute built inside a named Bloblang map is
not seen. Running each seed's sample through Bento (tests/open_data.py) is the upgrade path.

Usage: temporal-audit.py [--seed DIR] [--out FILE]; prints the non-ok rows, writes the JSON report.
"""

import argparse
import functools
import importlib.util
import json
import re
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
SEED = ROOT / "components/context-gateway/seed"
_datacube = importlib.util.spec_from_file_location("datacube", ROOT / "scripts/open-data/datacube.py")
datacube = importlib.util.module_from_spec(_datacube)
_datacube.loader.exec_module(datacube)
KINDS = {"Property", "GeoProperty", "Relationship", "LanguageProperty", "ListProperty", "VocabProperty", "JsonProperty"}
GEOJSON = {"Point", "LineString", "Polygon", "MultiPoint", "MultiLineString", "MultiPolygon", "GeometryCollection", "Feature", "FeatureCollection"}
MEMBERS = {"type", "value", "object", "observedAt", "unitCode", "datasetId", "languageMap", "vocab", "json", "valueList", "coordinates"}
PLATFORM_HISTORY = "P28D"  # ANTARES_RETENTION_HISTORY_DEFAULT on the platform deployment (DM-97)
VERDICTS = ("ok", "loses-history", "unbounded-growth", "duplicates-rows", "stamped-on-receive")


def skip_string(text: str, i: int) -> int:
    """The index after the string literal that starts at text[i]."""
    quote, i = text[i], i + 1
    while i < len(text) and text[i] != quote:
        i += 2 if text[i] == "\\" else 1
    return i + 1


def strings_blanked(text: str) -> str:
    """The text with each string literal's inside replaced by spaces, so braces in it do not count."""
    out, i = list(text), 0
    while i < len(text):
        if text[i] == '"':
            end = skip_string(text, i)
            out[i + 1 : end - 1] = " " * max(0, end - i - 2)
            i = end
        elif text[i] == "#":
            end = text.find("\n", i)
            end = len(text) if end < 0 else end
            out[i:end] = " " * (end - i)
            i = end
        else:
            i += 1
    return "".join(out)


def enclosing(blank: str, at: int) -> tuple[int, int] | None:
    """The `{` before `at` that is not closed before it, and its matching `}`."""
    depth, start = 0, at
    while start >= 0:
        c = blank[start]
        if c == "}":
            depth += 1
        elif c == "{":
            if depth == 0:
                break
            depth -= 1
        start -= 1
    if start < 0:
        return None
    depth, end = 0, start
    while end < len(blank):
        if blank[end] == "{":
            depth += 1
        elif blank[end] == "}":
            depth -= 1
            if depth == 0:
                return start, end + 1
        end += 1
    return None


def member(text: str, blank: str, obj: tuple[int, int], name: str) -> str | None:
    """The expression of `name` at the object's own level, or None."""
    start, end = obj
    for m in re.finditer(rf'"{name}"\s*:\s*', text[start:end]):
        at = start + m.start()
        if enclosing(blank, at) != obj:
            continue
        i, depth = start + m.end(), 0
        j = i
        while j < end:
            c = blank[j]
            if c in "{[(":
                depth += 1
            elif c in "}])":
                if depth == 0:
                    break
                depth -= 1
            elif c in ",\n" and depth == 0:
                break
            j += 1
        return text[i:j].strip()
    return None


def observed_source(expr: str | None) -> str:
    """mapped, constant or received: where a row's observedAt comes from."""
    if expr is None or "now()" in expr:
        return "received"
    if re.fullmatch(r'"[^"]*"', expr):
        return "constant"
    return "mapped"


def scan(text: str, default_type: str | None) -> dict[tuple[str, str], dict]:
    """Each (entity type, attribute) the mapping writes, with its observedAt, unitCode and datasetId."""
    blank = strings_blanked(text)
    found: dict[tuple[str, str], dict] = {}
    entity_type = default_type
    for m in re.finditer(r'"type"\s*:\s*"([A-Za-z_][\w-]*)"', text):
        if blank[m.start()] != '"':  # inside another string literal or a comment
            continue
        name = m.group(1)
        if name in GEOJSON:
            continue
        if name not in KINDS:
            entity_type = name
            continue
        obj = enclosing(blank, m.start())
        if obj is None or entity_type is None:
            continue
        # A key in quotes, `root.<name> =`, or a key a variable names (`$attribute:`), kept as is.
        keys = [k.group(1) or k.group(2) or k.group(3)
                for k in re.finditer(r'(?:"([A-Za-z_@][\w@]*)"\s*:|root\.([A-Za-z_]\w*)\s*=|(\$[A-Za-z_]\w*)\s*:)',
                                     text[max(0, obj[0] - 600) : obj[0]])]
        keys = [k for k in keys if k not in MEMBERS and not k.startswith("_")]
        if not keys:
            continue
        attr = keys[-1]
        row = found.setdefault((entity_type, attr), {"observedAt": set(), "unitCode": set(), "datasetId": set(), "receivedWithUnit": False})
        source = observed_source(member(text, blank, obj, "observedAt"))
        row["observedAt"].add(source)
        row["receivedWithUnit"] |= source == "received" and member(text, blank, obj, "unitCode") is not None
        for field in ("unitCode", "datasetId"):
            value = member(text, blank, obj, field)
            if value:
                row[field].add(value.strip('"') if re.fullmatch(r'"[^"]*"', value) else "mapped")
    return found


def mapping_text(spec: dict, bento: str | None) -> str:
    """Every Bloblang the pipeline runs: its steps' (a datacube step's as jc-core renders it), its
    compute's and the bento.yaml beside it."""
    parts = [datacube.mapping(s["datacube"]) if s.get("kind") == "datacube" else s.get("bloblang", "")
             for s in spec.get("steps", []) or []]
    parts.append((spec.get("compute") or {}).get("bloblang") or "")
    if bento:
        parts.append(bento)
    return "\n".join(parts)


def recording(rule: dict | None, attr: str) -> str:
    if rule is None:
        return "every"
    own = (rule.get("attributes") or {}).get(attr) or {}
    return own.get("record") or rule.get("record") or "every"


def verdict(*, history: str, record: str, observed: set[str], measured: bool, per_interval: bool) -> str:
    """The first rule that holds, in the order the module docstring lists them."""
    if history == "off":
        return "ok"
    if "constant" in observed or (record == "onChange" and per_interval):
        return "loses-history"
    if history == "forever":
        return "unbounded-growth"
    if "received" in observed and record == "every":
        return "duplicates-rows"
    if "received" in observed and measured:
        return "stamped-on-receive"
    return "ok"


def load_folder(folder: Path) -> list[tuple[str, dict, Path]]:
    """(repository path, document, file) of each seed file the folder's index names."""
    index = yaml.safe_load((folder / "index.yaml").read_text()) or {}
    docs = []
    for name, path in index.items():
        file = folder / name
        if file.suffix not in (".yaml", ".yml") or not file.exists():
            continue
        try:
            for doc in yaml.safe_load_all(file.read_text()):
                docs.append((path, doc, file))
        except yaml.YAMLError:
            continue
    return docs


@functools.cache
def model_slots(folder: Path, project: str, space: str, model: str | None) -> dict:
    """slot name -> (ucum_code, per interval) of the space's LinkML model."""
    if not model:
        return {}
    index = yaml.safe_load((folder / "index.yaml").read_text()) or {}
    want = f"projects/{project}/spaces/{space}/datamodels/{model}.linkml.yaml"
    for name, path in index.items():
        if path == want and (folder / name).exists():
            linkml = yaml.safe_load((folder / name).read_text()) or {}
            slots = dict(linkml.get("slots") or {})
            for cls in (linkml.get("classes") or {}).values():
                slots.update((cls or {}).get("attributes") or {})
            return {
                n: ((s or {}).get("unit") or {}).get("ucum_code") if isinstance((s or {}).get("unit"), dict) else None
                for n, s in slots.items()
            } | {f"{n}#perInterval": ((s or {}).get("annotations") or {}).get("quantity") in ("perInterval", {"value": "perInterval"})
                 for n, s in slots.items()}
    return {}


def mode_words(mode: dict) -> str:
    """One PL-80 end-time mode in words: `from` a field (`plus`, `notAfter`), `rolling` or `at`."""
    if "from" in mode:
        return " ".join([f"from {mode['from']}"] + [f"{k} {mode[k]}" for k in ("plus", "notAfter") if mode.get(k)])
    if "rolling" in mode:
        return f"rolling {mode['rolling']}"
    if "at" in mode:
        return f"at {mode['at']}"
    return "unknown mode"


def lifetime_of(spec: dict) -> list[str]:
    """What ends a pipeline's entities: each `spec.lifetime` entry (PL-80) and the `expiry` window (PL-64)."""
    out = set()
    for lt in spec.get("lifetime") or []:
        if lt.get("entity"):
            out.add(f"{lt.get('type')}: {mode_words(lt['entity'])}")
        for attrs in lt.get("attributes") or []:
            out.add(f"{lt.get('type')}.{','.join(attrs.get('names') or [])}: {mode_words(attrs)}")
    if spec.get("expiry"):
        out.add(f"expiry {(spec['expiry'] or {}).get('after')}")
    return sorted(out)


def audit_folder(folder: Path) -> list[dict]:
    docs = load_folder(folder)
    spaces = {(d["metadata"]["namespace"], d["metadata"]["name"]): d for _, d, _ in docs if isinstance(d, dict) and d.get("kind") == "ContextSpace"}
    endpoints = {(d["metadata"]["namespace"], d["metadata"]["name"]): d["spec"].get("contextSpaceRef") for _, d, _ in docs
                 if isinstance(d, dict) and d.get("kind") == "Endpoint"}
    by_path = {path: file for path, _, file in docs}
    rows = []
    for path, doc, _ in docs:
        if not isinstance(doc, dict) or doc.get("kind") != "Pipeline":
            continue
        project, name, spec = doc["metadata"]["namespace"], doc["metadata"]["name"], doc.get("spec") or {}
        bento_file = by_path.get(path.rsplit("/", 1)[0] + "/bento.yaml")
        outputs = spec["outputs"] if "outputs" in spec else [{"targetEndpoint": spec.get("targetEndpoint"), **(spec.get("output") or {})}]
        targets = []
        for out in outputs:
            endpoint = (out.get("targetEndpoint") or "").rsplit(":", 1)[-1]
            space = endpoints.get((project, endpoint))
            if space:
                targets.append((space, out.get("type")))
        if not targets:
            continue
        lifetime = lifetime_of(spec)
        text = mapping_text(spec, bento_file.read_text() if bento_file else None)
        if '"expiresAt"' in text:
            lifetime.append("expiresAt in mapping")
        default_type = targets[0][1] if len({t for _, t in targets}) == 1 else None
        space_name = targets[0][0]
        space_doc = spaces.get((project, space_name)) or {}
        rules = {r.get("entityType"): r for r in (space_doc.get("spec") or {}).get("retention") or []}
        model = ((space_doc.get("spec") or {}).get("dataModelRef") or {}).get("name")
        slots = model_slots(folder, project, space_name, model)
        for (entity_type, attr), seen in sorted(scan(text, default_type).items()):
            rule = rules.get(entity_type)
            history = (rule or {}).get("history") or (rules.get("*") or {}).get("history") or PLATFORM_HISTORY
            record = "none" if history == "off" else recording(rule, attr)
            rows.append({
                "project": project,
                "space": space_name,
                "pipeline": name,
                "entityType": entity_type,
                "attribute": attr,
                "record": record,
                "observedAt": sorted(seen["observedAt"]),
                "unitCode": sorted(seen["unitCode"]) or ([f"model {slots[attr]}"] if slots.get(attr) else []),
                "datasetId": sorted(seen["datasetId"]),
                "history": history,
                "lifetime": lifetime,
                "verdict": verdict(history=history, record=record, observed=seen["observedAt"],
                                   measured=seen["receivedWithUnit"] or bool(slots.get(attr)),
                                   per_interval=bool(slots.get(f"{attr}#perInterval"))),
            })
    return rows


def audit(seed: Path) -> list[dict]:
    return [row for folder in sorted(p.parent for p in seed.glob("*/index.yaml")) for row in audit_folder(folder)]


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--seed", type=Path, default=SEED)
    parser.add_argument("--out", type=Path, default=Path("temporal-audit.json"))
    args = parser.parse_args(argv)
    rows = audit(args.seed)
    counts = {v: sum(r["verdict"] == v for r in rows) for v in VERDICTS}
    args.out.write_text(json.dumps({"rows": rows, "verdicts": counts}, indent=1, ensure_ascii=False) + "\n")
    for (project, space, pipeline), lifetime in sorted({(r["project"], r["space"], r["pipeline"]): r["lifetime"] for r in rows}.items()):
        print(f'lifetime {project}/{space} {pipeline}: {"; ".join(lifetime) or "none"}')
    for r in rows:
        if r["verdict"] != "ok":
            print(f'{r["verdict"]:20} {r["project"]}/{r["pipeline"]} {r["entityType"]}.{r["attribute"]} '
                  f'record={r["record"]} observedAt={",".join(r["observedAt"])} history={r["history"]}')
    print(json.dumps(counts), file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
