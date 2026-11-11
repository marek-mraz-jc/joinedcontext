import json
import sys
import tarfile
from pathlib import Path

import yaml

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import slice as sl  # noqa: E402

BUNDLE = Path(__file__).resolve().parent / "synth-export.tgz"


def write(root: Path, rel: str, text: str) -> None:
    (root / rel).parent.mkdir(parents=True, exist_ok=True)
    (root / rel).write_text(text)


def test_a_slice_drops_the_entries_and_connectors_of_the_projects_it_left_out(tmp_path):
    files = tmp_path / "files"
    project = "kind: Project\nmetadata: {name: %s}\n"
    write(files, "o/configuration/projects/kept.yaml", project % "kept")
    write(files, "o/configuration/projects/gone.yaml", project % "gone")
    write(files, "o/configuration/projects/notes.yaml", "kind: Note\n")
    write(files, "o/kept/assistant/deployments/a.yaml", yaml.safe_dump({"kind": "AssistantDeployment", "spec": {
        "connectors": [{"endpoint": "gone/air"}, {"endpoint": "kept/air"}, {"endpoint": "own"}]}}))
    write(files, "o/kept/assistant/deployments/b.yaml", "kind: AssistantDeployment\n# kept as written\nspec: {}\n")
    assert sl.close(files) == ["o/configuration/projects/gone.yaml", "o/kept/assistant/deployments/a.yaml"]
    assert sorted(p.name for p in (files / "o/configuration/projects").iterdir()) == ["kept.yaml", "notes.yaml"]
    spec = yaml.safe_load((files / "o/kept/assistant/deployments/a.yaml").read_text())["spec"]
    assert spec["connectors"] == [{"endpoint": "kept/air"}, {"endpoint": "own"}]
    assert "# kept as written" in (files / "o/kept/assistant/deployments/b.yaml").read_text()


def test_the_committed_bundle_names_no_project_app_group_or_connector_it_does_not_hold(tmp_path):
    """What `jcctl validate` refused on the bundle before T-6732: registry entries without their
    repository, a connector into project slovensko, the App's groups left in its source repository."""
    with tarfile.open(BUNDLE) as t:
        t.extractall(tmp_path, filter="data")
    files = tmp_path / "gitea" / "files"
    repos = {(r["owner"], r["name"]) for r in json.loads((tmp_path / "gitea" / "repos.json").read_text())}
    for f in (files / "joinedcontext" / "configuration" / "projects").glob("*.yaml"):
        assert ("joinedcontext", f.stem) in repos, f"{f.name} names a project the bundle holds no repository of"
    assert sl.close(files) == []
    assert not list(files.glob("joinedcontext-apps/*/grants"))
    groups = {f.stem for f in (files / "joinedcontext/configuration/users/groups").glob("*.yaml")}
    app = (files / "joinedcontext/helsinki/apps/air-quality/app.yaml").read_text()
    for name in ("air-quality-viewer", "air-quality-steward"):
        assert name in app and name in groups
