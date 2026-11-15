"""T-0046: the monthly restore drill (OPS-11, OPS-12).

The script under test is read out of the rendered ConfigMap, so what runs here is the text that
reaches the cluster. `kubectl` is a stub: it answers the source's primary, the clusters' row
counts and the replay lag from a fixture, records every Cluster it is asked to apply or delete
and keeps the last result patched into the result ConfigMap.
"""

import gzip
import json
import os
import re
import shutil
import stat
import subprocess
import textwrap
from pathlib import Path

import pytest
import yaml
from conftest import PROJECT_ROOT

NAME = "postgres-cluster-dr-drill"
# The keys smartcity's dumps are sealed to: its environment value (T-6982), never a literal here.
SMARTCITY_RECIPIENTS = yaml.safe_load(re.search(
    r"^\s*ageRecipients: (\[.*\])$",
    (PROJECT_ROOT / "ci/example-deployments/environments/smartcity/global.yaml.gotmpl").read_text(), re.M)[1])
DUMP = "2026-10-24T0230Z.sql.gz.age"


def age_identity(tmp_path: Path) -> tuple[Path, str]:
    """An age identity under `tmp_path` and its public key, from the real `age-keygen`: the dumps
    of these tests are sealed and opened by the `age` CLI, which writes the same files the
    chart's files/age.py does (the CI venv has no python3-cryptography to run age.py itself;
    tests/test_postgres_local_backup.py runs it in the image the cluster runs)."""
    if not shutil.which("age-keygen"):
        pytest.skip("age not installed: the dump is age-encrypted (T-5839), ci.yml installs it")
    identity = tmp_path / "identity.txt"
    if not identity.exists():
        subprocess.run(["age-keygen", "-o", str(identity)], check=True, capture_output=True)
    public = subprocess.run(["age-keygen", "-y", str(identity)], check=True, capture_output=True, text=True)
    return identity, public.stdout.strip()


def seal(tmp_path: Path, data: bytes) -> bytes:
    _, public = age_identity(tmp_path)
    return subprocess.run(["age", "-r", public], input=data, check=True, capture_output=True).stdout


def age_stub(tmp_path: Path, bin_: Path) -> None:
    """`python3 <age.py> decrypt --identity-file F`, as the drill calls it, answered by the age
    CLI with the test identity, and refusing as age.py does (`age.py: ...` on stderr)."""
    identity, _ = age_identity(tmp_path)
    (bin_ / "python3").write_text(textwrap.dedent(f"""\
        #!/bin/sh
        [ "$2" = decrypt ] && [ "$3" = --identity-file ] || {{ echo "stub python3: unexpected $*" >&2; exit 64; }}
        age -d -i {identity} 2>/dev/null || {{ echo "age.py: chunk 0 is damaged or the file is truncated" >&2; exit 1; }}
        """))
    (bin_ / "python3").chmod(0o755)


def one(docs: list[dict], kind: str, name: str) -> dict:
    found = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert len(found) == 1, f"expected exactly one {kind}/{name}, got {len(found)}"
    return found[0]


@pytest.fixture(scope="module")
def local(rendered):
    return rendered("local")


@pytest.fixture(scope="module")
def production(rendered):
    return rendered("production")


@pytest.fixture(scope="module")
def smartcity(rendered):
    return rendered("smartcity")


@pytest.fixture(scope="module")
def dev(rendered):
    return rendered("dev")


def env_of(cronjob: dict) -> dict[str, str]:
    container = cronjob["spec"]["jobTemplate"]["spec"]["template"]["spec"]["containers"][0]
    return {e["name"]: e["value"] for e in container["env"]}


TARGET = "2026-10-21T21:00:00Z"


