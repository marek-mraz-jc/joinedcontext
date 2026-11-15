"""T-4904: dev Postgres sized for the broker, and pg_stat_statements closed to application logins.

The parameters live in dev's environment, not in the development profile, because smartcity
renders that profile too and prod follows only once dev is measured with them. The
stat-statements Job takes the extension's PUBLIC grants away; the docker test runs the Job's
rendered script against the image it runs in, so the SQL is proven before an apply.
"""

import shutil
import subprocess
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
IMAGES = yaml.safe_load((ROOT / "components/postgres/images.yaml").read_text())["postgres"]
ROLE_JOB_IMAGE = f"{IMAGES['roleJob']['repository']}@{IMAGES['roleJob']['digest']}"

DEV_PARAMETERS = {
    "shared_buffers": "128MB",
    "effective_cache_size": "1GB",
    "work_mem": "4MB",
    "track_io_timing": "on",
    "log_min_duration_statement": "500ms",
    "log_parameter_max_length": "0",
    "pg_stat_statements.track": "top",
    "pg_stat_statements.max": "5000",
    "max_wal_size": "4GB",
    "checkpoint_timeout": "15min",
}

# The development profile's sizing (T-4933), which dev, local and smartcity all render.
PROFILE_PARAMETERS = {"max_connections": "55", "max_parallel_workers": "2", "autovacuum_work_mem": "32MB"}

requires_docker = pytest.mark.skipif(shutil.which("docker") is None, reason="docker not installed")


def cluster(docs: list[dict]) -> dict:
    found = [d for d in docs if d.get("kind") == "Cluster" and d["metadata"]["name"] == "postgres-cluster"]
    assert len(found) == 1
    return found[0]


def stat_statements_job(docs: list[dict]) -> dict:
    found = [d for d in docs if d.get("kind") == "Job" and d["metadata"]["name"] == "postgres-cluster-stat-statements"]
    assert len(found) == 1
    return found[0]["spec"]["template"]["spec"]["containers"][0]


def env_of(container: dict) -> dict:
    return {e["name"]: e.get("value") for e in container["env"]}


def test_dev_postgres_carries_the_broker_sizing(rendered):
    spec = cluster(rendered("dev"))["spec"]
    assert spec["postgresql"]["parameters"] == PROFILE_PARAMETERS | DEV_PARAMETERS
    # The overlay adds parameters and nothing else: the TLS-only pg_hba of the base stays.
    assert spec["postgresql"]["pg_hba"] == ["hostnossl all all all reject", "hostssl all all all scram-sha-256"]
    # The limit is unchanged: the node has no memory to give it.
    assert spec["resources"]["limits"]["memory"] == "2Gi"


def test_prod_waits_for_the_dev_measurement(rendered):
    parameters = cluster(rendered("smartcity"))["spec"]["postgresql"].get("parameters", {})
    # Only Decidim's connections differ from the profile (T-6448, environments/smartcity/postgres).
    assert parameters == PROFILE_PARAMETERS | {"max_connections": "61"}
    assert env_of(stat_statements_job(rendered("smartcity")))["PRELOADED"] == "false"


def test_the_job_waits_for_the_library_on_dev(rendered):
    container = stat_statements_job(rendered("dev"))
    env = env_of(container)
    assert env["PRELOADED"] == "true"
    assert env["PGDATABASE"] == "postgres"
    assert container["securityContext"]["readOnlyRootFilesystem"] is True


def test_the_script_survives_kubernetes_variable_expansion(rendered):
    # Kubernetes turns $$ into $ in command/args, so DO $$ ... $$
    # reached psql as DO $ and the hook failed on dev; a named dollar quote passes unchanged.
    script = stat_statements_job(rendered("dev"))["command"][2]
    assert "$$" not in script


@requires_docker
def test_application_logins_cannot_read_the_statistics(rendered):
    script = stat_statements_job(rendered("dev"))["command"][2]
    harness = """
set -eu
export PGHOST=/tmp PGDATA=/tmp/data PGDATABASE=postgres PGUSER=postgres
PATH=$(ls -d /usr/lib/postgresql/*/bin):$PATH
initdb -U postgres --auth=trust >/dev/null
pg_ctl -w -o "-k /tmp -c listen_addresses= -c shared_preload_libraries=pg_stat_statements" start >/dev/null
psql -qc "CREATE ROLE app LOGIN" -c "CREATE ROLE mon LOGIN IN ROLE pg_read_all_stats" -c "CREATE DATABASE antares OWNER app"
cat >/tmp/job.sh
PRELOADED=true sh /tmp/job.sh >/dev/null
PRELOADED=true sh /tmp/job.sh >/dev/null 2>&1
psql -qc "CREATE DATABASE later"
for db in antares later postgres; do
  for user in app mon; do
    if psql -U "$user" -d "$db" -Atc "SELECT 1 FROM pg_stat_statements LIMIT 1" >/dev/null 2>&1; then r=read; else r=denied; fi
    if psql -U "$user" -d "$db" -Atc "SELECT 1 FROM pg_stat_statements(true) LIMIT 1" >/dev/null 2>&1; then f=read; else f=denied; fi
    if psql -U "$user" -d "$db" -Atc "SELECT pg_stat_statements_reset()" >/dev/null 2>&1; then x=reset; else x=denied; fi
    echo "$db $user $r $f $x"
  done
done
"""
    result = subprocess.run(
        ["docker", "run", "--rm", "-i", "--network", "none", "--entrypoint", "sh", ROLE_JOB_IMAGE, "-c",
         harness + "\n"],
        check=False, input=script, capture_output=True, text=True, timeout=180,
    )
    assert result.returncode == 0, result.stderr
    lines = result.stdout.split()
    rows = [tuple(lines[i:i + 5]) for i in range(0, len(lines), 5)]
    # template1 carries the closed extension, so a database created after the Job is closed too.
    assert rows == [
        (db, user, *(("denied", "denied") if user == "app" else ("read", "read")), "denied")
        for db in ("antares", "later", "postgres")
        for user in ("app", "mon")
    ]
