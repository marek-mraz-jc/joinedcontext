"""T-4955: releases of the deployment, `scripts/release.py`.

Each test runs in a throwaway git repository with a bare repository as its origin, so the
tags, the pushes and the refusals are real git; `kubectl` is a stub that records its stdin.
"""

import json
import os
import re
import stat
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import release  # noqa: E402

DIGEST = "sha256:" + "a" * 64
IDENTITY = ["-c", "user.name=t", "-c", "user.email=ops@example.org"]


def git(repo: Path, *args: str) -> str:
    return subprocess.run(["git", *IDENTITY, *args], cwd=repo, check=True, capture_output=True,
                          text=True).stdout.strip()


@pytest.fixture
def repo(tmp_path):
    origin = tmp_path / "origin.git"
    subprocess.run(["git", "init", "-q", "--bare", "-b", "main", str(origin)], check=True)
    work = tmp_path / "work"
    subprocess.run(["git", "clone", "-q", str(origin), str(work)], check=True, capture_output=True)
    git(work, "config", "user.name", "t")
    git(work, "config", "user.email", "ops@example.org")
    git(work, "checkout", "-q", "-b", "main")
    (work / "components/platform").mkdir(parents=True)
    (work / "components/platform/images.yaml").write_text(yaml.safe_dump({"platform": {
        "gateway": {"repository": "ghcr.io/x/joinedcontext-platform", "tag": "main", "digest": DIGEST},
        "portal": {"repository": "ghcr.io/x/joinedcontext-portal", "tag": "main", "digest": DIGEST}}}))
    (work / "releases").mkdir()
    (work / "releases/next.yaml").write_text("---\nsteps: []\n")
    git(work, "add", "-A")
    git(work, "commit", "-q", "-m", "T-0001: start")
    git(work, "push", "-q", "origin", "main")
    return work


def commit(repo: Path, subject: str) -> None:
    (repo / "f.txt").write_text(subject)
    git(repo, "add", "f.txt")
    git(repo, "commit", "-q", "-m", subject)
    git(repo, "push", "-q", "origin", "main")


@pytest.mark.parametrize("previous,part,expected", [
    (None, "patch", "v0.1.0"), ("v0.1.0", "patch", "v0.1.1"), ("v0.1.9", "minor", "v0.2.0"),
    ("v0.4.2", "major", "v1.0.0"), ("v1.2.3-rc.1", "patch", "v1.2.4")])
def test_bump(previous, part, expected):
    assert release.bump(previous, part) == expected


def test_bump_refuses_a_foreign_part_or_version():
    with pytest.raises(release.Refused):
        release.bump("v1.0.0", "huge")
    with pytest.raises(release.Refused):
        release.bump("1.0", "patch")


def test_changelog_from_fixtures_newest_first_with_tasks_and_own_images(tmp_path):
    notes = __import__("release-notes")
    images = [f"ghcr.io/x/joinedcontext-platform:main@{DIGEST}", f"r/other:1@{DIGEST}"]
    (tmp_path / "v0.1.0.md").write_text(notes.notes("v0.1.0", None, images, [], ["T-0012: a", "plain", "T-12 is too short"], "2026-10-01"))
    (tmp_path / "v0.10.0.md").write_text(notes.notes("v0.10.0", "v0.2.0", images, [], ["T-0030 and T-0005: b"]))
    (tmp_path / "v0.2.0.md").write_text(notes.notes("v0.2.0", "v0.1.0", [], [], ["T-31: c", "T-31: again"]))
    (tmp_path / "next.yaml").write_text("steps: []\n")
    text = release.changelog(tmp_path)
    heads = [line for line in text.splitlines() if line.startswith("## ")]
    assert heads == ["## v0.10.0", "## v0.2.0", "## v0.1.0 (2026-10-01)"]
    assert "Tasks: T-0005, T-0030\n" in text and "Tasks: T-0012\n" in text
    assert "Tasks: none" in text  # v0.2.0: T-31 has too few digits to be a task id
    assert f"- platform: `ghcr.io/x/joinedcontext-platform:main@{DIGEST}`" in text
    assert "- portal: not pinned" in text and "r/other" not in text


