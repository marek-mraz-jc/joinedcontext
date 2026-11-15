"""`just dev-apply` syncs only the releases whose render moved since dev's applied commit (T-6135).

Each case is a range (the files it touches) and two renders, base and HEAD, one directory per
release as helmfile writes them; the plan is the release list or a full sync. Whatever the
planner cannot judge safely is a full sync: a skipped release would leave a NetworkPolicy,
RBAC or Kyverno change unapplied.
"""

import importlib.util
from datetime import UTC, datetime, timedelta
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
BASE = "4a1de6980a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d"
NOW = datetime(2026, 10, 31, 14, 0, 59, tzinfo=UTC)  # one second before a minute boundary (T-6199)
RECENT = f"{NOW - timedelta(hours=1):%Y-%m-%dT%H:%M:%SZ}"

spec = importlib.util.spec_from_file_location("apply_scoped", ROOT / "scripts/apply-pins.py")
apply_pins = importlib.util.module_from_spec(spec)
spec.loader.exec_module(apply_pins)


def deployment(name, image):
    return {"kind": "Deployment", "metadata": {"name": name, "namespace": "dev",
            "annotations": {"checksum/secret": image}},
            "spec": {"template": {"spec": {"containers": [{"image": image}]}}}}


def config(name, data):
    return {"kind": "ConfigMap", "metadata": {"name": name, "namespace": "dev"}, "data": data}


def secret(name, value):
    return {"kind": "Secret", "metadata": {"name": name, "namespace": "dev"}, "data": {"p": value}}


def crd(version):
    return {"kind": "CustomResourceDefinition", "metadata": {"name": "endpoints.jc.example"},
            "spec": {"versions": [{"name": version}]}}


BEFORE = {
    "portal-portal": [deployment("portal", "p@sha256:1")],
    "gitea-seed": [config("seed", {"bbsk.yaml": "a: 1"})],
    "keycloak-keycloak": [deployment("keycloak", "k@sha256:1"), config("realm", {"x": "1"})],
    "secrets-db-portal": [secret("db", "YQ==")],
    "operator-crds": [crd("v1")],
}


def write(root, releases):
    for name, objects in releases.items():
        (root / name / "chart/templates").mkdir(parents=True)
        (root / name / "chart/templates/all.yaml").write_text(yaml.safe_dump_all(objects))


def change(**moved):
    out = {k: list(v) for k, v in BEFORE.items()}
    for name, objects in moved.items():
        if objects is None:
            out.pop(name)
        else:
            out[name] = objects
    return out


def plan(monkeypatch, tmp_path, files, head, again=None, base=BASE, full_at=RECENT, listed=(), now=NOW):
    """The plan for a range touching `files` whose HEAD renders `head` (and `again` the second time)."""
    def renders(environment, base, judge):
        dirs = [tmp_path / "old", tmp_path / "new", tmp_path / "again"]
        for d, releases in zip(dirs, [BEFORE, head, head if again is None else again]):
            write(d, releases)
        return judge(*dirs)

    monkeypatch.setattr(apply_pins.changes, "changed", lambda b, h: files)
    monkeypatch.setattr(apply_pins, "_renders", renders)
    monkeypatch.setattr(apply_pins, "component_releases",
                        lambda environment, components: apply_pins.releases_of(list(listed), components))
    try:
        return apply_pins.plan("dev", base, full_at, now)
    except apply_pins.Refused as e:
        return f"full: {e}"


@pytest.mark.parametrize("files, head, expected", [
    # a pin moves the one release that renders the image
    (["components/portal/images.yaml"], change(**{"portal-portal": [deployment("portal", "p@sha256:2")]}),
     ["portal-portal"]),
    # a seed edit moves the seed release and the bootstrap whose Job commits it to the forge (T-6593)
    (["seeds/bbsk/pipelines/air.yaml"], change(**{"gitea-seed": [config("seed", {"bbsk.yaml": "a: 2"})]}),
     ["gitea-bootstrap", "gitea-seed"]),
    # a values change in one component moves that component's release
    (["components/keycloak/values/keycloak/base-values.yaml.gotmpl"],
     change(**{"keycloak-keycloak": [deployment("keycloak", "k@sha256:1"), config("realm", {"x": "2"})]}),
     ["keycloak-keycloak"]),
    # a range whose render did not move syncs nothing
    (["README.md"], change(), []),
])
def test_a_range_syncs_only_the_releases_whose_render_moved(monkeypatch, tmp_path, files, head, expected):
    assert plan(monkeypatch, tmp_path, files, head) == expected


