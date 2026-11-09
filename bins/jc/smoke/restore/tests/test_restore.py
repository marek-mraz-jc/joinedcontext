"""Selftests of the restore suite on fixtures: a stand-in for docker and up.sh (the live proof is a
rehearsal run).

    python3 -m pytest bins/jc/smoke/restore/tests -q
"""

import importlib.util
import json
import os
import sys
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
RESTORE = HERE.parent
sys.path.insert(0, str(RESTORE.parent))

from common import Results

_spec = importlib.util.spec_from_file_location("smoke_restore_suite", RESTORE / "suite.py")
suite = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(suite)


def checks(*rows):
    return {"checks": [{"id": i, "target": "t", "result": r, "actual": f"answered {r}"} for i, r in rows]}


def by_id(res):
    return {r["id"]: r for r in res.rows}


def test_the_backup_dir_must_be_under_rehearsal_home_and_never_evidence(tmp_path):
    rh = tmp_path / "rh"
    assert suite.backup_dir_ok(rh / "backups" / "prod-1", rh) == ""
    assert "not under" in suite.backup_dir_ok(tmp_path / "elsewhere", rh)
    assert "evidence" in suite.backup_dir_ok(rh / "evidence" / "x", rh)
    assert "evidence" in suite.backup_dir_ok(Path("/workspace/evidence/jc-smoke-restore"), Path("/workspace"))


def test_same_results_pass_and_any_change_or_missing_check_fails():
    res = Results("restore", "http://localhost:1")
    before = {r["id"]: r for r in checks(("a", "pass"), ("b", "fail"), ("c", "skip"), ("gone", "pass"))["checks"]}
    after = {r["id"]: r for r in checks(("a", "pass"), ("b", "fail"), ("c", "pass"), ("new", "skip"))["checks"]}
    suite.compare(res, "names", before, after, "run names")
    got = {k: v["result"] for k, v in by_id(res).items()}
    # b fails on both sides and still passes here: the copy must answer as before, not absolutely
    assert got == {"control.before-passes.names": "pass", "names/a": "pass", "names/b": "pass",
                   "names/c": "fail", "names/gone": "fail", "names/new": "fail"}


def test_a_before_run_without_a_single_pass_fails_its_control():
    # Both sides refused everything (say jc answered nothing): equal rows prove nothing.
    res = Results("restore", "http://localhost:1")
    same = {r["id"]: r for r in checks(("a", "fail"), ("b", "skip"))["checks"]}
    suite.compare(res, "endpoints", same, dict(same), "run endpoints")
    assert by_id(res)["control.before-passes.endpoints"]["result"] == "fail"
    res2 = Results("restore", "http://localhost:1")
    suite.compare(res2, "endpoints", same, None, "run endpoints")
    assert by_id(res2)["endpoints.results"]["result"] == "fail"


def test_paths_and_ports_move_to_the_copy(tmp_path):
    src, dst = tmp_path / "prod", tmp_path / "prod-restore"
    argv = ["--base-url", "http://localhost:42310", "--state", str(src / "smoke-state"), "--export", "/e",
            "--admin-url", "http://127.0.0.1:55660", "--log-file", str(src / "jc.log")]
    out = suite.rewrite_argv(argv, {42310: 42400, 55660: 55700}, src, dst)
    assert out == ["--base-url", "http://localhost:42400", "--state", str(dst / "smoke-state"), "--export", "/e",
                   "--admin-url", "http://127.0.0.1:55700", "--log-file", str(dst / "jc.log")]


def test_a_smoke_state_view_is_rebuilt_with_links_into_the_copy(tmp_path):
    src, dst = tmp_path / "prod", tmp_path / "prod-restore"
    (src / "state").mkdir(parents=True)
    (src / "state" / "portal-admin-client-secret").write_text("x")
    (src / "smoke-state").mkdir()
    os.symlink(src / "state" / "portal-admin-client-secret", src / "smoke-state" / "portal-admin-client-secret")
    assert not suite.copied(src / "smoke-state", src) and suite.copied(src / "state", src)
    suite.remap_view(src / "smoke-state", dst / "smoke-state", src, dst)
    link = dst / "smoke-state" / "portal-admin-client-secret"
    assert os.readlink(link) == str(dst / "state" / "portal-admin-client-secret")
    assert oct(os.stat(dst / "smoke-state").st_mode & 0o777) == "0o700"


def test_counts_parse_and_differ():
    c = suite.parse_counts("public.a|3\npublic.b|0\n\nnoise\n")
    assert c == {"public.a": 3, "public.b": 0}
    assert suite.count_diff(c, {"public.a": 3, "public.b": 0}) == []
    assert suite.count_diff(c, {"public.a": 2}) == ["public.a: 3 -> 2", "public.b: 0 -> absent"]


