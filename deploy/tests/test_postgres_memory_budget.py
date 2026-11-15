"""T-4933: every environment's Postgres fits its memory limit at its worst case.

T-4904 raised shared_buffers on dev to 512MB of a 2Gi limit and the kernel killed the instance
nine times in two hours: the per-connection memory, not shared_buffers, is most of the pod. The
cgroup reading before T-4904 (2026-10-20 22:45Z, 128MB shared_buffers, ~60 client connections
and ~8 server processes): anon 1290Mi, shmem 158Mi. So each process holds about 19MiB of its
own (catalog and plan caches over the partitioned broker tables, sort buffers in use), and the
shared segment is shared_buffers plus about 30MiB.

The worst case this test sums, in MiB:

    shared_buffers + 30
    + (max_connections + parallel workers) x (19 + work_mem)
    + autovacuum_max_workers x (autovacuum_work_mem, else maintenance_work_mem)
    + maintenance_work_mem                      (one manual VACUUM, CREATE INDEX or migration)

and it must stay under 80 % of the container limit, the rest left to the page cache and to a
query that holds more than one work_mem. CNPG starts Postgres with max_parallel_workers and
max_worker_processes at 32, so an environment that does not cap them counts 32 workers.
"""

import re

import pytest
from test_postgres_tuning import cluster

ENVIRONMENTS = ["local", "dev", "smartcity", "stage", "staging", "production", "minimal", "broker-only", "jc-dev", "jc-prod"]
PER_PROCESS_MIB = 19
OTHER_SHARED_MIB = 30
SHARE_OF_LIMIT = 0.8
# PostgreSQL's defaults, and CNPG's where it sets its own.
DEFAULTS = {
    "shared_buffers": "128MB",
    "max_connections": "100",
    "work_mem": "4MB",
    "maintenance_work_mem": "64MB",
    "autovacuum_work_mem": "-1",
    "autovacuum_max_workers": "3",
    "max_parallel_workers": "32",
    "max_worker_processes": "32",
}
PG_UNITS = {"kB": 1 / 1024, "MB": 1, "GB": 1024, "TB": 1024 * 1024}
K8S_UNITS = {"Ki": 1 / 1024, "Mi": 1, "Gi": 1024, "K": 1000 / 1024**2, "M": 1000**2 / 1024**2, "G": 1000**3 / 1024**2}


def pg_mib(value: str) -> float:
    match = re.fullmatch(r"(\d+)\s*(kB|MB|GB|TB)?", value)
    assert match, f"not a PostgreSQL memory size: {value!r}"
    # A bare number is in kB for these parameters (8kB pages for shared_buffers are never written bare here).
    return int(match[1]) * PG_UNITS[match[2] or "kB"]


def k8s_mib(value: str) -> float:
    match = re.fullmatch(r"(\d+)(Ki|Mi|Gi|K|M|G)", value)
    assert match, f"not a Kubernetes memory quantity: {value!r}"
    return int(match[1]) * K8S_UNITS[match[2]]


def worst_case_mib(parameters: dict) -> float:
    p = DEFAULTS | parameters
    work_mem = pg_mib(p["work_mem"])
    maintenance = pg_mib(p["maintenance_work_mem"])
    autovacuum = maintenance if p["autovacuum_work_mem"] == "-1" else pg_mib(p["autovacuum_work_mem"])
    parallel = min(int(p["max_parallel_workers"]), int(p["max_worker_processes"]))
    processes = int(p["max_connections"]) + parallel
    return (
        pg_mib(p["shared_buffers"])
        + OTHER_SHARED_MIB
        + processes * (PER_PROCESS_MIB + work_mem)
        + int(p["autovacuum_max_workers"]) * autovacuum
        + maintenance
    )


def test_the_sum_counts_what_killed_dev():
    # The T-4904 values (work_mem 8MB, autovacuum_work_mem 64MB, CNPG's 32 workers) sum to
    # far more than the 2Gi limit they were OOMKilled under; the values before them do too,
    # which is why dev sat at 2047Mi of 2048Mi with page cache squeezed out.
    t4904 = {"shared_buffers": "512MB", "work_mem": "8MB", "maintenance_work_mem": "128MB", "autovacuum_work_mem": "64MB"}
    assert worst_case_mib(t4904) > 2048
    assert worst_case_mib({}) > 2048 * SHARE_OF_LIMIT


@pytest.mark.parametrize("env", ENVIRONMENTS)
def test_postgres_worst_case_stays_under_80_percent_of_its_limit(rendered, env):
    docs = [d for d in rendered(env) if d.get("kind") == "Cluster" and d["metadata"]["name"] == "postgres-cluster"]
    if not docs:
        pytest.skip(f"{env} renders no postgres-cluster")
    spec = cluster(docs)["spec"]
    limit = k8s_mib(spec["resources"]["limits"]["memory"])
    worst = worst_case_mib(spec["postgresql"].get("parameters", {}))
    assert worst <= SHARE_OF_LIMIT * limit, f"{env}: worst case {worst:.0f}MiB over 80 % of {limit:.0f}MiB"