def run(docs: list[dict], tmp_path: Path, rows: dict[str, tuple[str, str]], mem_mib: int = 8192,
        ready: bool = True, tables: dict[str, tuple[str, str]] | None = None,
        people: str = "3", schema: tuple[str, str] = ("20261001", "20261001"),
        dump: dict | None = None, handoff: bool = True, **extra) -> tuple[int, dict, list[str]]:
    """Runs the drill with a stub kubectl. `rows` maps a database to (source, copy) totals, a
    copy of "" meaning the database is missing from the copy; `tables` maps a database to the
    (source, point-in-time copy) per-table answers, `name|count` lines, and defaults to one
    table holding the source's total in both. `people` is what the copy's realm answers and
    `schema` the Portal's (source, copy) migration. Returns the exit code, the result the
    script patched into its ConfigMap, and the stub's log of creates and deletes; the
    manifest of the n-th create is in `tmp_path / f"created.{n}"`.

    `dump` stands for the nightly dump's volume (T-5556): `counts` the file the dump job wrote
    beside it, `restored` the per-database `name|count` answers of the restored copy, `data` the
    gzip bytes (a real gzip of a line by default), sealed with age, `raw` the file's bytes as
    they are (unsealed) and `psql_error` a line the restore's psql
    prints before it fails. With `dump`, the script first runs as the init container does
    (`dr-drill.sh dump`, stub initdb, postgres and psql, /backup under `tmp_path`, so gzip judges
    the file), then as the drill on its handoff. `handoff=False` drops what the init container
    wrote."""
    script = one(docs, "ConfigMap", f"{NAME}-script")["data"]["dr-drill.sh"]
    pitr = one(docs, "ConfigMap", f"{NAME}-script")["data"]["cluster-pitr.yaml"]
    (tmp_path / "cluster-pitr.yaml").write_text(pitr)
    (tmp_path / "meminfo").write_text(f"MemTotal: 16000000 kB\nMemAvailable: {mem_mib * 1024} kB\n")
    if tables is None:
        tables = {db: (f"public.t|{source}", f"public.t|{source}") for db, (source, _) in rows.items()}
    for db, (source, copy) in rows.items():
        (tmp_path / f"postgres-cluster-1.{db}").write_text(source)
        (tmp_path / f"postgres-cluster-1.{db}.tables").write_text(tables[db][0])
        (tmp_path / f"postgres-cluster-1.{db}.schema").write_text(schema[0])
        if copy != "":
            (tmp_path / f"postgres-cluster-drill-1.{db}").write_text(copy)
            (tmp_path / f"postgres-cluster-drill-1.{db}.tables").write_text(tables[db][1])
            (tmp_path / f"postgres-cluster-drill-1.{db}.schema").write_text(schema[1])
    backup = tmp_path / "backup"
    backup.mkdir()
    base = {**os.environ, "PATH": f"{tmp_path}:{os.environ['PATH']}", "MEMINFO": str(tmp_path / "meminfo"),
            "POLL_SECONDS": "0", "HANDOFF": str(tmp_path / "handoff")}
    if dump is not None:
        newest = backup / DUMP
        (backup / "2026-10-23T0230Z.sql.gz.age").write_bytes(seal(tmp_path, gzip.compress(b"-- older")))
        # A plain dump of before T-5839 sorts after the sealed ones and is never read.
        (backup / "2026-10-25T0230Z.sql.gz").write_bytes(gzip.compress(b"-- plain"))
        if "raw" in dump:
            newest.write_bytes(dump["raw"])
        else:
            newest.write_bytes(seal(tmp_path, dump.get("data", gzip.compress(b"SELECT 1;\n"))))
        age_stub(tmp_path, tmp_path)
        (backup / f"{newest.name}.counts").write_text(dump["counts"])
        for db, answer in dump.get("restored", {}).items():
            (tmp_path / f"restored.{db}").write_text(answer)
        fail = dump.get("psql_error")
        stubs = {
            # psql -X -At -d <db> -c <query> counts; psql -X -q ... -d postgres restores stdin.
            "psql": textwrap.dedent(f"""\
                #!/bin/sh
                [ "$2" = -At ] && exec cat {tmp_path}/restored.$4
                cat >{tmp_path}/restore-input.sql
                {f"echo '{fail}' >&2; exit 3" if fail else ""}
                """),
            "initdb": "#!/bin/sh\nmkdir -p \"$2\" && echo row > \"$2/PG_VERSION\"\n",
            "postgres": "#!/bin/sh\nexec sleep 30\n",
            "pg_isready": "#!/bin/sh\n",
        }
        for name, text in stubs.items():
            (tmp_path / name).write_text(text)
            (tmp_path / name).chmod(0o755)
        init = one(docs, "CronJob", NAME)["spec"]["jobTemplate"]["spec"]["template"]["spec"]["initContainers"][0]
        assert init["command"] == ["/bin/sh", "/opt/drill/dr-drill.sh", "dump"]
        first = subprocess.run(["sh", "-s", "dump"], input=script, text=True, capture_output=True, timeout=30,
                               env={**base, **{e["name"]: e["value"] for e in init["env"]},
                                    "BACKUP_DIR": str(backup), "DRILL_DATA": str(tmp_path / "drill"), **extra})
        assert first.returncode == 0, first.stderr
        assert not (tmp_path / "drill" / "data").exists(), "the restored copy is deleted in its container"
        if not handoff:
            subprocess.run(["rm", "-r", str(tmp_path / "handoff")], check=True)
    stub = tmp_path / "kubectl"
    stub.write_text(textwrap.dedent(f"""\
        #!/bin/sh
        d={tmp_path}
        case "$1" in
          create) echo "create" >> $d/log; n=$(grep -c create $d/log); cp "$3" $d/created.$n ;;
          delete) echo "delete $3" >> $d/log ;;
          wait) {"exit 0" if ready else "exit 1"} ;;
          get) echo postgres-cluster-1 ;;
          patch) printf '%s' "$7" > $d/result ;;
          exec)
            pod=$2; shift 9; db=$1; query=$3
            case "$query" in
              *pg_last_xact_replay_timestamp*) echo 42 ;;
              *"FROM pg_database"*) printf '%s\\n' {" ".join(rows)} ;;
              *clock_timestamp*) echo "{TARGET}|0/5000000" ;;
              *"FROM realm"*) echo {people} ;;
              *_sqlx_migrations*) cat "$d/$pod.$db.schema" ;;
              *"ORDER BY 1") cat "$d/$pod.$db.tables" 2>/dev/null || {{ echo "no database $db" >&2; exit 2; }} ;;
              *) cat "$d/$pod.$db" 2>/dev/null || {{ echo "no database $db" >&2; exit 2; }} ;;
            esac ;;
          *) echo "stub kubectl: unexpected $*" >&2; exit 64 ;;
        esac
        """))
    stub.chmod(stub.stat().st_mode | stat.S_IEXEC)
    env = {**base, **env_of(one(docs, "CronJob", NAME)), "TARGET_SETTLE_SECONDS": "0", **extra}
    script = script.replace("/opt/drill/cluster-pitr.yaml", str(tmp_path / "cluster-pitr.yaml"))
    result = subprocess.run(["sh", "-"], input=script.replace("/opt/drill/cluster.yaml", "/dev/null"),
                            text=True, capture_output=True, env=env, timeout=30)
    patched = json.loads((tmp_path / "result").read_text())["data"]
    log = (tmp_path / "log").read_text().split("\n") if (tmp_path / "log").exists() else []
    line = [json.loads(l) for l in result.stdout.splitlines() if l.startswith("{")]
    assert len(line) == 1 and line[0]["result"] == patched["result"], result.stdout + result.stderr
    return result.returncode, patched, [l for l in log if l]


