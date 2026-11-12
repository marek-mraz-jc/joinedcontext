"""What the export says each Endpoint must answer: its manifests (Endpoint, Policy, ContextSpace
from gitea/files), the Antares tenant of its space, the types and counts its Policies grant the
public, and one entity and one history series per type (antares/*.jsonl, indexed once into a
cache keyed by the export's exportedAt)."""

from __future__ import annotations

import datetime as dt
import hashlib
import json
import re
from dataclasses import dataclass, field
from pathlib import Path

import yaml  # PyYAML ships with /opt/jc-py (and every python the sandbox runs)

READ_OPS = {"retrieveOps", "queryEntity", "retrieveEntity", "queryBatch"}
RETRIEVE_OPS = {"retrieveOps", "retrieveEntity"}
# CIM 009 Table 4.20-2: retrieveOps is retrieveEntity and queryEntity, no history (Antares
# operations.rs RETRIEVE; prod's describe_access lists no temporal action for it).
TEMPORAL_OPS = {"queryTemporal", "retrieveTemporal"}
DEFAULT_CONTEXT = "https://uri.etsi.org/ngsi-ld/default-context/"
LOCATION = "https://uri.etsi.org/ngsi-ld/location"
CLOSED = {"organization", "project-list"}


def short_type(t: str) -> str:
    """`https://uri.etsi.org/ngsi-ld/default-context/Vehicle` and `Vehicle` are one type."""
    if t.startswith(DEFAULT_CONTEXT):
        return t[len(DEFAULT_CONTEXT):]
    return re.split(r"[/#:]", t)[-1] if ("/" in t or "#" in t) else t


def urn_segment(project: str, space: str, pin: str | None) -> str:
    """jc-core kinds::urn_segment: the Tenant of a space."""
    return pin or f"{project}-{space}"


@dataclass
class Selector:
    type: str            # short type name, "*" for every type
    pattern: str | None  # idPattern


@dataclass
class Grant:
    """One bound Policy, as far as the expectations need it."""
    name: str
    assignee: dict
    operations: set
    selectors: list      # [Selector]; empty = every type
    narrowed: str = ""   # why counts cannot be compared (q, scopeQ, validity, attribute lists)

    def reads(self) -> bool:
        return bool(self.operations & READ_OPS)

    def temporal(self) -> bool:
        return bool(self.operations & TEMPORAL_OPS)

    def public(self) -> bool:
        return self.assignee.get("kind") == "role" and self.assignee.get("id") == "public"

    def creates(self) -> bool:
        """Grants Create Entity (POST /entities): CIM 009 Table 4.20-2 puts it in redirectionOps."""
        return bool(self.operations & {"createEntity", "redirectionOps"})

    def writes(self) -> bool:
        return bool(self.operations - READ_OPS - TEMPORAL_OPS
                    - {o for o in self.operations if o.startswith("retrieve")}
                    - {"createSubscription", "updateSubscription", "deleteSubscription"})


@dataclass
class Endpoint:
    project: str
    name: str
    space: str
    tenant: str
    slug: str
    audience: str
    representations: list
    grants: list                       # bound Policies [Grant]
    ical: list = field(default_factory=list)      # names of representations.ical outputs
    ical_types: dict = field(default_factory=dict)  # ical output name -> the types its sources read
    datasets: list = field(default_factory=list)  # CKAN dataset names (public catalogue)
    slices: list = field(default_factory=list)    # spec.catalog.datasets names
    file: str = ""
    caller_role: bool = False  # every admitted caller holds endpoint:{project}/{name} (AP-96)
    # short types some Policy of the space grants a write to ("*" = every type): a Pipeline or
    # workload may add rows of them while a suite reads, bound to this Endpoint or not
    written: set = field(default_factory=set)

    @property
    def closed(self) -> bool:
        return self.audience in CLOSED

    def own_role(self, grant: "Grant") -> bool:
        """With callerRole every caller it admits holds `endpoint:{project}/{name}` here."""
        return self.caller_role and grant.assignee.get("kind") == "role" \
            and grant.assignee.get("id") == f"endpoint:{self.project}/{self.name}"

    def public_grants(self) -> list:
        return [g for g in self.grants if (g.public() or self.own_role(g)) and g.reads()]

    def selectors(self, grants) -> list | None:
        """The selectors a set of grants opens; None = every type of the tenant."""
        out = []
        for g in grants:
            if not g.selectors:
                return None
            out += g.selectors
        return out


