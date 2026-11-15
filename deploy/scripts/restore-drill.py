#!/usr/bin/env python3
"""The monthly restore and upgrade drill on dev (T-2802, OPS-11, OPS-12).

    scripts/restore-drill.py --out <summary.json> [--portal-image <image@sha256:…>]

It copies every database of dev's `postgres-cluster` (the Portal's, the realm's, the forge's,
the catalogue's) into a throwaway CNPG cluster in a namespace of its own, checks that every
table holds the rows it held at the copy, reads the realm's users and clients and the forge's
repositories from the copy, and records how long the restore took. With `--portal-image` it
then runs that Portal image's migrations against the copied Portal database: the upgrade drill.

- dev archives no WAL until the offsite bucket exists (T-0046, the owner's). Until then the
  drill streams a `pg_dump` of the live database, and the summary says so: the RPO of that
  copy is zero by construction and says nothing about a real backup.
- The dump goes from one pod to the other through this process, never through a file: the
  realm's database holds its client secrets, and they stay inside the cluster.
- The namespace is deleted when the drill ends, however it ends, and a namespace a crashed
  drill left behind is deleted when the next one starts.

The `just dev-restore-drill` recipe runs it behind the dev guard; `tasks/file-failures` files
what failed. Exit 0 when nothing failed, 1 otherwise.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import time
from datetime import UTC, datetime
from pathlib import Path

SOURCE_NS = "dev"
SOURCE_CLUSTER = "postgres-cluster"
DRILL_LABEL = "joinedcontext.com/drill"
DRILL_CLUSTER = "drill"
# The Portal's database is the drill cluster's own application database, owned by `portal`, so
# CNPG writes `drill-app` with its URI and the upgrade drill connects as the tables' owner.
PORTAL_DB = "portal"
MEMORY_CEILING = 75
RTO_CEILING_S = 1800
# Exact counts in one statement: count(*) per table through query_to_xml, so no table name is
# ever interpolated by this script.
COUNTS_SQL = (
    "select table_schema || '.' || table_name, "
    "(xpath('/row/c/text()', query_to_xml(format('select count(*) as c from %I.%I', table_schema, table_name), "
    "false, true, '')))[1]::text "
    "from information_schema.tables where table_type = 'BASE TABLE' "
    "and table_schema not in ('pg_catalog', 'information_schema') order by 1"
)
DATABASES_SQL = "select datname from pg_database where not datistemplate and datname <> 'postgres' order by 1"
# What must be there in a copy that works: (database, table, what it holds).
KEY_TABLES = (
    ("keycloak", "public.user_entity", "the realm's people"),
    ("keycloak", "public.client", "the realm's clients"),
    ("gitea", "public.repository", "the forge's repositories"),
)


def result(key: str, verdict: str, title: str, detail: str = "") -> dict:
    return {"key": key, "verdict": verdict, "title": title, "detail": detail, "evidence": ""}


# --- parsing and verdicts -----------------------------------------------------------------------


def parse_counts(text: str) -> dict[str, int]:
    """`psql -At` rows `schema.table|count` → {table: count}."""
    counts = {}
    for line in text.splitlines():
        if "|" in line:
            table, _, count = line.rpartition("|")
            counts[table] = int(count)
    return counts


def compare(before: dict[str, int], after: dict[str, int], restored: dict[str, int]) -> list[str]:
    """Tables whose restored count lies outside what the source held around the copy.

    The source keeps running while it is copied, so a table may gain or lose rows between the
    count before and the count after: the copy is right when it lies between the two.
    """
    wrong = []
    for table in sorted(set(before) | set(after) | set(restored)):
        if table not in restored:
            wrong.append(f"{table} is missing from the copy")
            continue
        low = min(before.get(table, 0), after.get(table, 0))
        high = max(before.get(table, 0), after.get(table, 0))
        if not low <= restored[table] <= high:
            wrong.append(f"{table}: {restored[table]} rows, the source held {low}…{high}")
    return wrong


def memory_percent(top: str) -> int:
    values = [int(line.split()[4].rstrip("%")) for line in top.splitlines() if len(line.split()) >= 5]
    if not values:
        raise ValueError("kubectl top nodes printed no node")
    return max(values)


def drill_cluster(namespace: str, image: str, size_gi: int) -> dict:
    return {
        "apiVersion": "postgresql.cnpg.io/v1",
        "kind": "Cluster",
        "metadata": {"name": DRILL_CLUSTER, "namespace": namespace, "labels": {DRILL_LABEL: "true"}},
        "spec": {
            "instances": 1,
            "imageName": image,
            "storage": {"size": f"{size_gi}Gi"},
            "bootstrap": {"initdb": {"database": PORTAL_DB, "owner": PORTAL_DB}},
            "resources": {"requests": {"cpu": "100m", "memory": "256Mi"}, "limits": {"memory": "1Gi"}},
        },
    }


def size_gi(source_bytes: int) -> int:
    """Room for the copy and its indexes rebuilt, at least 2 GiB."""
    return max(2, -(-source_bytes * 2 // 2**30))


def portal_pod(namespace: str, image: str) -> dict:
    """The upgrade drill: the next Portal image's migrations, and nothing else, against the copy."""
    return {
        "apiVersion": "v1",
        "kind": "Pod",
        "metadata": {"name": "portal-migrate", "namespace": namespace, "labels": {DRILL_LABEL: "true"}},
        "spec": {
            "restartPolicy": "Never",
            "automountServiceAccountToken": False,
            # The image's `nonroot` user by number: the kubelet cannot verify a name as non-root.
            "securityContext": {"runAsNonRoot": True, "runAsUser": 65532, "runAsGroup": 65532,
                                "seccompProfile": {"type": "RuntimeDefault"}},
            "containers": [{
                "name": "migrate",
                "image": image,
                "args": ["migrate"],
                "env": [{"name": "JC_PORTAL_DATABASE_URL",
                         "valueFrom": {"secretKeyRef": {"name": f"{DRILL_CLUSTER}-app", "key": "uri"}}}],
                "securityContext": {"allowPrivilegeEscalation": False, "readOnlyRootFilesystem": True,
                                    "capabilities": {"drop": ["ALL"]}},
                "resources": {"requests": {"cpu": "50m", "memory": "64Mi"}, "limits": {"memory": "256Mi"}},
            }],
        },
    }


