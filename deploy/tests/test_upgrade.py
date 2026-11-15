"""T-3794: `just upgrade <environment> <version>` and the release notes it reads.

`scripts/upgrade.py` runs in a throwaway git repository holding a tagged release, with
`kubectl`, `helm` and `helmfile` replaced by one stub that answers from the test's settings and
records every call. What the cluster would have done is therefore the call log: no backup
taken, no sync, a rollback to the revision before.
"""

import json
import os
import shutil
import stat
import subprocess
import sys
import textwrap
from pathlib import Path

import re
import pytest
import yaml

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))
import upgrade  # noqa: E402

GIB = 1024**3

STUB = textwrap.dedent('''\
    #!/usr/bin/env python3
    import json, os, sys
    tool, args = os.path.basename(sys.argv[0]), sys.argv[1:]
    state = os.environ["FAKE_DIR"]
    stdin = sys.stdin.read() if "-f" in args and "-" in args else ""
    with open(os.path.join(state, "calls"), "a") as log:
        log.write(json.dumps([tool, *args, json.loads(stdin)["kind"] if stdin else ""]) + "\\n")
    synced = os.path.exists(os.path.join(state, "synced"))
    def out(value): print(json.dumps(value)); sys.exit(0)
    if tool == "kubectl":
        joined = " ".join(args)
        if "/readyz" in joined: print("ok"); sys.exit(0)
        if "stats/summary" in joined: out({"node": {"fs": {"availableBytes": int(os.environ["FAKE_FREE"])}}})
        if "metadata.name=jc-release" in joined:
            out({"items": [{"metadata": {"namespace": "prod"}, "data": {"version": "v1.0.0"}}]})
        if "metadata.name=apisix-edge" in joined: out({"items": [{"spec": {"dnsNames": ["city.example"]}}]})
        if args[:2] == ["get", "nodes"]: out({"items": [{"metadata": {"name": "n1"}}]})
        if args[:2] == ["get", "clusters.postgresql.cnpg.io"]:
            spec = {"backup": {"barmanObjectStore": {"destinationPath": "s3://b"}}} if os.environ["FAKE_BACKUP"] != "none" else {}
            out({"items": [{"metadata": {"name": "postgres-cluster", "namespace": "prod"}, "spec": spec}]})
        if args[:2] == ["get", "backups.postgresql.cnpg.io"]:
            phase = os.environ["FAKE_BACKUP"]
            out({"status": {"phase": phase, "stoppedAt": "2026-10-12T00:00:00Z", "error": "bucket refused"}})
        if args[0] == "apply": sys.exit(0)
    if tool == "helm":
        if args[0] == "list":
            out([{"name": "portal-portal", "namespace": "prod", "revision": "2" if synced else "1"}]
                + ([{"name": "mqtt-brokers-brokers", "namespace": "prod", "revision": "1"}] if synced else []))
        if args[0] == "rollback": sys.exit(0)
    if tool == "helmfile":
        if "sync" in args: open(os.path.join(state, "synced"), "w").close(); sys.exit(int(os.environ.get("FAKE_SYNC_RC", "0")))
        if "diff" in args: print("diff: portal image"); sys.exit(0)
    sys.stderr.write("stub: unexpected " + tool + " " + " ".join(args)); sys.exit(64)
    ''')

NOTES = textwrap.dedent('''\
    ---
    version: v1.1.0
    previous: v1.0.0
    images:
    - rmqtt/rmqtt:0.24.0@sha256:{d}
    steps:
    - name: reindex the catalogue
      run: releases/steps/v1.1.0-reindex.sh
    ---

    # Release v1.1.0
    ''').format(d="0" * 64)


def git(repo, *args):
    subprocess.run(["git", *args], cwd=repo, check=True, capture_output=True)