@pytest.mark.parametrize("files, head, reason", [
    (["components/viz/values/server/base-values.yaml.gotmpl"],
     {**BEFORE, "viz-server": [deployment("viz", "v")]}, "releases added at HEAD: viz-server"),
    (["components/secrets/charts/secrets-generator/values.yaml"], change(**{"gitea-seed": None}),
     "releases gone at HEAD: gitea-seed"),
    (["components/operator/charts.yaml"], change(**{"operator-crds": [crd("v2")]}),
     "operator-crds changes CustomResourceDefinition -/endpoints.jc.example"),
    (["deployment/helmfile.yaml"], change(), "changes how helmfile builds releases"),
    (["components/portal/images.yaml", "components/prepare/helmfile.yaml.gotmpl", "ci/example-deployments/helmfile.yaml"],
     change(), "changes how helmfile builds releases (ci/example-deployments/helmfile.yaml)"),
    (["defaults/deployment/environments/default.yaml"], change(), "changes how helmfile builds releases"),
    (["components/portal/images.yaml"], change(), "no trusted applied commit"),
    (None, change(), "cannot read the range"),
])
def test_what_the_render_cannot_judge_is_a_full_sync(monkeypatch, tmp_path, files, head, reason):
    base = "" if reason.startswith("no trusted") else BASE
    got = plan(monkeypatch, tmp_path, files, head, base=base)
    assert got.startswith("full: ") and reason in got, got


def test_a_failed_render_is_a_full_sync(monkeypatch, capsys):
    monkeypatch.setattr(apply_pins.changes, "changed", lambda b, h: ["components/portal/images.yaml"])

    def fails(environment, base, judge):
        raise apply_pins.Refused("render of dev in /tmp/x failed:\nError: chart not found")

    monkeypatch.setattr(apply_pins, "_renders", fails)
    # main() reads the wall clock, so the last full sync is an hour before now, not before NOW
    recent = f"{datetime.now(UTC) - timedelta(hours=1):%Y-%m-%dT%H:%M:%SZ}"
    assert apply_pins.main(["plan", "dev", BASE, recent]) == 0
    out = capsys.readouterr()
    assert out.out.strip() == "full"
    assert "full sync: render of dev" in out.err


def test_a_secrets_chart_re_render_alone_selects_nothing(monkeypatch, tmp_path):
    # the secrets chart invents a fresh value per render: both HEAD renders differ, so it is masked
    head = change(**{"secrets-db-portal": [secret("db", "Yg==")],
                     "portal-portal": [deployment("portal", "p@sha256:1") | {"metadata": {
                         "name": "portal", "namespace": "dev", "annotations": {"checksum/secret": "new"}}}]})
    again = change(**{"secrets-db-portal": [secret("db", "Yw==")],
                      "portal-portal": [deployment("portal", "p@sha256:1") | {"metadata": {
                          "name": "portal", "namespace": "dev", "annotations": {"checksum/secret": "again"}}}]})
    assert plan(monkeypatch, tmp_path, ["seeds/x.yaml"], head, again) == []


def test_a_deterministic_secret_change_is_synced(monkeypatch, tmp_path):
    # both HEAD renders agree, so the Secret is compared unmasked and its new value is applied
    head = change(**{"secrets-db-portal": [secret("db", "Yg==")]})
    assert plan(monkeypatch, tmp_path, ["components/secrets/secrets.yaml"], head) == ["secrets-db-portal"]


