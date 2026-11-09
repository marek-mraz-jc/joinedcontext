"""Every name production serves, read from the export bundle (EXPORT.md) alone: Projects, Context
Spaces, Endpoints (slug, audience, enabled surfaces, CKAN dataset names), McpServers, Apps,
Dashboards, the realm and its client ids. Connectors (/c/{id}) live in the Portal database at
runtime and are not in the bundle."""

from __future__ import annotations

import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent / "endpoints"))

import export as ex

# The path each enabled surface answers under /api/endpoint/{slug}, and how it is asked (method,
# path suffix): one cheap call that only a served surface of a known slug answers with anything
# but 404. csv (a whole-type read) and ical (per output name) are left to the endpoints suite.
SURFACES = {
    "ngsi-ld": ("GET", "/ngsi-ld/v1/types"),
    "odata": ("GET", "/odata/v4/"),
    "ogc-features": ("GET", "/ogc/features"),
    "sta": ("GET", "/sta/v1.1"),
    "websocket": ("GET", "/ws"),
    "mcp": ("POST", "/mcp"),
}


CONFIG_REPO = "configuration"


def project_repos(files: Path) -> set[tuple[str, str]]:
    """(owner, repo) of every repository holding its own Project manifest (`project.yaml` naming
    the repository): the projects the Portal serves. An App's source repository
    (`<project>_<app>`) or a repository of another kind is none."""
    out = set()
    for f in files.glob("*/*/project.yaml"):
        if any(d.get("kind") == "Project" and (d.get("metadata") or {}).get("name") == f.parent.name
               for d in ex._load_yaml(f)):
            out.add((f.parent.parent.name, f.parent.name))
    return out


def _docs(files: Path, kind: str):
    """(path relative to files, document) of every YAML document of `kind` in a project
    repository or the configuration repository: where the Portal reads manifests from."""
    repos = project_repos(files)
    for f in sorted(files.rglob("*.yaml")):
        rel = f.relative_to(files)
        if (rel.parts[0], rel.parts[1]) not in repos and rel.parts[1] != CONFIG_REPO:
            continue
        try:
            text = f.read_text()
        except UnicodeDecodeError:
            continue
        if f"kind: {kind}" not in text:
            continue
        for d in ex._load_yaml(f):
            if d.get("kind") == kind and isinstance(d.get("metadata"), dict):
                yield rel, d


def _project(rel: Path, d: dict) -> str:
    """metadata.namespace, else the repository the file is in (gitea/files/<owner>/<repo>/...)."""
    return d["metadata"].get("namespace") or rel.parts[1]


def projects(files: Path) -> list[str]:
    """A Project counts once its repository exists: the configuration repository also lists
    Projects (projects/<p>.yaml), and a sliced bundle can keep those without the repository."""
    return sorted(repo for _, repo in project_repos(files))


def spaces(files: Path) -> list[str]:
    return sorted({f"{_project(r, d)}/{d['metadata']['name']}" for r, d in _docs(files, "ContextSpace")})


def mcp_servers(files: Path) -> list[dict]:
    out = {}
    for r, d in _docs(files, "McpServer"):
        p, n = _project(r, d), d["metadata"]["name"]
        out[(p, n)] = {"project": p, "name": n, "path": f"/api/mcp/{p}/{n}",
                       "audience": str((d.get("spec") or {}).get("audience") or ""), "file": str(r)}
    return [out[k] for k in sorted(out)]


def apps(files: Path) -> list[str]:
    return sorted({f"{_project(r, d)}/{d['metadata']['name']}" for r, d in _docs(files, "App")})


def dashboards(files: Path) -> list[str]:
    return sorted({f"{_project(r, d)}/{d['metadata']['name']}" for r, d in _docs(files, "Dashboard")})


# Keycloak's built-in administration realm: EXPORT.md leaves it out of a bundle (a synthetic one
# may still carry it), no client of the deployment signs in there, and kaisatsu has none.
MASTER = "master"


def realms(export: Path) -> dict:
    """{realm name: sorted client ids} of keycloak/realm-*.json."""
    out = {}
    for f in sorted((export / "keycloak").glob("realm-*.json")):
        doc = json.loads(f.read_text())
        name = doc.get("realm") or f.stem[len("realm-"):]
        if name == MASTER:
            continue
        out[name] = sorted(
            {c["clientId"] for c in doc.get("clients") or [] if isinstance(c, dict) and c.get("clientId")})
    return out


def load(export: Path) -> dict:
    files = export / "gitea" / "files"
    eps = ex.load_endpoints(files)
    return {"projects": projects(files), "spaces": spaces(files), "endpoints": eps,
            "mcpServers": mcp_servers(files), "apps": apps(files), "dashboards": dashboards(files),
            "datasets": sorted({n for e in eps for n in e.datasets}), "realms": realms(export)}