@pytest.fixture
def repo(tmp_path):
    """A repository whose tag v1.1.0 holds the release notes, a step and the two scripts the
    upgrade runs from the release's tree, plus stubs for the cluster's tools."""
    repo = tmp_path / "repo"
    (repo / "scripts").mkdir(parents=True)
    shutil.copy(ROOT / "scripts/upgrade.py", repo / "scripts/upgrade.py")
    (repo / "scripts/wait-rollouts.sh").write_text('#!/bin/sh\necho "wait-rollouts $1"\nexit 0\n')
    (repo / "scripts/smoke.sh").write_text('#!/bin/sh\necho "smoke $1"\nexit "${FAKE_SMOKE_RC:-0}"\n')
    (repo / "scripts/edge-smoke.sh").write_text('#!/bin/sh\necho "edge-smoke $1 $2"\nexit 0\n')
    (repo / "releases/steps").mkdir(parents=True)
    (repo / "releases/v1.1.0.md").write_text(NOTES)
    (repo / "releases/steps/v1.1.0-reindex.sh").write_text('touch "$FAKE_DIR/step-ran"\n')
    for script in (repo / "scripts").glob("*.sh"):
        script.chmod(0o755)
    (repo / ".gitignore").write_text("deployment/\n")
    git(repo, "init", "-q", "-b", "main")
    git(repo, "-c", "user.name=t", "-c", "user.email=t@t", "add", "-A")
    git(repo, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "release")
    git(repo, "tag", "v1.1.0")
    (repo / "deployment/environments/prod").mkdir(parents=True)
    (repo / "deployment/environments/dev").mkdir(parents=True)
    (repo / "deployment/environments/testing").mkdir(parents=True)
    (repo / "deployment/environments/testing/global.yaml.gotmpl").write_text(
        "---\n# Generated\nglobal:\n  profile: development\n  instanceSlug: dev\n")
    (repo / "deployment/helmfile.yaml").write_text("---\n")
    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    for tool in ("kubectl", "helm", "helmfile"):
        (bin_dir / tool).write_text(STUB)
        (bin_dir / tool).chmod((bin_dir / tool).stat().st_mode | stat.S_IEXEC)
    state = tmp_path / "state"
    state.mkdir()
    return repo, bin_dir, state


def upgrade_run(repo, *args, free_gib=50, backup="completed", smoke=0, sync=0):
    repo_dir, bin_dir, state = repo
    env = {**os.environ, "PATH": f"{bin_dir}:{os.environ['PATH']}", "FAKE_DIR": str(state),
           "FAKE_FREE": str(int(free_gib * GIB)), "FAKE_BACKUP": backup,
           "FAKE_SMOKE_RC": str(smoke), "FAKE_SYNC_RC": str(sync)}
    result = subprocess.run([sys.executable, str(repo_dir / "scripts/upgrade.py"), *args],
                            capture_output=True, text=True, env=env, cwd=repo_dir)
    calls = [json.loads(line) for line in (state / "calls").read_text().splitlines()] if (state / "calls").exists() else []
    return result, calls


def applied(calls, kind):
    return [c for c in calls if c[0] == "kubectl" and c[1] == "apply" and c[-1] == kind]


def test_check_shows_the_jump_and_the_steps_and_changes_nothing(repo):
    result, calls = upgrade_run(repo, "prod", "v1.1.0", "--check")
    assert result.returncode == 0, result.stderr
    assert "prod: v1.0.0 -> v1.1.0" in result.stdout
    assert "step after apply: reindex the catalogue (releases/steps/v1.1.0-reindex.sh)" in result.stdout
    assert "diff: portal image" in result.stdout
    assert "--check: nothing was changed" in result.stdout
    assert not [c for c in calls if c[0] == "helmfile" and "sync" in c]
    assert not [c for c in calls if c[0] == "kubectl" and c[1] == "apply"]
    # The operator's checkout never moves; the release's worktree is gone again.
    assert subprocess.run(["git", "worktree", "list"], cwd=repo[0], capture_output=True, text=True).stdout.count("\n") == 1


def test_low_disk_refuses_before_anything_happens(repo):
    result, calls = upgrade_run(repo, "prod", "v1.1.0", free_gib=2)
    assert result.returncode == 1
    assert "REFUSED: node n1 has 2.0 GiB free, the upgrade needs 5.0" in result.stderr
    assert not applied(calls, "Backup")
    assert not [c for c in calls if c[0] == "helmfile"]


