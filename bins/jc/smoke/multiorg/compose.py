#!/usr/bin/env python3
"""Compose one rehearsal export bundle (deploy/rehearsal/EXPORT.md format) holding several
Organizations taken from several read-only exports, for the `multiorg` smoke suite.

    compose.py --org hel=<dev export> --org bbsk=<dev export>:bbsk --org ctl-a= \
               --base <dev export> --out <bundle> [--entities-per-type 300] [--endpoints-per-space 0] \
               [--no-installation]

`--base` gives the realm and the installation's configuration repository; each `--org NAME=EXPORT`
takes that Organization's manifest, its projects (configuration/projects/*.yaml whose
`organizationRef` is NAME), their repositories (joinedcontext/<project>, joinedcontext-apps/<project>_*),
their Antares tenants (<project> and <project>-*) and their dashboards from EXPORT.
`--org NAME=EXPORT:P1,P2` makes Organization NAME of EXPORT's projects P1, P2 whatever Organization
they had (their organizationRef rewritten); its manifest is EXPORT's NAME or, when EXPORT has none,
one written from P1's domain and title. `--org NAME=` is an Organization with no project (a control
that makes an empty Organization's leak visible). `--no-installation` drops `spec.installation` from
every Organization: operators are then the platform-admins group alone. `--endpoints-per-space N`
keeps the first N Endpoints of each space by file name (bratislava's 453 hold the first reconciler
pass for minutes). Every other Organization and project is left out. The output never holds more
than the inputs: no people, no secret values (the inputs hold none, EXPORT.md)."""
import argparse
import json
import re
import shutil
from pathlib import Path

CONFIG = ("joinedcontext", "configuration")


def org_of_project(text: str) -> str | None:
    m = re.search(r"^\s*organizationRef:\s*([\w.-]+)", text, re.M)
    return m.group(1) if m else None


def projects_of(export: Path | None, org: str, named: list[str] | None = None) -> dict[str, Path]:
    if export is None:
        return {}
    root = export / "gitea" / "files" / CONFIG[0] / CONFIG[1] / "projects"
    if named is not None:
        missing = [p for p in named if not (root / f"{p}.yaml").is_file()]
        if missing:
            raise SystemExit(f"{export} holds no project {', '.join(missing)}")
        return {p: root / f"{p}.yaml" for p in named}
    return {p.stem: p for p in sorted(root.glob("*.yaml")) if org_of_project(p.read_text()) == org}


def with_org(text: str, org: str) -> str:
    """A Project manifest moved to Organization `org`."""
    return re.sub(r"^(\s*organizationRef:\s*)[\w.-]+", rf"\g<1>{org}", text, count=1, flags=re.M)


def organization_manifest(org: str, project_text: str | None) -> str:
    """An Organization of its own for `org`: the domain and title of its first project, or for an
    empty control `<org>.example` (RFC 2606) and its name."""
    domain = re.search(r"^\s*domain:\s*([\w.-]+)", project_text or "", re.M)
    title = re.search(r"^\s*title:\s*(\{.*\})\s*$", project_text or "", re.M)
    return ("apiVersion: joinedcontext.com/v1alpha1\nkind: Organization\nmetadata:\n"
            f"  name: {org}\n  namespace: org\n  title: {title.group(1) if title else '{ en: ' + org + ' }'}\n"
            f"spec:\n  domain: {domain.group(1) if domain else org + '.example'}\n  locales: [en]\n"
            "  defaultLocale: en\n  projects:\n    visibility: organization\n    creation: org-admin\n")


def project_text(export: Path | None, project: str | None) -> str | None:
    """The project's own manifest (its repository's project.yaml, which carries domain and title)."""
    f = export / "gitea" / "files" / CONFIG[0] / project / "project.yaml" if export and project else None
    return f.read_text() if f and f.is_file() else None


def without_installation(text: str) -> str:
    return re.sub(r"^\s*installation:\s*true\s*\n", "", text, flags=re.M)


def cap_endpoints(repo: Path, per_space: int) -> int:
    """Keeps the first `per_space` Endpoints of each space by file name; answers how many went."""
    dropped = 0
    for space in sorted(repo.glob("spaces/*/endpoints")):
        for f in sorted(space.glob("*.yaml"))[per_space:]:
            f.unlink()
            dropped += 1
    return dropped