def _load_yaml(path: Path):
    try:
        docs = list(yaml.safe_load_all(path.read_text()))
    except (yaml.YAMLError, UnicodeDecodeError):
        return []
    return [d for d in docs if isinstance(d, dict)]


def _grant(name: str, spec: dict) -> Grant:
    selectors, narrowed = [], []
    for info in spec.get("information") or []:
        # propertyNames/relationshipNames narrow the attributes an entity shows, never which
        # entities a read finds: counts and ids stay comparable.
        for e in info.get("entities") or []:
            selectors.append(Selector(short_type(str(e.get("type", "*"))), e.get("idPattern")))
    for k in ("q", "scopeQ", "validity"):
        if spec.get(k):
            narrowed.append(k)
    return Grant(name, spec.get("assignee") or {}, set(spec.get("operations") or []), selectors,
                 ", ".join(sorted(set(narrowed))))


def own_grant(spec: dict, project: str, endpoint: str) -> bool:
    """Portal views.rs own_grant: assignee role `endpoint:{project}/{endpoint}` or `.../{role}`."""
    a = spec.get("assignee") or {}
    own = f"endpoint:{project}/{endpoint}"
    rest = str(a.get("id", ""))[len(own):] if str(a.get("id", "")).startswith(own) else None
    return a.get("kind") == "role" and rest is not None and (
        rest == "" or (rest.startswith("/") and len(rest) > 1 and "/" not in rest[1:]))


def load_endpoints(files: Path) -> list[Endpoint]:
    """Every Endpoint under gitea/files/<owner>/<project>/spaces/<space>/endpoints/*.yaml."""
    spaces, policies = {}, {}
    for f in files.glob("*/*/spaces/*/space.yaml"):
        for d in _load_yaml(f):
            if d.get("kind") == "ContextSpace":
                project = d["metadata"].get("namespace") or f.parts[-4]
                spaces[(project, d["metadata"]["name"])] = d.get("spec") or {}
    for f in files.glob("*/*/spaces/*/policies/*.yaml"):
        for d in _load_yaml(f):
            if d.get("kind") == "Policy":
                s = d.get("spec") or {}
                project = d["metadata"].get("namespace") or f.parts[-5]
                ref = s.get("contextSpaceRef")
                space = ref.get("name") if isinstance(ref, dict) else ref or f.parts[-3]
                policies.setdefault((project, space), []).append((d["metadata"]["name"], s))
    apps = set()
    for f in list(files.glob("*/*/apps/*/app.yaml")) + list(files.glob("*/*/apps/*.yaml")):
        for d in _load_yaml(f):
            if d.get("kind") == "App":
                apps.add((d["metadata"].get("namespace") or f.relative_to(files).parts[1], d["metadata"]["name"]))
    written = {k: {s.type for g in gs if g.writes() for s in g.selectors or [Selector("*", None)]}
               for k, gs in ((k, [_grant(n, p) for n, p in v]) for k, v in policies.items())}
    out = []
    for f in sorted(files.glob("*/*/spaces/*/endpoints/*.yaml")):
        for d in _load_yaml(f):
            if d.get("kind") != "Endpoint":
                continue
            s, meta = d.get("spec") or {}, d.get("metadata") or {}
            project = meta.get("namespace") or f.parts[-5]
            ref = s.get("contextSpaceRef")
            space = ref.get("name") if isinstance(ref, dict) else ref
            tenant = urn_segment(project, space, (spaces.get((project, space)) or {}).get("urnSegment"))
            refs = s.get("policyRefs") or ([s["policyRef"]] if s.get("policyRef") else None)
            bound = []
            for pname, pspec in policies.get((project, space), []):
                if refs is not None and not any(
                        str(r).split(":")[-2:] == [tenant, pname] for r in refs):
                    continue
                # An App's Endpoint keeps only the App's own grants (Portal views.rs bound(), AP-139).
                if meta["name"].startswith("app-") and (project, meta["name"][4:]) in apps \
                        and not own_grant(pspec, project, meta["name"]):
                    continue
                bound.append(_grant(pname, pspec))
            catalog = s.get("catalog") or {}
            slices = [x["name"] for x in catalog.get("datasets") or [] if isinstance(x, dict) and x.get("name")]
            ckan = (s.get("publish") or {}).get("ckan")
            datasets = (slices or [ckan.get("name") or meta["name"]]) if isinstance(ckan, dict) else []
            ical = {o["name"]: [x.get("type") for x in o.get("sources") or [] if isinstance(x, dict)]
                    for o in (s.get("representations") or {}).get("ical") or [] if isinstance(o, dict) and o.get("name")}
            out.append(Endpoint(
                project=project, name=meta["name"], space=space, tenant=tenant, slug=str(s.get("slug")),
                audience=str(s.get("audience")), representations=list(s.get("enabledRepresentations") or []),
                grants=bound, ical=list(ical), ical_types=ical,
                datasets=datasets, slices=slices, file=str(f.relative_to(files)),
                caller_role=bool(s.get("callerRole")), written=written.get((project, space), set())))
    return out