def test_a_failed_backup_refuses_the_apply(repo):
    result, calls = upgrade_run(repo, "prod", "v1.1.0", backup="failed")
    assert result.returncode == 1
    assert "failed: bucket refused" in result.stderr
    assert len(applied(calls, "Backup")) == 1
    assert not [c for c in calls if c[0] == "helmfile"]


def test_no_backup_configured_refuses_unless_the_operator_says_so_and_that_is_logged(repo):
    result, calls = upgrade_run(repo, "prod", "v1.1.0", backup="none")
    assert result.returncode == 1
    assert "no backup is configured for prod/postgres-cluster" in result.stderr
    assert not [c for c in calls if c[0] == "helmfile"]

    result, calls = upgrade_run(repo, "prod", "v1.1.0", "--i-have-a-backup", backup="none")
    assert result.returncode == 0, result.stdout + result.stderr
    assert "passed --i-have-a-backup for prod/postgres-cluster: no backup taken there" in result.stdout
    assert not applied(calls, "Backup")


def test_a_red_smoke_rolls_back_what_changed_and_deletes_nothing(repo):
    result, calls = upgrade_run(repo, "prod", "v1.1.0", smoke=1)
    assert result.returncode == 1
    rollbacks = [c[1:-1] for c in calls if c[0] == "helm" and c[1] == "rollback"]
    assert rollbacks == [["rollback", "portal-portal", "1", "-n", "prod", "--wait", "--timeout", "1800s"]]
    assert "prod/mqtt-brokers-brokers is new in this release and stays installed" in result.stdout
    assert "restore prod/postgres-cluster-pre-v1-1-0-" in result.stdout
    assert "Nothing was deleted." in result.stdout
    assert not [c for c in calls if c[0] in ("kubectl", "helm") and "delete" in c]
    assert not applied(calls, "ConfigMap")


def test_green_runs_the_steps_and_records_the_version(repo):
    result, calls = upgrade_run(repo, "prod", "v1.1.0")
    assert result.returncode == 0, result.stdout + result.stderr
    assert (repo[2] / "step-ran").exists()
    assert "smoke https://city.example" in result.stdout
    assert len(applied(calls, "Backup")) == 1
    assert len(applied(calls, "ConfigMap")) == 1
    assert not [c for c in calls if c[0] == "helm" and c[1] == "rollback"]


def test_the_rollouts_are_awaited_in_the_instance_the_environment_names(repo):
    # ci-full 39342677129: environment testing installs instance dev; waiting on a namespace
    # named after the environment found none and rolled a green upgrade back.
    result, _ = upgrade_run(repo, "testing", "v1.1.0")
    assert result.returncode == 0, result.stdout + result.stderr
    assert "wait-rollouts dev" in result.stdout
    result, _ = upgrade_run(repo, "prod", "v1.1.0")
    assert "wait-rollouts prod" in result.stdout


def test_a_failed_sync_rolls_back_without_running_the_steps(repo):
    result, _ = upgrade_run(repo, "prod", "v1.1.0", sync=1)
    assert result.returncode == 1
    assert not (repo[2] / "step-ran").exists()
    assert "RED: rolling back" in result.stdout


def test_the_named_smoke_of_the_release_gates_the_run(repo):
    # ci-full 39362707152 (T-4795): the k3d upgrade job gates on scripts/edge-smoke.sh; smoke.sh
    # stays the default for a live installation.
    result, calls = upgrade_run(repo, "prod", "v1.1.0", "--smoke", "scripts/edge-smoke.sh", smoke=1)
    assert result.returncode == 0, result.stdout + result.stderr
    assert "edge-smoke https://city.example https://idm.city.example" in result.stdout
    assert "smoke https://city.example\n" not in result.stdout
    assert len(applied(calls, "ConfigMap")) == 1


def test_a_smoke_the_release_does_not_carry_is_refused_before_the_sync(repo):
    result, calls = upgrade_run(repo, "prod", "v1.1.0", "--smoke", "scripts/missing.sh")
    assert result.returncode == 1
    assert "v1.1.0 has no scripts/missing.sh" in result.stderr
    assert not [c for c in calls if c[0] == "helmfile" and "sync" in c]


