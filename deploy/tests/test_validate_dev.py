"""T-2799: scripts/validate-dev.py reads the dev cluster and writes a summary for tasks/file-failures.

The cluster is not here, so every parser is fed what kubectl, helmfile, openssl and cosign
print. A failure has to come out as `fail` with a key that stays the same from run to run
(tasks/file-failures deduplicates on it), and a check that could not measure has to come out
as `error`, never as a pass.
"""

import importlib.util
import json
from datetime import UTC, datetime, timedelta
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parent.parent / "scripts" / "validate-dev.py"
spec = importlib.util.spec_from_file_location("validate_dev", SCRIPT)
vd = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vd)

NOW = datetime(2026, 9, 25, 8, 0, tzinfo=UTC)
DIGEST = "sha256:" + "ab" * 32


def verdicts(results):
    return {r["key"]: r["verdict"] for r in results}


# --- drift


DIFF = """Comparing release=portal, chart=charts/portal
dev, portal, Deployment (apps) has changed:
-         replicas: 1
+         replicas: 2
dev, portal-extra, ConfigMap (v1) has been added:
dev, portal, Deployment (apps) has changed:
Comparing release=apisix, chart=charts/apisix
"""


def test_no_diff_is_a_pass():
    assert verdicts(vd.drift_results("", 0)) == {"drift/dev": "pass"}


def test_each_compared_release_is_one_result_and_a_drifting_one_fails_once():
    # T-4956: one task per drifting release, closed when the release compares clean again.
    results = vd.drift_results(DIFF, 2)
    assert verdicts(results) == {"drift/dev/portal": "fail", "drift/dev/apisix": "pass"}
    portal = next(r for r in results if r["key"] == "drift/dev/portal")
    assert portal["title"] == "drift on dev: release portal differs from main (2 objects)"
    assert "ConfigMap dev/portal-extra is in main but not on dev" in portal["detail"]
    assert "Deployment dev/portal differs from main" in portal["detail"]


def test_a_planted_kubectl_edit_shows_up_as_drift_of_its_release_on_prod():
    # What helm-diff --three-way-merge prints after `kubectl scale deploy/ckan --replicas 3` on prod.
    planted = "Comparing release=ckan, chart=charts/ckan\njc, ckan, Deployment (apps) has changed:\n-  replicas: 3\n+  replicas: 1\n"
    assert verdicts(vd.drift_results(planted, 2, "smartcity")) == {"drift/smartcity/ckan": "fail"}
    assert verdicts(vd.drift_results("Comparing release=ckan, chart=charts/ckan\n", 0, "smartcity")) == {
        "drift/smartcity/ckan": "pass"}


def test_the_diff_reads_live_objects_and_never_applies(monkeypatch):
    seen = {}

    def fake_run(argv, **kwargs):
        seen["argv"] = argv
        return type("Run", (), {"stdout": "", "stderr": "", "returncode": 0})()

    monkeypatch.setattr(vd.subprocess, "run", fake_run)
    vd.helmfile_diff("smartcity")
    argv = seen["argv"]
    assert argv[argv.index("-e") + 1] == "smartcity" and "diff" in argv
    assert "--three-way-merge" in argv and "--suppress-secrets" in argv
    assert not {"sync", "apply", "destroy"} & set(argv)
    # T-5702: the drift identity reads no Secret in kube-system or cert-manager, so no release there is diffed.
    assert argv[argv.index("--selector") + 1] == "namespace!=kube-system,namespace!=cert-manager" and argv.index("--selector") < argv.index("diff")


@pytest.mark.parametrize("code,output", [(1, "Error: helm-diff plugin not installed"), (2, "changes, but no header")])
def test_a_diff_that_did_not_run_or_cannot_be_read_is_an_error(code, output):
    assert verdicts(vd.drift_results(output, code)) == {"drift/dev": "error"}


# --- images


def pods(*entries):
    return {"items": [
        {"metadata": {"namespace": ns, "name": name},
         "spec": {"containers": [{"image": image}], "initContainers": [{"image": init}] if init else []}}
        for ns, name, image, init in entries
    ]}


def test_repository_drops_tag_and_digest_but_keeps_a_registry_port():
    assert vd.repository(f"ghcr.io/o/portal:1.2@{DIGEST}") == "ghcr.io/o/portal"
    assert vd.repository("localhost:5000/app:dev") == "localhost:5000/app"
    assert vd.repository("busybox") == "busybox"


def test_only_platform_namespaces_count_and_init_containers_do():
    found = vd.pod_images(pods(
        ("dev", "portal-1", f"ghcr.io/marek-mraz-jc/portal@{DIGEST}", "cr.l5d.io/linkerd/proxy-init:v2"),
        ("kube-system", "coredns-1", "rancher/coredns:1.12", None),
        ("dev-helsinki-apps", "app-1", f"forge/app@{DIGEST}", None),
    ))
    assert set(found) == {f"ghcr.io/marek-mraz-jc/portal@{DIGEST}", "cr.l5d.io/linkerd/proxy-init:v2", f"forge/app@{DIGEST}"}