class FakeOps:
    """docker, pg_dump and up.sh stood in for: writes results the way the inner suites would."""

    def __init__(self, a, home, rhome, backup, after=None, restored=None, held=()):
        self.a, self.home, self.rhome, self.backup = a, home, rhome, backup
        self.dst = SimpleNamespace(container=f"rh-pg-{rhome.name}")
        self.counts = {}
        self.after_rows = after
        self.restored = restored
        self.held = list(held)
        self.calls = []
        self.torn = False

    def check_disk(self):
        return {"databases": ["antares", "portal"], "needBytes": 1}

    def suite(self, name, argv, out):
        self.calls.append((name, argv))
        out.mkdir(parents=True, exist_ok=True)
        side = "after" if out.parent.name == "after" else "before"
        rows = (self.after_rows or {}).get(name) if side == "after" else None
        doc = rows or checks(("one", "pass"), ("two", "fail"))
        (out / "results.json").write_text(json.dumps(doc))
        return 0, f"run {name}"

    def dump_roles(self):
        (self.backup / "roles.sql").write_text("-- roles")

    def dump(self, db):
        self.counts[db] = {"public.t": 2}
        (self.backup / f"{db}.dump").write_text("dump")

    def files(self):
        (self.backup / "home.tar").write_text("tar")

    def fresh(self):
        return self.held

    def relocate(self):
        return []

    def restore_roles(self):
        pass

    def restore(self, db):
        return (self.restored or {}).get(db, {"public.t": 2})

    def start(self):
        return 999999

    def converge(self, timeout):
        return 2

    def teardown(self):
        self.torn = True


def args(tmp_path, **kw):
    home = tmp_path / "rh" / "prod"
    home.mkdir(parents=True)
    (home / "jc.toml").write_text('url = "postgres://portal@127.0.0.1:55460/portal"\n')
    (home / "jc.pid").write_text("1\n")
    (home / "state").mkdir()
    (home / "state" / "portal-admin-client-secret").write_text("s")
    (home / "smoke-state").mkdir()
    os.symlink(home / "state" / "portal-admin-client-secret", home / "smoke-state" / "portal-admin-client-secret")
    a = SimpleNamespace(base_url="http://localhost:42310", home=str(home), suites="endpoints,names", extra={},
                        backup_dir=None, restore_env=None, restore_port=42311, restore_pg_port=55461,
                        jc_bin="/bin/true", converge=1, max_disk=0.92, suite_timeout=10, keep=False, export="/e",
                        state=str(home / "smoke-state"), log_file=str(home / "jc.log"), admin_url=None, admin_token_file=None)
    for k, v in kw.items():
        setattr(a, k, v)
    return a


def run_fake(tmp_path, monkeypatch, **fake):
    monkeypatch.setenv("REHEARSAL_HOME", str(tmp_path / "rh"))
    monkeypatch.setattr(suite, "unknown_slug", lambda base: (404, "/api/endpoint/x/ngsi-ld/v1/types"))
    made = {}

    def factory(*a):
        made["ops"] = FakeOps(*a, **fake)
        return made["ops"]
    res = Results("restore", "http://localhost:42310")
    meta = suite.execute(args(tmp_path), res, tmp_path / "out", factory)
    return res, meta, made["ops"]


def test_a_faithful_copy_passes_every_row_and_times_each_step(tmp_path, monkeypatch):
    res, meta, ops = run_fake(tmp_path, monkeypatch)
    rows = by_id(res)
    assert [r for r in res.rows if r["result"] != "pass"] == []
    for step in ("before", "dump.roles", "dump.antares", "dump.portal", "files", "fresh", "relocate", "restore.roles",
                 "restore.antares", "restore.portal", "start", "converge", "after"):
        assert rows[f"step.{step}"]["result"] == "pass" and step in meta["steps"]
    assert {"control.fresh-empty", "control.data.antares", "control.own-process", "control.unknown-slug",
            "endpoints/t/two", "names/t/one"} <= set(rows)
    # the backup lies under $REHEARSAL_HOME/backups, private, and the copy was torn down
    backup = Path(meta["backup"])
    assert backup.parent == tmp_path / "rh" / "backups" and oct(backup.stat().st_mode & 0o777) == "0o700"
    assert ops.torn and meta["rollbackSeconds"] >= 0
    # the after-run called the copy's port, the before-run the source's
    assert ops.calls[0][1][1] == "http://localhost:42310" and ops.calls[-1][1][1] == "http://localhost:42311"
    # --state (names declares it) moved to the copy's rebuilt view; endpoints takes no --state
    names_after = ops.calls[-1][1]
    assert names_after[names_after.index("--state") + 1] == str(tmp_path / "rh" / "prod-restore" / "smoke-state")
    assert (tmp_path / "rh" / "prod-restore" / "smoke-state" / "portal-admin-client-secret").is_symlink()


def test_a_changed_answer_a_lost_row_and_a_fresh_db_that_was_not_empty_fail(tmp_path, monkeypatch):
    res, _, _ = run_fake(tmp_path, monkeypatch, after={"names": checks(("one", "fail"), ("two", "fail"))},
                         restored={"portal": {"public.t": 1}}, held=["portal"])
    bad = {r["id"] for r in res.rows if r["result"] == "fail"}
    assert bad == {"names/t/one", "control.data.portal", "control.fresh-empty"}


