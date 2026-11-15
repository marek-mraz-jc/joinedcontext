"""The ci set a range takes (T-6110, T-6152) and the per-release dev apply of a pin (T-6110)."""

import importlib.util
import os
import re
import shutil
import subprocess
from pathlib import Path

import pytest
import yaml
from test_dev_recipes import write_nodes

ROOT = Path(__file__).resolve().parent.parent


def _load(path, name):
    spec = importlib.util.spec_from_file_location(name, ROOT / path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


changes = _load("scripts/ci/changes.py", "ci_changes")
apply_pins = _load("scripts/apply-pins.py", "apply_pins")


@pytest.mark.parametrize("files, short", [
    (["components/portal/images.yaml"], True),
    (["components/portal/images.yaml", "components/context-gateway/images.yaml"], True),
    # a chart file beside a pin, a seed beside a pin, a pin under another name: the full set
    (["components/portal/images.yaml", "components/portal/charts/portal/templates/deployment.yaml"], False),
    (["components/portal/images.yaml", "components/context-gateway/seed/helsinki/helsinki-endpoint-catalog.yaml"], False),
    (["components/portal/values/images.yaml"], False),
    (["images/viz/images.yaml"], False),
    (["components/portal/images.yaml.orig"], False),
    ([], False),
])
def test_only_a_range_of_images_yaml_files_takes_the_short_set(files, short):
    assert changes.pins_only(files) is short


def git(repo, *args):
    return subprocess.run(["git", "-C", str(repo), *args], check=True, capture_output=True, text=True).stdout.strip()


@pytest.fixture
def repo(tmp_path):
    git(tmp_path, "init", "-q")
    git(tmp_path, "config", "user.email", "t@example.com")
    git(tmp_path, "config", "user.name", "t")
    (tmp_path / "components/portal").mkdir(parents=True)
    (tmp_path / "components/portal/images.yaml").write_text("a: 1\n")
    (tmp_path / "components/portal/seed.yaml").write_text("a: 1\n")
    git(tmp_path, "add", ".")
    git(tmp_path, "commit", "-qm", "base")
    return tmp_path


def outputs(repo, base, head="HEAD"):
    done = subprocess.run(["python3", str(ROOT / "scripts/ci/changes.py"), base, head], cwd=repo,
                          capture_output=True, text=True, check=True)
    return dict(line.split("=", 1) for line in done.stdout.splitlines())


def run(repo, base, head="HEAD"):
    return f"pins_only={outputs(repo, base, head)['pins_only']}"


def test_the_cli_reads_the_range_from_git(repo):
    base = git(repo, "rev-parse", "HEAD")
    (repo / "components/portal/images.yaml").write_text("a: 2\n")
    git(repo, "commit", "-qam", "pin")
    assert run(repo, base).splitlines()[0] == "pins_only=true"
    pin = git(repo, "rev-parse", "HEAD")
    (repo / "components/portal/seed.yaml").write_text("a: 2\n")
    (repo / "components/portal/images.yaml").write_text("a: 3\n")
    git(repo, "commit", "-qam", "pin and seed")
    assert run(repo, pin) == "pins_only=false"


@pytest.mark.parametrize("base", ["0" * 40, "", "not-a-sha", "deadbeefdeadbeef", "full"])
def test_an_unreadable_base_takes_the_full_set(repo, base):
    assert run(repo, base) == "pins_only=false"
    assert outputs(repo, base)["full"] == "true"


TESTS = {
    "tests/test_portal_chart.py": 'CHART = ROOT / "components/portal/charts/portal"',
    "tests/test_helsinki.py": 'SEED = ROOT / "components" / "context-gateway" / "seed" / "helsinki"',
    "tests/test_every_seed.py": 'for p in (ROOT / "components/context-gateway/seed").rglob("*.yaml"): ...',
    "tests/test_agent_runner.py": 'def test_x(rendered): assert rendered("dev")',
    "tests/test_apply_pins.py": 'load("scripts/apply-pins.py")',
    "tests/test_networkpolicies.py": "def test_y(rendered): ...",
    "tests/test_other.py": "assert True",
}
ENVS = ["local", "production", "dev", "smartcity", "minimal"]


@pytest.mark.parametrize("files, tests", [
    # a chart file: the tests naming its chart and the render-wide ones
    (["components/portal/charts/portal/templates/deployment.yaml"], ["tests/test_networkpolicies.py", "tests/test_portal_chart.py"]),
    # a seed file: the tests naming its directory, in either spelling
    (["components/context-gateway/seed/helsinki/endpoint.yaml"],
     ["tests/test_every_seed.py", "tests/test_helsinki.py", "tests/test_networkpolicies.py"]),
    # a component no test names by path: the test file named after it
    (["components/agent-runner/values/proxy.yaml.gotmpl"], ["tests/test_agent_runner.py", "tests/test_networkpolicies.py"]),
    # a script: the tests naming it; a test file: itself
    (["scripts/apply-pins.py", "tests/test_other.py"], ["tests/test_apply_pins.py", "tests/test_other.py"]),
    # nothing a test reads
    (["README.md", "ci/test-durations.json"], []),
])
def test_a_change_selects_the_test_files_that_read_it(files, tests):
    got = changes.scope(files, TESTS, ENVS)
    assert got["full"] is False and got["tests"] == tests
    assert got["envs"] == ["production"] and got["demo"] is False


@pytest.mark.parametrize("files", [
    ["scripts/nobody-reads-this.sh"],                     # an unknown path
    ["components/portal/images.yaml", "tests/conftest.py"],  # read by every test
    ["defaults/environment/global.yaml"],                 # read by every render
    ["ci/golden/ENVIRONMENTS"],
    [],
    None,
])
def test_an_unknown_or_shared_path_selects_the_full_set(files):
    got = changes.scope(files, TESTS, ENVS)
    assert got == {"full": True, "tests": [], "envs": ENVS, "pins": True, "demo": True}


def test_an_environment_file_renders_that_environment_beside_production():
    got = changes.scope(["ci/example-deployments/environments/smartcity/portal.yaml.gotmpl",
                         "ci/golden/minimal.txt", "tests/test_other.py"], TESTS, ENVS)
    assert got["envs"] == ["production", "smartcity", "minimal"] and got["tests"] == ["tests/test_other.py"]


def test_a_pin_verifies_signatures_and_a_demo_change_renders_the_demo():
    assert changes.scope(["components/portal/images.yaml"], {**TESTS, "tests/test_p.py": "components/portal/images.yaml"},
                         ENVS)["pins"] is True
    got = changes.scope(["demos/bbsk/helmfile.yaml.gotmpl", "tests/test_other.py"],
                        {**TESTS, "tests/test_demo.py": 'ROOT / "demos/bbsk"'}, ENVS)
    assert got["demo"] is True and got["pins"] is False and "tests/test_demo.py" in got["tests"]


def test_the_cli_prints_the_scoped_set(repo):
    base = git(repo, "rev-parse", "HEAD")
    (repo / "tests").mkdir()
    (repo / "tests/test_seed.py").write_text('SEED = "components/portal/seed.yaml"\n')
    (repo / "components/portal/seed.yaml").write_text("a: 2\n")
    git(repo, "add", ".")
    git(repo, "commit", "-qm", "seed")
    got = outputs(repo, base)
    assert (got["full"], got["tests"], got["envs"], got["pins"]) == ("false", "tests/test_seed.py", "production", "false")


def write_release(root, name, objects):
    (root / name / "chart/templates").mkdir(parents=True)
    (root / name / "chart/templates/all.yaml").write_text(yaml.safe_dump_all(objects))


def deployment(image):
    return {"kind": "Deployment", "metadata": {"name": "portal", "namespace": "dev",
            "annotations": {"checksum/secret": image}},
            "spec": {"template": {"spec": {"containers": [{"image": image}]}}}}


def test_only_a_release_whose_objects_moved_is_applied(tmp_path):
    old, new = tmp_path / "old", tmp_path / "new"
    write_release(old, "portal-portal", [deployment("p@sha256:1")])
    write_release(new, "portal-portal", [deployment("p@sha256:2")])
    # a random secret value and its checksum annotation move on every render: not a change
    write_release(old, "secrets-db-portal", [{"kind": "Secret", "metadata": {"name": "db"}, "data": {"p": "YQ=="}}])
    write_release(new, "secrets-db-portal", [{"kind": "Secret", "metadata": {"name": "db"}, "data": {"p": "Yg=="}}])
    write_release(old, "agent-runner-proxy", [deployment("x")])
    write_release(new, "agent-runner-proxy", [deployment("x"), {"kind": "ConfigMap", "metadata": {"name": "c"}}])
    write_release(new, "viz-server", [deployment("v")])
    assert apply_pins.moved(old, new) == ["agent-runner-proxy", "portal-portal", "viz-server"]


def test_a_release_gone_is_refused(tmp_path):
    old, new = tmp_path / "old", tmp_path / "new"
    write_release(old, "portal-portal", [deployment("a")])
    write_release(old, "portal-ui", [deployment("a")])
    write_release(new, "portal-portal", [deployment("a")])
    with pytest.raises(apply_pins.Refused, match="portal-ui"):
        apply_pins.moved(old, new)


def test_a_range_with_more_than_pins_is_refused(monkeypatch):
    monkeypatch.setattr(apply_pins.changes, "changed", lambda base, head: ["components/portal/images.yaml", "justfile"])
    with pytest.raises(apply_pins.Refused, match="justfile"):
        apply_pins.releases("dev", "abc1234")
    monkeypatch.setattr(apply_pins.changes, "changed", lambda base, head: None)
    with pytest.raises(apply_pins.Refused, match="cannot read"):
        apply_pins.releases("dev", "abc1234")
    monkeypatch.setattr(apply_pins.changes, "changed", lambda base, head: [])
    assert apply_pins.releases("dev", "abc1234") == []


def write_values(root, key, values):
    namespace, name, component, chart = key.split("/", 3)
    (root / namespace / name / component).mkdir(parents=True)
    (root / namespace / name / component / f"{chart}.yaml").write_text(yaml.safe_dump(values))


def test_a_pin_moves_the_releases_whose_values_moved(tmp_path):
    old, new = tmp_path / "old", tmp_path / "new"
    for root, digest in ((old, "sha256:1"), (new, "sha256:2")):
        write_values(root, "dev/portal-portal/portal/..%2F..%2Fcharts%2Fworkload", {"image": {"digest": digest}})
        write_values(root, "dev/keycloak-app/keycloak/cc2-codecentric%2Fkeycloakx", {"image": digest})
        write_values(root, "dev/networkpolicies-portal/networkpolicies/.%2Fcharts%2Fnetworkpolicies", {"a": 1})
    assert apply_pins.values_moved(old, new) == ["keycloak-app keycloak remote", "portal-portal portal local"]
    # a local chart is a directory of its component
    root = tmp_path / "repo"
    (root / "components/portal/charts/p").mkdir(parents=True)
    write_values(old, "dev/portal-ui/portal/charts%2Fp", {"d": 1})
    write_values(new, "dev/portal-ui/portal/charts%2Fp", {"d": 2})
    apply_pins.ROOT, kept = root, apply_pins.ROOT
    try:
        assert "portal-ui portal local" in apply_pins.values_moved(old, new)
    finally:
        apply_pins.ROOT = kept


def test_a_release_added_removed_or_unlabelled_is_refused(tmp_path):
    old, new = tmp_path / "old", tmp_path / "new"
    write_values(old, "dev/a/a/c", {"d": 1})
    write_values(new, "dev/a/a/c", {"d": 1})
    write_values(new, "dev/b/b/c", {"d": 1})
    with pytest.raises(apply_pins.Refused, match="dev/b"):
        apply_pins.values_moved(old, new)
    write_values(old, "dev/b/<no value>/c", {"d": 1})
    write_values(old, "dev/c/<no value>/c", {"d": 1})
    write_values(new, "dev/c/<no value>/c", {"d": 2})
    with pytest.raises(apply_pins.Refused, match="no component label on the release dev/c"):
        apply_pins.values_moved(old, new)
    with pytest.raises(apply_pins.Refused, match="no release"):
        apply_pins.values_moved(old, tmp_path / "none")


def test_a_pin_reruns_the_tests_of_its_images_and_of_every_images_yaml():
    tests = {
        "tests/test_portal.py": 'yaml.safe_load((ROOT / "components/portal/images.yaml").read_text())',
        "tests/test_gateway.py": '(ROOT / "components" / "context-gateway" / "images.yaml").read_text()',
        "tests/test_keycloak.py": 'IMAGES = ROOT / "components/keycloak/images.yaml"',
        "tests/test_every.py": 'for p in ROOT.glob("components/*/images.yaml"): ...',
        "tests/test_unnamed.py": '"""reads images.yaml through a helper"""',
        "tests/test_other.py": "assert True",
    }
    assert changes.pin_tests(["components/portal/images.yaml"], tests) == [
        "tests/test_every.py", "tests/test_portal.py", "tests/test_unnamed.py"]
    assert changes.pin_tests(["components/context-gateway/images.yaml"], tests) == [
        "tests/test_every.py", "tests/test_gateway.py", "tests/test_unnamed.py"]


HELMFILE_STUB = """#!/usr/bin/env bash
# template: one release directory per component, its object the component's images.yaml;
# write-values: each release's values file at the output template, its values the component's
# images.yaml, chart charts/w (a directory of a and agent-runner, a repository chart for b);
# diff and sync: the arguments they were given.
set -eu
case " $* " in
  *" write-values "*)
    tpl=""; prev=""
    for a in "$@"; do [ "$prev" = --output-file-template ] && tpl="$a"; prev="$a"; done
    for r in a-x b-x agent-runner-proxy; do
      c=${r%-*}
      mkdir -p "${tpl%%"{{"*}dev/$r/$c"
      cp "components/$c/images.yaml" "${tpl%%"{{"*}dev/$r/$c/charts%2Fw.yaml"
    done ;;
  *" template "*)
    out=""; prev=""
    for a in "$@"; do [ "$prev" = --output-dir ] && out="$a"; prev="$a"; done
    for r in a-x b-x agent-runner-proxy; do
      c=${r%-*}
      mkdir -p "$out/$r"
      { echo "kind: ConfigMap"; echo "metadata: {name: $c}"; echo "data:"; sed 's/^/  /' "components/$c/images.yaml"; } > "$out/$r/t.yaml"
    done ;;
  *) echo "HELMFILE ${JC_PREPARE_SKIP:+JC_PREPARE_SKIP=$JC_PREPARE_SKIP }$*" ;;
esac
"""

KUBECTL_STUB = """#!/usr/bin/env bash
case "$*" in
  "config view"*) echo "https://203.0.113.10:6443" ;;
  *"get configmap jc-release"*) cat "$STUB_DIR/applied" ;;
  *"get configmap jc-apply-red"*) cat "$STUB_DIR/red" 2>/dev/null || true ;;
  *"create configmap jc-apply-red"*) echo "HOLD $*" ;;
  "apply -f -") sed 's/^/KUBECTL-APPLY /' ;;
  *) echo "KUBECTL $*" ;;
esac
"""


@pytest.fixture
def tree(tmp_path):
    """A copy of the repository's apply path with helmfile, kubectl and the cluster scripts stubbed."""
    if shutil.which("just") is None:
        pytest.skip("just not installed")
    work, stubs = tmp_path / "work", tmp_path / "bin"
    for rel in ("justfile", "scripts/apply-pins.py", "scripts/ci/changes.py", "scripts/release-diff.py",
                "defaults/deployment", "ci/example-deployments"):
        src, dst = ROOT / rel, work / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        (shutil.copytree if src.is_dir() else shutil.copy2)(src, dst)
    # These cases play the pin logic, not the old dev environment's retirement (test_dev_recipes).
    (work / "ci/example-deployments/environments/dev/RETIRED").unlink(missing_ok=True)
    for name in ("verify-pinned-images.py", "wait-rollouts.sh", "release.py", "smoke-component.sh", "render.sh"):
        # a component named in $STUB_DIR/red-smoke fails its smoke
        (work / "scripts" / name).write_text(f'#!/bin/sh\necho "RAN {name} $*"\n'
                                             f'grep -qx "{name} $*" "$STUB_DIR/fail" 2>/dev/null && exit 1; exit 0\n')
        (work / "scripts" / name).chmod(0o755)
    stubs.mkdir()
    for name, text in (("helmfile", HELMFILE_STUB), ("kubectl", KUBECTL_STUB)):
        (stubs / name).write_text(text)
        (stubs / name).chmod(0o755)
    for c in ("a", "b", "agent-runner"):
        (work / "components" / c).mkdir(parents=True)
        if c != "b":
            (work / "components" / c / "charts/w").mkdir(parents=True)
            (work / "components" / c / "charts/w/Chart.yaml").write_text("name: w\n")
        (work / "components" / c / "images.yaml").write_text("digest: '1'\n")
    git(work, "init", "-q")
    git(work, "config", "user.email", "t@example.com")
    git(work, "config", "user.name", "t")
    git(work, "add", ".")
    git(work, "commit", "-qm", "applied")
    (tmp_path / "applied").write_text(git(work, "rev-parse", "HEAD"))
    env = {**os.environ, "PATH": f"{stubs}:{os.environ['PATH']}", "STUB_DIR": str(tmp_path), "JUST_NO_DOTENV": "true",
           "JC_ENVIRONMENTS": str(write_nodes(tmp_path / "environments"))}
    return work, env


def just(work, env, *args, recipe="dev-apply-pins"):
    return subprocess.run(["just", recipe, *args], check=False, cwd=work, env=env, capture_output=True, text=True)


def pin_a(work):
    (work / "components/a/images.yaml").write_text("digest: '2'\n")
    git(work, "commit", "-qam", "pin a")


def test_dev_apply_pins_syncs_only_the_release_the_pin_moved(tree):
    work, env = tree
    (work / "components/a/images.yaml").write_text("digest: '2'\n")
    git(work, "commit", "-qam", "pin a")
    dry = just(work, env, "", "true")
    assert dry.returncode == 0, dry.stderr
    assert "HELMFILE -f deployment/helmfile.yaml -e dev diff --concurrency 4 --selector name=a-x" in dry.stdout
    assert "name=b-x" not in dry.stdout and "sync" not in dry.stdout and "RAN" not in dry.stdout
    done = just(work, env)
    assert done.returncode == 0, done.stderr
    # T-6155: a local chart syncs without chart repositories or dependency builds, and a pin moves no
    # namespace, so the prepare hook stands down
    assert ("HELMFILE JC_PREPARE_SKIP=pins -f deployment/helmfile.yaml -e dev sync --concurrency 4 --skip-deps "
            "--selector name=a-x\n") in done.stdout
    lines = [re.sub(r" /\S+", " RENDER", line) for line in done.stdout.splitlines() if line.startswith("RAN")]
    # T-6155: the render, its signature check and the wait cover the synced release alone, not
    # every workload of dev (CNPG included)
    assert lines == ["RAN render.sh dev RENDER --deployed --selector name=a-x",
                     "RAN verify-pinned-images.py dev --render RENDER --scoped",
                     "RAN release.py start dev",
                     "RAN wait-rollouts.sh dev 600 a-x", "RAN release.py record dev dev",
                     "RAN smoke-component.sh dev.joinedcontext.com dev a"]
    assert re.search(r"planned, verified, synced and rolled out in \d+ s", done.stdout)
    assert "running on dev and smoked" in done.stdout and "HOLD" not in done.stdout


def test_a_range_with_more_than_pins_takes_the_full_apply_and_smoke(tree):
    work, env = tree
    (work / "components/a/images.yaml").write_text("digest: '2'\n")
    (work / "components/a/values.yaml").write_text("x: 1\n")
    git(work, "add", ".")
    git(work, "commit", "-qm", "pin a and a value")
    dry = just(work, env, "", "true")
    assert dry.returncode == 0, dry.stderr
    assert "the full just dev-apply and just dev-smoke instead" in dry.stdout and "HELMFILE" not in dry.stdout
    # dev-smoke reads the live hosts: the copy's recipe is a stub (the later definition wins)
    text = (work / "justfile").read_text()
    (work / "justfile").write_text("set allow-duplicate-recipes\n" + text + "\ndev-smoke:\n\t@echo RAN dev-smoke\n")
    done = just(work, env)
    assert done.returncode == 0, done.stderr
    # dev-apply plans by render (only a-x moved) and syncs it, then dev-smoke runs
    out = done.stdout
    assert out.index("the full just dev-apply") < out.index("-e dev sync") < out.index("RAN dev-smoke")
    assert "RAN smoke-component" not in out


def test_a_repository_chart_syncs_with_its_repositories(tree):
    work, env = tree
    for c in ("a", "b"):
        (work / f"components/{c}/images.yaml").write_text("digest: '2'\n")
    git(work, "commit", "-qam", "pin a and b")
    done = just(work, env)
    assert done.returncode == 0, done.stderr
    assert "sync --concurrency 4 --selector name=a-x --selector name=b-x\n" in done.stdout
    assert "RAN smoke-component.sh dev.joinedcontext.com dev b" in done.stdout


def test_the_prepare_hook_stands_down_for_a_pin_only_sync():
    hook = (ROOT / "components/prepare/helmfile.yaml.gotmpl").read_text()
    assert 'if [ "${JC_PREPARE_SKIP:-}" = pins ]; then' in hook
    text = (ROOT / "justfile").read_text()
    pins = text[text.index("\ndev-apply-pins "):text.index("\ndev-smoke-component ")]
    assert pins.count("JC_PREPARE_SKIP=pins helmfile") == 1
    full = text[text.index("\ndev-apply full"):text.index("\n_smoke ")]
    assert "JC_PREPARE_SKIP" not in full


def pin_a_and_the_proxy(work):
    for c in ("a", "agent-runner"):
        (work / f"components/{c}/images.yaml").write_text("digest: '2'\n")
    git(work, "commit", "-qam", "pin a and agent-runner")


def test_the_spend_guard_keeps_agent_runner_proxy_out_of_a_pin_apply(tree):
    work, env = tree
    pin_a_and_the_proxy(work)
    done = just(work, env)
    assert done.returncode == 0, done.stderr
    assert "skipping agent-runner-proxy: the spend guard" in done.stdout
    assert "sync --concurrency 4 --skip-deps --selector name=a-x\n" in done.stdout
    assert "name=agent-runner-proxy" not in done.stdout
    assert "RAN wait-rollouts.sh dev 600 a-x\n" in done.stdout
    assert "smoke-component.sh dev.joinedcontext.com dev agent-runner" not in done.stdout
    # spend_guard=off says the guard was lifted: the proxy syncs with the rest
    lifted = just(work, env, "", "false", "off")
    assert lifted.returncode == 0, lifted.stderr
    assert "skipping" not in lifted.stdout
    assert "--selector name=a-x --selector name=agent-runner-proxy" in lifted.stdout
    bogus = just(work, env, "", "false", "maybe")
    assert bogus.returncode == 2 and "spend_guard is on or off" in bogus.stderr and "HELMFILE" not in bogus.stdout


def test_a_pin_of_the_proxy_alone_syncs_nothing_under_the_guard(tree):
    work, env = tree
    (work / "components/agent-runner/images.yaml").write_text("digest: '2'\n")
    git(work, "commit", "-qam", "pin agent-runner")
    done = just(work, env)
    assert done.returncode == 0, done.stderr
    assert "skipping agent-runner-proxy" in done.stdout and "HELMFILE" not in done.stdout


def test_the_full_path_keeps_the_guard(tree):
    work, env = tree
    (work / "components/agent-runner/images.yaml").write_text("digest: '2'\n")
    (work / "components/a/values.yaml").write_text("x: 1\n")
    git(work, "add", ".")
    git(work, "commit", "-qm", "pin agent-runner and a value")
    text = (work / "justfile").read_text()
    (work / "justfile").write_text("set allow-duplicate-recipes\n" + text + "\ndev-smoke:\n\t@echo RAN dev-smoke\n")
    done = just(work, env)
    assert done.returncode == 0, done.stderr
    assert "skipping agent-runner-proxy" in done.stdout and "name=agent-runner-proxy" not in done.stdout
    full = just(work, env, "true", recipe="dev-apply")
    assert full.returncode == 0, full.stderr
    assert "sync --concurrency 4 --selector name!=agent-runner-proxy\n" in full.stdout
    lifted = just(work, env, "true", "off", recipe="dev-apply")
    assert lifted.returncode == 0, lifted.stderr
    assert "HELMFILE -f deployment/helmfile.yaml -e dev sync --concurrency 4\n" in lifted.stdout


def test_a_red_smoke_holds_the_release_until_the_full_path_clears_it(tree):
    work, env = tree
    stub_dir = Path(env["STUB_DIR"])
    (stub_dir / "fail").write_text("smoke-component.sh dev.joinedcontext.com dev a\n")
    pin_a(work)
    red = just(work, env)
    assert red.returncode != 0
    assert "KUBECTL-APPLY HOLD -n dev create configmap jc-apply-red" in red.stdout
    assert "--from-literal=releases=a-x" in red.stdout
    assert "red" in red.stderr and "held: a-x" in red.stderr
    # the next per-change apply of a-x is refused before any sync
    (stub_dir / "red").write_text("a-x")
    (work / "components/a/images.yaml").write_text("digest: '3'\n")
    git(work, "commit", "-qam", "pin a again")
    held = just(work, env)
    assert held.returncode != 0
    assert "a-x went red in a per-change apply" in held.stderr
    assert "sync" not in held.stdout and "RAN" not in held.stdout


def test_a_held_release_does_not_hold_another(tree):
    work, env = tree
    (Path(env["STUB_DIR"]) / "red").write_text("b-x")
    pin_a(work)
    done = just(work, env)
    assert done.returncode == 0, done.stderr
    assert "--selector name=a-x" in done.stdout


def test_a_failed_rollout_holds_the_release_and_skips_the_record(tree):
    work, env = tree
    (Path(env["STUB_DIR"]) / "fail").write_text("wait-rollouts.sh dev 600 a-x\n")
    pin_a(work)
    done = just(work, env)
    assert done.returncode != 0
    assert "RAN release.py record" not in done.stdout and "RAN smoke-component" not in done.stdout
    assert "--from-literal=releases=a-x" in done.stdout


def test_a_green_dev_smoke_releases_the_hold():
    text = (ROOT / "justfile").read_text()
    body = text[text.index("\ndev-smoke:"):text.index("\n_smoke-sweeps ")]
    assert '[ "$rc" -ne 0 ] || kubectl -n dev delete configmap jc-apply-red --ignore-not-found' in body


WAIT_KUBECTL = """#!/usr/bin/env bash
case "$*" in
  "get ns -o name") echo namespace/dev ;;
  *"-o json"*) cat <<'JSON'
{"items": [
 {"kind": "Deployment", "metadata": {"name": "portal", "annotations": {"meta.helm.sh/release-name": "portal-portal"}}},
 {"kind": "StatefulSet", "metadata": {"name": "postgres", "annotations": {"meta.helm.sh/release-name": "cnpg"}}},
 {"kind": "Deployment", "metadata": {"name": "bare"}}]}
JSON
  ;;
  "rollout status"*) echo "ROLLOUT $3"; [ "$3" != deployment/portal ] || [ ! -e "$STUB_DIR/stuck" ] ;;
  *) echo "UNEXPECTED $*"; exit 9 ;;
esac
"""


@pytest.mark.parametrize("stuck", [False, True])
def test_a_scoped_wait_waits_for_the_named_releases_alone(tmp_path, stuck):
    (tmp_path / "kubectl").write_text(WAIT_KUBECTL)
    (tmp_path / "kubectl").chmod(0o755)
    if stuck:
        (tmp_path / "stuck").write_text("")
    env = {**os.environ, "PATH": f"{tmp_path}:{os.environ['PATH']}", "STUB_DIR": str(tmp_path)}
    done = subprocess.run([str(ROOT / "scripts/wait-rollouts.sh"), "dev", "60", "portal-portal"], env=env,
                          capture_output=True, text=True, check=False)
    rolled = [line for line in done.stdout.splitlines() if line.startswith(("ROLLOUT", "UNEXPECTED"))]
    assert rolled == ["ROLLOUT deployment/portal"]
    assert done.returncode == (1 if stuck else 0), done.stdout + done.stderr


def test_the_scoped_set_gets_a_shard_per_fifteen_measured_minutes():
    scoped = {"full": False, "tests": ["tests/test_a.py", "tests/test_b.py"]}
    assert changes.shards(scoped, {"test_a.py": 100.0}) == 1
    assert changes.shards(scoped, {"test_a.py": 900.0, "test_b.py": 1.0}) == 2
    assert changes.shards(scoped, {"test_a.py": 99999.0}) == changes.SHARDS
    assert changes.shards({"full": False, "tests": []}, {}) == 1
    assert changes.shards({"full": True, "tests": []}, {}) == changes.SHARDS
