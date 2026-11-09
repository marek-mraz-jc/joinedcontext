"""What the charts-apps suite expects, read from an export bundle and the deployment's Grafana files.

Dashboards come in two shapes:
- the deployment's provisioning files (`rehearsal.py dashboards`): `<project>.dashboard.json` (an
  /api/dashboards/db body), `<project>.datasource.<name>.json`, `<project>.folder.json`, and plain
  platform models (`portal.json`) that live in daiya organization 1;
- the export's `grafana/dashboards/org<N>-<uid>.json` (the source Grafana's own dashboards, keyed by
  its organization number). Organization 1 is the platform's; another number is the project whose
  `project-<p>` dashboard sits in the same organization.

The Portal reaches organization 1 as project `org` (ORG_NAMESPACE)."""

from __future__ import annotations

import datetime as dt
import json
import re
from dataclasses import dataclass, field
from pathlib import Path

ORG_PROJECT = "org"
EXPORT_NAME = re.compile(r"org(\d+)-(.+)\.json")
PROMETHEUS = "prometheus"


@dataclass
class Panel:
    id: int
    title: str
    type: str
    ds_type: str
    ds_uid: str
    targets: list

    def query_free(self) -> bool:
        return self.type in ("row", "text", "news", "dashlist") or not self.targets


@dataclass
class Dashboard:
    project: str
    uid: str
    title: str
    panels: list
    sources: list = field(default_factory=list)  # file names it came from
    problem: str = ""  # why its project could not be told


@dataclass
class Datasource:
    project: str
    uid: str
    type: str
    slug: str


def _kind(name: str) -> str:
    if EXPORT_NAME.fullmatch(name):
        return "export"
    for kind in ("folder", "datasource", "dashboard"):
        if f".{kind}." in name or name.endswith(f".{kind}.json"):
            return kind
    return "model"


def _ds(value, default_uid: str = "") -> tuple[str, str]:
    if isinstance(value, dict):
        return str(value.get("type") or ""), str(value.get("uid") or default_uid)
    if isinstance(value, str):  # an old-style datasource name
        return ("prometheus" if value.lower().startswith("prometheus") else ""), value
    return "", default_uid


def panels_of(model: dict) -> list[Panel]:
    out = []
    for p in model.get("panels") or []:
        for q in [p, *(p.get("panels") or [])]:
            if "id" not in q:
                continue
            ptype, puid = _ds(q.get("datasource"))
            targets = q.get("targets") or []
            if not puid and targets:  # a panel taking each target's datasource
                ptype, puid = _ds(targets[0].get("datasource"))
            out.append(Panel(int(q["id"]), str(q.get("title") or ""), str(q.get("type") or ""),
                             ptype, puid, targets))
    return out


def load_dashboards(dirs: list[Path]) -> tuple[list[Dashboard], list[Datasource]]:
    """Every dashboard of the given dirs once per (project, uid), and the files' datasources."""
    by_key: dict[tuple[str, str], Dashboard] = {}
    datasources = []
    export_files = []
    for d in dirs:
        for f in sorted(Path(d).glob("*.json")):
            kind = _kind(f.name)
            if kind == "folder":
                continue
            doc = json.loads(f.read_text())
            if kind == "export":
                export_files.append((f, doc))
                continue
            if kind == "datasource":
                slug = urlparse_slug(doc.get("url", ""))
                datasources.append(Datasource(f.name.split(".", 1)[0], doc.get("uid", ""), doc.get("type", ""), slug))
                continue
            project = f.name.split(".", 1)[0] if kind == "dashboard" else ORG_PROJECT
            model = doc.get("dashboard", doc) if kind == "dashboard" else doc
            _add(by_key, project, model, f.name)
    # the source Grafana's organizations: 1 is the platform's, another holds `project-<p>`
    org_project = {"1": ORG_PROJECT}
    for f, doc in export_files:
        n, uid = EXPORT_NAME.fullmatch(f.name).groups()
        if uid.startswith("project-") and n != "1":
            org_project[n] = uid[len("project-"):]
    for f, doc in export_files:
        n, _ = EXPORT_NAME.fullmatch(f.name).groups()
        model = doc.get("dashboard", doc)
        project = org_project.get(n)
        dash = _add(by_key, project or f"source-org-{n}", model, f.name)
        if not project:
            dash.problem = f"source Grafana organization {n} holds no project-<p> dashboard"
    return sorted(by_key.values(), key=lambda x: (x.project, x.uid)), datasources


