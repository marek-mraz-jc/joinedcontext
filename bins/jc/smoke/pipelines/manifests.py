"""The Pipelines of an export (gitea/files/<owner>/<project>/pipelines/), with what the smoke
suite needs of each: its DataSource's type and its target Endpoint's slug and type."""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

import yaml

RESIDENT = {"mqtt", "nats", "websocket", "kafka", "amqp"}


# Each cron field's range, as prod's Portal reads it (joinedcontext-portal src/reconciler/run_queue.rs
# Cron::parse: minute 0-59, hour 0-23, day 1-31, month 1-12, weekday 0-7).
CRON_FIELDS = (("minute", 0, 59), ("hour", 0, 23), ("day", 1, 31), ("month", 1, 12), ("weekday", 0, 7))


def cron_out_of_range(schedule: str) -> str | None:
    """The first field of a five-field `schedule` with a number outside its range, which prod's
    Portal refuses as jc's does (so the refusal is prod's behaviour, not a jc defect); None when
    every number is in range or the text is no five fields."""
    fields = str(schedule or "").split()
    if len(fields) != 5:
        return None
    for text, (name, low, high) in zip(fields, CRON_FIELDS):
        for part in text.split(","):
            for n in part.split("/")[0].split("-"):
                if n.isdigit() and not low <= int(n) <= high:
                    return name
    return None


@dataclass
class Pipeline:
    project: str
    name: str
    file: str
    spec: dict
    bento: bool
    source_type: str = ""        # the DataSource's spec.type
    source_url: str = ""         # the DataSource's spec.http.url, when it has one
    slug: str = ""               # the target Endpoint's slug
    out_type: str = ""
    secret_refs: list = field(default_factory=list)

    @property
    def key(self) -> str:
        return f"{self.project}/{self.name}"

    @property
    def resident(self) -> bool:
        return self.source_type in RESIDENT

    @property
    def scheduled(self) -> bool:
        s = self.spec
        return bool(s.get("schedule") or s.get("period")) or s.get("class") == "scheduled"


def _docs(path: Path):
    try:
        return [d for d in yaml.safe_load_all(path.read_text()) if isinstance(d, dict)]
    except (yaml.YAMLError, UnicodeDecodeError, OSError):
        return []


def load_pipelines(files: Path) -> list[Pipeline]:
    """Every Pipeline under gitea/files/<owner>/<project>/, with its DataSource's type and the
    slug of its target Endpoint (urn:ngsi-ld:Endpoint:<orgDomain>:<space>:<name>)."""
    endpoints, sources = {}, {}
    for f in files.glob("*/*/spaces/*/endpoints/*.yaml"):
        for d in _docs(f):
            if d.get("kind") == "Endpoint":
                spec = d.get("spec") or {}
                endpoints.setdefault((f.parts[-5], d["metadata"]["name"]), []).append(
                    (f.parts[-3], spec.get("slug", "")))
    for f in files.glob("*/*/**/*.yaml"):
        if "/datasources/" not in str(f) and "/pipelines/" not in str(f):
            continue
        for d in _docs(f):
            if d.get("kind") == "DataSource":
                spec = d.get("spec") or {}
                sources[(d["metadata"].get("namespace") or f.parts[-3], d["metadata"]["name"])] = \
                    (spec.get("type", ""), str((spec.get("http") or {}).get("url") or ""))
    out = []
    for f in sorted(files.glob("*/*/pipelines/**/*.yaml")):
        for d in _docs(f):
            if d.get("kind") != "Pipeline":
                continue
            meta, spec = d.get("metadata") or {}, d.get("spec") or {}
            project = meta.get("namespace") or f.relative_to(files).parts[1]
            p = Pipeline(project, meta["name"], str(f.relative_to(files)), spec,
                         (f.parent / "bento.yaml").exists())
            outs = spec.get("outputs") or [spec.get("output") or {}]
            first_out = outs[0] if outs and isinstance(outs[0], dict) else {}
            src = spec.get("source") or (spec.get("sources") or [{}])[0] or {}
            ref = src.get("dataSourceRef") or {}
            ref = ref.get("name") if isinstance(ref, dict) else ref
            p.source_type, p.source_url = sources.get((project, ref), (src.get("type", "") or "", ""))
            urn = str(spec.get("targetEndpoint") or first_out.get("targetEndpoint") or "")
            parts = urn.split(":")
            if len(parts) >= 4:
                # the URN's space is the space name or its urnSegment ({project}-{space})
                found = endpoints.get((project, parts[-1]), [])
                p.slug = next((slug for space, slug in found
                               if len(parts) >= 6 and parts[-2] in (space, f"{project}-{space}")),
                              found[0][1] if found else "")
            p.out_type = str((spec.get("output") or {}).get("type") or first_out.get("type") or "")
            p.secret_refs = [s.get("name") for s in spec.get("secretRefs") or []]
            out.append(p)
    return out