def test_without_an_archive_the_drill_restores_nothing_and_says_so(local, tmp_path):
    assert env_of(one(local, "CronJob", NAME))["BACKUPS_ENABLED"] == "false"
    code, result, log = run(local, tmp_path, {"portal": ("10", "10")})
    assert code == 0
    assert result["result"] == "no-backup" and "no off-site backup to drill" in result["detail"]
    assert log == [], "nothing is created or deleted without an archive"


def test_a_full_restore_passes_with_rto_rpo_and_counts_read_from_the_clusters(production, tmp_path):
    code, result, log = run(production, tmp_path, {"portal": ("1000", "990"), "keycloak": ("50", "50")})
    assert code == 0, result
    assert result["result"] == "passed"
    assert result["rpoSeconds"] == "42" and result["rtoSeconds"].isdigit()
    assert result["detail"] == "portal=990/1000 keycloak=50/50"
    assert log[0] == "delete postgres-cluster-drill" and log.count("create") == 2
    assert log[-1] == "delete postgres-cluster-drill", "the copy is deleted when the drill ends"
    assert result["pitrTarget"] == TARGET and result["pitrRtoSeconds"].isdigit()
    assert result["pitrDetail"] == "2 tables match"


def test_the_second_restore_goes_back_to_the_instant_of_the_counts(production, tmp_path):
    code, result, log = run(production, tmp_path, {"portal": ("5", "5")})
    assert code == 0, result
    first = (tmp_path / "created.1").read_text()
    second = (tmp_path / "created.2").read_text()
    assert first == "" and "recoveryTarget" not in first, "the first restore is the latest"
    assert f'targetTime: "{TARGET}"' in second and "__TARGET_TIME__" not in second
    assert log == ["delete postgres-cluster-drill", "create", "delete postgres-cluster-drill",
                   "create", "delete postgres-cluster-drill"], "one copy at a time"


def test_a_table_the_point_in_time_restore_lost_fails_the_drill(production, tmp_path):
    tables = {"portal": ("public.a|100\npublic.b|7\npublic.c|1000", "public.a|100\npublic.c|985"),
              "antares": ("public.entities|0", "public.entities|1")}
    code, result, log = run(production, tmp_path, {"portal": ("1107", "1107"), "antares": ("0", "0")},
                            tables=tables)
    assert code != 0 and result["result"] == "failed"
    assert result["pitrDetail"] == "short: portal.public.b(missing)", "one row and 1.5 % are within tolerance"
    assert log[-1] == "delete postgres-cluster-drill"


def test_a_copy_without_people_or_at_another_schema_fails(production, tmp_path):
    code, result, _ = run(production, tmp_path, {"portal": ("9", "9"), "keycloak": ("9", "9")},
                          people="0", schema=("20261001", "20260901"))
    assert code != 0 and result["result"] == "failed"
    assert "keycloak(no enabled person in an enabled realm)" in result["detail"]
    assert "portal(schema 20260901 of 20261001)" in result["detail"]
    assert result["pitrTarget"] == TARGET and result["pitrRtoSeconds"] == "", "no second restore"


def test_a_database_short_of_rows_or_missing_fails_the_drill(production, tmp_path):
    code, result, log = run(production, tmp_path, {"portal": ("1000", "900"), "gitea": ("7", "")})
    assert code != 0
    assert result["result"] == "failed"
    assert "portal(900 of 1000 rows)" in result["detail"] and "gitea(missing)" in result["detail"]
    assert log[-1] == "delete postgres-cluster-drill"


@pytest.mark.parametrize("text", ['x","result":"passed","y":"\\', "tab\there\nnewline", "plain"])
def test_a_result_field_is_one_json_string_whatever_a_name_holds(text):
    # F-R2DEP3-015: the result JSON was built by concatenation, so a database or table name
    # holding `"` broke the patch, or carried a later "result" key of its own.
    script = Path(__file__).resolve().parent.parent / "components/postgres/charts/databases/files/dr-drill.sh"
    j = next(line for line in script.read_text().splitlines() if line.startswith("j() {"))
    said = subprocess.run(["sh", "-c", j + '\nj "$1"', "sh", text], capture_output=True, text=True, check=True).stdout
    assert json.loads(said) == "".join(c for c in text if ord(c) >= 0x20)