def test_dev_apply_falls_back_to_a_full_sync_and_keeps_its_gates():
    text = (ROOT / "justfile").read_text()
    body = text[text.index("\ndev-apply full="):text.index("\n_smoke ")]
    assert "apply-pins.py plan dev" in body and "|| plan=full" in body
    assert "*-dirty) base=\"\"" in body
    order = ["_dev-guard", "verify-pinned-images.py dev", "-e dev sync", "wait-rollouts.sh dev", "release.py record dev dev"]
    assert [body.index(s) for s in order] == sorted(body.index(s) for s in order)


LISTED = [
    {"name": "portal-portal", "installed": True, "labels": "chart:portal,component:portal,name:portal-portal"},
    {"name": "portal-ui", "installed": True, "labels": "component:portal,name:portal-ui"},
    {"name": "portal-mail-catcher", "installed": False, "labels": "component:portal,name:portal-mail-catcher"},
    {"name": "gitea-seed", "installed": True, "labels": "component:gitea,name:gitea-seed"},
    {"name": "secrets-db-portal", "installed": True, "labels": "component:secrets,name:secrets-db-portal"},
]


def test_a_mixed_batch_names_exactly_the_releases_it_changes(monkeypatch, tmp_path):
    # T-6187, the 0ca12ec4 range: a pin, a seed edit and a chart edit were a full sync of 369
    # releases. That range also moved the prepare hook, which a scoped sync skips
    # (JC_PREPARE_SKIP=pins): that one file is a full sync now (F-R2DEP3-011).
    files = ["components/portal/images.yaml", "seeds/bbsk/pipelines/air.yaml",
             "components/keycloak/charts/realm/templates/realm.yaml"]
    head = change(**{"portal-portal": [deployment("portal", "p@sha256:2")],
                     "gitea-seed": [config("seed", {"bbsk.yaml": "a: 2"})],
                     "keycloak-keycloak": [deployment("keycloak", "k@sha256:1"), config("realm", {"x": "2"})]})
    assert plan(monkeypatch, tmp_path, files, head, listed=LISTED) == [
        "gitea-bootstrap", "gitea-seed", "keycloak-keycloak", "portal-portal"]


@pytest.mark.parametrize("structure", ["components/portal/helmfile.yaml.gotmpl", "components/portal/component.yaml"])
def test_a_component_helmfile_change_syncs_that_components_installed_releases(monkeypatch, tmp_path, structure):
    # hooks, needs and conditions do not show in the render: every release of the component goes
    assert plan(monkeypatch, tmp_path, [structure], change(), listed=LISTED) == ["portal-portal", "portal-ui"]


def test_a_change_to_a_component_without_releases_is_a_full_sync(monkeypatch, tmp_path):
    # F-R2DEP3-011: prepare's helmfile holds the namespace and proxy-annotation hooks and no
    # release, so the scoped plan synced nothing and the change waited for the 6 h full sync.
    got = plan(monkeypatch, tmp_path, ["components/prepare/helmfile.yaml.gotmpl"], change(), listed=LISTED)
    assert got.startswith("full: ") and "changes the hooks of prepare" in got, got


def test_an_unchanged_secrets_release_is_skipped_although_it_renders_fresh_values(monkeypatch, tmp_path):
    # a portal pin beside the generator's random Secret (both HEAD renders differ): masked as
    # scripts/release-diff.py masks it, the secrets release renders identically and is skipped
    head = change(**{"secrets-db-portal": [secret("db", "Yg==")], "portal-portal": [deployment("portal", "p@sha256:2")]})
    again = change(**{"secrets-db-portal": [secret("db", "Yw==")], "portal-portal": [deployment("portal", "p@sha256:2")]})
    got = plan(monkeypatch, tmp_path, ["components/portal/images.yaml"], head, again, listed=LISTED)
    assert got == ["portal-portal"]
    masked = apply_pins.objects(tmp_path / "new" / "secrets-db-portal")["Secret dev/db"][1]
    assert "Yg==" not in masked


