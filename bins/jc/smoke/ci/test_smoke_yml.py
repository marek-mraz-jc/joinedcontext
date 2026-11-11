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


def test_the_gate_never_turns_the_apps_off():
    doc = yaml.safe_load(workflow.read_text())
    job = doc["jobs"]["all"]
    for env in (doc.get("env", {}), job.get("env", {}), *(s.get("env", {}) for s in steps())):
        assert "REHEARSAL_APPS" not in env
    for s in steps():
        assert "REHEARSAL_APPS" not in s.get("run", "")