def test_a_tag_fails_once_per_repository_and_ours_are_verified():
    calls = []

    def verify(image):
        calls.append(image)
        return (image.endswith(DIGEST), "no matching signatures")

    images = {
        f"ghcr.io/marek-mraz-jc/portal@{DIGEST}": ["dev/portal-1"],
        f"ghcr.io/marek-mraz-jc/gateway@sha256:{'cd' * 32}": ["dev/gw-1"],
        f"docker.io/library/postgres:17@{DIGEST}": ["dev/pg-1"],
        "cr.l5d.io/linkerd/proxy:edge-25.1": ["dev/portal-1"],
        "cr.l5d.io/linkerd/proxy:edge-25.2": ["dev/gw-1"],
    }
    results = vd.image_results(images, verify)
    assert verdicts(results) == {
        "image/signature/ghcr.io/marek-mraz-jc/portal": "pass",
        "image/signature/ghcr.io/marek-mraz-jc/gateway": "fail",
        "image/unpinned/cr.l5d.io/linkerd/proxy": "fail",
    }
    # A third-party image is pinned but not ours to sign.
    assert all("postgres" not in c for c in calls)
    unsigned = next(r for r in results if r["verdict"] == "fail" and "signature" in r["key"])
    # The digest is cut short, so the board's redaction of long runs keeps it readable.
    assert "cdcdcdcdcdcd…" in unsigned["detail"] and "cd" * 32 not in unsigned["detail"]


def test_a_new_digest_of_an_unsigned_image_keeps_the_key():
    first = vd.image_results({f"ghcr.io/marek-mraz-jc/x@{DIGEST}": ["dev/a"]}, lambda i: (False, "no"))
    second = vd.image_results({f"ghcr.io/marek-mraz-jc/x@sha256:{'ef' * 32}": ["dev/b"]}, lambda i: (False, "no"))
    assert first[0]["key"] == second[0]["key"]


def test_two_digests_of_one_repository_are_one_result_and_the_unsigned_one_fails_it():
    images = {f"ghcr.io/marek-mraz-jc/x@{DIGEST}": ["dev/a"], f"ghcr.io/marek-mraz-jc/x@sha256:{'ef' * 32}": ["dev/b"]}
    results = vd.image_results(images, lambda i: (i.endswith(DIGEST), "no matching signatures"))
    assert verdicts(results) == {"image/signature/ghcr.io/marek-mraz-jc/x": "fail"}
    assert "efefefefefef…" in results[0]["detail"] and "abababababab" not in results[0]["detail"]


def test_only_third_party_pinned_images_give_no_result():
    assert vd.image_results({f"docker.io/library/postgres@{DIGEST}": ["dev/pg"]}, lambda i: (False, "")) == []


def test_no_cosign_is_an_error_not_a_pass():
    results = vd.image_results({f"ghcr.io/marek-mraz-jc/x@{DIGEST}": ["dev/a"]}, lambda i: (None, "install cosign"))
    assert verdicts(results) == {"image/cosign": "error"}


def test_no_platform_pod_is_an_error():
    assert verdicts(vd.image_results({}, lambda i: (True, ""))) == {"image/pods": "error"}


# --- expiry


def test_openssl_end_date_with_a_padded_day_parses():
    assert vd.not_after("notAfter=Dec  4 10:00:00 2026 GMT\n") == datetime(2026, 12, 4, 10, 0, tzinfo=UTC)


def test_a_certificate_inside_the_floor_fails_and_one_unread_is_an_error():
    results = vd.cert_results([
        ("dev", "portal-tls", NOW + timedelta(days=60)),
        ("dev", "idm-tls", NOW + timedelta(days=20)),
        ("linkerd", "linkerd-identity-issuer", NOW - timedelta(days=1)),
        ("dev", "broken", None),
    ], NOW)
    assert verdicts(results) == {
        "expiry/certificate/dev/portal-tls": "pass",
        "expiry/certificate/dev/idm-tls": "fail",
        "expiry/certificate/linkerd/linkerd-identity-issuer": "fail",
        "expiry/certificate/dev/broken": "error",
    }


