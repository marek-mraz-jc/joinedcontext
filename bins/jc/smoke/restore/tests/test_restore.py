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


def test_a_restored_db_is_vacuumed_and_analyzed_before_the_copy_serves(tmp_path, monkeypatch):
    dump = tmp_path / "antares.dump"
    dump.write_bytes(b"dump")
    ran = []

    def fake_run(argv, **kw):
        ran.append(argv[argv.index("rh-pg-copy") + 1:])
        return SimpleNamespace(returncode=0, stdout=b"", stderr=b"")
    monkeypatch.setattr(suite.subprocess, "run", fake_run)
    suite.Pg("rh-pg-copy", 55461).restore("antares", dump)
    assert [r[0] for r in ran] == ["pg_restore", "vacuumdb"]
    assert ran[1] == ["vacuumdb", "--analyze", "-p", "55461", "-d", "antares"]


class FakeOps:
    """docker, pg_dump and up.sh stood in for: writes results the way the inner suites would."""

    def __init__(self, a, home, rhome, backup, after=None, restored=None, held=(), roles=None, born=(), unsettled=(), pending=()):
        self.a, self.home, self.rhome, self.backup = a, home, rhome, backup
        self.dst = SimpleNamespace(container=f"rh-pg-{rhome.name}")
        self.counts = {}
        self.after_rows = after
        self.restored = restored
        self.held = list(held)
        self.calls = []
        self.torn = False
        # pid -> --role: the source (pid 1 in jc.pid) and the copy (999999) held alike by default
        self.roles = roles or {"1": HELD, "999999": HELD}
        self.started_role = None
        self.born_types = born
        self.unsettled = unsettled
        self.pending = pending

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
        # the tar of a faithful backup: the runner's files of the source home land in the copy's
        self.rhome.mkdir(parents=True, exist_ok=True)
        for f in suite.HOME_FILES:
            if (self.home / f).exists():
                (self.rhome / f).write_bytes((self.home / f).read_bytes())
        return self.held

    def relocate(self):
        return []

    def restore_roles(self):
        pass

    def restore(self, db):
        return (self.restored or {}).get(db, {"public.t": 2})

    def start(self, role):
        self.started_role = role
        return 999999

    def role(self, pid):
        return self.roles.get(str(pid), "")

    def converge(self, timeout):
        return 2

    def settle(self, base, paths, timeout):
        self.settled = (base, paths, timeout)
        return list(self.unsettled)

    def drain(self, timeout, window):
        self.drained = (timeout, window, len(self.calls))
        return list(self.pending), 0.0 if not self.pending else timeout

    def born(self, since):
        self.since = since
        return set(self.born_types)

    def teardown(self):
        self.torn = True


HELD = "kaisatsu,dataroom,daiya,antares,gateway,portal,shako,model-tools,assistant,viz"


def args(tmp_path, **kw):
    home = tmp_path / "rh" / "prod"
    home.mkdir(parents=True)
    (home / "jc.toml").write_text('url = "postgres://portal@127.0.0.1:55460/portal"\n')
    (home / "jc.pid").write_text("1\n")
    (home / "state").mkdir()
    (home / "state" / "portal-admin-client-secret").write_text("s")
    (home / "smoke-state").mkdir()
    os.symlink(home / "state" / "portal-admin-client-secret", home / "smoke-state" / "portal-admin-client-secret")
    (home / "ekiben-resources.yaml").write_text("caches: {}\n")
    (home / "gtfs-realtime.proto").write_text("syntax = 'proto2';\n")
    a = SimpleNamespace(base_url="http://localhost:42310", home=str(home), suites="endpoints,names", extra={},
                        backup_dir=None, restore_env=None, restore_port=42311, restore_pg_port=55461,
                        jc_bin="/bin/true", converge=1, settle=7, drain=0, drain_window=900, max_disk=0.92, suite_timeout=10, keep=False, export="/e",
                        state=str(home / "smoke-state"), log_file=str(home / "jc.log"), admin_url=None, admin_token_file=None)
    for k, v in kw.items():
        setattr(a, k, v)
    return a


def run_fake(tmp_path, monkeypatch, a=None, **fake):
    monkeypatch.setenv("REHEARSAL_HOME", str(tmp_path / "rh"))
    monkeypatch.setattr(suite, "unknown_slug", lambda base: (404, "/api/endpoint/x/ngsi-ld/v1/types"))
    made = {}

    def factory(*a):
        made["ops"] = FakeOps(*a, **fake)
        return made["ops"]
    res = Results("restore", "http://localhost:42310")
    meta = suite.execute(a or args(tmp_path), res, tmp_path / "out", factory)
    return res, meta, made["ops"]


