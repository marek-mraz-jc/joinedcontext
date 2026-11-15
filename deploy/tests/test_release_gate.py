"""T-0051: the production release gate, `scripts/release-diff.py` and release-gate.yml."""

import importlib.util
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("release_diff", ROOT / "scripts" / "release-diff.py")
release_diff = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release_diff)

WORKFLOW = ROOT / ".github" / "workflows" / "release-gate.yml"


def render(tmp_path: Path, name: str, docs: list[dict]) -> str:
    path = tmp_path / name
    path.write_text(yaml.safe_dump_all(docs))
    return str(path)


def deployment(image: str, checksum: str) -> dict:
    return {"apiVersion": "apps/v1", "kind": "Deployment", "metadata": {"name": "portal", "namespace": "jc"},
            "spec": {"template": {"metadata": {"annotations": {"checksum/secret": checksum}},
                                  "spec": {"containers": [{"name": "portal", "image": image}]}}}}


def secret(value: str) -> dict:
    return {"apiVersion": "v1", "kind": "Secret", "metadata": {"name": "db", "namespace": "jc"},
            "data": {"password": value}, "stringData": {"token": value}}


def diff(tmp_path, old, new, **kw) -> str:
    return release_diff.report(release_diff.objects(render(tmp_path, "old.yaml", old)),
                               release_diff.objects(render(tmp_path, "new.yaml", new)), **kw)


def test_a_rerender_with_fresh_secrets_changes_nothing(tmp_path):
    out = diff(tmp_path, [secret("czFyZW5kZXIx"), deployment("portal@sha256:a", "111")],
               [secret("czJyZW5kZXIy"), deployment("portal@sha256:a", "222")], max_bytes=10_000)
    assert out.startswith("**0 added, 0 removed, 0 changed** of 2 objects")
    assert "czFyZW5kZXIx" not in out and "czJyZW5kZXIy" not in out


def test_an_image_change_shows_its_diff_and_never_a_secret_value(tmp_path):
    out = diff(tmp_path, [secret("b2xkLXZhbHVl"), deployment("portal@sha256:a", "1")],
               [secret("bmV3LXZhbHVl"), deployment("portal@sha256:b", "2"),
                {"apiVersion": "v1", "kind": "ConfigMap", "metadata": {"name": "new", "namespace": "jc"}}],
               max_bytes=10_000)
    assert "**1 added, 0 removed, 1 changed** of 3 objects" in out
    assert "- `ConfigMap jc/new`" in out
    assert "-      - image: portal@sha256:a" in out and "+      - image: portal@sha256:b" in out
    assert "b2xkLXZhbHVl" not in out and "bmV3LXZhbHVl" not in out


def test_a_removed_object_is_listed(tmp_path):
    out = diff(tmp_path, [secret("eA==")], [], max_bytes=10_000)
    assert "### Removed" in out and "- `Secret jc/db`" in out


def test_a_full_summary_names_what_it_leaves_out(tmp_path):
    old = [deployment(f"img@sha256:{i}", "x") | {"metadata": {"name": f"d{i}", "namespace": "jc"}} for i in range(5)]
    new = [deployment(f"img@sha256:{i}{i}", "x") | {"metadata": {"name": f"d{i}", "namespace": "jc"}} for i in range(5)]
    old.append(secret("eA==") | {"kind": "ConfigMap", "data": {"seed": "a\n" * 2_000}})
    new.append(secret("eA==") | {"kind": "ConfigMap", "data": {"seed": "b\n" * 2_000}})
    out = diff(tmp_path, old, new, max_bytes=2_000)
    assert len(out.encode()) <= 2_000
    assert "<code>Deployment jc/d0</code>" in out  # a big diff first leaves room for the next ones
    assert "the whole diff is in the run's artifact: `ConfigMap jc/db`" in out


def test_the_gate_waits_on_production_and_holds_no_cluster_credential():
    text = WORKFLOW.read_text()
    flow = yaml.safe_load(text)
    on = flow[True]  # PyYAML reads the bare key `on` as True
    assert on["push"] == {"tags": ["v*"]} and "workflow_dispatch" in on and "schedule" not in on
    assert flow["permissions"] == {"contents": "read"}
    assert flow["jobs"]["approve"]["environment"] == "production"
    assert flow["jobs"]["approve"]["needs"] == ["diff", "sweep"]
    assert "secrets." not in text  # no repository or environment secret is read
    runs = [step["run"] for job in flow["jobs"].values() for step in job.get("steps", []) if "run" in step]
    for word in ("KUBECONFIG", "kubectl", " sync", " apply"):
        assert not [r for r in runs if word in r.replace("`just prod-apply`", "")], word


def test_the_gate_refuses_a_digest_without_the_scan_record():
    # T-6154: dev pins before image-scan.yml ran; the release gate and prod-apply read its result.
    flow = yaml.safe_load(WORKFLOW.read_text())
    runs = [step.get("run", "") for step in flow["jobs"]["diff"]["steps"]]
    gate = [r for r in runs if "verify-pinned-images.py smartcity" in r]
    assert gate and "--scanned" in gate[0] and "release.yaml" in gate[0]
    render = next(i for i, r in enumerate(runs) if "scripts/render.sh smartcity" in r)
    assert runs.index(gate[0]) > render
    justfile = (ROOT / "justfile").read_text()
    prod = justfile.split("\nprod-apply:\n", 1)[1].split("\n\n", 1)[0]
    assert "verify-pinned-images.py smartcity --scanned" in prod
