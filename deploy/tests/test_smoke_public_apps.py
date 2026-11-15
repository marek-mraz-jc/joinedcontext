"""scripts/smoke-public-apps.py (T-5774): every public App of the seeds answers on its own host."""

import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
_spec = importlib.util.spec_from_file_location("smoke_public_apps", ROOT / "scripts/smoke-public-apps.py")
apps = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(apps)

APP = """apiVersion: joinedcontext.com/v1alpha1
kind: App
metadata: {{ name: {name}, namespace: p }}
spec: {{ kind: ui, visibility: {visibility}, lifecycle: {lifecycle} }}
"""


def write(root: Path, rel: str, **fields) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(APP.format(**fields))


def test_only_published_public_apps_are_read_and_the_test_project_only_on_development(tmp_path):
    write(tmp_path, "components/gitea/apps/a/app.yaml", name="a", visibility="public", lifecycle="published")
    write(tmp_path, "components/gitea/apps/b/app.yaml", name="b", visibility="project", lifecycle="published")
    write(tmp_path, "components/context-gateway/seed/x/x-app.yaml", name="c", visibility="public", lifecycle="draft")
    write(tmp_path, "components/context-gateway/seed/x/y-app.yaml", name="d", visibility="public", lifecycle="published")
    write(tmp_path, "components/context-gateway/seed/sharing/s.yaml", name="s", visibility="public", lifecycle="published")
    (tmp_path / "components/context-gateway/seed/x/policy.yaml").write_text("kind: Policy\n---\n")
    assert apps.public_apps(tmp_path) == ["a", "d"]
    assert apps.public_apps(tmp_path, development=True) == ["a", "d", "s"]


def test_the_seeds_public_apps_include_prods_gitea_built_ones():
    names = apps.public_apps()
    assert {"event-day-planner", "kpi-forecast", "helsinki-events", "hsl-transport", "bbsk-podujatia"} <= set(names)
    assert "sharing-public" not in names
    assert "sharing-public" in apps.public_apps(development=True)


def test_a_host_without_a_bundle_an_ingress_or_a_200_is_ko():
    assert apps.verdict(200, b"<html>app</html>") == "ok"
    assert "no built bundle" in apps.verdict(404, b"<p>This page does not exist in the application.</p>")
    assert "no built bundle" in apps.verdict(200, b"This page does not exist in the application.")
    assert "Ingress" in apps.verdict(0, b"", "no answer")
    assert apps.verdict(503, b"") == "ko: 503"


def test_the_shared_smoke_runs_it_and_dev_includes_the_test_project():
    justfile = (ROOT / "justfile").read_text()
    smoke = justfile[justfile.index("_smoke domain namespace:"):justfile.index("dev-smoke:")]
    assert 'scripts/smoke-public-apps.py "{{ domain }}" $([ "{{ namespace }}" = dev ] && echo --development) || rc=1' in smoke
