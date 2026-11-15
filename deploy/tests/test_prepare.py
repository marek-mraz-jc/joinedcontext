"""The prepare component as dev builds it: the hook that readies the namespaces (T-1666, OPS-19).

Prepare renders no manifest; its whole contract is the `prepare` hook of its helmfile, run before
every other release. The shared suites never see it. These pin what that script does on dev: only
on sync and apply, every namespace a component deploys into is ensured, Linkerd injection and the
authenticated default inbound policy are set on each, and a refusal is logged with its words."""

import re
import shutil
import subprocess
from pathlib import Path

import pytest
from conftest import TREE_IGNORE, load_yaml_all, set_global

PROJECT_ROOT = Path(__file__).resolve().parents[1]

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")


def build(root=PROJECT_ROOT):
    """`helmfile build` of dev in `root`, the repository or an edited copy of it."""
    if not (PROJECT_ROOT / "deployment/environments/dev").is_dir():
        pytest.skip("run `just _dev-assemble` first")
    return subprocess.run(
        ["helmfile", "-f", "deployment/helmfile.yaml", "-e", "dev", "build"],
        cwd=root, capture_output=True, text=True,
    )


def edited(tmp_path, **values):
    """A copy of the tree with `global.*` defaults edited (`--state-values-set` reaches no component)."""
    if not (PROJECT_ROOT / "deployment/environments/dev").is_dir():
        pytest.skip("run `just _dev-assemble` first")
    tree = tmp_path / "repo"
    shutil.copytree(PROJECT_ROOT, tree, ignore=TREE_IGNORE)
    for dotted, value in values.items():
        set_global(tree, dotted.replace("__", "."), value)
    return tree


def prepare_hook(root=PROJECT_ROOT):
    built = build(root)
    assert built.returncode == 0, built.stderr
    states = [s for s in load_yaml_all(built.stdout) if isinstance(s, dict)]
    (prepare,) = [s for s in states if (s.get("commonLabels") or {}).get("component") == "prepare"]
    (only,) = prepare["hooks"]
    assert only["events"] == ["prepare"] and only["command"] == "sh"
    return only["args"][-1]


@pytest.fixture(scope="module")
def hook():
    """The prepare hook's shell script, as `helmfile build` resolves it for dev."""
    return prepare_hook()


@pytest.fixture(scope="module")
def dev(rendered):
    return rendered("dev")


# The injected proxy's resources as defaults/environment/global.yaml sets them (T-5554); Linkerd
# gives linkerd-network-validator the same proxy.resources, so both meet require-requests-limits.
PROXY = (
    " config.linkerd.io/proxy-cpu-request=10m"
    " config.linkerd.io/proxy-memory-limit=250Mi"
    " config.linkerd.io/proxy-memory-request=10Mi"
)


def first_command(script):
    """Where the first kubectl command starts (the comments mention kubectl too)."""
    return script.index("\nkubectl ")


def ensured(script):
    return re.findall(r'kubectl get namespace "([^"]+)" >/dev/null 2>&1 \|\| kubectl create namespace "\1"', script)


@requires_helmfile
def test_the_hook_changes_the_cluster_only_on_sync_and_apply(hook):
    guard = hook.index('if [ "$cmd" != "sync" ] && [ "$cmd" != "apply" ]; then')
    assert hook.index("exit 0", guard) < first_command(hook), "a diff or a template must not touch the cluster"


@requires_helmfile
def test_a_refusal_is_logged_with_its_words(hook):
    assert "set -eu" in hook
    assert hook.index("exec 2>&1") < first_command(hook)


@requires_helmfile
def test_every_namespace_a_component_deploys_into_is_ensured(hook, dev):
    wanted = {d["metadata"]["namespace"] for d in dev if d.get("metadata", {}).get("namespace")}
    assert wanted, "the dev render names no namespace"
    # A namespace its own release renders as a Namespace object exists before anything in it
    # (helm installs Namespaces first), and it carries its own labels: the app-tests sandbox of
    # T-2675 is restricted and unmeshed, which the hook's Linkerd annotation would undo.
    owned = {d["metadata"]["name"] for d in dev if d.get("kind") == "Namespace"}
    assert not owned & set(ensured(hook)), f"ensured and meshed by the hook too: {sorted(owned & set(ensured(hook)))}"
    # kube-system is the distribution's: node-storage puts k3s's own ConfigMap there (T-3431), and
    # the hook must never mesh it.
    assert "kube-system" not in ensured(hook)
    missing = wanted - owned - set(ensured(hook)) - {"kube-system"}
    assert not missing, f"not ensured: {sorted(missing)}"