def test_the_committed_changelog_is_the_render_of_the_notes():
    assert (ROOT / "CHANGELOG.md").read_text() == release.changelog(ROOT / "releases"), \
        "run scripts/release.py changelog"


def test_cut_writes_notes_changelog_and_an_annotated_tag_and_pushes_without_force(repo):
    commit(repo, "T-0102: first change")
    assert release.cut("minor", push=True, cwd=repo, today="2026-10-21") == "v0.1.0"
    assert git(repo, "cat-file", "-t", "refs/tags/v0.1.0") == "tag"
    assert "release v0.1.0: 2 tasks" in git(repo, "tag", "-n1", "v0.1.0")
    assert git(repo, "ls-remote", "--tags", "origin", "v0.1.0")
    assert git(repo, "rev-parse", "origin/main") == git(repo, "rev-parse", "HEAD")
    meta, _ = release.read_notes(repo / "releases/v0.1.0.md")
    assert meta["date"] == "2026-10-21" and meta["previous"] is None
    assert "Tasks: T-0001, T-0102" in (repo / "CHANGELOG.md").read_text()
    commit(repo, "T-0103: second")
    assert release.cut("patch", push=True, cwd=repo, today="2026-10-22") == "v0.1.1"
    changelog = (repo / "CHANGELOG.md").read_text()
    assert changelog.index("## v0.1.1") < changelog.index("## v0.1.0")
    assert "Since v0.1.0" in changelog and "Tasks: T-0103\n" in changelog


def test_cut_refuses_a_dirty_tree_and_a_head_that_is_not_origin_main(repo):
    (repo / "components/platform/images.yaml").write_text("changed: true\n")
    with pytest.raises(release.Refused, match="uncommitted"):
        release.cut("patch", push=False, cwd=repo)
    git(repo, "checkout", "-q", "--", ".")
    (repo / "f.txt").write_text("local")
    git(repo, "add", "f.txt")
    git(repo, "commit", "-q", "-m", "local only")
    with pytest.raises(release.Refused, match="not origin/main"):
        release.cut("patch", push=False, cwd=repo)
    assert not git(repo, "tag", "--list")


def test_prod_apply_refuses_an_untagged_sha(repo):
    commit(repo, "T-0200: untagged")
    with pytest.raises(release.Refused, match="no annotated release tag"):
        release.verify("HEAD", cwd=repo)


def test_verify_refuses_a_lightweight_or_unpushed_tag_and_a_dirty_tree(repo):
    git(repo, "tag", "v0.1.0")
    with pytest.raises(release.Refused, match="lightweight v0.1.0"):
        release.verify("HEAD", cwd=repo)
    git(repo, "tag", "-d", "v0.1.0")
    git(repo, "tag", "-a", "v0.1.0", "-m", "r")
    with pytest.raises(release.Refused, match="not on origin"):
        release.verify("HEAD", cwd=repo)
    git(repo, "push", "-q", "origin", "refs/tags/v0.1.0")
    assert release.verify("HEAD", cwd=repo) == "v0.1.0"
    (repo / "releases/next.yaml").write_text("steps: [x]\n")
    with pytest.raises(release.Refused, match="uncommitted"):
        release.verify("HEAD", cwd=repo)


def test_the_previous_tag_verifies_for_a_rollback(repo):
    release.cut("patch", push=True, cwd=repo, today="2026-10-21")
    commit(repo, "T-0300: next")
    release.cut("patch", push=True, cwd=repo, today="2026-10-21")
    git(repo, "checkout", "-q", "--detach", "v0.1.0")
    assert release.verify("HEAD", cwd=repo) == "v0.1.0"


@pytest.fixture
def kubectl_stub(tmp_path, monkeypatch):
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    stub = bin_dir / "kubectl"
    stub.write_text(textwrap.dedent(f"""\
        #!/usr/bin/env python3
        import json, sys
        if sys.argv[1] == "apply":
            open({str(tmp_path / 'applied.json')!r}, "w").write(sys.stdin.read())
        elif sys.argv[3] == "patch":
            open({str(tmp_path / 'patched.json')!r}, "w").write(json.dumps(sys.argv[4:]))
        elif sys.argv[4:6] == ["configmap", "jc-release"]:
            try:
                print(open({str(tmp_path / 'applied.json')!r}).read())
            except FileNotFoundError:
                sys.stderr.write('configmaps "jc-release" not found'); sys.exit(1)
        """))
    stub.chmod(stub.stat().st_mode | stat.S_IEXEC)
    monkeypatch.setenv("PATH", f"{bin_dir}:{os.environ['PATH']}")
    return tmp_path / "applied.json"