def test_a_faithful_copy_passes_every_row_and_times_each_step(tmp_path, monkeypatch):
    res, meta, ops = run_fake(tmp_path, monkeypatch)
    rows = by_id(res)
    assert [r for r in res.rows if r["result"] != "pass"] == []
    for step in ("before", "dump.roles", "dump.antares", "dump.portal", "files", "fresh", "relocate", "restore.roles",
                 "restore.antares", "restore.portal", "start", "converge", "after"):
        assert rows[f"step.{step}"]["result"] == "pass" and step in meta["steps"]
    assert {"control.fresh-empty", "control.data.antares", "control.own-process", "control.unknown-slug",
            "control.role", "control.held", "endpoints/t/two", "names/t/one"} <= set(rows)
    # the copy started on the held source's role
    assert ops.started_role == HELD and rows["control.held"]["actual"] == "held on both"
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


def per_type(*rows):
    return {"checks": [{"id": i, "target": f"ep {t}", "result": r, "actual": r} for t, i, r in rows]}


def test_a_type_first_written_after_the_before_run_is_no_restore_fault():
    # smoke 43666862140 (T-6953): Vehicle file rows appeared after, Pipelines wrote the type since.
    before = suite.load_rows(per_type(("Event", "anon.ogc.csv", "pass")))
    after = suite.load_rows(per_type(("Event", "anon.ogc.csv", "pass"), ("Vehicle", "anon.ogc.csv", "pass")))
    res = Results("restore", "x")
    suite.compare(res, "endpoints", before, after, "run endpoints", {"t1 Vehicle"}, {"ep": "t1"})
    rows = by_id(res)
    assert [r for r in res.rows if r["result"] == "fail"] == []
    assert rows["endpoints/ep Vehicle/anon.ogc.csv"]["result"] == "skip"
    assert rows["endpoints/ep Event/anon.ogc.csv"]["result"] == "pass"


def test_a_type_held_at_dump_time_and_missing_after_stays_red():
    # present before and absent after fails even for a born type; absent before fails without proof
    before = suite.load_rows(per_type(("Event", "anon.ogc.csv", "pass"), ("Tree", "anon.count", "pass")))
    after = suite.load_rows(per_type(("Vehicle", "anon.ogc.csv", "pass"), ("Tree", "anon.count", "fail")))
    res = Results("restore", "x")
    suite.compare(res, "endpoints", before, after, "run endpoints", {"t1 Event", "t1 Tree"}, {"ep": "t1"})
    bad = {r["id"] for r in res.rows if r["result"] == "fail"}
    assert bad == {"endpoints/ep Event/anon.ogc.csv", "endpoints/ep Vehicle/anon.ogc.csv",
                   "endpoints/ep Tree/anon.count"}


def test_born_types_needs_every_iri_of_a_short_name_new_in_its_tenant():
    text = ("t1|https://smartdatamodels.org/dataModel.Transportation/Vehicle|new\n"
            "t1|https://uri.etsi.org/ngsi-ld/default-context/Event|new\n"
            "t1|https://example.org/ns#Event|old\n"
            "t1|https://smartdatamodels.org/dataModel.Environment/AirQualityObserved|old\n"
            "t2|https://uri.etsi.org/ngsi-ld/default-context/Event|new\n")
    assert suite.born_types(text) == {"t1 Vehicle", "t2 Event"}
    sql = suite.BORN_SQL.format(since="2026-11-15T03:00:00Z")
    assert "'2026-11-15T03:00:00Z'::timestamptz" in sql and "GROUP BY tenant_id, t" in sql