@requires_helmfile
def test_each_namespace_is_ensured_once_and_quoted(hook):
    names = ensured(hook)
    assert names and len(names) == len(set(names)), names
    assert not re.search(r"kubectl \w+ namespace [^\"\s]", hook), "an unquoted namespace name"


@requires_helmfile
def test_every_ensured_namespace_is_meshed_and_admits_only_authenticated_traffic_in_one_write(hook):
    # Linkerd's default inbound policy decides what a pod without its own Server admits; on dev
    # that is traffic from a meshed, authenticated workload of this cluster and nothing else.
    # Both annotations go in one command: Kyverno's require-meshed-namespace-inbound-policy
    # refuses a namespace marked for injection without the policy, so marking it first failed on
    # every namespace created after the policy was installed (ci-full 36096443083, T-2826).
    for name in ensured(hook):
        assert (
            f'kubectl annotate namespace "{name}" linkerd.io/inject=enabled '
            f'config.linkerd.io/default-inbound-policy="cluster-authenticated"{PROXY} --overwrite'
        ) in hook, name
    assert not re.search(r"kubectl annotate namespace \"[^\"]+\" linkerd.io/inject=enabled --overwrite", hook), (
        "injection written on its own, without the inbound policy"
    )
    assert "default-inbound-policy-" not in hook, "dev must not clear the policy"


def annotations(script):
    return re.findall(r"kubectl annotate namespace .*", script)


@requires_helmfile
def test_the_proxy_resources_come_from_the_global_values(hook, tmp_path):
    lines = annotations(prepare_hook(edited(
        tmp_path,
        serviceMesh__proxyResources__memoryLimit="96Mi",
        serviceMesh__proxyResources__cpuRequest="",
    )))
    assert len(lines) == len(ensured(hook)), lines
    for line in lines:
        assert "config.linkerd.io/proxy-memory-limit=96Mi config.linkerd.io/proxy-memory-request=10Mi --overwrite" in line
        # An emptied value removes what an earlier apply wrote (re-apply), it never keeps it.
        assert " config.linkerd.io/proxy-cpu-request- " in line, line
        assert "proxy-cpu-request=" not in line, "an emptied value still writes its annotation"


@requires_helmfile
def test_an_unmeshed_namespace_gets_no_proxy_resources(hook, dev, tmp_path):
    # The hook writes the proxy keys only together with linkerd.io/inject=enabled; a namespace its
    # own release renders unmeshed (the app-tests sandbox, T-2675) is not the hook's to annotate.
    assert hook.count("proxy-memory-limit") == len(ensured(hook))
    for line in annotations(hook):
        assert "linkerd.io/inject=enabled" in line
    disabled = [
        d["metadata"] for d in dev
        if d.get("kind") == "Namespace" and (d["metadata"].get("annotations") or {}).get("linkerd.io/inject") == "disabled"
    ]
    assert disabled, "dev renders no unmeshed namespace: the check below would pass vacuously"
    for meta in disabled:
        assert meta["name"] not in ensured(hook), meta["name"]
        assert not any(k.startswith("config.linkerd.io/proxy-") for k in meta["annotations"]), meta["name"]
    unmeshed = prepare_hook(edited(tmp_path, serviceMesh__patchNamespaces=False))
    assert "proxy-memory" not in unmeshed and "linkerd.io/inject" not in unmeshed


@requires_helmfile
def test_a_proxy_resource_that_is_no_quantity_is_refused(tmp_path):
    built = build(edited(tmp_path, serviceMesh__proxyResources__memoryLimit="64Mi --overwrite; id"))
    assert built.returncode != 0
    assert "global.serviceMesh.proxyResources.memoryLimit" in built.stderr + built.stdout