def test_a_restore_that_never_becomes_ready_fails_and_is_deleted(production, tmp_path):
    code, result, log = run(production, tmp_path, {"portal": ("1", "1")}, ready=False)
    assert code != 0 and result["result"] == "failed"
    assert "did not become ready" in result["detail"]
    assert log[-1] == "delete postgres-cluster-drill"


def test_a_full_node_skips_the_drill_visibly_and_restores_nothing(production, tmp_path):
    code, result, log = run(production, tmp_path, {"portal": ("1", "1")}, mem_mib=900,
                            TIMEOUT_MINUTES="0")
    assert code != 0 and result["result"] == "skipped"
    assert "900 MiB" in result["detail"]
    assert log == []


def test_the_job_is_held_to_its_own_names(production):
    role = one(production, "Role", NAME)
    named = [r for r in role["rules"] if r["verbs"] != ["create"] or r["resources"] != ["clusters"]]
    assert all(r.get("resourceNames") for r in named), "every verb but create is held to names"
    clusters = [r for r in role["rules"] if r["resources"] == ["clusters"] and r.get("resourceNames")]
    on_source = {v for r in clusters if "postgres-cluster" in r["resourceNames"] for v in r["verbs"]}
    assert on_source == {"get"}, f"the source Cluster is only read, got {on_source}"
    on_copy = {v for r in clusters if "postgres-cluster-drill" in r["resourceNames"] for v in r["verbs"]}
    assert "delete" in on_copy and "patch" not in on_copy
    exec_names = next(r for r in role["rules"] if r["resources"] == ["pods/exec"])["resourceNames"]
    assert "postgres-cluster-drill-1" in exec_names and "postgres-cluster-1" in exec_names
    assert not [d for d in production if d.get("kind") in ("ClusterRole", "ClusterRoleBinding")
                and d["metadata"]["name"].endswith("dr-drill")]
    copy = one(production, "ConfigMap", f"{NAME}-script")["data"]["cluster.yaml"]
    assert "ACCESS_SECRET_KEY" in copy and "serverName: postgres-cluster" in copy
    assert "joinedcontext.com/drill" in copy
    assert "recoveryTarget" not in copy
    pitr = one(production, "ConfigMap", f"{NAME}-script")["data"]["cluster-pitr.yaml"]
    pitr = yaml.safe_load(pitr)
    assert pitr["spec"]["bootstrap"]["recovery"].pop("recoveryTarget") == {"targetTime": "__TARGET_TIME__"}
    assert pitr == yaml.safe_load(copy), "the two restores differ only in their target"


# ---- the nightly local dump (T-5556)

COUNTS = "portal.public.a|100\nportal.public.b|7\nkeycloak.public.user_entity|3\n"


def test_prod_restores_its_local_dump_and_still_says_it_has_no_off_site_backup(smartcity, tmp_path):
    env = env_of(one(smartcity, "CronJob", NAME))
    assert env["BACKUPS_ENABLED"] == "false" and env["LOCAL_BACKUP"] == "true"
    restored = {"portal": "public.a|101\npublic.b|7", "keycloak": "public.user_entity|3"}
    code, result, log = run(smartcity, tmp_path, {}, dump={"counts": COUNTS, "restored": restored})
    assert code == 0, result
    assert result["result"] == "no-backup", "the dump is no off-site backup, the alert keeps saying so"
    assert result["localDump"] == "local-dump-passed"
    assert result["localDumpDetail"] == f"{DUMP}: 3 tables match", "the newest dump"
    assert result["localDumpRtoSeconds"].isdigit()
    newest = tmp_path / "backup" / DUMP
    assert result["localDumpBytes"] == str(newest.stat().st_size)
    assert log == [], "the dump restores in the drill's own pod: nothing created, no Cluster without an archive"



def test_the_dumps_role_grants_restore_under_the_drills_own_superuser(smartcity, tmp_path):
    """T-6725: pg_dumpall names each membership's grantor, and PostgreSQL 16 refuses one that is
    not the bootstrap superuser (`drill` here): jc-prod's first restore stopped at
    `GRANT pg_monitor ... GRANTED BY postgres`. The roles section loses the clause, a line after
    the first \\connect (a database's own text) reaches psql as written."""
    data = (b"CREATE ROLE postgres;\n"
            b"GRANT pg_monitor TO cnpg_metrics_exporter WITH INHERIT TRUE GRANTED BY postgres;\n"
            b"\\connect portal\n"
            b"GRANT x TO y GRANTED BY z;\n")
    restored = {"portal": "public.a|100\npublic.b|7", "keycloak": "public.user_entity|3"}
    code, result, _ = run(smartcity, tmp_path, {}, dump={"counts": COUNTS, "restored": restored, "data": gzip.compress(data)})
    assert code == 0 and result["localDump"] == "local-dump-passed", result
    assert (tmp_path / "restore-input.sql").read_text().splitlines() == [
        "CREATE ROLE postgres;",
        "GRANT pg_monitor TO cnpg_metrics_exporter WITH INHERIT TRUE;",
        "\\connect portal",
        "GRANT x TO y GRANTED BY z;",
    ]

