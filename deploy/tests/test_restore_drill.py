"""T-2802: scripts/restore-drill.py copies dev's databases into a throwaway cluster and checks them.

The cluster is not here. What psql and kubectl print is fed as fixtures, and the whole drill
runs once against a fake kubectl, to prove the namespace is deleted however the drill ends and
that no dump ever touches a file.
"""

import importlib.util
import json
import subprocess
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parent.parent / "scripts" / "restore-drill.py"
spec = importlib.util.spec_from_file_location("restore_drill", SCRIPT)
rd = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rd)


def verdicts(results):
    return {r["key"]: r["verdict"] for r in results}


def test_counts_parse_from_psql_rows():
    assert rd.parse_counts("public.a|3\npublic.b|0\n\n") == {"public.a": 3, "public.b": 0}


def test_a_copy_between_the_counts_before_and_after_is_whole():
    before = {"public.events": 100, "public.people": 5}
    after = {"public.events": 104, "public.people": 5}
    assert rd.compare(before, after, {"public.events": 102, "public.people": 5}) == []
    wrong = rd.compare(before, after, {"public.events": 90})
    assert wrong == ["public.events: 90 rows, the source held 100…104", "public.people is missing from the copy"]


def test_the_drill_cluster_is_small_labelled_and_owns_the_portal_database():
    cluster = rd.drill_cluster("jc-drill-1", "ghcr.io/cloudnative-pg/postgresql@sha256:" + "a" * 64, 3)
    assert cluster["metadata"]["labels"] == {rd.DRILL_LABEL: "true"}
    assert cluster["spec"]["instances"] == 1 and cluster["spec"]["storage"] == {"size": "3Gi"}
    assert cluster["spec"]["bootstrap"]["initdb"] == {"database": "portal", "owner": "portal"}


@pytest.mark.parametrize("size,gi", [(0, 2), (2**30, 2), (3 * 2**30, 6), (3 * 2**30 + 1, 7)])
def test_the_volume_holds_twice_the_source_and_at_least_two_gib(size, gi):
    assert rd.size_gi(size) == gi


def test_the_migrate_pod_runs_restricted_and_reads_the_uri_from_the_drill_secret():
    pod = rd.portal_pod("jc-drill-1", "ghcr.io/x/portal@sha256:" + "b" * 64)
    container = pod["spec"]["containers"][0]
    assert container["args"] == ["migrate"]
    assert container["env"] == [{"name": "JC_PORTAL_DATABASE_URL",
                                 "valueFrom": {"secretKeyRef": {"name": "drill-app", "key": "uri"}}}]
    assert pod["spec"]["securityContext"]["runAsUser"] == 65532
    assert container["securityContext"]["capabilities"]["drop"] == ["ALL"]
    assert pod["spec"]["automountServiceAccountToken"] is False


def test_key_tables_must_hold_rows():
    copies = {"keycloak": {"public.user_entity": 12, "public.client": 0}, "gitea": {}}
    assert verdicts(rd.key_results(copies)) == {
        "restore/keycloak/public.user_entity": "pass",
        "restore/keycloak/public.client": "fail",
        "restore/gitea/public.repository": "fail",
    }


def test_an_unpinned_portal_image_is_refused(tmp_path):
    with pytest.raises(SystemExit):
        rd.main(["--out", str(tmp_path / "s.json"), "--portal-image", "ghcr.io/x/portal:latest"])


class FakeCluster:
    """kubectl as the drill calls it, recording every call."""

    def __init__(self, memory="40%", fail_on=None):
        self.calls, self.memory, self.fail_on = [], memory, fail_on

    def kubectl(self, *args, stdin=None, timeout=300):
        self.calls.append(args)
        if self.fail_on and self.fail_on in args:
            raise RuntimeError(f"kubectl {self.fail_on}: refused")
        joined = " ".join(args)
        if args[:2] == ("get", "ns"):
            return "namespace/jc-drill-old\n"
        if args[:2] == ("top", "nodes"):
            return f"node-1 500m 5% 9000Mi {self.memory}\n"
        if "instanceRole=primary" in joined:
            # The drill's namespace is jc-drill-<UTC stamp>; the source is `dev`.
            return "drill-1" if "jc-drill-" in joined else "postgres-cluster-1"
        if "jsonpath={.spec.imageName}" in joined:
            return "ghcr.io/cloudnative-pg/postgresql@sha256:" + "c" * 64
        if "readyInstances" in joined:
            return "1"
        if "psql" in args:
            sql = args[-1]
            if sql == rd.DATABASES_SQL:
                return "keycloak\nportal\n"
            if "pg_database_size" in sql:
                return str(2**30)
            if sql == rd.COUNTS_SQL:
                database = args[args.index("-d") + 1]
                return {"keycloak": "public.user_entity|4\npublic.client|9\n", "portal": "public.runs|7\n"}[database]
            return ""
        return ""


