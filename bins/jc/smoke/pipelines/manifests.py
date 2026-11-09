"""The Pipelines of an export (gitea/files/<owner>/<project>/pipelines/), with what the smoke
suite needs of each: its DataSource's type and its target Endpoint's slug and type."""

from __future__ import annotations

from dataclasses import dataclass, field
from pathlib import Path

import yaml

RESIDENT = {"mqtt", "nats", "websocket", "kafka", "amqp"}
# Sources the Portal runs and deletes whatever their clock (run_queue.rs is_resident: a poll, a
# feed and a read that ends): without a period or schedule such a pipeline runs once after apply
# (Clock::Once) and its stream is in the runner only while that run lasts.
RUN_AND_DELETE = {"http", "gtfs-rt"}


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
    slug: str = ""               # the target Endpoint's slug
    out_type: str = ""
    secret_refs: list = field(default_factory=list)
    # The runner variables its DataSource's credentials reach the runner as (the Portal's
    # pipeline_references: an explicit envVar, else jc-core env_var_of DS_{SOURCE}_{KEY}).
    source_vars: list = field(default_factory=list)

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

    @property
    def runs_once(self) -> bool:
        """No clock and a source the Portal runs and deletes: one run after apply, no stream held."""
        return not self.scheduled and self.source_type in RUN_AND_DELETE


def _env_part(text: str) -> str:
    return "".join(c.upper() if c.isascii() and c.isalnum() else "_" for c in str(text))


def source_vars(name: str, spec: dict) -> list:
    """The runner variables a DataSource's credentials reach the runner as, as the Portal names
    them (jc-core DataSourceSpec::secret_refs and env_var_of): `spec.secrets`, an http source's
    `authorization.headerRef`, an mqtt `passwordRef`, a TLS `caCertRef`; a typed source's
    `spec.secrets` entry only under its own envVar."""
    typed = spec.get("type") in ("http", "mqtt", "websocket", "gtfs-rt")
    refs = [r for r in spec.get("secrets") or [] if not typed or (isinstance(r, dict) and r.get("envVar"))]
    refs.append(((spec.get("http") or {}).get("authorization") or {}).get("headerRef"))
    refs.append((spec.get("mqtt") or {}).get("passwordRef"))
    refs.append((spec.get("tls") or {}).get("caCertRef"))
    out = []
    for ref in refs:
        if not isinstance(ref, dict):
            continue
        var = ref.get("envVar") or (f"DS_{_env_part(name)}_{_env_part(ref['key'])}" if ref.get("key")
                                    else f"DS_{_env_part(name)}")
        out.append(var)
    return out


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
                    d.get("spec") or {}
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
            source = sources.get((project, ref))
            p.source_type = (source.get("type", "") if source is not None else src.get("type", "")) or ""
            p.source_vars = source_vars(ref, source) if source is not None else []
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