# --------------------------------------------------------------------------------------------
# Antares index

_HEAD = re.compile(r'^\{"tenant": "([^"]+)", "entity": \{"id": "((?:[^"\\]|\\.)*)", "type": "([^"]+)"')


def patterns_needed(endpoints) -> dict:
    """(tenant, short type) -> sorted idPatterns any public grant names."""
    need: dict = {}
    for e in endpoints:
        for g in e.grants:
            for sel in g.selectors:
                if sel.pattern:
                    need.setdefault((e.tenant, sel.type), set()).add(sel.pattern)
    return {k: sorted(v) for k, v in need.items()}


_DURATION = re.compile(r"^P(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?)?$")
# A `current` retention rule at most this long removes the imported rows during a run (the
# rehearsal keeps one day): the rows of its type are as transient as rows carrying expiresAt.
TRANSIENT = dt.timedelta(days=1)


def duration(s) -> dt.timedelta | None:
    """An ISO 8601 day-time duration (P1D, PT5M, PT1H30M); None for any other form."""
    m = _DURATION.match(str(s or ""))
    if not m or not any(m.groups()):
        return None
    d, h, mi, se = (int(x or 0) for x in m.groups())
    return dt.timedelta(days=d, hours=h, minutes=mi, seconds=se)


def transient_types(files: Path) -> set:
    """(tenant, short type) of every space retention rule whose `current` is at most TRANSIENT:
    the broker deletes such an Entity once it has not been modified for that long (T-3900), so a
    bundle's rows of the type are gone minutes after the import (T-6833: helsinki's Vehicle rule
    PT5M emptied helsinki-transport before the mcp suite read it)."""
    out = set()
    for f in files.glob("*/*/spaces/*/space.yaml"):
        for d in _load_yaml(f):
            if d.get("kind") != "ContextSpace":
                continue
            spec = d.get("spec") or {}
            tenant = urn_segment(d["metadata"].get("namespace") or f.parts[-4], d["metadata"]["name"],
                                 spec.get("urnSegment"))
            for rule in spec.get("retention") or []:
                keep = duration(rule.get("current")) if isinstance(rule, dict) else None
                if keep is not None and keep <= TRANSIENT and rule.get("entityType"):
                    out.add((tenant, short_type(str(rule["entityType"]))))
    return out