def test_backups_skip_without_a_bucket_and_fail_when_stale():
    clusters = {"items": [
        {"metadata": {"namespace": "dev", "name": "none"}, "spec": {}},
        {"metadata": {"namespace": "dev", "name": "fresh"}, "spec": {"backup": {"b": 1}},
         "status": {"lastSuccessfulBackup": "2026-09-25T02:00:00Z"}},
        {"metadata": {"namespace": "dev", "name": "stale"}, "spec": {"backup": {"b": 1}},
         "status": {"lastSuccessfulBackup": "2026-09-23T02:00:00Z"}},
        {"metadata": {"namespace": "dev", "name": "never"}, "spec": {"backup": {"b": 1}}, "status": {}},
        {"metadata": {"namespace": "other", "name": "theirs"}, "spec": {}},
    ]}
    assert verdicts(vd.backup_results(clusters, NOW)) == {
        "expiry/backup/dev/none": "skip",
        "expiry/backup/dev/fresh": "pass",
        "expiry/backup/dev/stale": "fail",
        "expiry/backup/dev/never": "fail",
    }


# --- kyverno


READY = {"conditions": [{"type": "Ready", "status": "True"}]}


def test_a_rule_in_audit_or_a_policy_not_ready_fails():
    policies = [
        {"metadata": {"name": "per-rule"}, "status": READY,
         "spec": {"rules": [{"name": "r", "validate": {"failureAction": "Enforce"}}, {"name": "m", "mutate": {}}]}},
        {"metadata": {"name": "deprecated"}, "status": READY,
         "spec": {"validationFailureAction": "enforce", "rules": [{"name": "r", "validate": {}}]}},
        {"metadata": {"name": "audit"}, "status": READY, "spec": {"rules": [{"name": "r", "validate": {}}]}},
        {"metadata": {"name": "pending", "namespace": "dev"}, "status": {},
         "spec": {"rules": [{"name": "r", "validate": {"failureAction": "Enforce"}}]}},
    ]
    assert verdicts(vd.policy_results(policies)) == {
        "kyverno/policy/per-rule": "pass",
        "kyverno/policy/deprecated": "pass",
        "kyverno/policy/audit": "fail",
        "kyverno/policy/dev/pending": "fail",
    }


def test_report_failures_group_by_rule_and_ignore_other_namespaces():
    reports = [
        {"metadata": {"namespace": "dev"}, "scope": {"kind": "Pod", "name": "portal-1"},
         "results": [{"policy": "require-drop-all", "rule": "drop", "result": "fail"},
                     {"policy": "require-ro-rootfs", "rule": "ro", "result": "pass"}]},
        {"metadata": {"namespace": "dev-helsinki-apps"},
         "results": [{"policy": "require-drop-all", "rule": "drop", "result": "fail",
                      "resources": [{"kind": "Pod", "name": "app-1"}]}]},
        {"metadata": {"namespace": "kube-system"},
         "results": [{"policy": "require-drop-all", "rule": "drop", "result": "fail"}]},
    ]
    results = vd.report_results(reports)
    assert verdicts(results) == {"kyverno/violation/require-drop-all/drop": "fail"}
    assert results[0]["detail"] == "dev-helsinki-apps/Pod/app-1, dev/Pod/portal-1"


def test_clean_reports_pass():
    assert verdicts(vd.report_results([{"metadata": {"namespace": "dev"}, "results": []}])) == {"kyverno/reports": "pass"}


# --- probes


def test_internal_ports_open_to_the_internet_fail_and_a_dead_443_is_an_error():
    open_ports = {443, 10250}
    assert verdicts(vd.edge_results("node", lambda h, p: p in open_ports)) == {"probe/internet/internal-ports": "fail"}
    assert verdicts(vd.edge_results("node", lambda h, p: p == 443)) == {"probe/internet/internal-ports": "pass"}
    assert verdicts(vd.edge_results("node", lambda h, p: False)) == {"probe/internet/443": "error"}


def test_probe_script_settles_on_the_first_target_then_tries_each():
    script = vd.probe_script({"postgres": ("10.0.0.5", 5432), "other-app": ("10.0.0.9", 8080)}, ("10.0.0.5", 5432))
    assert script.index("SETTLED-AFTER") < script.index("REACHED-postgres") < script.index("PROBE-RAN")
    assert "nc -w 5 -z 10.0.0.9 8080" in script and "REACHED-dns" in script


def test_probe_output_decides_each_pair():
    out = "SETTLED-AFTER=8s\nREACHED-dns\nREACHED-other-app\nPROBE-RAN\n"
    assert verdicts(vd.probe_results(out, ["postgres", "other-app"])) == {
        "probe/app/postgres": "pass",
        "probe/app/other-app": "fail",
    }


def test_a_probe_that_never_ran_or_has_no_dns_proves_nothing():
    assert verdicts(vd.probe_results("pod timed out", ["postgres"])) == {"probe/app": "error"}
    assert verdicts(vd.probe_results("SETTLED-AFTER=0s\nPROBE-RAN", ["postgres"])) == {"probe/app/dns": "fail"}


