"""scripts/smoke-edge-wave.py against a fake cluster whose Portal writes through the real admission
policy (T-5318): the Portal's edge wave is judged by the kyverno CLI against secrets-policy.yaml as
the chart renders it today (6921dc37 added apisix-standalone-config) and as it was before, when the
Portal's write was refused with 422 on prod (2026-10-20) and every smoke stayed green."""

import base64
import importlib.util
import json
import shutil
import subprocess
from pathlib import Path

import pytest
from test_portal_namespaces_policy import CHART, judged

SCRIPT = Path(__file__).resolve().parent.parent / "scripts/smoke-edge-wave.py"
spec = importlib.util.spec_from_file_location("smoke_edge_wave", SCRIPT)
edge = importlib.util.module_from_spec(spec)
spec.loader.exec_module(edge)

SERVED = base64.b64encode(b"routes: []").decode()

needs_tools = pytest.mark.skipif(not (shutil.which("kyverno") and shutil.which("helm")), reason="kyverno and helm")


class Cluster:
    """The served Secret, the Portal's log and, on each poll, one Portal sync; `kubectl` answers
    the calls the script makes."""

    def __init__(self, admits, marked=True):
        annotations = {edge.MARKER: edge.MARKED} if marked else {}
        self.secret = {"apiVersion": "v1", "kind": "Secret", "type": "Opaque", "data": {"apisix.yaml": SERVED},
                       "metadata": {"name": edge.SECRET, "namespace": "dev", "resourceVersion": "100", "annotations": annotations}}
        # `history`: the hour before the step; `log`: the lines since it began.
        self.admits, self.history, self.log, self.calls = admits, [], [], []

    def bump(self):
        meta = self.secret["metadata"]
        meta["resourceVersion"] = str(int(meta["resourceVersion"]) + 1)

    def kubectl(self, *args):
        self.calls.append(args)
        meta = self.secret["metadata"]
        if args[:2] == ("get", "secrets"):
            return json.dumps({"items": [self.secret]})
        if args[:2] == ("get", "pods"):
            return "dev"
        if args[0] == "logs":
            return "\n".join(self.log if any(a.startswith("--since-time=") for a in args) else self.history + self.log)
        if args[0] == "annotate":
            change = args[5]
            if any(a.startswith("--resource-version=") and a != f"--resource-version={meta['resourceVersion']}" for a in args):
                raise RuntimeError("Conflict")
            if change.endswith("-"):
                meta["annotations"].pop(change[:-1], None)
            else:
                key, value = change.split("=", 1)
                meta["annotations"][key] = value
            self.bump()
            return ""
        raise AssertionError(args)

    def sync(self, _seconds):
        """The Portal leader's converge_edge_file: the same file back, the marker on it."""
        meta = self.secret["metadata"]
        if meta["annotations"].get(edge.MARKER) == edge.MARKED:
            return
        if self.admits(self.secret):
            meta["annotations"][edge.MARKER] = edge.MARKED
            self.bump()
        else:
            self.log.append('WARN joinedcontext_portal::reconciler::daemon: edge file not written '
                            'reason=the API server answered 422 for Secret apisix-standalone-config')


def run(cluster, monkeypatch, timeout=30):
    monkeypatch.setattr(edge, "kubectl", cluster.kubectl)
    ticks = iter(range(0, 10_000, 5))
    return edge.wave(timeout, clock=lambda: next(ticks), sleep=cluster.sync)


@pytest.fixture(scope="module")
def policies(tmp_path_factory):
    out = tmp_path_factory.mktemp("edge-wave")
    today = subprocess.run(
        ["helm", "template", "portal-namespaces", str(CHART), "--show-only", "templates/secrets-policy.yaml",
         "--set", "release=dev", "--set", "serviceAccount.namespace=dev"],
        capture_output=True, text=True, check=True,
    ).stdout
    assert "|apisix-standalone-config|" in today
    (out / "today.yaml").write_text(today)
    # Before 6921dc37 the allow-list had no apisix-standalone-config.
    (out / "before.yaml").write_text(today.replace("|apisix-standalone-config|", "|"))
    return out


def portal_writes(policy, tmp_path):
    return lambda secret: judged(policy, secret, tmp_path, operation="UPDATE") == "pass"


@needs_tools
def test_the_wave_is_green_under_todays_policy(policies, tmp_path, monkeypatch):
    cluster = Cluster(portal_writes(policies / "today.yaml", tmp_path))
    ok, said = run(cluster, monkeypatch)
    assert ok, said
    assert cluster.secret["metadata"]["annotations"][edge.MARKER] == edge.MARKED
    assert "resourceVersion 100 -> 102" in said
    assert cluster.secret["data"] == {"apisix.yaml": SERVED}