@pytest.mark.parametrize("args, message", [
    (["prod", "v1.1.0", "--smoke", "../outside.sh"], "is not a script scripts/<name>.sh"),
    (["prod", "v1.1.0", "--smoke", "/bin/true"], "is not a script scripts/<name>.sh"),
    (["prod", "v1.1.0", "--smoke", "scripts/../../x.sh"], "is not a script scripts/<name>.sh"),
    (["dev", "v1.1.0"], "dev is applied with `just dev-apply`"),
    (["prod", "1.1.0"], "is not a release tag"),
    (["prod", "v9.9.9"], "failed"),
])
def test_refusals_before_the_cluster_is_touched(repo, args, message):
    result, calls = upgrade_run(repo, *args)
    assert result.returncode == 1
    assert message in result.stderr
    assert not [c for c in calls if c[0] in ("helmfile", "helm")]


@pytest.mark.parametrize("text, message", [
    (NOTES.replace("version: v1.1.0", "version: v1.0.9"), "is the notes of"),
    (NOTES.replace("releases/steps/v1.1.0-reindex.sh", "../outside.sh"), "runs outside the release"),
    (NOTES.replace("  run:", "  shell:"), "a step is {name, run}"),
    ("# no front matter\n", "has no front matter"),
])
def test_notes_that_are_not_a_release_are_refused(text, message):
    with pytest.raises(upgrade.Refused, match=message.replace("{", r"\{").replace("}", r"\}")):
        upgrade.parse_notes("v1.1.0", text)


def test_only_releases_that_advanced_are_rolled_back():
    before = [{"name": "a", "namespace": "x", "revision": "3"}, {"name": "b", "namespace": "x", "revision": "1"}]
    after = [{"name": "a", "namespace": "x", "revision": "4"}, {"name": "b", "namespace": "x", "revision": "1"},
             {"name": "c", "namespace": "x", "revision": "1"}]
    assert upgrade.changed_releases(before, after) == [("a", "x", 3)]
    assert upgrade.new_releases(before, after) == ["x/c"]


def test_release_notes_read_every_pin_and_refuse_one_without_a_digest(tmp_path):
    sys.path.insert(0, str(ROOT / "scripts"))
    notes = __import__("release-notes")
    components = tmp_path / "components"
    (components / "a").mkdir(parents=True)
    (components / "a/images.yaml").write_text(yaml.safe_dump(
        {"a": {"x": {"gateway": {"repository": "r/x", "tag": "1", "digest": "sha256:" + "1" * 64}},
               "db": None}}))
    assert notes.images(components) == ["r/x:1@sha256:" + "1" * 64]
    (components / "b").mkdir()
    (components / "b/images.yaml").write_text(yaml.safe_dump({"b": {"y": {"repository": "r/y", "tag": "2"}}}))
    with pytest.raises(SystemExit, match="b/images.yaml.b.y is not pinned by digest"):
        notes.images(components)
    text = notes.notes("v1.1.0", "v1.0.0", ["r/x:1@sha256:" + "1" * 64],
                       [{"name": "s", "run": "releases/steps/s.sh"}], ["T-1: a change"])
    parsed = upgrade.parse_notes("v1.1.0", text)
    assert parsed.images == ["r/x:1@sha256:" + "1" * 64]
    assert parsed.steps == [{"name": "s", "run": "releases/steps/s.sh"}]


def test_the_real_pins_all_carry_a_digest():
    notes = __import__("release-notes")
    assert len(notes.images(ROOT / "components")) > 20


@pytest.mark.parametrize("cluster, version", [
    ("a" * 51, "v1.2.3"),               # the 63rd character lands on a `-` (F-R2DEP3-022)
    ("postgres-cluster", "v1.2.3+build.7"),  # `+` is no DNS character
    ("x" * 70, "v1.0.0"),
])
def test_a_backup_name_is_a_dns_label_whatever_the_cluster_and_version(cluster, version):
    name = upgrade.backup_name(cluster, version, "20261031120000")
    assert re.fullmatch(r"[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?", name), name