def key_results(copies: dict[str, dict[str, int]]) -> list[dict]:
    results = []
    for database, table, what in KEY_TABLES:
        rows = copies.get(database, {}).get(table)
        results.append(result(
            f"restore/{database}/{table}", "pass" if rows else "fail",
            f"the copy holds {rows} of {what}" if rows else f"the copy holds none of {what}",
            "" if rows else f"{database}.{table} is {'empty' if rows == 0 else 'missing'} in the restored database",
        ))
    return results


# --- the drill ----------------------------------------------------------------------------------


def kubectl(*args: str, stdin: str | None = None, timeout: int = 300) -> str:
    run = subprocess.run(["kubectl", *args], input=stdin, capture_output=True, text=True, timeout=timeout)
    if run.returncode != 0:
        raise RuntimeError(f"kubectl {' '.join(args[:3])}: {run.stderr.strip()[-300:]}")
    return run.stdout


def primary(namespace: str, cluster: str) -> str:
    name = kubectl("get", "pods", "-n", namespace, "-l", f"cnpg.io/cluster={cluster},cnpg.io/instanceRole=primary",
                   "-o", "jsonpath={.items[0].metadata.name}").strip()
    if not name:
        raise RuntimeError(f"no primary pod of {namespace}/{cluster}")
    return name


def psql(namespace: str, pod: str, database: str, sql: str) -> str:
    return kubectl("exec", "-n", namespace, pod, "-c", "postgres", "--", "psql", "-v", "ON_ERROR_STOP=1",
                   "-At", "-d", database, "-c", sql)


