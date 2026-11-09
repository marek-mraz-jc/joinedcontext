#!/usr/bin/env python3
"""Compose one rehearsal export bundle (deploy/rehearsal/EXPORT.md format) holding several
Organizations taken from several read-only exports, for the `multiorg` smoke suite.

    compose.py --org hel=<dev export> --org bratislava=<dev export> --org barcelona=<prod export> \
               --base <dev export> --out <bundle> [--entities-per-type 300]

`--base` gives the realm and the installation's configuration repository; each `--org NAME=EXPORT`
takes that Organization's manifest, its projects (configuration/projects/*.yaml whose
`organizationRef` is NAME), their repositories (joinedcontext/<project>, joinedcontext-apps/<project>_*),
their Antares tenants (<project> and <project>-*) and their dashboards from EXPORT. Every other
Organization and project is left out. The output never holds more than the inputs: no people, no
secret values (the inputs hold none, EXPORT.md)."""
import argparse
import json
import re
import shutil
from pathlib import Path

CONFIG = ("joinedcontext", "configuration")


def org_of_project(text: str) -> str | None:
    m = re.search(r"^\s*organizationRef:\s*([\w.-]+)", text, re.M)
    return m.group(1) if m else None


def projects_of(export: Path, org: str) -> dict[str, Path]:
    root = export / "gitea" / "files" / CONFIG[0] / CONFIG[1] / "projects"
    return {p.stem: p for p in sorted(root.glob("*.yaml")) if org_of_project(p.read_text()) == org}


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


def compose(base: Path, orgs: dict[str, Path], out: Path, per_type: int) -> dict:
    if out.exists():
        shutil.rmtree(out)
    (out / "gitea" / "files").mkdir(parents=True)
    owner_of: dict[str, tuple[str, Path]] = {}  # project -> (org, export)
    for org, export in orgs.items():
        if not (export / "gitea" / "files" / CONFIG[0] / CONFIG[1] / "organizations" / f"{org}.yaml").exists():
            raise SystemExit(f"{export} holds no Organization {org}")
        for project in projects_of(export, org):
            owner_of[project] = (org, export)
    projects = sorted(owner_of)
    summary = {"organizations": {o: {"source": str(e), "projects": []} for o, e in orgs.items()}}
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
        src = export / "gitea" / "files" / CONFIG[0] / CONFIG[1]
        shutil.copy2(src / "organizations" / f"{org}.yaml", cfg_out / "organizations" / f"{org}.yaml")
        for project, path in projects_of(export, org).items():
            shutil.copy2(path, cfg_out / "projects" / f"{project}.yaml")

    # Repositories: the configuration repository, then each project's from its Organization's export.
    listing = [r for r in json.loads((base / "gitea" / "repos.json").read_text())
               if (r["owner"], r["name"]) == CONFIG]
    for org, export in orgs.items():
        for r in json.loads((export / "gitea" / "repos.json").read_text()):
            p = repo_project(r["owner"], r["name"], projects)
            if p and owner_of[p][0] == org:
                listing.append(r)
                shutil.copytree(export / "gitea" / "files" / r["owner"] / r["name"],
                                out / "gitea" / "files" / r["owner"] / r["name"])
    (out / "gitea" / "repos.json").write_text(json.dumps(listing, indent=1))
    summary["repos"] = len(listing)

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
        for f in (export / "grafana" / "dashboards").glob("*.json"):
            m = re.match(r"org\d+-project-([\w-]+)\.json$", f.name) or re.match(r"([\w-]+)\.(folder|dashboard|datasource)", f.name)
            if m and m.group(1) in owner_of and owner_of[m.group(1)][0] == org:
                shutil.copy2(f, out / "grafana" / "dashboards" / f.name)

    # manifest.json last: the importer reads a bundle only once it is settled.
    times = [json.loads((e / "manifest.json").read_text())["exportedAt"] for e in {base, *orgs.values()}]
    manifest = {"env": "multiorg", "exportedAt": min(times),
                "source": "composed by bins/jc/smoke/multiorg/compose.py from read-only exports",
                "multiorg": summary, "entitiesPerType": per_type}
    (out / "manifest.json").write_text(json.dumps(manifest, indent=1))
    return summary


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--base", required=True, type=Path)
    ap.add_argument("--org", action="append", required=True, help="NAME=EXPORT")
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--entities-per-type", type=int, default=300)
    a = ap.parse_args(argv)
    orgs = {}
    for spec in a.org:
        name, _, path = spec.partition("=")
        orgs[name] = Path(path)
    s = compose(a.base, orgs, a.out, a.entities_per_type)
    print(json.dumps({o: v["projects"] for o, v in s["organizations"].items()} | {"repos": s["repos"]}))


if __name__ == "__main__":
    main()
