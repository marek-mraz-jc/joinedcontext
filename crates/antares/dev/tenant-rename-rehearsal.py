#!/usr/bin/env python3
"""Rehearses the Tenant rename (T-7039, migration 0037) on a restored copy of a broker database.

    tenant-rename-rehearsal.py <libpq url of the copy> <map.json> <log>

<map.json> is {old: new} for every Tenant to move (the Portal's map, or a synthetic one for a
timing run); a Tenant of the copy that the map lacks is reported as unmapped and stays. A run that
stopped is resumed with the map of the Tenants not yet moved: the ones `tenant_rename` records
count as renamed earlier. Per Tenant:
antares_tenant_digest(old) before, antares_tenant_rename(old, new) in its own transaction, then
antares_tenant_digest(new); the digests must be equal. Then the whole-database row counts per table
must be equal, no old name may hold a row (text still naming an old name inside a longer string is
reported, `text_still_naming_old`, and stays), and a rollback of the smallest Tenant must give its
digest back. The log names Tenants by index and size only, never a row, so it can go into
evidence/. Exit status 0 only when every check holds."""

import json
import subprocess
import sys
import time


def psql(url: str, sql: str, *args: str) -> list[list[str]]:
    params = [f"-v{i}={a}" for i, a in enumerate(args, 1)]
    run = subprocess.run(["psql", url, "-X", "-At", "-F", "\t", "-v", "ON_ERROR_STOP=1", *params, "-c", sql]
                         if not args else
                         ["psql", url, "-X", "-At", "-F", "\t", "-v", "ON_ERROR_STOP=1", *params],
                         input=None if not args else sql, capture_output=True, text=True, check=False)
    if run.returncode:
        raise RuntimeError(run.stderr.strip())
    return [line.split("\t") for line in run.stdout.splitlines() if line]


def digest(url: str, tenant: str) -> list[list[str]]:
    return psql(url, "SELECT tbl, row_count, digest FROM antares_tenant_digest(:'1')", tenant)


TABLE_COUNTS = """
SELECT c.relname, (xpath('/row/n/text()', query_to_xml(format('SELECT count(*) AS n FROM %I', c.relname),
       false, true, '')))[1]::text
  FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace AND n.nspname = 'public'
 WHERE c.relkind IN ('r', 'p') AND NOT c.relispartition AND c.relname <> 'spatial_ref_sys'
 ORDER BY 1"""


def main(url: str, map_path: str, log_path: str) -> int:
    with open(map_path) as f:
        mapping: dict[str, str] = json.load(f)
    with open(log_path, "w") as out:
        return rehearse(url, mapping, out)


def rehearse(url: str, mapping: dict[str, str], out) -> int:
    def say(line: str) -> None:
        out.write(line + "\n")
        out.flush()
        print(line)

    ok = True
    tenants = [r[0] for r in psql(url, "SELECT tenant_id FROM tenants ORDER BY tenant_id")]
    sizes = {r[0]: int(r[1]) for r in psql(
        url, "SELECT tenant_id, count(*) FROM entity_values GROUP BY 1")}
    index = {t: f"T{i:03d}" for i, t in enumerate(sorted(tenants, key=lambda t: -sizes.get(t, 0)))}
    say(f"tenants {len(tenants)}, mapped {sum(t in mapping for t in tenants)}")
    done = {r[0] for r in psql(url, "SELECT new FROM tenant_rename")}  # a resumed run's earlier part
    say(f"renamed earlier {len(done)}")
    for t in tenants:
        if t not in mapping and t not in done and t != "default" and psql(url, "SELECT tenant_reserved(:'1')", t)[0][0] != "t":
            say(f"unmapped {index[t]} entity_values={sizes.get(t, 0)}: stays under its name")
    say("attr_rows_short rows " + psql(url, "SELECT count(*) FROM attr_rows_short")[0][0])
    say("database size " + psql(url, "SELECT pg_size_pretty(pg_database_size(current_database()))")[0][0])
    before = dict(psql(url, TABLE_COUNTS))
    for table, n in before.items():
        say(f"before {table} {n}")
    started = time.monotonic()
    for t in sorted(mapping, key=lambda t: -sizes.get(t, 0)):
        new = mapping[t]
        d0 = digest(url, t)
        t0 = time.monotonic()
        result = json.loads(psql(url, "SELECT antares_tenant_rename(:'1', :'2')", t, new)[0][0])
        took = time.monotonic() - t0
        d1 = digest(url, new)
        same = d0 == d1
        ok &= same
        moved = {k: v for k, v in result.items() if k not in ("old", "new")}
        say(f"rename {index[t]} {took:.1f}s digest {'equal' if same else 'DIFFERENT'} "
            f"tables {len(d0)} rows {sum(int(r[1]) for r in d0)} moved {json.dumps(moved, sort_keys=True)}")
        if not same:
            for a, b in zip(d0, d1):
                if a != b:
                    say(f"  differs {a[0]} {a[1]}->{b[1]}")
    say(f"renamed {len(mapping)} in {time.monotonic() - started:.0f}s")
    after = dict(psql(url, TABLE_COUNTS))
    for table in sorted(set(before) | set(after)):
        equal = before.get(table) == after.get(table) or table == "tenant_rename"
        ok &= equal
        say(f"after {table} {after.get(table)} {'equal' if equal else 'DIFFERENT from ' + str(before.get(table))}")
    left = psql(url, "SELECT count(*) FROM tenant_rename r WHERE EXISTS (SELECT 1 FROM tenants WHERE tenant_id = r.old)")
    ok &= left[0][0] == "0"
    say(f"old names still in tenants {left[0][0]}")
    smallest = min(mapping, key=lambda t: sizes.get(t, 0))
    d_new = digest(url, mapping[smallest])
    psql(url, "SELECT antares_tenant_rename_rollback(:'1')", mapping[smallest])
    back = digest(url, smallest) == d_new
    psql(url, "SELECT antares_tenant_rename(:'1', :'2')", smallest, mapping[smallest])
    again = digest(url, mapping[smallest]) == d_new
    ok &= back and again
    say(f"rollback {index[smallest]} digest {'equal' if back else 'DIFFERENT'}, renamed again {'equal' if again else 'DIFFERENT'}")
    say("RESULT " + ("green" if ok else "RED"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main(*sys.argv[1:4]))
