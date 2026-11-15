"""The databases chart's role Job, run as rendered against a real Postgres of its pinned image.

It creates the extra login role of a database (ckan_datastore_read_*). Its script once pasted the
password into the SQL text: the statement then sat in pg_stat_statements on dev, readable by every
pg_read_all_stats member (F-R2DEP3-014), and a role made earlier with more rights only printed a
warning (F-R2DEP3-016)."""

import shutil
import subprocess
import time
import uuid
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
CHART = ROOT / "components" / "postgres" / "charts" / "databases"
IMAGE = yaml.safe_load((CHART / "values.yaml").read_text())["roleJob"]["image"]
# A quote, a semicolon and a backslash: what a pasted literal cannot carry.
PASSWORD = "it's;a\\pass\"word"

pytestmark = [
    pytest.mark.skipif(shutil.which("docker") is None or shutil.which("helm") is None,
                       reason="needs docker and helm"),
    pytest.mark.xdist_group("postgres-role-job"),
]


def role_script() -> str:
    rendered = subprocess.run(
        ["helm", "template", "databases", str(CHART), "--namespace", "dev",
         "--set", "databases.app.name=app", "--set", "databases.app.user=app",
         "--set", "databases.app.additionalRoles[0].name=app_read",
         "--set", "databases.app.additionalRoles[0].type=readonly",
         "--set", "databases.app.additionalRoles[0].credentialsSecret.name=db-app-read",
         "--set", "securityDefaults.podSecurityContext.runAsNonRoot=true",
         "--set", "securityDefaults.securityContext.readOnlyRootFilesystem=true"],
        capture_output=True, text=True, check=True,
    ).stdout
    job = next(d for d in yaml.safe_load_all(rendered)
               if d and d["kind"] == "Job" and "create-role" in d["metadata"]["name"])
    return job["spec"]["template"]["spec"]["containers"][0]["command"][2]


@pytest.fixture(scope="module")
def postgres():
    name = f"role-job-{uuid.uuid4().hex[:8]}"
    subprocess.run(
        ["docker", "run", "-d", "--rm", "--name", name, "--entrypoint", "bash", IMAGE, "-c",
         "initdb -D /tmp/d -U postgres --auth-local=trust --auth-host=scram-sha-256 >/dev/null"
         " && exec postgres -D /tmp/d -c shared_preload_libraries=pg_stat_statements"
         " -c listen_addresses=127.0.0.1 -c unix_socket_directories=/tmp"],
        check=True, capture_output=True,
    )
    try:
        for _ in range(60):
            if sql(name, "SELECT 1") == "1":
                break
            time.sleep(1)
        sql(name, "CREATE ROLE app LOGIN")
        sql(name, "CREATE DATABASE app OWNER app")
        sql(name, "CREATE EXTENSION pg_stat_statements", db="app")
        sql(name, "ALTER ROLE postgres PASSWORD 'su'")
        yield name
    finally:
        subprocess.run(["docker", "rm", "-f", name], capture_output=True)


def sql(name: str, query: str, db: str = "postgres") -> str:
    done = subprocess.run(["docker", "exec", name, "psql", "-h", "/tmp", "-U", "postgres", "-d", db,
                           "-tA", "-c", query], capture_output=True, text=True)
    return done.stdout.strip()


def run_job(name: str, role: str = "app_read", role_type: str = "readonly") -> subprocess.CompletedProcess:
    env = {"HOME": "/tmp", "PGHOST": "127.0.0.1", "PGPORT": "5432", "PGDATABASE": "app",
           "PGUSER": "postgres", "PGPASSWORD": "su", "ROLE_NAME": role,
           "ROLE_PASSWORD": PASSWORD, "ROLE_TYPE": role_type, "DB_OWNER": "app"}
    flags = [f for key, value in env.items() for f in ("-e", f"{key}={value}")]
    return subprocess.run(["docker", "exec", *flags, name, "/bin/sh", "-c", role_script()],
                          capture_output=True, text=True)


def test_the_role_logs_in_with_a_password_a_literal_could_not_carry(postgres):
    done = run_job(postgres)
    assert done.returncode == 0, done.stdout + done.stderr
    login = subprocess.run(["docker", "exec", "-e", f"PGPASSWORD={PASSWORD}", postgres, "psql",
                            "-h", "127.0.0.1", "-U", "app_read", "-d", "app", "-tA", "-c", "SELECT 1"],
                           capture_output=True, text=True)
    assert login.stdout.strip() == "1", login.stderr


def test_the_password_reaches_no_statement_statistics(postgres):
    assert run_job(postgres).returncode == 0
    seen = sql(postgres, "SELECT count(*) FROM pg_stat_statements"
                         " WHERE strpos(query, 'pass\"word') > 0 OR query ~* 'app_read.*password'", db="app")
    assert seen == "0", "a statement carrying the password is in pg_stat_statements"


def test_a_login_role_keeps_the_grants_its_program_migrated(postgres):
    # jc's daiya grants its tables in schema public to daiya_app in a one-time migration: a rerun
    # of the job at the next upgrade must not revoke them (type login).
    assert run_job(postgres, "app_login", "login").returncode == 0
    sql(postgres, "CREATE TABLE IF NOT EXISTS migrated (id int)", db="app")
    sql(postgres, "GRANT SELECT, INSERT ON migrated TO app_login", db="app")
    done = run_job(postgres, "app_login", "login")
    assert done.returncode == 0, done.stdout + done.stderr
    assert sql(postgres, "SELECT has_table_privilege('app_login', 'migrated', 'INSERT')", db="app") == "t"
    # Red check: the readonly type strips the same grant.
    assert run_job(postgres, "app_login", "readonly").returncode == 0
    assert sql(postgres, "SELECT has_table_privilege('app_login', 'migrated', 'INSERT')", db="app") == "f"


def test_a_role_made_earlier_as_superuser_loses_its_rights(postgres):
    sql(postgres, "ALTER ROLE app_read SUPERUSER CREATEDB CREATEROLE")
    done = run_job(postgres)
    assert done.returncode == 0, done.stdout + done.stderr
    assert sql(postgres, "SELECT rolsuper OR rolcreatedb OR rolcreaterole FROM pg_roles"
                         " WHERE rolname = 'app_read'") == "f"
