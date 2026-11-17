#!/usr/bin/env python3
"""The old -> new Tenant map of the rename (T-7039, T-7193, ARCH/TEN-10), from two exports.

    tenant-rename-map.py <tenants.txt> <projects.tsv> [<pins.json>] > map.json

<tenants.txt>: one Tenant per line, the broker's `SELECT tenant_id FROM tenants`.
<projects.tsv>: the Portal's project registry, one `name<TAB>org uuid<TAB>project uuid` per line:
    psql "$PORTAL_DB" -AtF $'\\t' -c "SELECT p.name, o.key, p.id FROM project_ids p
        JOIN organizations o ON o.name = p.organization WHERE p.parent IS NULL AND NOT p.reserved"
<pins.json>: optional {tenant: [project, space]} for a space whose manifest pins `urnSegment`, so its
Tenant is not `{project}-{space}`.

A Tenant `{project}-{space}` of a registered project maps to `{OrgId}__{ProjectId}__{space}` (the
jc-core TenantName::of_space rule; bins/jc/smoke/endpoints/export.py tenant_name). The output is
{"map": {old: new}, "unmapped": {old: reason}}: every Tenant the map lacks needs a recorded decision
before the window (map it by a pin, keep it admin-only, or purge it after the dump; design 10.6).
Two projects whose names both prefix a Tenant (`a` and `a-b` for `a-b-c`) leave it unmapped as
ambiguous rather than guess."""

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "bins/jc/smoke/endpoints"))
import export

RESERVED = {"default", "distsub-index", "antares-views", "antares-policy", "antares-grants",
            "antares-channels"}


def plan(tenants: list[str], projects: dict[str, tuple[str, str]], pins: dict) -> dict:
    out: dict = {"map": {}, "unmapped": {}}
    for tenant in tenants:
        if tenant in RESERVED or tenant.startswith("snap-"):
            continue
        if tenant in pins:
            project, space = pins[tenant]
            candidates = [(project, space)] if project in projects else []
        else:
            candidates = [(p, tenant[len(p) + 1:]) for p in projects
                          if tenant.startswith(p + "-") and len(tenant) > len(p) + 1]
        if export.is_tenant_name(tenant) and "__" in tenant:
            out["unmapped"][tenant] = "already a composed name"
        elif not candidates:
            out["unmapped"][tenant] = "no registered project names it: map by a pin, keep or purge"
        elif len(candidates) > 1:
            names = ", ".join(sorted(p for p, _ in candidates))
            out["unmapped"][tenant] = f"ambiguous between projects {names}: map by a pin"
        else:
            project, space = candidates[0]
            org, pid = projects[project]
            try:
                out["map"][tenant] = export.tenant_name(org, pid, space)
            except ValueError as err:
                out["unmapped"][tenant] = str(err)
    targets: dict[str, list[str]] = {}
    for old, new in out["map"].items():
        targets.setdefault(new, []).append(old)
    for new, olds in targets.items():
        if len(olds) > 1:
            for old in olds:
                del out["map"][old]
                out["unmapped"][old] = f"{len(olds)} Tenants would become one: decide which by a pin"
    return out


def main(tenants_path: str, projects_path: str, pins_path: str | None = None) -> int:
    tenants = [t.strip() for t in Path(tenants_path).read_text().splitlines() if t.strip()]
    projects = {}
    for line in Path(projects_path).read_text().splitlines():
        if line.strip():
            name, org, pid = line.split("\t")
            projects[name] = (org, pid)
    pins = json.loads(Path(pins_path).read_text()) if pins_path else {}
    json.dump(plan(tenants, projects, pins), sys.stdout, indent=2, sort_keys=True)
    print()
    return 0


if __name__ == "__main__":
    sys.exit(main(*sys.argv[1:4]))