@pytest.mark.parametrize("full_at, reason", [
    (None, "no full sync recorded"),
    ("", "no full sync recorded"),
    ("yesterday", "no full sync recorded"),
    (f"{NOW - timedelta(hours=7):%Y-%m-%dT%H:%M:%SZ}", "the last full sync ran 7.0 h ago (every 6 h)"),
    # the boundary: a full sync exactly JC_FULL_SYNC_HOURS ago is due
    (f"{NOW - timedelta(hours=6):%Y-%m-%dT%H:%M:%SZ}", "the last full sync ran 6.0 h ago (every 6 h)"),
])
def test_the_periodic_full_sync_converges_drift(monkeypatch, tmp_path, full_at, reason):
    # a scoped plan judges the render, never live objects: drift waits for the periodic full sync
    got = plan(monkeypatch, tmp_path, ["components/portal/images.yaml"], change(), full_at=full_at)
    assert got.startswith("full: ") and reason in got, got


def test_the_full_sync_interval_is_set_by_the_environment(monkeypatch):
    at = datetime(2026, 10, 31, 8, 0, tzinfo=UTC)
    monkeypatch.setenv("JC_FULL_SYNC_HOURS", "1")
    assert apply_pins.full_sync_due("2026-10-31T07:30:00Z", at) is None
    assert "every 1 h" in apply_pins.full_sync_due("2026-10-31T06:59:00Z", at)


def test_the_plan_reads_the_clock_it_is_given(monkeypatch, tmp_path):
    # one injected `now` per plan: a run crossing a minute boundary judges the same age (T-6199)
    monkeypatch.delenv("JC_FULL_SYNC_HOURS", raising=False)
    full_at = f"{NOW - timedelta(hours=7):%Y-%m-%dT%H:%M:%SZ}"
    for run in range(200):
        at = NOW + timedelta(minutes=run // 2, seconds=run % 2)  # hh:mm:59, then hh:mm+1:00
        got = plan(monkeypatch, tmp_path, ["components/portal/images.yaml"], change(), full_at=full_at, now=at)
        assert got == f"full: {apply_pins.full_sync_due(full_at, at)}", got


def test_dev_apply_reads_the_real_clock_once():
    text = (ROOT / "scripts/apply-pins.py").read_text()
    assert text.count("datetime.now(") == 1 and "datetime.now(UTC))" in text[text.index("def main("):]


def test_dev_apply_reads_and_stamps_the_last_full_sync():
    text = (ROOT / "justfile").read_text()
    body = text[text.index("\ndev-apply full="):text.index("\n_smoke ")]
    assert "{.data.fullSyncAt}" in body and 'apply-pins.py plan dev "$base" "$full_at"' in body
    assert '--full)' in body[body.index("release.py record dev dev"):]


@pytest.mark.parametrize("release", ["gitea-seed", "gitea-seed-11", "gitea-sample-apps", "gitea-sample-apps-7"])
def test_a_moved_seed_release_also_runs_the_forge_bootstrap(monkeypatch, tmp_path, release):
    # T-6593: the KPI model moved in gitea-seed-N's ConfigMap and the forge kept the old one,
    # because only gitea-bootstrap's post-upgrade Job commits the seed and it was not synced
    monkeypatch.setitem(BEFORE, release, [config("seed", {"helsinki-kpi.yaml": "derivedFrom: string"})])
    head = change(**{release: [config("seed", {"helsinki-kpi.yaml": "derivedFrom: list"})]})
    assert plan(monkeypatch, tmp_path, ["components/context-gateway/seed/helsinki/x.yaml"], head) == [
        "gitea-bootstrap", release]

def test_a_release_outside_the_seed_runs_no_bootstrap(monkeypatch, tmp_path):
    head = change(**{"keycloak-keycloak": [deployment("keycloak", "k@sha256:2"), config("realm", {"x": "1"})]})
    assert plan(monkeypatch, tmp_path, ["components/keycloak/images.yaml"], head) == ["keycloak-keycloak"]
