"""The local k3d harness that ci-full's deployment-variants job drives.

Its failures cost a 45-minute job to discover, so the cheap preconditions are asserted here.
"""

import re
import shutil
import subprocess
from pathlib import Path

import yaml

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


CI_FULL = PROJECT_ROOT / ".github/workflows/ci-full.yml"


def k3d_deploy_job():
    return yaml.safe_load(CI_FULL.read_text())["jobs"]["k3d-deploy"]


def test_the_nightly_deployment_job_runs_the_one_variant_that_fits_a_runner():
    """0,0,0 is single namespace, all-in-one, no mesh. Any other triplet on a 7 GB runner is
    the eight-variant sweep's job, and a job that quietly grew a second variant is a job that
    stopped finishing inside its timeout (TS-19)."""
    job = k3d_deploy_job()
    runs = [step["run"] for step in job["steps"] if "run" in step]
    variants = [line for line in "\n".join(runs).splitlines() if "test-deployment-variants.sh" in line]
    assert variants == [
        "./scripts/test-deployment-variants.sh --timeout 240 --after ./scripts/k3d-rotate-each-class.sh 0,0,0"
    ], variants
    assert job["if"] == "github.event_name != 'push'", "nightly and on dispatch, never a push gate"
    # The Done-when asks for a median under 25 minutes; the ceiling has to be near it, or a
    # job that doubled in length is only discovered by a human noticing. 35, not 30: green runs
    # took up to 29 minutes and 2 of 27 were cut at 30 (T-6438).
    assert job["timeout-minutes"] <= 35, job["timeout-minutes"]


def test_the_variant_sweep_runs_every_variant_in_a_job_of_its_own():
    """A variant takes 10 to 12 minutes; eight in one job never finished inside its cap and the
    sweep was cancelled at the fourth (T-2826). One job per triplet, all eight, each capped."""
    job = yaml.safe_load(CI_FULL.read_text())["jobs"]["deployment-variants"]
    variants = job["strategy"]["matrix"]["variant"]
    assert sorted(variants) == [f"{a},{b},{c}" for a in "01" for b in "01" for c in "01"]
    assert job["strategy"]["fail-fast"] is False, "one red variant must not cancel the others"
    runs = [step["run"] for step in job["steps"] if "run" in step]
    assert "./scripts/test-deployment-variants.sh --timeout 240 ${{ matrix.variant }}" in runs
    assert job["timeout-minutes"] <= 35, job["timeout-minutes"]


def test_the_nightly_deployment_job_keeps_what_it_needs_to_explain_a_failure():
    """A red 25-minute job whose logs are gone by the time anyone looks is a red job nobody
    fixes. Both steps are `if: failure()`, so a green run uploads nothing."""
    steps = k3d_deploy_job()["steps"]
    collect = next(step for step in steps if "diagnostics" in step.get("run", ""))
    assert collect["if"] == "failure()"
    for command in ("kubectl get all -A", "kubectl describe pod", "kubectl logs", "--previous"):
        assert command in collect["run"], command

    upload = next(step for step in steps if step.get("uses", "").startswith("actions/upload-artifact"))
    assert upload["if"] == "failure()"
    assert upload["with"]["path"] == "diagnostics/"


def test_the_nightly_deployment_job_installs_every_tool_the_harness_demands():
    """The harness stops at its pre-flight with `missing required tools`, which costs a job
    and says nothing about the deployment. `linkerd` is the exception: it is demanded only by
    a variant that enables the mesh, and this job runs the one that does not."""
    needed = set(re.search(r"^need=\(([^)]*)\)", VARIANTS.read_text(), re.M).group(1).split())
    needed |= {"k3d", "curl"}  # added by the managed-cluster branch, which this job takes
    needed -= {"kubectl", "helm", "yq", "curl"}  # every GitHub runner image ships these
    job = yaml.safe_dump(k3d_deploy_job())
    if "./.github/actions/tools" in job:
        job += " helmfile just"  # the local action installs both at the tools.env version (T-4954, T-5237)
    for tool in sorted(needed):
        assert tool in job, f"the harness needs {tool} and the job never installs it"


ROTATE_EACH = PROJECT_ROOT / "scripts/k3d-rotate-each-class.sh"


def test_the_nightly_deployment_job_rotates_one_credential_of_each_class_and_smokes_again():
    """T-2843 (OPS-45): the import cache, CloudNativePG, helm re-making a deleted Secret and the
    readers coming back are only proven on a running cluster. The harness runs the rotations
    after its smoke and smoke-tests again; a rotation that exits non-zero fails the variant."""
    body = ROTATE_EACH.read_text()
    code = "\n".join(line for line in body.splitlines() if not line.lstrip().startswith("#"))
    assert "set -euo pipefail" in code, "a failed rotation must stop the script and fail the job"
    rotated = re.findall(r"^rotate --secret (\S+)", code, re.M)
    assert rotated == ["keycloak-client-activity-ingest", "db-portal", "gitea-token-gateway", "portal-cookie-key"], rotated
    assert "./scripts/rotate-secret.sh" in code
    # Never with verification off: curl trusts the harness's CA for these measurements.
    assert "CURL_CA_BUNDLE" in code and " -k" not in code and "--insecure" not in code

    harness = VARIANTS.read_text()
    run_variant = harness[harness.index("run_variant() {"):].split("\n}\n")[0]
    after = run_variant[run_variant.index('if [[ -n "$AFTER" ]]'):]
    # The step, then the same smoke again, inside the variant, so its failure fails the job.
    assert after.index('bash -c "$AFTER"') < after.index("smoke_platform || return 1"), after
    assert "return 1" in after.split("smoke_platform")[0], "a failed step after the smoke has to fail the variant"

    steps = k3d_deploy_job()["steps"]
    hosts = next(step for step in steps if "/etc/hosts" in step.get("run", ""))
    for host in ("joinedcontext.test", "idm.joinedcontext.test"):
        assert host in hosts["run"], host
    runs = [step.get("run", "") for step in steps]
    assert runs.index(hosts["run"]) < next(i for i, run in enumerate(runs) if "--after" in run)


def test_the_step_after_the_smoke_refuses_to_run_without_the_scratch_environment(tmp_path):
    """Run outside the harness it would apply to an environment nobody named; it stops first."""
    result = subprocess.run(
        ["bash", str(ROTATE_EACH)], cwd=str(PROJECT_ROOT), capture_output=True, text=True,
        env={"PATH": "/usr/bin:/bin", "HOME": str(tmp_path)},
    )
    assert result.returncode != 0
    assert "JC_ENV" in result.stderr


UPGRADE = PROJECT_ROOT / "scripts/ci-upgrade-e2e.sh"


def test_the_upgrade_job_runs_the_end_to_end_script_with_the_tags_it_upgrades_between():
    """T-3997: the previous tag must be in the checkout, so the clone is a full one."""
    job = yaml.safe_load(CI_FULL.read_text())["jobs"]["upgrade"]
    assert job["if"] == "github.event_name != 'push'"
    checkout = next(s for s in job["steps"] if s.get("uses", "").startswith("actions/checkout@"))
    assert checkout["with"]["fetch-depth"] == 0
    assert any(s.get("run") == "./scripts/ci-upgrade-e2e.sh" for s in job["steps"])
    assert any(s.get("if") == "failure()" and "upload-artifact" in s.get("uses", "") for s in job["steps"])


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