def tenant_project(tenant: str, projects: list[str]) -> str | None:
    """The project a tenant belongs to: the longest project name equal to it or followed by '-'."""
    hits = [p for p in projects if tenant == p or tenant.startswith(p + "-")]
    return max(hits, key=len) if hits else None


def repo_project(owner: str, name: str, projects: list[str]) -> str | None:
    if owner == CONFIG[0]:
        return name if name in projects else None
    if owner == "joinedcontext-apps":
        return tenant_project(name.split("_", 1)[0], projects) if "_" in name else None
    return None


def compose(base: Path, orgs: dict, out: Path, per_type: int, per_space: int = 0,
            installation: bool = True) -> dict:
    """`orgs`: name -> export (its own Organization and projects), (export, [projects]) or None (empty)."""
    if out.exists():
        shutil.rmtree(out)
    (out / "gitea" / "files").mkdir(parents=True)
    named = {o: v[1] if isinstance(v, tuple) else None for o, v in orgs.items()}
    orgs = {o: v[0] if isinstance(v, tuple) else v for o, v in orgs.items()}
    owner_of: dict[str, tuple[str, Path]] = {}  # project -> (org, export)
    for org, export in orgs.items():
        if export is None:
            continue
        has = (export / "gitea" / "files" / CONFIG[0] / CONFIG[1] / "organizations" / f"{org}.yaml").exists()
        if not has and named[org] is None:
            raise SystemExit(f"{export} holds no Organization {org}")
        for project in projects_of(export, org, named[org]):
            if project in owner_of:
                raise SystemExit(f"project {project} named for both {owner_of[project][0]} and {org}")
            owner_of[project] = (org, export)
    projects = sorted(owner_of)
    summary = {"organizations": {o: {"source": str(e) if e else "none", "projects": []} for o, e in orgs.items()}}
    for p, (o, _) in owner_of.items():
        summary["organizations"][o]["projects"].append(p)

    # The configuration repository: base's, with only the chosen Organizations and projects, each
    # taken from its own export.
    cfg_src = base / "gitea" / "files" / CONFIG[0] / CONFIG[1]
    cfg_out = out / "gitea" / "files" / CONFIG[0] / CONFIG[1]
    shutil.copytree(cfg_src, cfg_out, ignore=shutil.ignore_patterns())
    for f in (cfg_out / "organizations").glob("*.yaml"):
        f.unlink()
    for f in (cfg_out / "projects").glob("*.yaml"):
        f.unlink()
    for org, export in orgs.items():
        mine = projects_of(export, org, named[org])
        src = export / "gitea" / "files" / CONFIG[0] / CONFIG[1] / "organizations" / f"{org}.yaml" if export else None
        text = src.read_text() if src and src.is_file() else \
            organization_manifest(org, project_text(export, next(iter(mine), None)))
        (cfg_out / "organizations" / f"{org}.yaml").write_text(text if installation else without_installation(text))
        for project, path in mine.items():
            (cfg_out / "projects" / f"{project}.yaml").write_text(with_org(path.read_text(), org))

    # Repositories: the configuration repository, then each project's from its Organization's export.
    listing = [r for r in json.loads((base / "gitea" / "repos.json").read_text())
               if (r["owner"], r["name"]) == CONFIG]
    dropped = 0
    for org, export in orgs.items():
        for r in json.loads((export / "gitea" / "repos.json").read_text()) if export else []:
            p = repo_project(r["owner"], r["name"], projects)
            if p and owner_of[p][0] == org:
                listing.append(r)
                dst = out / "gitea" / "files" / r["owner"] / r["name"]
                shutil.copytree(export / "gitea" / "files" / r["owner"] / r["name"], dst)
                if r["owner"] == CONFIG[0]:
                    own = dst / "project.yaml"
                    if own.is_file():
                        own.write_text(with_org(own.read_text(), org))
                    if per_space:
                        dropped += cap_endpoints(dst, per_space)
    (out / "gitea" / "repos.json").write_text(json.dumps(listing, indent=1))
    summary["repos"] = len(listing)
    summary["endpointsDropped"] = dropped

    # Realm: the base's (one realm, groups of every project; people were never in it).
    (out / "keycloak").mkdir()
    for f in (base / "keycloak").glob("realm-*.json"):
        shutil.copy2(f, out / "keycloak" / f.name)

    # Antares: the tenants of the chosen projects, from each project's own export, at most
    # per_type entities of each type a tenant (0: all); history only of the entities kept.
    (out / "antares").mkdir()
    kept: dict[str, set] = {}
    counts: dict[str, dict] = {}
    with (out / "antares" / "entities.jsonl").open("w") as w:
        for org, export in orgs.items():
            if export is None:
                continue
            with (export / "antares" / "entities.jsonl").open() as r:
                for line in r:
                    tenant = line[12:line.index('"', 12)] if line.startswith('{"tenant": "') else json.loads(line)["tenant"]
                    p = tenant_project(tenant, projects)
                    if not p or owner_of[p][0] != org:
                        continue
                    c = counts.setdefault(tenant, {"org": org, "entities": 0, "temporal": 0, "types": {}})
                    row = json.loads(line)
                    t = row["entity"]["type"]
                    if per_type and c["types"].get(t, 0) >= per_type:
                        continue
                    c["types"][t] = c["types"].get(t, 0) + 1
                    kept.setdefault(tenant, set()).add(row["entity"]["id"])
                    c["entities"] += 1
                    w.write(line)
    with (out / "antares" / "temporal.jsonl").open("w") as w:
        for org, export in orgs.items():
            if export is None:
                continue
            with (export / "antares" / "temporal.jsonl").open() as r:
                for line in r:
                    row = json.loads(line)
                    if row["id"] in kept.get(row["tenant"], ()) and counts[row["tenant"]]["org"] == org:
                        counts[row["tenant"]]["temporal"] += 1
                        w.write(line)
    summary["tenants"] = counts

    # Dashboards: the installation's (org1-*) from base, a project's from its Organization's export.
    (out / "grafana" / "dashboards").mkdir(parents=True)
    for f in (base / "grafana" / "dashboards").glob("org1-*.json"):
        shutil.copy2(f, out / "grafana" / "dashboards" / f.name)
    for org, export in orgs.items():
        for f in (export / "grafana" / "dashboards").glob("*.json") if export else []:
            m = re.match(r"org\d+-project-([\w-]+)\.json$", f.name) or re.match(r"([\w-]+)\.(folder|dashboard|datasource)", f.name)
            if m and m.group(1) in owner_of and owner_of[m.group(1)][0] == org:
                shutil.copy2(f, out / "grafana" / "dashboards" / f.name)

    # manifest.json last: the importer reads a bundle only once it is settled.
    times = [json.loads((e / "manifest.json").read_text())["exportedAt"] for e in {base, *orgs.values()} if e]
    manifest = {"env": "multiorg", "exportedAt": min(times),
                "source": "composed by bins/jc/smoke/multiorg/compose.py from read-only exports",
                "multiorg": summary, "entitiesPerType": per_type}
    (out / "manifest.json").write_text(json.dumps(manifest, indent=1))
    return summary


def parse_orgs(specs: list[str]) -> dict:
    orgs: dict = {}
    for spec in specs:
        name, _, path = spec.partition("=")
        if not re.fullmatch(r"[a-z0-9][a-z0-9-]*", name) or name in orgs:
            raise SystemExit(f"--org {spec}: a new lower-case Organization name is needed")
        path, colon, names = path.partition(":")
        if not path:
            orgs[name] = None
        elif colon:
            orgs[name] = (Path(path), [p for p in names.split(",") if p])
        else:
            orgs[name] = Path(path)
    return orgs


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--base", required=True, type=Path)
    ap.add_argument("--org", action="append", required=True,
                    help="NAME=EXPORT, NAME=EXPORT:PROJECT[,PROJECT] or NAME= (no project)")
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--entities-per-type", type=int, default=300)
    ap.add_argument("--endpoints-per-space", type=int, default=0)
    ap.add_argument("--no-installation", action="store_true")
    a = ap.parse_args(argv)
    s = compose(a.base, parse_orgs(a.org), a.out, a.entities_per_type, a.endpoints_per_space,
                not a.no_installation)
    print(json.dumps({o: v["projects"] for o, v in s["organizations"].items()} | {"repos": s["repos"]}))


if __name__ == "__main__":
    main()