def copy_database(source: str, target: str, namespace: str, database: str) -> str:
    """pg_dump in the source pod, piped into pg_restore in the drill pod, through no file.

    pg_restore's warnings (an object the fresh database already has) come back as text: the row
    counts decide whether the copy is whole, not the exit code.
    """
    role = ["--role", PORTAL_DB] if database == PORTAL_DB else []
    dump = subprocess.Popen(["kubectl", "exec", "-n", SOURCE_NS, source, "-c", "postgres", "--",
                             "pg_dump", "-Fc", "-d", database], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    restore = subprocess.run(["kubectl", "exec", "-i", "-n", namespace, target, "-c", "postgres", "--",
                              "pg_restore", "--no-owner", "--no-privileges", *role, "-d", database],
                             stdin=dump.stdout, capture_output=True, timeout=3600)
    dump.stdout.close()
    dump_err = dump.stderr.read().decode(errors="replace")
    if dump.wait(timeout=60) != 0:
        raise RuntimeError(f"pg_dump {database}: {dump_err.strip()[-300:]}")
    return "" if restore.returncode == 0 else " ".join(restore.stderr.decode(errors="replace").split())[-300:]


def wait_ready(namespace: str, deadline: float) -> str:
    while time.monotonic() < deadline:
        ready = kubectl("get", "clusters.postgresql.cnpg.io", "-n", namespace, DRILL_CLUSTER,
                        "-o", "jsonpath={.status.readyInstances}").strip()
        if ready.isdigit() and int(ready) >= 1:
            return primary(namespace, DRILL_CLUSTER)
        time.sleep(10)
    raise RuntimeError(f"the drill cluster had no ready instance within {RTO_CEILING_S} s")


def drill(portal_image: str | None) -> list[dict]:
    results = [result("restore/source", "skip",
                      "the drill copied the live database: dev archives no backup yet (T-0046)",
                      "the RPO of a live copy is zero by construction; the drill restores from the archive once dev has one")]
    for stale in kubectl("get", "ns", "-l", f"{DRILL_LABEL}=true", "-o", "name").split():
        kubectl("delete", stale, "--wait=false")
    used = memory_percent(kubectl("top", "nodes", "--no-headers"))
    if used >= MEMORY_CEILING:
        return results + [result("restore/run", "error", f"a node uses {used} % of its memory, so no drill cluster was started",
                                 f"the drill starts below {MEMORY_CEILING} %")]
    source = primary(SOURCE_NS, SOURCE_CLUSTER)
    image = kubectl("get", "clusters.postgresql.cnpg.io", "-n", SOURCE_NS, SOURCE_CLUSTER,
                    "-o", "jsonpath={.spec.imageName}").strip()
    databases = psql(SOURCE_NS, source, "postgres", DATABASES_SQL).split()
    total = int(psql(SOURCE_NS, source, "postgres", "select sum(pg_database_size(datname)) from pg_database").strip() or 0)
    namespace = f"jc-drill-{datetime.now(UTC):%Y%m%d%H%M%S}"
    started = time.monotonic()
    try:
        kubectl("create", "namespace", namespace)
        kubectl("label", "namespace", namespace, f"{DRILL_LABEL}=true")
        kubectl("apply", "-f", "-", stdin=json.dumps(drill_cluster(namespace, image, size_gi(total))))
        target = wait_ready(namespace, started + RTO_CEILING_S)
        copies: dict[str, dict[str, int]] = {}
        for database in databases:
            before = parse_counts(psql(SOURCE_NS, source, database, COUNTS_SQL))
            if database != PORTAL_DB:
                psql(namespace, target, "postgres", f'create database "{database}"')
            warnings = copy_database(source, target, namespace, database)
            after = parse_counts(psql(SOURCE_NS, source, database, COUNTS_SQL))
            copies[database] = parse_counts(psql(namespace, target, database, COUNTS_SQL))
            wrong = compare(before, after, copies[database])
            results.append(result(f"restore/{database}", "fail" if wrong else "pass",
                                  f"{database}: {len(copies[database])} tables " + ("differ" if wrong else "restored"),
                                  "; ".join(wrong[:10]) + (f" (pg_restore: {warnings})" if warnings else "")))
        rto = time.monotonic() - started
        results += key_results(copies)
        results.append(result("restore/gitea/git-data", "skip",
                              "the forge's git objects live on its volume; the drill copies its database only",
                              "the manifests themselves come back from the forge's volume or a clone, not from Postgres"))
        results.append(result("restore/rto", "pass" if rto <= RTO_CEILING_S else "fail",
                              f"restore time {rto / 60:.1f} min for {total / 2**30:.2f} GiB", f"ceiling {RTO_CEILING_S // 60} min"))
        if portal_image and PORTAL_DB in copies:
            results.append(upgrade(namespace, portal_image))
        else:
            results.append(result("upgrade/migrations", "skip", "no --portal-image given, so no upgrade was tried"))
    except (RuntimeError, ValueError, subprocess.SubprocessError, OSError) as error:
        results.append(result("restore/run", "error", "the drill did not finish", str(error)[-400:]))
    finally:
        subprocess.run(["kubectl", "delete", "namespace", namespace, "--wait=false", "--ignore-not-found"],
                       capture_output=True, timeout=120)
    return results


def upgrade(namespace: str, image: str) -> dict:
    # The URI comes from CNPG's `drill-app` secret inside the namespace; it is never read out here.
    kubectl("apply", "-f", "-", stdin=json.dumps(portal_pod(namespace, image)))
    deadline = time.monotonic() + 600
    while time.monotonic() < deadline:
        phase = kubectl("get", "pod", "-n", namespace, "portal-migrate", "-o", "jsonpath={.status.phase}").strip()
        if phase in ("Succeeded", "Failed"):
            logs = kubectl("logs", "-n", namespace, "portal-migrate")
            ok = phase == "Succeeded"
            return result("upgrade/migrations", "pass" if ok else "fail",
                          f"{image.split('@')[0]} {'migrated' if ok else 'failed to migrate'} a copy of dev's Portal database",
                          "" if ok else " ".join(logs.split())[-400:])
        time.sleep(5)
    return result("upgrade/migrations", "error", "the migration pod did not finish in 10 minutes")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", required=True, type=Path)
    parser.add_argument("--portal-image", help="the next Portal image, pinned by digest")
    args = parser.parse_args(argv)
    if args.portal_image and "@sha256:" not in args.portal_image:
        parser.error("--portal-image must be pinned by digest")
    results = drill(args.portal_image)
    summary = {"check": "restore-drill", "repo": "joinedcontext-deployment",
               "run": datetime.now(UTC).strftime("%Y-%m-%dT%H:%MZ"), "requirements": ["OPS-11", "OPS-12"],
               "results": results}
    args.out.write_text(json.dumps(summary, indent=2) + "\n")
    bad = [r for r in results if r["verdict"] in ("fail", "error")]
    for r in results:
        print(f"{r['verdict']:5} {r['title']}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
