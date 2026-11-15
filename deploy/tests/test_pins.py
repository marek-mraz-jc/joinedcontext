"""Exact pins in the files a reader sees, no lock file, tool versions in one file (T-4954).

docs/analysis/iac-practices.md R1-R4. A gate that never fails reads exactly like a clean
repository, so each check is fed a planted floating chart version, a tag-only image and a
helmfile.lock, and must refuse each one.
"""

import importlib.util
import json
import re
import subprocess
import sys
from pathlib import Path

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[1]
CHART_PINS = ROOT / "scripts/ci/check-chart-pins.py"
IMAGE_DIGESTS = ROOT / "scripts/ci/check-image-digests.py"
TOOLS = ROOT / "tools.env"
WORKFLOWS = ROOT / ".github/workflows"


def run(script: Path, *args: str, stdin: str = "") -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(script), *args],
        input=stdin,
        capture_output=True,
        text=True,
        check=False,
    )


def release(chart: str, version: str) -> dict:
    return {"name": "r", "namespace": "ns", "chart": chart, "version": version}


@pytest.mark.parametrize(
    "chart",
    ["cc2-codecentric/keycloakx", "oci://ghcr.io/cloudnative-pg/charts/cluster"],
)
@pytest.mark.parametrize(
    "version", ["", "~7.2.0", "^7", ">=7.0.0", "7.2.x", "7.2", "*", "latest"]
)
def test_a_floating_remote_chart_version_fails(chart, version):
    result = run(CHART_PINS, "-", stdin=json.dumps([release(chart, version)]))
    assert result.returncode == 1, result.stdout
    assert "pin one exact chart version" in result.stdout


@pytest.mark.parametrize("version", ["7.2.0", "v1.19.0", "0.7.0-rc.1", "1.2.3+build.4"])
def test_an_exact_remote_chart_version_passes(version):
    result = run(
        CHART_PINS, "-", stdin=json.dumps([release("cc2-vector/vector", version)])
    )
    assert result.returncode == 0, result.stdout


@pytest.mark.parametrize(
    "chart", ["charts/theme", "./charts/networkpolicies", "../../charts/workload"]
)
def test_a_local_chart_needs_no_version(chart):
    assert run(CHART_PINS, "-", stdin=json.dumps([release(chart, "")])).returncode == 0


def test_a_helmfile_lock_fails(tmp_path):
    assert run(CHART_PINS, "--no-lock", str(tmp_path)).returncode == 0
    (tmp_path / "components/keycloak").mkdir(parents=True)
    (tmp_path / "components/keycloak/helmfile.lock").write_text("version: v1.8.1\n")
    result = run(CHART_PINS, "--no-lock", str(tmp_path))
    assert result.returncode == 1
    assert "helmfile.lock" in result.stdout


def pod(image: str) -> str:
    return yaml.safe_dump(
        {
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {"name": "p"},
            "spec": {"containers": [{"name": "c", "image": image}]},
        }
    )


def test_a_tag_only_image_fails(tmp_path):
    rendered = tmp_path / "rendered.yaml"
    rendered.write_text(pod("quay.io/keycloak/keycloak:26.8.0"))
    result = run(IMAGE_DIGESTS, str(rendered))
    assert result.returncode != 0
    assert "quay.io/keycloak/keycloak:26.8.0" in result.stdout + result.stderr
    rendered.write_text(pod("quay.io/keycloak/keycloak:26.8.0@sha256:" + "0" * 64))
    assert run(IMAGE_DIGESTS, str(rendered)).returncode == 0


def tools() -> dict[str, str]:
    lines = [
        line
        for line in TOOLS.read_text().splitlines()
        if line and not line.startswith("#")
    ]
    assert all(re.fullmatch(r"[A-Z0-9_]+=\S+", line) for line in lines), lines
    return dict(line.split("=", 1) for line in lines)


def test_every_tool_version_is_exact_and_every_download_has_a_checksum():
    pinned = tools()
    for key, value in pinned.items():
        if key.endswith("_VERSION"):
            # Linkerd ships only edge releases, named edge-YY.M.N: exact all the same.
            assert re.fullmatch(r"(v|edge-)?\d+\.\d+\.\d+", value), f"{key}={value}"
    downloaded = {
        "KUBECONFORM",
        "CONFTEST",
        "GITLEAKS",
        "KYVERNO",
        "SOPS",
        "AGE",
        "TRIVY",
    }
    for tool in downloaded:
        assert re.fullmatch(r"[0-9a-f]{64}", pinned.get(f"{tool}_CHECKSUM", "")), tool