def test_jc_prods_scratch_holds_every_database_it_restores(rendered):
    # jc-prod's databases were 21.7 GiB live on 2026-11-11; at the default 8Gi the kubelet evicted
    # the drill mid-restore (T-6725, dr-drill-manual-0919).
    job = one(rendered("jc-prod"), "CronJob", NAME)["spec"]["jobTemplate"]["spec"]["template"]["spec"]
    scratch = next(v for v in job["volumes"] if v["name"] == "dump-data")["emptyDir"]["sizeLimit"]
    assert scratch.endswith("Gi") and int(scratch[:-2]) >= 27, scratch


def test_dev_restores_the_dump_and_then_the_archive(dev, tmp_path):
    restored = {"portal": "public.a|100\npublic.b|7", "keycloak": "public.user_entity|3"}
    code, result, log = run(dev, tmp_path, {"portal": ("10", "10"), "keycloak": ("3", "3")},
                            dump={"counts": COUNTS, "restored": restored})
    assert code == 0, result
    assert result["result"] == "passed" and result["localDump"] == "local-dump-passed"
    assert log.count("create") == 2, "after the dump, the latest and the point-in-time copy"


def test_a_table_restored_between_its_two_counts_matches_and_one_outside_fails(smartcity, tmp_path):
    """T-6725, jc-prod 2026-11-11: subscriptions counted 70, then 52 in the dump pg_dumpall took
    minutes later. A count between the dump job's two counts matches; one beyond both does not."""
    counts = "portal.public.a|70|50\nportal.public.b|7\nkeycloak.public.user_entity|3\n"
    restored = {"portal": "public.a|52\npublic.b|7", "keycloak": "public.user_entity|3"}
    code, result, _ = run(smartcity, tmp_path, {}, dump={"counts": counts, "restored": restored})
    assert code == 0 and result["localDump"] == "local-dump-passed", result
    restored["portal"] = "public.a|40\npublic.b|7"
    (tmp_path / "beyond").mkdir()
    code, result, _ = run(smartcity, tmp_path / "beyond", {}, dump={"counts": counts, "restored": restored})
    assert code != 0 and result["localDumpDetail"] == f"{DUMP} short: portal.public.a(40 of 50..70)", result


def test_a_dump_missing_a_table_fails_the_drill_and_names_the_table(smartcity, tmp_path):
    restored = {"portal": "public.a|100", "keycloak": "public.user_entity|3"}
    code, result, log = run(smartcity, tmp_path, {}, dump={"counts": COUNTS, "restored": restored})
    assert code != 0
    assert result["result"] == "failed" and result["localDump"] == "failed"
    assert result["localDumpDetail"] == f"{DUMP} short: portal.public.b(missing)"
    assert log == []


def test_a_truncated_dump_fails_the_drill(smartcity, tmp_path):
    whole = gzip.compress(os.urandom(4096))
    code, result, log = run(smartcity, tmp_path, {}, dump={"counts": COUNTS, "data": whole[:2000]})
    assert code != 0 and result["result"] == "failed" and result["localDump"] == "failed"
    assert "did not restore: gzip:" in result["localDumpDetail"]
    assert "unexpected end of file" in result["localDumpDetail"]
    assert result["localDumpRtoSeconds"] == ""


def test_a_truncated_or_foreign_sealed_dump_fails_the_drill_at_the_decryption(smartcity, tmp_path):
    sealed = seal(tmp_path, gzip.compress(os.urandom(200_000)))
    code, result, log = run(smartcity, tmp_path, {}, dump={"counts": COUNTS, "raw": sealed[:70_000]})
    assert code != 0 and result["localDump"] == "failed", result
    assert result["localDumpDetail"] == f"{DUMP} did not restore: age.py: chunk 0 is damaged or the file is truncated"
    assert log == []


def test_a_dump_file_that_is_not_sealed_is_refused(smartcity, tmp_path):
    code, result, _ = run(smartcity, tmp_path, {}, dump={"counts": COUNTS, "raw": gzip.compress(b"SELECT 1;\n")})
    assert code != 0 and result["localDump"] == "failed"
    assert result["localDumpDetail"].startswith(f"{DUMP} did not restore: age.py:"), result


def test_a_refused_statement_fails_without_copying_a_value_into_the_result(smartcity, tmp_path):
    error = 'psql:<stdin>:812: ERROR:  invalid input syntax for type integer: "4111 1111 1111 1111"'
    code, result, _ = run(smartcity, tmp_path, {}, dump={"counts": COUNTS, "psql_error": error})
    assert code != 0 and result["localDump"] == "failed"
    assert result["localDumpDetail"] == (f"{DUMP} did not restore: "
                                         "psql:<stdin>:812: ERROR:  invalid input syntax for type integer: ")
    assert "4111" not in json.dumps(result)