def test_a_type_another_tenant_held_before_is_still_born_in_the_endpoints_tenant():
    # smoke 43896843578 (T-7146): q7gad2u6 listed no TrafficFlowObserved before and 1000 its Pipeline
    # wrote after; another Endpoint's tenant held older seeded rows of the type, so a per-type proof
    # (born: []) failed the row. The proof is per tenant; a slug of no known tenant has none.
    born = suite.born_types("helsinki|https://uri.etsi.org/ngsi-ld/default-context/TrafficFlowObserved|new\n"
                            "zilina|https://uri.etsi.org/ngsi-ld/default-context/TrafficFlowObserved|old\n")
    before = suite.load_rows({"checks": []})
    after = suite.load_rows({"checks": [
        {"id": "anon.types.written", "target": f"{slug} TrafficFlowObserved", "result": "pass", "actual": "count=1000"}
        for slug in ("q7", "4aw6", "gone")]})
    res = Results("restore", "x")
    suite.compare(res, "endpoints", before, after, "run endpoints", born, {"q7": "helsinki", "4aw6": "zilina"})
    rows = by_id(res)
    assert rows["endpoints/q7 TrafficFlowObserved/anon.types.written"]["result"] == "skip"
    assert rows["endpoints/4aw6 TrafficFlowObserved/anon.types.written"]["result"] == "fail"
    assert rows["endpoints/gone TrafficFlowObserved/anon.types.written"]["result"] == "fail"


def test_the_compare_excuses_only_the_types_the_copy_proves_born(tmp_path, monkeypatch):
    after = {"endpoints": {**per_type(("Event", "one", "pass"), ("Vehicle", "one", "pass")), "tenants": {"ep": "t1"}}}
    res, meta, ops = run_fake(tmp_path, monkeypatch, after=after, born={"t1 Vehicle", "t2 Event"})
    rows = by_id(res)
    assert rows["step.born"]["result"] == "pass" and meta["born"] == ["t1 Vehicle", "t2 Event"]
    assert meta["bornSince"] == ops.since
    assert rows["endpoints/ep Vehicle/one"]["result"] == "skip"
    assert rows["endpoints/ep Event/one"]["result"] == "fail"  # absent before, no proof


TYPES = "/api/endpoint/ep/ngsi-ld/v1/types"


def test_the_copy_is_asked_only_the_discovery_calls_the_source_answered_whole():
    rows = {"checks": [
        {"id": "anon.types.match", "target": "ep", "result": "pass", "call": f"GET {TYPES}", "actual": "missing []"},
        {"id": "anon.types.match", "target": "cut", "result": "pass", "call": "GET /api/endpoint/cut/ngsi-ld/v1/types",
         "actual": "missing [], extra []; warning 199 antares \"entity scan ceiling reached\""},
        {"id": "anon.types.match", "target": "red", "result": "fail", "call": "GET /api/endpoint/red/ngsi-ld/v1/types",
         "actual": "missing ['Tree']"},
        {"id": "person.types.match", "target": "p", "result": "pass", "call": "GET /api/endpoint/p/ngsi-ld/v1/types",
         "actual": "missing []"}]}
    assert suite.discovery_calls(suite.load_rows(rows)) == [TYPES]
    assert suite.discovery_calls(None) == []


COUNT = "/api/endpoint/ep/ngsi-ld/v1/entities?type=AirQualityObserved&count=true&limit=0"


def test_the_copy_is_also_asked_the_count_reads_the_source_answered():
    # smoke 43698237064: the copy's /types settled, its AirQualityObserved count 503'd after
    rows = {"checks": [
        {"id": "anon.types.written", "target": "ep AirQualityObserved", "result": "pass",
         "call": "GET /api/endpoint/ep/ngsi-ld/v1/entities?type=AirQualityObserved&count=true", "actual": "count=13723"},
        {"id": "anon.count", "target": "ep AirQualityObserved", "result": "pass", "call": f"GET {COUNT}",
         "actual": "13723"},
        {"id": "anon.count", "target": "ep Tree", "result": "fail",
         "call": "GET /api/endpoint/ep/ngsi-ld/v1/entities?type=Tree&count=true&limit=0", "actual": "503"},
        {"id": "anon.odata.query", "target": "ep", "result": "pass",
         "call": "GET /api/endpoint/ep/odata/AirQualityObserved?$count=true", "actual": "ok"}]}
    assert suite.discovery_calls(suite.load_rows(rows)) == [COUNT]


class FakeHttp:
    def __init__(self, *answers):
        self.answers, self.asked = list(answers), 0

    def call(self, method, path):
        self.asked += 1
        return self.answers.pop(0) if len(self.answers) > 1 else self.answers[0]


def test_a_discovery_cut_at_its_deadline_or_a_503_is_not_settled():
    # smoke 43675981256 (T-6953): the copy's /types answered 200 with a 199 after 6.2 s, reads 503
    cut = (200, {"ngsild-warning": '199 antares "entity scan ceiling reached"'}, b"[]")
    assert suite.unsettled(FakeHttp(cut), [TYPES]) == [f'{TYPES}: 200 199 antares "entity scan ceiling reached"']
    assert suite.unsettled(FakeHttp((503, {}, b"")), [TYPES]) == [f"{TYPES}: 503"]
    assert suite.unsettled(FakeHttp((200, {}, b"[]")), [TYPES]) == []