def test_renovate_sees_every_tool_it_should_bump():
    config = json.loads((ROOT / "renovate.json").read_text())
    manager = next(
        m
        for m in config["customManagers"]
        if "/^tools\\.env$/" in m["managerFilePatterns"]
    )
    found = {
        m.group("depName")
        for pattern in manager["matchStrings"]
        for m in re.finditer(pattern.replace("(?<", "(?P<"), TOOLS.read_text())
    }
    commented = TOOLS.read_text().count("# renovate:")
    assert len(found) == commented >= 11, found
    # The cluster's version moves with the cluster, not with upstream.
    assert "KUBERNETES_VERSION" in tools()
    assert config["minimumReleaseAge"] == "7 days"
    assert config["baseBranchPatterns"] == ["main"]
    assert "gitlabci" not in config["enabledManagers"]


def digests(node) -> int:
    """Image entries (`repository` + `digest`) in a parsed images.yaml."""
    if isinstance(node, dict):
        return int("repository" in node and "digest" in node) + sum(
            digests(v) for v in node.values()
        )
    return 0


def test_renovate_matches_every_image_with_its_digest():
    config = json.loads((ROOT / "renovate.json").read_text())
    manager = next(
        m for m in config["customManagers"] if "images" in m["managerFilePatterns"][0]
    )
    pattern = re.compile(manager["matchStrings"][0].replace("(?<", "(?P<"))
    files = sorted(ROOT.glob("components/*/images.yaml"))
    matched = sum(len(pattern.findall(f.read_text())) for f in files)
    declared = sum(
        digests(doc) for f in files for doc in yaml.safe_load_all(f.read_text())
    )
    assert matched == declared > 40


def test_workflows_take_tool_versions_from_tools_env():
    for workflow in WORKFLOWS.glob("*.yml"):
        text = workflow.read_text()
        assert "releases/latest" not in text, workflow.name
        if workflow.name.startswith("reusable-"):
            continue  # other repositories call these with their own inputs
        assert "helmfile/helmfile-action@" not in text, (
            f"{workflow.name}: use ./.github/actions/tools"
        )
        # A version written into the workflow is a second source beside tools.env. LINKERD2_VERSION
        # names an edge build the k3d smoke installs; Renovate cannot order edge tags.
        written = set(re.findall(r"\b([A-Z0-9]+_VERSION): ", text)) - {
            "LINKERD2_VERSION"
        }
        assert not written, f"{workflow.name}: {written} belongs in tools.env"


def test_reusable_workflow_defaults_match_tools_env():
    """F-DEP1-034: a reusable workflow's default tool version and checksum are the ones ci.yml
    installs from tools.env, so a caller that keeps the defaults runs the same binary."""
    tools = dict(
        line.split("=", 1)
        for line in TOOLS.read_text().splitlines()
        if "=" in line and not line.startswith("#")
    )
    checked = 0
    for workflow in WORKFLOWS.glob("reusable-*.yml"):
        on = yaml.safe_load(workflow.read_text()).get(True) or {}
        inputs = (on.get("workflow_call") or {}).get("inputs") or {}
        for name, spec in inputs.items():
            match = re.fullmatch(r"(?:(.+)-)?(version|checksum)", name)
            if not match or match.group(1) == "kubernetes":
                continue
            tool = match.group(1) or workflow.stem.removeprefix("reusable-")
            key = f"{tool.upper()}_{match.group(2).upper()}"
            assert str(spec.get("default")) == tools.get(key), f"{workflow.name}: {name} != {key}"
            checked += 1
    assert checked >= 6


def test_just_comes_from_tools_env():
    """T-5237: setup-just resolved its version through the GitHub API and failed whole shards when
    the answer came back empty; .github/actions/tools installs the pinned, checksummed release."""
    action = (ROOT / ".github/actions/tools/action.yml").read_text()
    assert "JUST_CHECKSUM" in action and "sha256sum --check" in action
    for workflow in WORKFLOWS.glob("*.yml"):
        text = workflow.read_text()
        assert "extractions/setup-just@" not in text, (
            f"{workflow.name}: use ./.github/actions/tools"
        )
        if re.search(r"^\s+(- )?(run: )?just ", text, re.MULTILINE):
            assert "uses: ./.github/actions/tools" in text, (
                f"{workflow.name} runs just without it"
            )


def remote_charts(node) -> int:
    """Chart entries pulled from an HTTP(S) repository (`repository` + `chart` + `version`)."""
    if isinstance(node, dict):
        own = int(
            str(node.get("repository", "")).startswith("http")
            and "chart" in node
            and "version" in node
        )
        return own + sum(remote_charts(v) for v in node.values())
    return 0