@needs_tools
def test_the_wave_is_red_under_the_policy_before_6921dc37(policies, tmp_path, monkeypatch):
    cluster = Cluster(portal_writes(policies / "before.yaml", tmp_path))
    ok, said = run(cluster, monkeypatch)
    assert not ok
    assert "the API server answered 422 for Secret apisix-standalone-config" in said
    # The smoke put the marker back, so a helm sync does not rewrite the base over the Apps' routes.
    assert cluster.secret["metadata"]["annotations"][edge.MARKER] == edge.MARKED


def test_a_portal_that_never_syncs_is_red_and_the_marker_is_restored(monkeypatch):
    cluster = Cluster(admits=lambda _secret: False)
    cluster.sync = lambda _seconds: None
    ok, said = run(cluster, monkeypatch, timeout=30)
    assert not ok
    assert "did not write dev/apisix-standalone-config within 30 s" in said
    assert cluster.secret["metadata"]["annotations"][edge.MARKER] == edge.MARKED


def test_an_edge_file_the_portal_never_composed_is_red_and_left_alone(monkeypatch):
    cluster = Cluster(admits=lambda _secret: True, marked=False)
    ok, said = run(cluster, monkeypatch)
    assert not ok
    assert "the Portal never composed the edge file" in said
    assert not [call for call in cluster.calls if call[0] == "annotate"]


def test_a_portal_write_between_read_and_patch_is_not_overwritten(monkeypatch):
    cluster = Cluster(admits=lambda _secret: True)
    real = cluster.kubectl

    def racing(*args):
        if args[0] == "annotate" and args[5].endswith("-"):
            cluster.bump()
        return real(*args)

    monkeypatch.setattr(edge, "kubectl", racing)
    with pytest.raises(RuntimeError, match="Conflict"):
        edge.wave(30, clock=lambda: 0, sleep=cluster.sync)
    assert cluster.secret["metadata"]["annotations"][edge.MARKER] == edge.MARKED


def finished(seconds, leader=True):
    """A Portal "sync run finished" line as reconciler/daemon.rs sync_once logs it (JSON)."""
    return json.dumps({"timestamp": "2026-10-26T15:43:45.000000Z", "level": "INFO", "message": edge.SYNCED,
                       "manifests": 3360, "leader": leader, "elapsed_ms": int(seconds * 1000),
                       "target": "joinedcontext_portal::reconciler::daemon"})


def slow_portal(cluster, writes_at):
    """A clock and a sleep under which the Portal's leader writes the marker back `writes_at` s in."""
    now = [0.0]

    def sleep(seconds):
        now[0] += seconds
        if now[0] >= writes_at:
            cluster.sync(seconds)

    return (lambda: now[0]), sleep


# Prod 2026-10-26 14:59-15:51Z: one non-leader run, then the leader's (seconds).
PROD = [finished(82, leader=False)] + [finished(s) for s in (175, 452, 364, 532, 465, 623, 447)]


def test_the_wait_follows_the_measured_sync_length(monkeypatch):
    cluster = Cluster(admits=lambda _secret: True)
    cluster.history = PROD
    monkeypatch.setattr(edge, "kubectl", cluster.kubectl)
    clock, sleep = slow_portal(cluster, writes_at=700)
    ok, said = edge.wave(clock=clock, sleep=sleep)
    assert ok, said
    # The fixed 300 s of before T-5796 was red on the same Portal.
    cluster = Cluster(admits=lambda _secret: True)
    monkeypatch.setattr(edge, "kubectl", cluster.kubectl)
    clock, sleep = slow_portal(cluster, writes_at=700)
    ok, said = edge.wave(300, clock=clock, sleep=sleep)
    assert not ok
    assert "within 300 s (--timeout)" in said


def test_the_measured_timeout_is_two_leader_runs_and_the_interval_within_bounds():
    assert edge.measured_timeout(PROD) == (2 * 623 + 60, "2 x the longest of 7 leader sync runs in the last hour (623 s) + 60 s")
    assert edge.measured_timeout([finished(70), finished(82, leader=False)])[0] == edge.FLOOR
    assert edge.measured_timeout([finished(1200)])[0] == edge.CEILING
    timeout, said = edge.measured_timeout(["portal server listening"])
    assert timeout == edge.CEILING and "no leader sync run" in said


def test_two_leader_runs_without_the_write_end_the_wait_red(monkeypatch):
    cluster = Cluster(admits=lambda _secret: True)
    cluster.history = PROD
    # A follower's run converges nothing and does not count.
    cluster.sync = lambda _seconds: cluster.log.extend([finished(82, leader=False), finished(400)])
    ok, said = run(cluster, monkeypatch, timeout=None)
    assert not ok
    assert "two Portal leader sync runs finished without writing dev/apisix-standalone-config back" in said
    assert cluster.secret["metadata"]["annotations"][edge.MARKER] == edge.MARKED


def test_the_portal_log_skips_the_probe_pods_smoke_starts_with_kubectl_run(monkeypatch):
    calls = []
    monkeypatch.setattr(edge, "kubectl", lambda *args: calls.append(args) or "stage")
    edge.portal_log("--since=1m")
    assert all("app.kubernetes.io/name=portal-portal,!run" in call for call in calls)