def test_a_dump_without_its_counts_fails(smartcity, tmp_path):
    code, result, _ = run(smartcity, tmp_path, {}, dump={"counts": ""})
    assert code != 0 and result["localDump"] == "failed"
    assert result["localDumpDetail"] == f"no table counts beside {DUMP}"


def test_a_dump_restore_that_left_no_verdict_fails(smartcity, tmp_path):
    code, result, _ = run(smartcity, tmp_path, {}, dump={"counts": COUNTS}, handoff=False)
    assert code != 0 and result["localDump"] == "failed"
    assert result["localDumpDetail"] == "the dump's restore container left no verdict"


def test_a_full_node_skips_the_dump_and_the_drill_without_waiting_twice(smartcity, tmp_path):
    code, result, log = run(smartcity, tmp_path, {}, dump={"counts": COUNTS}, mem_mib=900,
                            TIMEOUT_MINUTES="0")
    assert code != 0 and result["result"] == "skipped" and result["localDump"] == "skipped"
    assert "900 MiB" in result["detail"] and log == []


def test_without_the_local_dump_the_stage_is_skipped_not_failed(production, tmp_path):
    assert env_of(one(production, "CronJob", NAME))["LOCAL_BACKUP"] == "false"
    code, result, log = run(production, tmp_path, {"portal": ("5", "5")})
    assert code == 0 and result["result"] == "passed"
    assert result["localDump"] == "off" and result["localDumpDetail"] == ""
    pod = one(production, "CronJob", NAME)["spec"]["jobTemplate"]["spec"]["template"]["spec"]
    assert "initContainers" not in pod and "backup" not in [v["name"] for v in pod["volumes"]]


def pod_of(docs: list[dict]) -> dict:
    return one(docs, "CronJob", NAME)["spec"]["jobTemplate"]["spec"]["template"]["spec"]


@pytest.mark.parametrize("env", ["production", "smartcity"])
def test_the_drill_creates_no_pod_and_execs_only_into_the_clusters(env, request):
    """The integrator's review of T-5556: `pods: create` cannot be held to a name, so a drill
    with it could start a pod as any ServiceAccount of the namespace with any Secret."""
    docs = request.getfixturevalue(env)
    rules = one(docs, "Role", NAME)["rules"]
    assert not [r for r in rules if "pods" in r["resources"]], "no pod rights at all"
    exec_names = next(r for r in rules if r["resources"] == ["pods/exec"])["resourceNames"]
    assert sorted(exec_names) == sorted([f"postgres-cluster-{n}" for n in range(1, 10)] + ["postgres-cluster-drill-1"])
    assert "dump-pod.yaml" not in one(docs, "ConfigMap", f"{NAME}-script")["data"]


def test_only_the_drill_container_holds_the_api_token(smartcity):
    pod = pod_of(smartcity)
    assert pod["automountServiceAccountToken"] is False and pod["enableServiceLinks"] is False
    drill, = pod["containers"]
    init, = pod["initContainers"]
    token = next(m for m in drill["volumeMounts"] if m["name"] == "kube-api-access")
    assert token == {"name": "kube-api-access", "mountPath": "/var/run/secrets/kubernetes.io/serviceaccount",
                     "readOnly": True}
    volume = next(v for v in pod["volumes"] if v["name"] == "kube-api-access")["projected"]["sources"]
    assert [list(s) for s in volume] == [["serviceAccountToken"], ["configMap"], ["downwardAPI"]]
    assert "kube-api-access" not in [m["name"] for m in init["volumeMounts"]]
    assert not [e for e in init["env"] if "valueFrom" in e], "no Secret reaches the dump's copy"


def test_the_dump_restores_closed_in_the_init_container(smartcity):
    pod = pod_of(smartcity)
    init, = pod["initContainers"]
    image = yaml.safe_load(one(smartcity, "ConfigMap", f"{NAME}-script")["data"]["cluster.yaml"])["spec"]["imageName"]
    assert init["image"] == image, "restored with the cluster's own Postgres"
    assert "ports" not in init
    sc = init["securityContext"]
    assert sc["readOnlyRootFilesystem"] is True and sc["allowPrivilegeEscalation"] is False
    assert sc["runAsUser"] == 26 and pod["securityContext"]["runAsNonRoot"] is True
    backup = next(v for v in pod["volumes"] if v["name"] == "backup")
    assert backup["persistentVolumeClaim"] == {"claimName": "postgres-cluster-local-backup", "readOnly": True}
    assert next(m for m in init["volumeMounts"] if m["name"] == "backup")["readOnly"] is True
    assert "emptyDir" in next(v for v in pod["volumes"] if v["name"] == "dump-data")
    script = one(smartcity, "ConfigMap", f"{NAME}-script")["data"]["dr-drill.sh"]
    assert "-c listen_addresses= " in script, "Postgres listens on no port"