def build_index(export: Path, patterns: dict) -> dict:
    """One pass over entities.jsonl and temporal.jsonl.

    tenants[tenant][short type] = {"iri", "count", "sample", "location", "expires",
                                   "patterns": {pattern: count}}
    temporal[tenant][short type] = {"id", "attr", "newest", "oldest", "rows"} of the id with the
    newest instance, plus "pseries": {pattern: {"id", "newest"}} of the newest id each idPattern
    a grant names matches (the endpoints suite), and "psample": {pattern: {"id", "attr", "newest"}}
    of the same (the mcp suite)."""
    compiled = {k: [(p, re.compile(p)) for p in v] for k, v in patterns.items()}
    tenants: dict = {}
    with (export / "antares" / "entities.jsonl").open("rb") as fh:
        for raw in fh:
            line = raw.decode("utf-8", errors="replace")
            m = _HEAD.match(line)
            if m:
                tenant, eid, typ = m.group(1), json.loads(f'"{m.group(2)}"'), m.group(3)
            else:
                try:
                    row = json.loads(line)
                except ValueError:
                    continue
                tenant, ent = row["tenant"], row["entity"]
                eid, typ = ent["id"], ent["type"] if isinstance(ent["type"], str) else ent["type"][0]
            st = short_type(typ)
            slot = tenants.setdefault(tenant, {}).setdefault(
                st, {"iri": typ, "count": 0, "sample": eid, "location": False, "expires": False,
                     "patterns": {}})
            slot["count"] += 1
            if not slot["location"] and f'"{LOCATION}"' in line:
                slot["location"], slot["sample"] = True, eid
            if "expiresAt" in line:
                slot["expires"] = True
            for p, rx in compiled.get((tenant, st), ()):
                if rx.search(eid):
                    slot["patterns"][p] = slot["patterns"].get(p, 0) + 1
                    slot.setdefault("psample", {}).setdefault(p, eid)
    for tenant, st in transient_types(export / "gitea" / "files"):
        if st in tenants.get(tenant, {}):
            tenants[tenant][st]["expires"] = True
    temporal: dict = {}
    with (export / "antares" / "temporal.jsonl").open("rb") as fh:
        for raw in fh:
            try:
                row = json.loads(raw)
            except ValueError:
                continue
            at = (row.get("instance") or {}).get("observedAt")
            if not at:
                continue
            st = short_type(row["type"])
            slot = temporal.setdefault(row["tenant"], {}).setdefault(
                st, {"id": row["id"], "attr": row["attr"], "newest": at, "oldest": at, "rows": 0, "ids": 0})
            slot["rows"] += 1
            slot["oldest"] = min(slot["oldest"], at)
            if at > slot["newest"]:
                slot.update(id=row["id"], attr=row["attr"], newest=at)
            for p, rx in compiled.get((row["tenant"], st), ()):
                if rx.search(row["id"]):
                    ps = slot.setdefault("pseries", {}).setdefault(p, {"id": row["id"], "newest": at})
                    if at > ps["newest"]:
                        ps.update(id=row["id"], newest=at)
                    best = slot.setdefault("psample", {}).get(p)
                    if best is None or at > best["newest"]:
                        slot["psample"][p] = {"id": row["id"], "attr": row["attr"], "newest": at}
    for per in temporal.values():
        for slot in per.values():
            slot.pop("ids", None)
    return {"tenants": tenants, "temporal": temporal}


def load_index(export: Path, endpoints, cache_dir: Path, log=print) -> dict:
    manifest = json.loads((export / "manifest.json").read_text())
    patterns = patterns_needed(endpoints)
    key = hashlib.sha256(json.dumps(["index-v5", manifest.get("exportedAt"), str(export.resolve()),
                                     sorted((list(k), v) for k, v in patterns.items())]).encode()).hexdigest()[:16]
    cache = cache_dir / f"index-{key}.json"
    if cache.exists():
        return json.loads(cache.read_text())
    log(f"indexing {export}/antares (once; cached as {cache})")
    index = build_index(export, patterns)
    index["exportedAt"] = manifest.get("exportedAt")
    index["synthetic"] = bool(manifest.get("synthetic"))
    index["cutoff"] = manifest.get("temporalCutoff") or (manifest.get("counts", {}).get("antares") or {}).get("cutoff")
    cache_dir.mkdir(parents=True, exist_ok=True)
    tmp = cache.with_suffix(".tmp")
    tmp.write_text(json.dumps(index))
    tmp.replace(cache)
    return index


def expected_types(e: Endpoint, grants, tenant_types: dict) -> tuple[dict, str]:
    """{short type: expected count or None (not comparable)} the grants open on the tenant,
    only types the export holds rows of; plus why counts are not comparable, if so."""
    sels = e.selectors(grants)
    narrowed = "; ".join(sorted({g.narrowed for g in grants if g.narrowed}))
    out = {}
    for st, slot in tenant_types.items():
        if sels is None:
            out[st] = slot["count"]
            continue
        mine = [s for s in sels if s.type in (st, "*")]
        if not mine:
            continue
        if any(s.pattern is None for s in mine):
            out[st] = slot["count"]
        elif len(mine) == 1:
            out[st] = slot["patterns"].get(mine[0].pattern, 0)
        else:
            out[st] = None  # several patterns overlap: a union the index does not hold
        if out[st] == 0:
            del out[st]
    if narrowed:
        out = {k: None for k in out}
    return out, narrowed


def seeded_ids(t: str) -> str:
    """The idPattern of the rows a synthetic bundle holds of `t` (`rehearsal.py synth` names them
    urn:ngsi-ld:<type>:rehearsal-<n>). On a rehearsal of a synthetic bundle the Portal runs the
    imported Pipelines against their live sources, so a type grows past the bundle by rows those
    Pipelines wrote; the seeded rows still count exactly."""
    return f"^urn:ngsi-ld:{re.escape(t)}:rehearsal-[0-9]+$"


def parse_time(s: str) -> dt.datetime:
    return dt.datetime.fromisoformat(s.replace("Z", "+00:00"))
