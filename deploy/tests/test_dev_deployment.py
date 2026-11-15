"""The local k3d harness that ci-full's deployment-variants job drives.

Its failures cost a 45-minute job to discover, so the cheap preconditions are asserted here.
"""

import re
import shutil
import subprocess
from pathlib import Path


PROJECT_ROOT = Path(__file__).resolve().parent.parent
DEV_DEPLOYMENT = PROJECT_ROOT / "dev-deployment"
STARTUP = DEV_DEPLOYMENT / "startup.sh"


def test_the_k3d_config_startup_asks_for_exists():
    """A rename that misses the file only shows as `no context exists with the name`."""
    configs = set(re.findall(r"k3d cluster create --config (\S+)", STARTUP.read_text()))
    assert configs, "startup.sh no longer creates a cluster from a config file"
    for config in configs:
        assert (DEV_DEPLOYMENT / config).is_file(), f"{config} is referenced but not in dev-deployment/"


def test_the_harness_only_needs_tools_a_ci_runner_has():
    """dos2unix cost a whole ci-full run; nothing here may depend on a desktop utility."""
    desktop_only = ("dos2unix", "unix2dos", "pbcopy", "open ", "xdg-open")
    for script in sorted(DEV_DEPLOYMENT.glob("*.sh")) + [PROJECT_ROOT / "scripts/test-deployment-variants.sh"]:
        body = "\n".join(
            line for line in script.read_text().splitlines() if not line.lstrip().startswith("#")
        )
        for tool in desktop_only:
            assert tool not in body, f"{script.name} calls {tool.strip()}, which no runner has"


VARIANTS = PROJECT_ROOT / "scripts/test-deployment-variants.sh"


def test_the_variant_harness_builds_the_deployment_tree_it_syncs_from():
    """`deployment/` is assembled, not committed, so a fresh checkout has no state file.

    The harness syncing from a tree it never built is invisible locally — anyone who has run
    `just dev-apply` has the directory already — and costs a 45-minute CI job to discover.
    """
    tracked = subprocess.run(
        ["git", "ls-files", "deployment"], cwd=str(PROJECT_ROOT), capture_output=True, text=True,
    ).stdout
    assert not tracked.strip(), "deployment/ is tracked now; this test's premise is stale"

    body = VARIANTS.read_text()
    assert "helmfile -f ./deployment/" in body, "harness no longer syncs from deployment/"
    assert "_dev-assemble" in body, "syncs from a tree a fresh checkout does not have"

    # Order inside run_variant, not order of definitions: assembling starts by removing
    # deployment/, so doing it after the scratch environment is written throws that away.
    run_variant = body[body.index("run_variant() {"):].split("\n}\n")[0]
    code = "\n".join(l for l in run_variant.splitlines() if not l.lstrip().startswith("#"))
    calls = [m.group(1) for m in re.finditer(r"\b(assemble_deployment|write_env_overlay|deploy_variant)\b", code)]
    assert calls[:3] == ["assemble_deployment", "write_env_overlay", "deploy_variant"], calls




ROTATE_EACH = PROJECT_ROOT / "scripts/k3d-rotate-each-class.sh"


def test_the_step_after_the_smoke_refuses_to_run_without_the_scratch_environment(tmp_path):
    """Run outside the harness it would apply to an environment nobody named; it stops first."""
    result = subprocess.run(
        ["bash", str(ROTATE_EACH)], cwd=str(PROJECT_ROOT), capture_output=True, text=True,
        env={"PATH": "/usr/bin:/bin", "HOME": str(tmp_path)},
    )
    assert result.returncode != 0
    assert "JC_ENV" in result.stderr


UPGRADE = PROJECT_ROOT / "scripts/ci-upgrade-e2e.sh"


def test_the_upgrade_script_runs_only_images_the_platform_pins():
    """The S3 store and its client are the artifact store's pins, read from its images.yaml,
    never a tag typed into the script."""
    text = UPGRADE.read_text()
    assert "components/artifact-store/images.yaml" in text
    assert re.findall(r'"\$\(image (\w+)\)"', text) == ["store", "bootstrap"]
    assert not re.search(r"\b[\w./-]+:[\w.-]+@sha256:", text)


def test_the_upgrade_script_refuses_without_a_release_to_upgrade_from(tmp_path):
    """No `v*` tag below HEAD: it says who cuts the first one and touches no cluster."""
    repo = tmp_path / "repo"
    (repo / "scripts").mkdir(parents=True)
    shutil.copy(UPGRADE, repo / "scripts")
    git = ["git", "-c", "user.name=t", "-c", "user.email=t@t.invalid", "-C", str(repo)]
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    subprocess.run([*git, "add", "-A"], check=True)
    subprocess.run([*git, "commit", "-qm", "x"], check=True)
    subprocess.run([*git, "tag", "v0.1.0-rc.1"], check=True)
    result = subprocess.run([str(repo / "scripts/ci-upgrade-e2e.sh")], cwd=repo, capture_output=True, text=True,
                            env={"PATH": "/usr/bin:/bin"})
    assert result.returncode == 1
    assert "no release tag" in result.stderr and "just release-notes v0.1.0" in result.stderr
    assert "upgrade:" not in result.stdout