def test_a_count_read_settles_only_with_its_count():
    assert suite.unsettled(FakeHttp((503, {}, b"")), [COUNT]) == [f"{COUNT}: 503"]
    assert suite.unsettled(FakeHttp((200, {}, b"[]")), [COUNT]) == [f"{COUNT}: 200 no count"]
    assert suite.unsettled(FakeHttp((200, {"ngsild-results-count": "13723"}, b"[]")), [COUNT]) == []


def test_settle_waits_for_a_whole_round_and_gives_up_at_its_timeout(monkeypatch):
    cut, whole = (200, {"warning": "199 antares x"}, b""), (200, {}, b"[]")
    http = FakeHttp(cut, cut, whole)
    monkeypatch.setattr(suite, "Http", lambda base, timeout: http)
    monkeypatch.setattr(suite.time, "sleep", lambda s: None)
    ops = SimpleNamespace()
    assert suite.Ops.settle(ops, "http://localhost:1", [TYPES], 60) == [] and http.asked == 3
    http = FakeHttp(cut)
    monkeypatch.setattr(suite, "Http", lambda base, timeout: http)
    assert suite.Ops.settle(ops, "http://localhost:1", [TYPES], 0) == [f"{TYPES}: 200 199 antares x"]


def test_a_copy_that_never_settles_is_red_and_the_after_run_still_shows_its_answers(tmp_path, monkeypatch):
    res, _, ops = run_fake(tmp_path, monkeypatch, unsettled=[f"{TYPES}: 503"])
    rows = by_id(res)
    assert rows["control.settled"]["result"] == "fail" and TYPES in rows["control.settled"]["reason"]
    assert rows["step.after"]["result"] == "pass" and "endpoints/t/two" in rows
    assert ops.settled == ("http://localhost:42311", [], 7)
    res, _, _ = run_fake(tmp_path / "2", monkeypatch)
    assert by_id(res)["control.settled"]["result"] == "pass"


def test_a_copy_on_another_role_than_the_source_fails_its_control(tmp_path, monkeypatch):
    # The red proof of the role rule: a held source compared with a copy running every module.
    res, _, ops = run_fake(tmp_path, monkeypatch, roles={"1": HELD, "999999": "all"})
    rows = by_id(res)
    assert rows["control.role"]["result"] == "fail" and rows["control.role"]["actual"] == "all"
    assert ops.started_role == HELD
    # a source whose --role cannot be read fails it too, whatever the copy runs
    res, _, ops = run_fake(tmp_path / "u", monkeypatch, roles={"999999": "all"})
    assert by_id(res)["control.role"]["result"] == "fail" and ops.started_role == ""


def test_pipelines_running_on_either_side_name_the_run_no_count_proof(tmp_path, monkeypatch):
    for i, roles in enumerate(({"1": "all", "999999": "all"}, {"1": HELD + ",ekiben", "999999": HELD})):
        res, _, _ = run_fake(tmp_path / str(i), monkeypatch, roles=roles)
        row = by_id(res)["control.held"]
        assert row["result"] == "skip" and "no count proof" in row["reason"] and "source" in row["reason"]
    assert suite.runs_ekiben("all") and suite.runs_ekiben("portal,ekiben") and not suite.runs_ekiben(HELD)
    assert not suite.runs_ekiben("ekiben-x")


def test_role_of_reads_a_running_process_flag():
    import subprocess as sp
    p = sp.Popen([sys.executable, "-c", "import time; time.sleep(30)", "--role", HELD])
    try:
        for _ in range(50):
            if suite.role_of(p.pid):
                break
            import time
            time.sleep(0.05)
        assert suite.role_of(p.pid) == HELD
    finally:
        p.kill()
        p.wait()
    assert suite.role_of(p.pid) == "" and suite.role_of("?") == ""