def test_the_dump_job_writes_the_counts_and_fails_a_dump_over_80_percent(smartcity):
    job = one(smartcity, "Job", "postgres-cluster-local-backup-now")["spec"]["template"]["spec"]
    container = job["containers"][0]
    env = {e["name"]: e.get("value") for e in container["env"]}
    # T-5840: the dumps' cap grows past the PVC request, which local-path cannot expand.
    assert env["VOLUME_SIZE"] == "40Gi"
    pvc = one(smartcity, "PersistentVolumeClaim", "postgres-cluster-local-backup")
    assert pvc["spec"]["resources"]["requests"]["storage"] == "10Gi", "the bound PVC's request never moves"
    script = container["command"][2]
    assert script.index('"$file.counts"') < script.index('mv "$file.part" "$file"')
    assert "over 80 % of the" in script and "-name '*.counts'" in script


# ---- the dump job itself (T-5556 deployer hold: an hourly apply must not fill Postgres's disk)


def dump_job(docs: list[dict], tmp_path: Path, mode: str, dump_bytes: int = 4096, size: str = "1Mi",
             disk_kib: tuple[int, int] = (10_000_000, 5_000_000), exclude: str = "",
             seal_fails: bool = False, rows_after: int = 3) -> tuple[int, str, Path]:
    """Runs the rendered dump script of the CronJob (mode dump) or the apply's Job (mode bind)
    over a scratch directory: psql answers two databases, pg_dumpall writes `dump_bytes` of noise
    (also kept in `dumpall.out`) and records its arguments, df reports `disk_kib` (total,
    available); after pg_dumpall ran, psql counts `rows_after` rows. `python3 age.py encrypt` is the age CLI sealing to the recipients it is handed
    (kept in `age.args`) and to the test identity, which stands for the drill key."""
    name = "postgres-cluster-local-backup"
    job = one(docs, "CronJob", name)["spec"]["jobTemplate"]["spec"] if mode == "dump" else one(docs, "Job", f"{name}-now")["spec"]
    container = job["template"]["spec"]["containers"][0]
    env = {e["name"]: e["value"] for e in container["env"] if "value" in e}
    assert env["MODE"] == mode
    backup, scratch, bin_ = tmp_path / "backup", tmp_path / "tmp", tmp_path / "bin"
    for d in (backup, scratch, bin_):
        d.mkdir(exist_ok=True)
    stubs = {
        "psql": f"""case "$*" in *datname*) printf 'antares\\nportal\\n' ;;
  *) if [ -e {tmp_path}/dumpall.args ]; then echo "public.t|{rows_after}"; else echo "public.t|3"; fi ;; esac""",
        "pg_dumpall": f"""echo "$*" > {tmp_path}/dumpall.args; head -c {dump_bytes} /dev/urandom | tee {tmp_path}/dumpall.out""",
        "python3": "exit 1" if seal_fails else f"""[ "$2 $3 $4" = "encrypt --identity-file /key/key" ] || exit 64; shift 4
echo "$*" > {tmp_path}/age.args
set -- $(for r; do printf -- '-r %s ' "$r"; done)
exec age "$@" -r {age_identity(tmp_path)[1]}""",
        "df": f"""printf 'Filesystem 1024-blocks Used Available Capacity Mounted\\n/dev/sda3 {disk_kib[0]} 0 {disk_kib[1]} 50%% /backup\\n'""",
    }
    for stub, body in stubs.items():
        (bin_ / stub).write_text(f"#!/bin/sh\n{body}\n")
        (bin_ / stub).chmod(0o755)
    script = container["command"][2].replace("/tmp/", f"{scratch}/").replace("/backup", str(backup))
    env |= {"VOLUME_SIZE": size, "PATH": f"{bin_}:{os.environ['PATH']}"}
    if exclude:
        env["EXCLUDE_DATABASES"] = exclude
    done = subprocess.run(["sh", "-c", script], env=env, capture_output=True, text=True, timeout=60)
    return done.returncode, done.stdout + done.stderr, backup


def test_the_apply_dumps_only_when_no_dump_of_the_last_20_h_is_there(smartcity, tmp_path):
    code, log, backup = dump_job(smartcity, tmp_path, "bind")
    assert code == 0, log
    assert len(list(backup.glob("*.sql.gz.age"))) == 1 and len(list(backup.glob("*.sql.gz.age.counts"))) == 1
    (tmp_path / "dumpall.args").unlink()
    code, log, backup = dump_job(smartcity, tmp_path, "bind")
    assert code == 0 and "none at this apply" in log, log
    assert not (tmp_path / "dumpall.args").exists(), "the second apply wrote no dump"
    assert len(list(backup.glob("*.sql.gz.age"))) == 1


def test_the_dump_on_the_volume_is_sealed_to_the_operator_and_the_drill_key(smartcity, tmp_path):
    """T-5839: no plain byte of the dump reaches the volume; the drill key opens it."""
    code, log, backup = dump_job(smartcity, tmp_path, "dump")
    assert code == 0, log
    dump, = backup.glob("*.sql.gz.age")
    assert dump.read_bytes().startswith(b"age-encryption.org/v1\n")
    assert (tmp_path / "age.args").read_text().split() == SMARTCITY_RECIPIENTS, \
        "smartcity's operator key (its environment value; the key itself: the private environments/tests, T-6984)"
    identity, _ = age_identity(tmp_path)
    opened = subprocess.run(["age", "-d", "-i", str(identity), str(dump)], check=True, capture_output=True).stdout
    assert gzip.decompress(opened) == (tmp_path / "dumpall.out").read_bytes()
    assert [p.name for p in backup.iterdir() if not p.name.endswith((".age", ".counts"))] == []