def test_record_writes_tag_sha_and_who_applied_and_show_prints_it(repo, kubectl_stub):
    assert release.show("dev").startswith("release: none recorded in dev")
    commit(repo, "T-0400: change")
    data = release.record("dev", "dev", cwd=repo)
    assert data["version"] == "untagged" and data["appliedBy"] == "ops@example.org"
    release.cut("patch", push=True, cwd=repo)
    data = release.record("smartcity", "smartcity", cwd=repo)
    manifest = json.loads(kubectl_stub.read_text())
    assert manifest["metadata"] == {"name": "jc-release", "namespace": "smartcity", "labels": {
        "app.kubernetes.io/part-of": "joinedcontext", "app.kubernetes.io/managed-by": "release.py"}}
    assert manifest["data"]["version"] == "v0.1.0"
    assert manifest["data"]["commit"] == git(repo, "rev-parse", "HEAD")
    assert release.show("smartcity").startswith("release: v0.1.0 (v0.1.0) applied by ops@example.org at ")


def test_a_full_apply_stamps_full_sync_at_and_a_scoped_one_carries_it(repo, kubectl_stub):
    # T-6187: apply-pins.py plans a full sync once fullSyncAt is old, so a scoped record keeps it
    assert "fullSyncAt" not in release.record("dev", "dev", cwd=repo)
    full = release.record("dev", "dev", cwd=repo, full=True)
    assert full["fullSyncAt"] == full["appliedAt"]
    commit(repo, "T-0401: change")
    scoped = release.record("dev", "dev", cwd=repo)
    assert scoped["fullSyncAt"] == full["fullSyncAt"]
    assert json.loads(kubectl_stub.read_text())["data"]["fullSyncAt"] == full["fullSyncAt"]


def test_an_apply_stamps_its_start_so_the_walker_sees_it_running(repo, kubectl_stub):
    # T-6425: appliedAt is written when an apply ends; a walk that ended before the apply did saw
    # nothing move and filed the 503 of the 07:1xZ k3s restart.
    at = release.start("dev")
    args = json.loads((kubectl_stub.parent / "patched.json").read_text())
    assert args[:4] == ["configmap", "jc-release", "--type", "merge"]
    assert json.loads(args[5]) == {"data": {"applyStartedAt": at}}
    assert re.fullmatch(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\dZ", at)


def test_every_dev_apply_stamps_its_start_before_it_syncs():
    text = (ROOT / "justfile").read_text()
    full = text[text.index("\ndev-apply full="):text.index("\n_smoke ")]
    assert full.index("release.py start dev") < full.index("helmfile -f deployment/helmfile.yaml -e dev sync")
    pins = text[text.index("\ndev-apply-pins "):]
    assert pins.index("release.py start dev") < pins.index("helmfile -f deployment/helmfile.yaml -e dev sync")
    # a dry run or an empty plan changes nothing, so it stamps nothing
    assert pins.index('if [ "{{ dry }}" = true ]') < pins.index("release.py start dev")


def test_the_recipes_gate_prod_on_a_tag_and_record_every_apply():
    text = (ROOT / "justfile").read_text()
    prod = text[text.index("\nprod-apply:"):text.index("\nprod-smoke:")]
    assert prod.index("just _prod-guard") < prod.index("release.py verify HEAD") < prod.index("helmfile")
    assert "release.py record smartcity smartcity" in prod
    dev = text[text.index("\ndev-apply full="):text.index("\n_smoke ")]
    assert dev.index("wait-rollouts.sh dev") < dev.index("release.py record dev dev")
    assert 'release.py show "{{ namespace }}"' in text[text.index("\n_smoke "):text.index("\ndev-smoke:")]
    assert 'git tag --annotate "$tag"' in text