def _add(by_key, project: str, model: dict, source: str) -> Dashboard:
    key = (project, str(model.get("uid")))
    if key not in by_key:
        by_key[key] = Dashboard(project, key[1], str(model.get("title") or ""), panels_of(model))
    by_key[key].sources.append(source)
    return by_key[key]


def urlparse_slug(url: str) -> str:
    m = re.search(r"/api/endpoint/([a-z2-7]{26,})", url or "")
    return m.group(1) if m else ""


# ---- what the data says a panel should show -------------------------------------------------------

def short_type(t: str) -> str:
    return t.rsplit("/", 1)[-1].rsplit("#", 1)[-1].rsplit(":", 1)[-1]


def temporal_index(export: Path, types: set[str]) -> dict:
    """(tenant, short type, attr) -> sorted observedAt strings, for the given short types only."""
    out: dict = {}
    path = export / "antares" / "temporal.jsonl"
    if not path.exists():
        return out
    with path.open("rb") as fh:
        for raw in fh:
            try:
                row = json.loads(raw)
            except ValueError:
                continue
            st = short_type(row.get("type", ""))
            if st not in types:
                continue
            at = (row.get("instance") or {}).get("observedAt")
            if at:
                out.setdefault((row["tenant"], st, short_type(row.get("attr", ""))), []).append(at)
    for v in out.values():
        v.sort()
    return out


def _attr(a: str) -> str:
    return short_type(a)


def expect_panel(panel: Panel, tenant: str | None, entity_counts: dict, temporal: dict,
                 since: dt.datetime) -> tuple[str, str]:
    """('data' | 'empty' | 'unknown', why) from the export: an entity query expects rows when the
    tenant holds entities of the type, a temporal query when an instance of one of its attributes
    was observed since `since` (the rehearsal keeps 24 h)."""
    if tenant is None:
        return "unknown", "the datasource names no Endpoint of the export"
    types = {t.get("entityType") for t in panel.targets if t.get("entityType")}
    if not types:
        return "unknown", "the panel's queries name no entityType"
    count = sum((entity_counts.get(tenant) or {}).get(t, {}).get("count", 0) for t in types)
    temporal_q = [t for t in panel.targets if t.get("queryType") == "temporal"]
    if not temporal_q:
        return ("data", f"{count} {','.join(sorted(types))} in tenant {tenant}") if count else \
            ("empty", f"tenant {tenant} holds no {','.join(sorted(types))}")
    cut = since.strftime("%Y-%m-%dT%H:%M:%S")
    n = 0
    for t in temporal_q:
        for a in t.get("attributes") or []:
            n += sum(1 for at in temporal.get((tenant, t.get("entityType"), _attr(a)), []) if at[:19] >= cut)
    return ("data", f"{n} instances since {cut}Z in tenant {tenant}") if n else \
        ("empty", f"no instance of {','.join(sorted(types))} observed since {cut}Z in tenant {tenant}")


# ---- Apps -------------------------------------------------------------------------------------------

@dataclass
class App:
    project: str
    name: str
    kind: str
    lifecycle: str
    visibility: str
    git: str  # owner/repo of spec.source.git, or ""
    file: str


def load_apps(files: Path) -> list[App]:
    import yaml  # PyYAML is in /opt/jc-py

    out = []
    for f in sorted(files.glob("*/*/**/app.yaml")):
        try:
            docs = [d for d in yaml.safe_load_all(f.read_text()) if isinstance(d, dict)]
        except yaml.YAMLError:
            continue
        for d in docs:
            if d.get("kind") != "App":
                continue
            meta, spec = d.get("metadata") or {}, d.get("spec") or {}
            url = ((spec.get("source") or {}).get("git") or {}).get("url", "")
            m = re.search(r"/([^/]+)/([^/]+?)(?:\.git)?$", url)
            out.append(App(str(meta.get("namespace") or f.relative_to(files).parts[1]), str(meta.get("name")),
                           str(spec.get("kind") or ""), str(spec.get("lifecycle") or ""),
                           str(spec.get("visibility") or ""), f"{m.group(1)}/{m.group(2)}" if m else "",
                           str(f.relative_to(files))))
    return out
