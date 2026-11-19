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


def run_job(name: str, role: str = "app_read", role_type: str = "readonly",
            grants: str = "", member_of: str = "[]") -> subprocess.CompletedProcess:
    env = {"HOME": "/tmp", "PGHOST": "127.0.0.1", "PGPORT": "5432", "PGDATABASE": "app",
           "PGUSER": "postgres", "PGPASSWORD": "su", "ROLE_NAME": role,
           "ROLE_PASSWORD": PASSWORD, "ROLE_TYPE": role_type, "DB_OWNER": "app",
           "ROLE_GRANTS": grants, "ROLE_MEMBER_OF": member_of}
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


def test_a_backup_role_reads_its_columns_only_and_never_passes_row_security(postgres):
    # T-7169: the Portal's data backup reader of a module's database.
    sql(postgres, "CREATE SCHEMA m AUTHORIZATION app; CREATE TABLE m.t (id int, org text, secret text);"
        " CREATE TABLE m.gone (id int); ALTER TABLE m.t ENABLE ROW LEVEL SECURITY;"
        " ALTER TABLE m.t FORCE ROW LEVEL SECURITY;"
        " CREATE POLICY p ON m.t USING (org = current_setting('m.org', true));"
        " INSERT INTO m.t VALUES (1, 'a', 's1'), (2, 'b', 's2')", db="app")
    grants = ('[{"schema": "m", "table": "t", "columns": ["id", "org"]},'
              ' {"schema": "m", "table": "later", "columns": ["id"]}]')
    sql(postgres, "CREATE ROLE app_backup LOGIN BYPASSRLS")
    sql(postgres, "GRANT SELECT ON m.gone TO app_backup", db="app")
    done = run_job(postgres, "app_backup", "backup", grants)
    assert done.returncode == 0, done.stdout + done.stderr
    assert sql(postgres, "SELECT rolbypassrls FROM pg_roles WHERE rolname = 'app_backup'") == "f"
    def reads(*queries: str) -> subprocess.CompletedProcess:
        commands = [f for q in queries for f in ("-c", q)]
        return subprocess.run(
            ["docker", "exec", "-e", f"PGPASSWORD={PASSWORD}", postgres, "psql", "-h", "127.0.0.1",
             "-U", "app_backup", "-d", "app", "-qtA", *commands], capture_output=True, text=True)
    seen = reads("SET m.org TO 'a'", "SELECT id, org FROM m.t")
    assert seen.stdout.split() == ["1|a"], seen.stdout + seen.stderr
    assert "permission denied" in reads("SELECT secret FROM m.t").stderr
    assert "permission denied" in reads("SELECT id FROM m.gone").stderr


def test_a_dml_role_writes_the_owners_tables_now_and_later_and_holds_nothing_more(postgres):
    # T-7310: Antares' request role. Rows of the owner's tables, a table the owner creates after
    # the job (next week's history partition) included; no function kept from PUBLIC; no group it
    # was granted by hand; never row-level security bypassed.
    sql(postgres, "CREATE TABLE IF NOT EXISTS before_job (id int);"
        " CREATE OR REPLACE FUNCTION owner_only() RETURNS int LANGUAGE sql AS 'SELECT 1';"
        " REVOKE EXECUTE ON FUNCTION owner_only() FROM PUBLIC;"
        " ALTER TABLE before_job OWNER TO app; ALTER FUNCTION owner_only() OWNER TO app", db="app")
    sql(postgres, "CREATE ROLE grp_escape NOLOGIN; CREATE ROLE app_req LOGIN BYPASSRLS;"
        " GRANT grp_escape TO app_req")
    done = run_job(postgres, "app_req", "dml")
    assert done.returncode == 0, done.stdout + done.stderr
    sql(postgres, "SET ROLE app; CREATE TABLE after_job (id int)", db="app")
    for table in ("before_job", "after_job"):
        assert sql(postgres, f"SELECT has_table_privilege('app_req', '{table}', 'INSERT')"
                             f" AND has_table_privilege('app_req', '{table}', 'DELETE')", db="app") == "t"
    assert sql(postgres, "SELECT has_table_privilege('app_req', 'after_job', 'TRUNCATE')", db="app") == "f"
    assert sql(postgres, "SELECT has_function_privilege('app_req', 'owner_only()', 'EXECUTE')",
               db="app") == "f"
    assert sql(postgres, "SELECT rolbypassrls FROM pg_roles WHERE rolname = 'app_req'") == "f"
    assert sql(postgres, "SELECT pg_has_role('app_req', 'grp_escape', 'MEMBER')") == "f"


def test_a_dml_role_joins_the_groups_it_names_and_the_group_cannot_log_in(postgres):
    done = run_job(postgres, "app_maint", "dml", member_of='["app_maintenance"]')
    assert done.returncode == 0, done.stdout + done.stderr
    assert sql(postgres, "SELECT pg_has_role('app_maint', 'app_maintenance', 'MEMBER')") == "t"
    assert sql(postgres, "SELECT rolcanlogin OR rolbypassrls FROM pg_roles"
                         " WHERE rolname = 'app_maintenance'") == "f"
    # A rerun without the group takes the membership away.
    assert run_job(postgres, "app_maint", "dml").returncode == 0
    assert sql(postgres, "SELECT pg_has_role('app_maint', 'app_maintenance', 'MEMBER')") == "f"


def test_a_dml_role_that_owns_a_table_fails_the_job(postgres):
    # An owner can switch its table's row-level security off: the request role owns nothing.
    assert run_job(postgres, "app_owner", "dml").returncode == 0
    sql(postgres, "CREATE TABLE owned_by_request (id int); ALTER TABLE owned_by_request OWNER TO app_owner",
        db="app")
    done = run_job(postgres, "app_owner", "dml")
    assert done.returncode != 0
    assert "owns a table" in done.stdout + done.stderr


def test_a_role_made_earlier_as_superuser_loses_its_rights(postgres):
    sql(postgres, "ALTER ROLE app_read SUPERUSER CREATEDB CREATEROLE")
    done = run_job(postgres)
    assert done.returncode == 0, done.stdout + done.stderr
    assert sql(postgres, "SELECT rolsuper OR rolcreatedb OR rolcreaterole FROM pg_roles"
                         " WHERE rolname = 'app_read'") == "f"
