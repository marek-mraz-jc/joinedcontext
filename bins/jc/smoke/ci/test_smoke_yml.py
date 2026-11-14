"""smoke.yml hands "run all" the App bundles BuildKit built (brief 120): apps.sh must never fall
back to `docker build --target apps` inside dockerd, and the gate never drops the App rows."""
import pathlib

import yaml

repo = pathlib.Path(__file__).resolve().parents[4]
workflow = repo / ".github/workflows/smoke.yml"


def steps():
    return yaml.safe_load(workflow.read_text())["jobs"]["all"]["steps"]


def test_run_all_takes_the_bundles_image_the_buildx_step_loads():
    run_all = next(s for s in steps() if s.get("name") == "run all")
    image = run_all.get("env", {}).get("REHEARSAL_JC_IMAGE")
    assert image, "run all has no REHEARSAL_JC_IMAGE: apps.sh would build the apps stage in dockerd"
    builds = [s for s in steps() if s.get("uses", "").startswith("docker/build-push-action@")]
    assert any(
        b["with"].get("tags") == image and b["with"].get("target") == "apps-bundles" and b["with"].get("load")
        for b in builds
    ), f"no build-push-action step loads {image} from the apps-bundles stage"
    names = [s.get("name") for s in steps()]
    assert names.index("run all") > max(steps().index(b) for b in builds)
    assert "FROM scratch AS apps-bundles\nCOPY --from=apps /srv/apps /srv/apps\n" in (
        repo / "bins/jc/Dockerfile"
    ).read_text()


def test_jcctl_is_built_beside_jc_before_the_import_validates_with_it():
    """import.sh refuses to import without JCCTL_BIN, default `jcctl` beside jc (target/debug)."""
    runs = [s.get("run", "") for s in steps()]
    jcctl = next(i for i, r in enumerate(runs) if "-p jcctl" in r)
    assert "vendor/joinedcontext-platform/Cargo.toml" in runs[jcctl] and "--target-dir target" in runs[jcctl]
    assert jcctl < [s.get("name") for s in steps()].index("run all")
    assert "JCCTL_BIN" not in str(steps())


def test_the_gate_never_turns_the_apps_off():
    doc = yaml.safe_load(workflow.read_text())
    job = doc["jobs"]["all"]
    for env in (doc.get("env", {}), job.get("env", {}), *(s.get("env", {}) for s in steps())):
        assert "REHEARSAL_APPS" not in env
    for s in steps():
        assert "REHEARSAL_APPS" not in s.get("run", "")


def test_the_logs_suite_looks_for_the_realm_secrets_and_jcs_state_files():
    # collect_secrets reads plain files directly in each dir: the realm client secrets sit in
    # secrets/realm and the suites' --state is a view of links, so each needs its own flag.
    gate = (repo / "bins/jc/smoke/ci/gate.sh").read_text()
    line = next(x for x in gate.splitlines() if x.startswith('args+=(--with "logs='))
    for d in ("$home/secrets ", "$home/secrets/realm ", "$home/state\""):
        assert f"--secrets-dir {d}" in line, d


def test_the_gate_judges_multiorg_in_host_and_path_mode_on_a_composed_bundle():
    # T-6803: on the one-installation synth the suite skipped its setup and judged nothing.
    gate = (repo / "bins/jc/smoke/ci/gate.sh").read_text()
    assert (repo / "bins/jc/smoke/ci/multiorg-export.tgz").is_file()
    assert "multiorg/compose.py" in gate and all(f"{o}=" in gate for o in ("hel", "zilina", "barcelona", "ctl-a", "ctl-b"))
    # T-6850: ten Organizations, none the installation's, operators the platform-admins group.
    assert "for o in bbsk banskabystrica praha slovensko bratislava;" in gate and "--no-installation" in gate
    withs = [x for x in gate.splitlines() if x.startswith('args+=(--with "multiorg')]
    assert [w.split('"')[1].split("=")[0] for w in withs] == ["multiorg", "multiorg@path"]
    assert all("--composed" in w and "--export $mo_export" in w for w in withs)
    assert "--hosts $mo_hosts" in gate and "REHEARSAL_JC_TOML_EXTRA=$mo_hosts multiorg mo-hosts" in gate