def test_a_dump_that_cannot_be_sealed_leaves_nothing_on_the_volume(smartcity, tmp_path):
    code, log, backup = dump_job(smartcity, tmp_path, "dump", seal_fails=True)
    assert code != 0 and "could not be encrypted" in log, log
    assert list(backup.iterdir()) == []


def test_the_nightly_dump_runs_although_a_recent_dump_is_there(smartcity, tmp_path):
    (tmp_path / "backup").mkdir()
    (tmp_path / "backup" / "2026-10-25T0230Z.sql.gz.age").write_bytes(b"x")
    code, log, _ = dump_job(smartcity, tmp_path, "dump")
    assert code == 0, log
    assert (tmp_path / "dumpall.args").read_text().split() == ["--clean", "--if-exists"]


def test_a_dump_that_outgrows_the_volume_stops_there_and_is_deleted(smartcity, tmp_path):
    code, log, backup = dump_job(smartcity, tmp_path, "dump", dump_bytes=3 * 1048576, size="1Mi")
    assert code != 0 and "outgrew the" in log and "excludeDatabases" in log, log
    assert list(backup.iterdir()) == [], "neither the part nor its counts stay on the volume"


def test_old_dumps_make_room_before_the_dump_and_full_volume_refuses(smartcity, tmp_path):
    backup = tmp_path / "backup"
    backup.mkdir()
    old = backup / "2026-10-01T0230Z.sql.gz"
    old.write_bytes(os.urandom(2 * 1048576))
    os.utime(old, (0, 0))
    code, log, _ = dump_job(smartcity, tmp_path, "dump", size="1Mi")
    assert code == 0 and not old.exists(), log
    recent = backup / "2026-10-25T0100Z.sql.gz.age"
    recent.write_bytes(os.urandom(1048576))
    code, log, _ = dump_job(smartcity, tmp_path, "dump", size="1Mi")
    assert code != 0 and "fill the 1Mi of localBackup.cap" in log, log


def test_a_dump_that_would_leave_the_node_disk_under_10_percent_free_does_not_start(smartcity, tmp_path):
    code, log, backup = dump_job(smartcity, tmp_path, "dump", size="1Mi", disk_kib=(10_000, 1_500))
    assert code != 0 and "under 10 % free" in log, log
    assert not (tmp_path / "dumpall.args").exists() and list(backup.iterdir()) == []


def test_an_excluded_database_is_neither_counted_nor_dumped(dev, smartcity, tmp_path):
    env = {e["name"]: e.get("value") for e in one(dev, "CronJob", "postgres-cluster-local-backup")
           ["spec"]["jobTemplate"]["spec"]["template"]["spec"]["containers"][0]["env"]}
    assert env["EXCLUDE_DATABASES"] == "antares ckan_datastore" and env["KEEP_DAYS"] == "1"
    assert env_of(one(smartcity, "CronJob", NAME))["LOCAL_BACKUP"] == "true"
    code, log, backup = dump_job(smartcity, tmp_path, "dump", exclude="antares")
    assert code == 0, log
    assert (tmp_path / "dumpall.args").read_text().split() == ["--clean", "--if-exists", "--exclude-database=antares"]
    counts, = backup.glob("*.counts")
    assert counts.read_text() == "portal.public.t|3\n"


def test_a_table_written_during_the_dump_carries_its_count_after_it(smartcity, tmp_path):
    """T-6725: pg_dumpall reaches each database minutes after the counts; a table that changed
    meanwhile gets its count after the dump as a third field, an unchanged one stays as it was."""
    code, log, backup = dump_job(smartcity, tmp_path, "dump", rows_after=5)
    assert code == 0, log
    counts, = backup.glob("*.counts")
    assert counts.read_text() == "antares.public.t|3|5\nportal.public.t|3|5\n"


@pytest.mark.parametrize("env", ["dev", "smartcity"])
def test_the_drill_and_the_dump_share_one_selinux_level(env, request):
    """T-6298: on an SELinux node each pod relabels the local-path volume to its own random MCS
    categories, so the drill read "Permission denied" on the dump the backup pod had written
    (prod 2026-11-01). Both pods carry one fixed level, still container_t and unprivileged."""
    docs = request.getfixturevalue(env)
    drill = pod_of(docs)
    dump = one(docs, "CronJob", "postgres-cluster-local-backup")["spec"]["jobTemplate"]["spec"]["template"]["spec"]
    for pod in (drill, dump):
        assert pod["securityContext"]["seLinuxOptions"] == {"level": "s0:c217,c602"}
        assert pod["securityContext"]["runAsNonRoot"] is True
        for c in pod["containers"] + pod.get("initContainers", []):
            assert "seLinuxOptions" not in c.get("securityContext", {}), "no spc_t or other type override"
            assert not c["securityContext"].get("privileged")
    backup = next(m for m in drill["initContainers"][0]["volumeMounts"] if m["mountPath"] == "/backup")
    assert backup["readOnly"] is True