def test_the_copy_starts_on_the_role_given_without_the_sources_realm_lines_or_hold(tmp_path, monkeypatch):
    """The copy has no export: up.sh exits 1 on REHEARSAL_REALM_LINES_* without one (gate
    2026-11-10T203500Z-gc, restore's second run died at step.start)."""
    for k in suite.COPY_ENV_DROPPED + ("REHEARSAL_ROLE",):
        monkeypatch.setenv(k, "/from/the/source")
    rhome = tmp_path / "prod-restore"
    rhome.mkdir()
    (rhome / "jc.pid").write_text("4242\n")
    seen = []
    monkeypatch.setattr(suite, "run", lambda argv, env=None, timeout=0, cwd=None: seen.append((argv, env)) or "")
    ops = suite.Ops(args(tmp_path, src_pg_port=55460), tmp_path / "rh" / "prod", rhome, tmp_path / "b")
    assert ops.start(HELD) == 4242
    argv, env = seen[-1]
    assert argv[0].endswith("deploy/rehearsal/up.sh") and argv[1] == "prod-restore"
    assert env["REHEARSAL_ROLE"] == HELD and not set(suite.COPY_ENV_DROPPED) & set(env)
    assert env["REHEARSAL_EXPORT"] == str(rhome / "no-export")
    ops.start("")
    assert "REHEARSAL_ROLE" not in seen[-1][1]


def test_the_runners_files_are_backed_up_and_compared(tmp_path, monkeypatch):
    class Lossy(FakeOps):
        def fresh(self):  # a backup that lost the proto
            held = super().fresh()
            (self.rhome / "gtfs-realtime.proto").unlink()
            return held
    assert {"ekiben-resources.yaml", "gtfs-realtime.proto"} <= set(suite.HOME_PARTS)
    res, _, _ = run_fake(tmp_path / "ok", monkeypatch)
    assert by_id(res)["control.home-files"]["result"] == "pass"
    monkeypatch.setenv("REHEARSAL_HOME", str(tmp_path / "lossy" / "rh"))
    res = Results("restore", "http://localhost:42310")
    suite.execute(args(tmp_path / "lossy"), res, tmp_path / "lossy" / "out", Lossy)
    row = by_id(res)["control.home-files"]
    assert row["result"] == "fail" and "gtfs-realtime.proto" in row["actual"]
    # a source without them: a named skip
    a = args(tmp_path / "none")
    for f in suite.HOME_FILES:
        (Path(a.home) / f).unlink()
    monkeypatch.setenv("REHEARSAL_HOME", str(tmp_path / "none" / "rh"))
    res = Results("restore", "http://localhost:42310")
    suite.execute(a, res, tmp_path / "none" / "out", FakeOps)
    assert by_id(res)["control.home-files"]["result"] == "skip"


def test_teardown_removes_the_copys_functions_container_by_name(tmp_path, monkeypatch):
    calls = []
    monkeypatch.setattr(suite.subprocess, "run", lambda argv, **kw: calls.append(argv))
    rhome = tmp_path / "prod-restore"
    rhome.mkdir()
    ops = suite.Ops(args(tmp_path, src_pg_port=55460), tmp_path / "rh" / "prod", rhome, tmp_path / "b")
    ops.teardown()
    removed = [c[-1] for c in calls if c[:3] == ["docker", "rm", "-f"]]
    assert removed == ["rh-pg-prod-restore", "rh-functions-42311", "rh-prom-prod-restore", "rh-promproxy-prod-restore"]
    assert not rhome.exists()


def pl(name, *messages):
    return {"metadata": {"namespace": "helsinki", "name": name},
            "status": {"conditions": [{"reason": "x", "message": m} for m in messages]}}


NOW = 1_800_000_000.0  # 2027-01-15T08:00:00Z


def test_only_a_pipeline_whose_first_run_is_still_to_write_is_pending():
    items = [pl("ran", "running since 2027-01-15T07:59:00Z; the last run ended 2027-01-15T07:58:00Z after 30 s"),
             pl("failed-once", "the last run, ended 2027-01-15T07:50:00Z, failed: 404; next try 2027-01-15T08:01:00Z"),
             pl("first-running", "running since 2027-01-15T07:59:00Z"),
             pl("first-queued", "queued, due 2027-01-15T07:55:00Z"),
             pl("due-soon", "next run 2027-01-15T08:10:00Z"),
             pl("due-later", "next run 2027-01-16T04:00:00Z"),
             pl("resident"), {"metadata": None, "status": None}]
    assert suite.first_runs_pending(items, NOW, 900) == [
        "helsinki/first-running running since 2027-01-15T07:59:00Z",
        "helsinki/first-queued queued, due 2027-01-15T07:55:00Z",
        "helsinki/due-soon first run due 2027-01-15T08:10:00Z"]


