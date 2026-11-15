"""T-5117: scripts/chaos-recovery.py kills one part of dev at a time and counts what was lost.

The cluster is not here: pod lists, pg_locks rows and request outcomes are fed as fixtures, and
kubectl is replaced, so what the test measures and which pod it kills is proven without dev.
"""

import importlib.util
import json
import sys
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parent.parent / "scripts" / "chaos-recovery.py"
spec = importlib.util.spec_from_file_location("chaos_recovery", SCRIPT)
cr = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cr)


def pod(name, uid, ready=True, ip="10.0.0.1", deleting=False):
    meta = {"name": name, "uid": uid, **({"deletionTimestamp": "x"} if deleting else {})}
    return {"metadata": meta, "status": {"podIP": ip, "conditions": [{"type": "Ready", "status": str(ready)}]}}


def test_healed_needs_every_replica_ready_and_the_killed_pod_gone():
    old = cr.pods([pod("b-1", "u1"), pod("b-2", "u2", ready=False)])
    assert not cr.healed(old, "u1", 1), "the killed pod is still listed"
    assert not cr.healed(cr.pods([pod("b-2", "u2", ready=False)]), "u1", 1)
    assert cr.healed(cr.pods([pod("b-2", "u2")]), "u1", 1)
    # CNPG brings a lost instance back under its old name: the uid tells them apart.
    assert cr.healed(cr.pods([pod("postgres-cluster-1", "new")]), "old", 1)
    assert not cr.healed(cr.pods([pod("p-1", "u3"), pod("p-2", "u4", deleting=True)]), "u1", 2)


def test_the_leader_is_the_pod_whose_address_holds_the_reconciler_lock():
    hi, lo = cr.leader_key_parts()
    assert (hi, lo) == (0x6A635F72, 0x65636F6E)
    two = cr.pods([pod("portal-a", "1", ip="10.0.0.5"), pod("portal-b", "2", ip="10.0.0.6")])
    assert cr.leader_pod(two, ["10.0.0.6"])["name"] == "portal-b"
    assert cr.leader_pod(two[:1], [])["name"] == "portal-a"
    with pytest.raises(RuntimeError, match="cannot tell the Portal leader"):
        cr.leader_pod(two, ["127.0.0.1"])


def test_victim_asks_postgres_for_the_lock_holder(monkeypatch):
    calls = []

    def kubectl(*args, timeout=60):
        calls.append(args)
        if args[:2] == ("get", "deployment"):
            return json.dumps({"spec": {"replicas": 2, "selector": {"matchLabels": {"app": "portal"}}}})
        if args[:2] == ("get", "pods") and "jsonpath" in args[-1]:
            return "postgres-cluster-1"
        if args[:2] == ("get", "pods"):
            return json.dumps({"items": [pod("portal-a", "1", ip="10.0.0.5"), pod("portal-b", "2", ip="10.0.0.6")]})
        if args[0] == "exec":
            assert "classid = 1784897394 and l.objid = 1701015406" in args[-1]
            return "10.0.0.5\n"
        raise AssertionError(args)

    monkeypatch.setattr(cr, "kubectl", kubectl)
    assert cr.victim("portal")["name"] == "portal-a"
    assert all(a[0] != "delete" for a in calls), "choosing a victim deletes nothing"


def test_a_failure_window_counts_errors_and_leaves_throttling_apart():
    events = [(9.0, "read", 0), (10.0, "read", 200), (11.0, "read", 503), (12.0, "write", 0),
              (13.0, "read", 429), (14.0, "write", 201), (15.5, "write", 409), (16.0, "read", 502), (30.0, "read", 0)]
    w = cr.window(events, 10.0, 20.0)
    assert w == {"reads": 4, "failedReads": 2, "writes": 3, "failedWrites": 1, "throttled": 1, "lastFailureS": 6.0}
    assert cr.window(events, 20.0, 25.0)["lastFailureS"] == 0.0


def test_writes_lost_duplicated_or_without_an_answer():
    outcomes = {"a": 201, "b": 201, "c": 0, "d": 409, "e": 503, "f": 422}
    present = {"a": True, "b": False, "c": True, "d": True, "e": False, "f": False}
    assert cr.writes_verdict(outcomes, present) == {
        "acknowledged": 2, "lost": 1, "duplicates": 1, "unknownOutcome": 2, "landedWithoutAck": 1}


def test_a_pipeline_is_stuck_when_it_ran_fine_and_fails_since_the_test():
    before = [{"pipeline": "air", "state": "ok"}, {"pipeline": "old", "state": "failing"},
              {"pipeline": "bus", "state": "ok"}]
    after = [{"pipeline": "air", "state": "failing", "lastErrorAt": "2026-10-21T22:05:00Z"},
             {"pipeline": "old", "state": "failing", "lastErrorAt": "2026-10-21T22:06:00Z"},
             {"pipeline": "bus", "state": "failing", "lastErrorAt": "2026-10-21T21:00:00Z"}]
    assert cr.stuck(before, after, "2026-10-21T22:00:00+00:00") == ["air"]


def test_passed_needs_every_target_healed_and_nothing_lost_or_stuck():
    good = {"targets": [{"healed": True}], "writes": {"lost": 0, "duplicates": 0},
            "pipelines": {"measured": True, "stuck": []}}
    assert cr.passed(good)
    for broken in ({"targets": [{"healed": False}]}, {"writes": {"lost": 1, "duplicates": 0}},
                   {"writes": {"lost": 0, "duplicates": 2}}, {"pipelines": {"measured": False, "stuck": []}},
                   {"pipelines": {"measured": True, "stuck": ["air"]}}):
        assert not cr.passed({**good, **broken}), broken


@pytest.mark.parametrize("argv", [["--targets", "keycloak"], ["--targets", ""], ["--read-rate", "0"],
                                  ["--write-rate", "500"]])
def test_bad_arguments_are_refused_before_the_cluster_is_asked(monkeypatch, argv):
    monkeypatch.setattr(cr, "kubectl", lambda *a, **k: pytest.fail("kubectl must not run"))
    monkeypatch.setattr(sys, "argv", ["chaos-recovery.py", *argv])
    assert cr.main() == 2


def test_only_dev_is_ever_targeted():
    assert cr.NAMESPACE == "dev" and cr.DOMAIN == "dev.joinedcontext.com"
    assert "smartcity" not in SCRIPT.read_text()
