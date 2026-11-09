"""The Pipelines of an export (gitea/files/<owner>/<project>/pipelines/), with what the smoke
suite needs of each: its DataSource's type and its target Endpoint's slug and type."""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

import yaml

RESIDENT = {"mqtt", "nats", "websocket", "kafka", "amqp"}


@dataclass
class Pipeline:
    project: str
    name: str
    file: str
    spec: dict
    bento: bool
    source_type: str = ""        # the DataSource's spec.type
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
                sources[(d["metadata"].get("namespace") or f.parts[-3], d["metadata"]["name"])] = \
                    (d.get("spec") or {}).get("type", "")
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
            p.source_type = sources.get((project, ref), src.get("type", "")) or ""
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