def test_app_probe_runs_restricted_with_the_app_selector_and_is_deleted():
    calls = []
    app = {"metadata": {"namespace": "dev-hel-apps", "name": "air-1",
                        "labels": {"app.kubernetes.io/name": "air", "joinedcontext.com/app": "true", "pod-template-hash": "x"}},
           "status": {"podIP": "10.0.1.1"}}
    other = {"metadata": {"namespace": "dev-hel-apps", "name": "hsl-1",
                          "labels": {"app.kubernetes.io/name": "hsl", "joinedcontext.com/app": "true"}},
             "status": {"podIP": "10.0.1.2"}}
    replies = {
        "pods-app": {"items": [app, other]},
        "networkpolicies": {"items": [{"spec": {"podSelector": {"matchLabels": {"app.kubernetes.io/name": "hsl"}}}},
                                      {"spec": {"podSelector": {"matchLabels": {"app.kubernetes.io/name": "air"}}}}]},
        "pods-pg": {"items": [{"status": {"podIP": "10.0.2.1"}}]},
    }

    def kubectl(*args, check=True):
        calls.append(args)
        if args[:2] == ("get", "pods"):
            return json.dumps(replies["pods-app" if "joinedcontext.com/app=true" in args else "pods-pg"])
        if args[:2] == ("get", "networkpolicies"):
            return json.dumps(replies["networkpolicies"])
        if args[0] == "run":
            return "SETTLED-AFTER=4s\nREACHED-dns\nPROBE-RAN\n"
        return ""

    results = vd.app_probe(kubectl)
    assert verdicts(results) == {"probe/app/postgres": "pass", "probe/app/other-app": "pass", "probe/app/other-app-mesh": "pass"}
    run = next(c for c in calls if c[0] == "run")
    overrides = json.loads(next(a for a in run if a.startswith("--overrides=")).removeprefix("--overrides="))
    # The App's own selector, nothing a ReplicaSet adopts by.
    assert overrides["metadata"]["labels"] == {"app.kubernetes.io/name": "air"}
    container = overrides["spec"]["containers"][0]
    assert container["readinessProbe"]["exec"]["command"] == ["false"]
    assert container["securityContext"]["capabilities"]["drop"] == ["ALL"]
    assert overrides["spec"]["securityContext"]["runAsNonRoot"] is True
    assert "10.0.2.1 5432" in container["command"][2] and "10.0.1.2 8080" in container["command"][2]
    assert calls[-1][:2] == ("delete", "pod") and "--ignore-not-found" in calls[-1]


def test_no_app_pod_skips_and_an_app_without_its_policy_fails():
    def only(items, policies=()):
        def kubectl(*args, check=True):
            if args[:2] == ("get", "networkpolicies"):
                return json.dumps({"items": list(policies)})
            return json.dumps({"items": items})
        return kubectl

    assert verdicts(vd.app_probe(only([]))) == {"probe/app": "skip"}
    app = {"metadata": {"namespace": "dev-x-apps", "name": "a", "labels": {"app.kubernetes.io/name": "a"}},
           "status": {"podIP": "10.0.0.1"}}
    assert verdicts(vd.app_probe(only([app]))) == {"probe/app/policy": "fail"}


# --- the run


def test_a_check_that_raises_is_an_error_result():
    def broken():
        raise RuntimeError("kubectl get pods: forbidden")

    assert verdicts(vd.guarded("images", broken)) == {"images/run": "error"}


def test_main_writes_a_summary_file_failures_accepts(tmp_path, monkeypatch):
    monkeypatch.setattr(vd, "run_checks", lambda skip, env: [vd.result("drift", "pass", "dev matches main")])
    out = tmp_path / "summary.json"
    assert vd.main(["--out", str(out), "--skip", "images,probes"]) == 0
    summary = json.loads(out.read_text())
    assert summary["check"] == "deployment" and summary["repo"] == "joinedcontext-deployment"
    assert summary["results"][0] == {"key": "drift", "verdict": "pass", "title": "dev matches main", "detail": "", "evidence": ""}

    monkeypatch.setattr(vd, "run_checks", lambda skip, env: [vd.result("drift", "error", "did not run")])
    assert vd.main(["--out", str(out)]) == 1


def test_prod_runs_the_drift_check_alone(tmp_path, monkeypatch):
    seen = {}
    monkeypatch.setattr(vd, "run_checks", lambda skip, env: seen.update(skip=skip, env=env) or [])
    assert vd.main(["--out", str(tmp_path / "s.json"), "--env", "smartcity"]) == 0
    assert seen == {"skip": set(vd.CHECKS) - {"drift"}, "env": "smartcity"}
    with pytest.raises(SystemExit):
        vd.main(["--out", str(tmp_path / "s.json"), "--env", "staging"])


def test_an_unknown_check_name_is_refused(tmp_path):
    with pytest.raises(SystemExit):
        vd.main(["--out", str(tmp_path / "s.json"), "--skip", "drfit"])