def test_without_home_or_with_a_backup_dir_under_evidence_nothing_runs(tmp_path, monkeypatch):
    monkeypatch.setenv("REHEARSAL_HOME", str(tmp_path / "rh"))
    res = Results("restore", "http://localhost:1")
    suite.execute(args(tmp_path, home=None), res, tmp_path / "o", FakeOps)
    assert res.rows[0]["result"] == "skip" and "--home" in res.rows[0]["reason"]
    res = Results("restore", "http://localhost:1")
    a = args(tmp_path / "b", backup_dir=str(tmp_path / "rh" / "evidence" / "dumps"))
    suite.execute(a, res, tmp_path / "o", FakeOps)
    assert res.rows[0]["result"] == "fail" and "evidence" in res.rows[0]["actual"]
    assert not (tmp_path / "rh" / "evidence" / "dumps").exists()


def test_a_failed_step_stops_the_run_and_still_tears_the_copy_down(tmp_path, monkeypatch):
    class Broken(FakeOps):
        def restore(self, db):
            raise RuntimeError("pg_restore: relation exists")
    monkeypatch.setenv("REHEARSAL_HOME", str(tmp_path / "rh"))
    monkeypatch.setattr(suite, "unknown_slug", lambda base: (404, "/x"))
    made = []
    res = Results("restore", "http://localhost:42310")
    suite.execute(args(tmp_path), res, tmp_path / "out", lambda *a: made.append(Broken(*a)) or made[-1])
    rows = by_id(res)
    assert rows["step.restore.antares"]["result"] == "fail" and "relation exists" in rows["step.restore.antares"]["reason"]
    assert "step.start" not in rows and made[0].torn


def test_main_writes_results_and_answers_0_on_a_named_skip(tmp_path):
    assert suite.main(["--base-url", "http://localhost:1", "--out", str(tmp_path / "o")]) == 0
    doc = json.loads((tmp_path / "o" / "results.json").read_text())
    assert doc["totals"] == {"pass": 0, "fail": 0, "skip": 1}


def test_relocate_names_and_rewrites_every_path_of_the_source_home(tmp_path):
    src, dst = tmp_path / "prod", tmp_path / "prod-restore"
    (dst / "state" / "realms").mkdir(parents=True)
    (dst / "state" / "realms" / "r.json").write_text(f'{{"secret": "$(file:{src}/secrets/realm/K)"}}')
    (dst / "state" / "plain.txt").write_text("nothing here")
    (dst / "state" / "gateway").mkdir()
    os.symlink(f"{src}/state/gateway/org-cache/t1", dst / "state" / "gateway" / "current")
    os.symlink("org-cache/t2", dst / "state" / "gateway" / "relative")
    (dst / "pgdata").mkdir()
    (dst / "pgdata" / "x").write_text(str(src) + "/")  # not a part of the backup: left alone
    found = suite.relocate(dst, src)
    assert len(found) == 2
    assert (dst / "state" / "realms" / "r.json").read_text() == f'{{"secret": "$(file:{dst}/secrets/realm/K)"}}'
    assert os.readlink(dst / "state" / "gateway" / "current") == f"{dst}/state/gateway/org-cache/t1"
    assert os.readlink(dst / "state" / "gateway" / "relative") == "org-cache/t2"
    assert suite.relocate(dst, src) == []


def test_a_restored_file_naming_the_source_home_fails_the_relocatable_control(tmp_path, monkeypatch):
    class Pinned(FakeOps):
        def relocate(self):
            return ["state/realms/smartcity.json: 3x /old/home/"]
    monkeypatch.setenv("REHEARSAL_HOME", str(tmp_path / "rh"))
    monkeypatch.setattr(suite, "unknown_slug", lambda base: (404, "/x"))
    res = Results("restore", "http://localhost:42310")
    suite.execute(args(tmp_path), res, tmp_path / "out", Pinned)
    bad = [r["id"] for r in res.rows if r["result"] == "fail"]
    assert bad == ["control.relocatable"] and by_id(res)["step.after"]["result"] == "pass"


def test_a_check_id_repeated_per_target_is_compared_per_target(tmp_path):
    rows = [{"id": "anon.types", "target": "a", "result": "pass"}, {"id": "anon.types", "target": "b", "result": "pass"},
            {"id": "anon.types", "target": "b", "result": "skip"}]
    (tmp_path / "before").mkdir()
    (tmp_path / "before" / "results.json").write_text(json.dumps({"checks": rows}))
    (tmp_path / "after").mkdir()
    rows[0] = {**rows[0], "result": "fail"}
    (tmp_path / "after" / "results.json").write_text(json.dumps({"checks": rows}))
    before, after = suite.load_checks(tmp_path / "before"), suite.load_checks(tmp_path / "after")
    assert set(before) == {"a/anon.types", "b/anon.types", "b/anon.types#2"}
    res = Results("restore", "http://localhost:1")
    suite.compare(res, "endpoints", before, after, "run endpoints")
    assert [r["id"] for r in res.rows if r["result"] == "fail"] == ["endpoints/a/anon.types"]