def test_the_whole_drill_restores_counts_and_deletes_its_namespace(monkeypatch):
    fake = FakeCluster()
    monkeypatch.setattr(rd, "kubectl", fake.kubectl)
    copied = []
    monkeypatch.setattr(rd, "copy_database", lambda source, target, ns, db: copied.append((source, target, db)) or "")
    deleted = []
    monkeypatch.setattr(rd.subprocess, "run", lambda cmd, **kw: deleted.append(cmd) or subprocess.CompletedProcess(cmd, 0))
    results = rd.drill(None)
    v = verdicts(results)
    assert v["restore/source"] == "skip" and v["upgrade/migrations"] == "skip"
    assert v["restore/keycloak"] == "pass" and v["restore/portal"] == "pass"
    assert v["restore/keycloak/public.user_entity"] == "pass" and v["restore/gitea/public.repository"] == "fail"
    assert v["restore/rto"] == "pass"
    # The leftover of a crashed drill goes first; the drill's own namespace goes last, always.
    assert ("delete", "namespace/jc-drill-old", "--wait=false") in fake.calls
    assert deleted and deleted[-1][:3] == ["kubectl", "delete", "namespace"]
    # The Portal's database comes from initdb; every other one is created before the copy.
    creates = [c for c in fake.calls if "psql" in c and c[-1].startswith("create database")]
    assert [c[-1] for c in creates] == ['create database "keycloak"']
    assert [db for _, _, db in copied] == ["keycloak", "portal"]


def test_a_full_node_starts_nothing(monkeypatch):
    fake = FakeCluster(memory="80%")
    monkeypatch.setattr(rd, "kubectl", fake.kubectl)
    results = rd.drill(None)
    assert verdicts(results)["restore/run"] == "error"
    assert not any(c[:2] == ("create", "namespace") for c in fake.calls)


def test_a_drill_that_breaks_still_deletes_its_namespace(monkeypatch):
    fake = FakeCluster(fail_on="apply")
    monkeypatch.setattr(rd, "kubectl", fake.kubectl)
    deleted = []
    monkeypatch.setattr(rd.subprocess, "run", lambda cmd, **kw: deleted.append(cmd) or subprocess.CompletedProcess(cmd, 0))
    results = rd.drill(None)
    assert verdicts(results)["restore/run"] == "error"
    assert deleted and deleted[-1][:3] == ["kubectl", "delete", "namespace"]


def test_the_dump_is_piped_and_never_written(monkeypatch):
    seen = {}

    class Dump:
        def __init__(self, cmd, stdout=None, stderr=None):
            seen["dump"] = cmd
            self.stdout = open("/dev/null", "rb")
            self.stderr = open("/dev/null", "rb")

        def wait(self, timeout=None):
            return 0

    def restore(cmd, stdin=None, capture_output=None, timeout=None):
        seen["restore"], seen["stdin"] = cmd, stdin
        return subprocess.CompletedProcess(cmd, 1, b"", b"pg_restore: warning: schema public already exists")

    monkeypatch.setattr(rd.subprocess, "Popen", Dump)
    monkeypatch.setattr(rd.subprocess, "run", restore)
    warnings = rd.copy_database("src-1", "drill-1", "jc-drill-1", "portal")
    assert seen["dump"][-4:] == ["pg_dump", "-Fc", "-d", "portal"]
    assert "--role" in seen["restore"] and seen["restore"][seen["restore"].index("--role") + 1] == "portal"
    assert seen["stdin"] is not None and "already exists" in warnings