class Clock:
    def __init__(self):
        self.t = NOW

    def time(self):
        return self.t

    def sleep(self, s):
        self.t += s


def test_the_wait_ends_once_the_first_runs_drained(monkeypatch):
    clock = Clock()
    monkeypatch.setattr(suite, "time", clock)
    rounds = [[pl("a", "queued, due 2027-01-15T07:55:00Z")], [pl("a", "running since 2027-01-15T08:00:05Z")],
              [pl("a", "next run 2027-01-15T08:30:00Z; the last run ended 2027-01-15T08:00:15Z after 10 s")]]
    assert suite.drain_wait(lambda: rounds.pop(0), 600, 900) == ([], 20.0) and rounds == []


def test_a_queue_that_never_drains_or_cannot_be_read_names_what_it_waited_for(monkeypatch):
    clock = Clock()
    monkeypatch.setattr(suite, "time", clock)
    left, waited = suite.drain_wait(lambda: [pl("stuck", "queued, due 2027-01-15T07:55:00Z")], 600, 900)
    assert left == ["helsinki/stuck queued, due 2027-01-15T07:55:00Z"] and waited == 600

    def refused():
        raise RuntimeError("GET /api/v1/projects/helsinki/pipelines?limit=1000: 403")
    assert suite.drain_wait(refused, 30, 900) == (["GET /api/v1/projects/helsinki/pipelines?limit=1000: 403"], 30)


def test_the_before_run_waits_for_the_drain_and_an_undrained_queue_is_a_named_red_row(tmp_path, monkeypatch):
    res, meta, ops = run_fake(tmp_path, monkeypatch, a=args(tmp_path, drain=600, drain_window=300))
    row = by_id(res)["control.drained"]
    assert row["result"] == "pass" and row["actual"] == "drained after 0 s" and meta["drainSeconds"] == 0
    assert ops.drained == (600, 300, 0)  # before any before-run suite
    pending = ["helsinki/air-quality queued, due 2027-01-15T07:55:00Z"]
    res, _, _ = run_fake(tmp_path / "2", monkeypatch, a=args(tmp_path / "2", drain=600), pending=pending)
    rows = by_id(res)
    assert rows["control.drained"]["result"] == "fail" and "helsinki/air-quality" in rows["control.drained"]["reason"]
    assert "600 s" in rows["control.drained"]["actual"] and rows["step.after"]["result"] == "pass"
    # off by default: no wait, no row
    res, _, ops = run_fake(tmp_path / "3", monkeypatch)
    assert "control.drained" not in by_id(res) and not hasattr(ops, "drained")


def test_ops_drain_reads_every_exported_projects_pipelines_as_a_signed_in_admin_and_signs_out(tmp_path, monkeypatch):
    for project in ("helsinki", "zilina"):
        (tmp_path / "e" / "gitea" / "files" / "joinedcontext" / project / "pipelines").mkdir(parents=True)
    answers = {"/api/v1/projects/helsinki/pipelines?limit=1000": (200, {}, json.dumps(
                   {"items": [{"metadata": {"name": "a"}, "status": {"conditions": [
                       {"message": "next run 2027-01-15T08:30:00Z; the last run ended 2027-01-15T08:00:15Z after 10 s"}]}}]}).encode()),
               "/api/v1/projects/zilina/pipelines?limit=1000": (403, {}, b"")}
    made = []

    class FakeAdmin:
        def __init__(self, base, state):
            self.browser = SimpleNamespace(call=lambda method, path: answers[path])
            self.closed = False
            made.append(self)

        def sign_in(self):
            pass

        def close(self):
            self.closed = True
    sys.path.insert(0, str(RESTORE.parent / "pipelines"))
    import people
    monkeypatch.setattr(people, "Admin", FakeAdmin)
    monkeypatch.setattr(suite.time, "sleep", lambda s: None)
    ops = SimpleNamespace(a=SimpleNamespace(export=str(tmp_path / "e"), base_url="http://127.0.0.1:1", state=str(tmp_path)))
    left, _ = suite.Ops.drain(ops, 0, 900)
    assert left == ["GET /api/v1/projects/zilina/pipelines?limit=1000: 403"] and made[0].closed
    answers["/api/v1/projects/zilina/pipelines?limit=1000"] = (200, {}, b'{"items": []}')
    assert suite.Ops.drain(ops, 0, 900)[0] == [] and made[1].closed