def test_renovate_matches_every_remote_chart_in_charts_yaml():
    """T-5019: each repository chart pin sits where Renovate's regex finds it."""
    config = json.loads((ROOT / "renovate.json").read_text())
    manager = next(
        m
        for m in config["customManagers"]
        if "/^components/[^/]+/charts\\.yaml$/" in m["managerFilePatterns"]
        and "helm" == m["datasourceTemplate"]
    )
    pattern = re.compile(manager["matchStrings"][0].replace("(?<", "(?P<"))
    files = sorted(ROOT.glob("components/*/charts.yaml"))
    found = [m.groupdict() for f in files for m in pattern.finditer(f.read_text())]
    declared = sum(
        remote_charts(doc) for f in files for doc in yaml.safe_load_all(f.read_text())
    )
    assert len(found) == declared
    broker = pattern.findall((ROOT / "components/context-broker/charts.yaml").read_text())
    assert [(depName, version) for _, depName, version in broker] == [("cluster", "0.7.0")]


def test_no_helmfile_pins_a_repository_chart_inline():
    """T-5019: a remote chart's version comes from components/*/charts.yaml, never a literal in a helmfile."""
    inline = re.compile(r"chart:\s*['\"]?cc2-[\w.-]+/[\w.-]+['\"]?\s*\n\s*version:\s*['\"]?\d")
    offenders = [
        str(f.relative_to(ROOT))
        for f in sorted(ROOT.glob("components/*/helmfile.yaml.gotmpl"))
        if inline.search(f.read_text())
    ]
    assert offenders == []


# pv- tags are never pinned (T-5547). A branch preview image is tagged pv-<slug>-<sha12> and signed
# by the source repository's preview.yml; pin-images reads only image.yml runs, asks the registry
# for the commit's own tag and verifies image.yml's identity on main, so no pv- image can reach a
# components/*/images.yaml. Each test below is one of those three doors.
PIN_IMAGES = ROOT / "scripts/pin-images.py"


@pytest.fixture
def pin_images():
    spec = importlib.util.spec_from_file_location("pin_images", PIN_IMAGES)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def recorder(calls, stdout="[]", returncode=1):
    return lambda *args, check=True: calls.append(args) or type("R", (), {"stdout": stdout, "returncode": returncode})()


def test_pv_tags_are_never_pinned_as_a_commit(pin_images, monkeypatch):
    monkeypatch.setenv("PIN_COMMIT", "pv-map-legend-0123456789ab")
    monkeypatch.setattr(pin_images, "green", lambda *a: pytest.fail("a pv- tag must be refused before any lookup"))
    with pytest.raises(SystemExit, match="full 40-hex commit"):
        pin_images.main("portal")


def test_pv_tags_are_never_pinned_from_a_preview_run(pin_images, monkeypatch):
    calls = []
    monkeypatch.setattr(pin_images, "run", recorder(calls, stdout='[{"headSha": "%s"}]' % ("c" * 40)))
    assert pin_images.built("joinedcontext-portal") == "c" * 40
    [args] = calls
    assert args[args.index("--workflow") + 1] == "image.yml" and args[args.index("--branch") + 1] == "main"


def test_pv_tags_are_never_pinned_by_their_signature(pin_images, monkeypatch):
    calls = []
    monkeypatch.setattr(pin_images, "run", recorder(calls))
    assert not pin_images.signed("joinedcontext-portal", "joinedcontext-portal", "sha256:" + "a" * 64)
    [args] = calls
    identity = args[args.index("--certificate-identity") + 1]
    assert identity == "https://github.com/marek-mraz-jc/joinedcontext-portal/.github/workflows/image.yml@refs/heads/main"


def test_pv_tags_are_never_pinned_by_tag(pin_images, monkeypatch):
    calls = []
    monkeypatch.setattr(pin_images, "run", recorder(calls, stdout=""))
    pin_images.published("joinedcontext-portal", "c" * 40)
    [args] = calls
    assert args[-1] == f"ghcr.io/marek-mraz-jc/joinedcontext-portal:{'c' * 40}"
    tags = [
        f"{f.relative_to(ROOT)}: {m.group(1)}"
        for f in sorted(ROOT.glob("components/*/images.yaml"))
        for m in re.finditer(r"^\s*tag:\s*['\"]?(pv-[^'\"\s]*)", f.read_text(), re.MULTILINE)
    ]
    assert tags == [], "a branch preview tag is never a deployed image"
