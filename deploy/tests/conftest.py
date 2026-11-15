"""Shared, cached environment renders.

Rendering one environment takes about half a minute, and several modules want the same
one, so the render happens once per run per environment and every test reads the same
list of documents.
"""

import fcntl
import functools
import importlib.util
import json
import os
import re
import shutil
import subprocess
from pathlib import Path

import pytest
import yaml

PROJECT_ROOT = Path(__file__).resolve().parent.parent
RENDER_SH = PROJECT_ROOT / "scripts/render.sh"
TREE_IGNORE = shutil.ignore_patterns(".git", "__pycache__", ".pytest_cache", "node_modules")
SEED = PROJECT_ROOT / "components/context-gateway/seed"
# The deployment code lives in deploy/ of jc (T-6985); its workflows are jc's: the image lanes and the
# pinned-image scan it brought, and ci.yml, whose deploy-* jobs gate it.
REPO_ROOT = PROJECT_ROOT.parent
DEPLOY_WORKFLOWS = sorted(p for p in (REPO_ROOT / ".github/workflows").glob("*.yml")
                          if p.name in ("ci.yml", "deploy-pinned-images.yml")
                          or re.fullmatch(r"image-(ckan|decidim|gitops|grafana|patched|viz)\.yml", p.name))
DEPLOY_TOOLS_ACTION = REPO_ROOT / ".github/actions/deploy-tools/action.yml"
# libyaml's loader resolves the same YAML 1.1 tags as SafeLoader (bare on/no stay bool) and
# parses the seed ten times faster: 0.7 s instead of 7.6 s for its 2,500 files (T-5550).
_LOADER = getattr(yaml, "CSafeLoader", yaml.SafeLoader)
# The public example environments; dev, stage, smartcity and jc-prod live in the private
# environments/ checkout (T-6984), which `just _dev-assemble` overlays when JC_ENVIRONMENTS names it.
PUBLIC_ENVS = frozenset(p.name for p in (PROJECT_ROOT / "ci/example-deployments/environments").iterdir()
                        if p.is_dir() and not p.name.startswith("_"))


def need_environment(env: str = "") -> None:
    """Skip a test of a private environment when this checkout has none; fail for any other gap.

    `deployment/` is gitignored and assembled from the committed defaults plus the example
    environments; rendering a half-built tree would test the wrong thing. jc's ci holds only the
    public examples, so a private environment's test runs where its values are: the host replay
    of tasks/autocheck, which assembles with JC_ENVIRONMENTS set.
    """
    if (PROJECT_ROOT / "deployment/environments" / env).is_dir():
        return
    if env and env not in PUBLIC_ENVS and not os.environ.get("JC_ENVIRONMENTS"):
        pytest.skip(f"environment {env} is private: set JC_ENVIRONMENTS to the environments/ checkout")
    pytest.fail(f"deployment/environments/{env} is missing: run `just _dev-assemble` first")


def load_yaml(text: str):
    return yaml.load(text, Loader=_LOADER)


def load_yaml_all(text: str) -> list:
    return list(yaml.load_all(text, Loader=_LOADER))


@functools.cache
def seed_documents(project: str = "") -> tuple[tuple[Path, object], ...]:
    """Every (path, document) of the context-gateway seed, or of one project's folder, by path.

    The seed is parsed once per process; a module that walked it per test or per parameter
    spent minutes re-reading the same files (T-5550). Callers filter by name and kind, and
    must not change the documents they get: every test of the process shares them.
    """
    if project:
        root = SEED / project
        return tuple((path, doc) for path, doc in seed_documents() if path.is_relative_to(root))
    return tuple((path, doc) for path in sorted(SEED.rglob("*.yaml")) for doc in load_yaml_all(path.read_text()))


@pytest.fixture(scope="session")
def rendered(tmp_path_factory, worker_id):
    cache: dict[str, list[dict]] = {}
    # Under xdist every worker is its own session, and each rendered the same environments
    # again (T-2895). The workers share the parent of their base temp directories, so the
    # first one to ask renders under a lock and the others read its file.
    base = tmp_path_factory.getbasetemp()
    out_dir = base.parent / "rendered" if worker_id != "master" else base / "rendered"
    out_dir.mkdir(exist_ok=True)

    def _rendered(env: str) -> list[dict]:
        if env not in cache:
            if shutil.which("helmfile") is None:
                pytest.skip("helmfile not installed")
            need_environment(env)
            out = out_dir / f"{env}.yaml"
            with open(out_dir / f"{env}.lock", "w") as lock:
                fcntl.flock(lock, fcntl.LOCK_EX)
                # A finished render is renamed into place, so a file that exists is whole.
                # In ci the render job's own render of this commit, fetched before pytest
                # (T-6111): each shard rendered the same three to six environments again.
                shared = Path(os.environ.get("JC_RENDERED_DIR") or "/nonexistent") / f"{env}.yaml"
                partial = out_dir / f"{env}.partial.yaml"
                if not out.exists() and shared.is_file():
                    shutil.copyfile(shared, partial)
                    partial.rename(out)
                if not out.exists():
                    result = subprocess.run(
                        [str(RENDER_SH), env, str(partial)],
                        cwd=str(PROJECT_ROOT), capture_output=True, text=True,
                    )
                    assert result.returncode == 0, result.stderr
                    partial.rename(out)
            cache[env] = [d for d in load_yaml_all(out.read_text()) if isinstance(d, dict)]
        return cache[env]

    return _rendered


@pytest.fixture(scope="module")
def own_tree(tmp_path_factory):
    """A copy of the repository for a module that writes `deployment/environments/testing`.

    Modules writing that one shared folder had to run on one xdist worker, where their renders
    queued for four and a half minutes of the fast lane (T-2895). The copy is 13 MB and takes a
    fifth of a second, so each module (on each worker) writes its own and they run side by side.
    """
    need_environment()
    tree = tmp_path_factory.mktemp("tree") / "repo"
    shutil.copytree(PROJECT_ROOT, tree, ignore=TREE_IGNORE)
    return tree


@pytest.fixture
def rendered_variant(tmp_path_factory):
    """Render an environment from a copy of the tree, with the committed defaults edited.

    A `--state-values-set` on the command line is merged after the environment values files
    have already rendered, so a flag one of those files reads — `global.ckan.sso`, the
    component list — cannot be overridden that way. The tree is a few megabytes; copying it
    is cheaper than a second committed environment that exists only to be rendered by a test.
    """
    made = tmp_path_factory.mktemp("variant")
    count = 0

    def _rendered(env: str, edit) -> list[dict]:
        nonlocal count
        if shutil.which("helmfile") is None:
            pytest.skip("helmfile not installed")
        need_environment(env)
        count += 1
        tree = made / f"tree-{count}"
        shutil.copytree(PROJECT_ROOT, tree, ignore=TREE_IGNORE)
        edit(tree)
        out = made / f"{env}-{count}.yaml"
        result = subprocess.run(
            [str(tree / "scripts/render.sh"), env, str(out)],
            cwd=str(tree), capture_output=True, text=True,
        )
        assert result.returncode == 0, result.stderr
        return [d for d in load_yaml_all(out.read_text()) if isinstance(d, dict)]

    return _rendered


def drop_component(tree, env: str, name: str):
    """Remove one component, and the comment above it, from an environment's list.

    The list is read while the environment values render, so `--state-values-set` cannot reach
    it; a copied tree is how an environment without a component is rendered. Edited as text
    because the file is a Go template and a YAML round trip would eat its directives.
    """
    path = tree / f"deployment/environments/{env}/global.yaml.gotmpl"
    lines = path.read_text().splitlines(keepends=True)
    kept = []
    for line in lines:
        if line.strip() == f"- {name}":
            while kept and kept[-1].lstrip().startswith("#"):
                kept.pop()
            continue
        kept.append(line)
    assert len(kept) < len(lines), f"{name} is not in the {env} component list"
    path.write_text("".join(kept))


def set_global(tree, dotted: str, value):
    """Edit one `global.*` value of the copied tree's committed defaults."""
    path = tree / "defaults/environment/global.yaml"
    values = yaml.safe_load(path.read_text())
    target = values["global"]
    *parents, leaf = dotted.split(".")
    for key in parents:
        target = target[key]
    target[leaf] = value
    path.write_text(yaml.safe_dump(values, sort_keys=False))


@pytest.fixture(scope="session")
def worker_id(request):
    """The xdist worker's name, or "master" in a run without xdist (its own fixture when
    pytest-xdist is installed; this one stands in when it is not)."""
    return getattr(request.config, "workerinput", {}).get("workerid", "master")


def pytest_configure(config):
    # Known without pytest-xdist installed too, so a serial run does not warn about the marker.
    config.addinivalue_line("markers", "xdist_group(name): run on the same xdist worker as the rest of the group")


def pytest_collection_modifyitems(config, items):
    """In a ci.yml shard (JC_TEST_SHARD=<shard>/<count>), only this shard's part of a file
    scripts/ci/test-shard.py splits over every shard (T-5886); a run without it keeps every test."""
    if not os.environ.get("JC_TEST_SHARD"):
        return
    shard, count = (int(n) for n in os.environ["JC_TEST_SHARD"].split("/"))
    spec = importlib.util.spec_from_file_location("test_shard", PROJECT_ROOT / "scripts/ci/test-shard.py")
    test_shard = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(test_shard)
    cut = test_shard.split(test_shard.files(), json.loads(test_shard.TABLE.read_text(encoding="utf-8")), count)
    seen: dict[str, int] = {}
    keep, drop = [], []
    for item in items:
        name = item.path.name
        if name in cut:
            seen[name] = seen.get(name, -1) + 1
            (keep if test_shard.mine(seen[name], shard, count) else drop).append(item)
        else:
            keep.append(item)
    if drop:
        config.hook.pytest_deselected(items=drop)
        items[:] = keep


def demo_id(name: str) -> str:
    """The fixed Keycloak user id the seeds name a demo person by (PF-126): `demo_id("demo.steward")`."""
    return yaml.safe_load((PROJECT_ROOT / "components/keycloak/demo-users.yaml").read_text())[name]["id"]


def seed_configmap(docs):
    """The forge seed as one ConfigMap: the bootstrap renders `gitea-bootstrap-seed` (the
    organization's files) and the gitea-seed releases one `gitea-seed-s<n>-<part>` per project
    part, so that none outgrows a ConfigMap's 1 MiB (T-3177, T-3998, T-4431); the Job mounts them
    all as one tree."""
    data = {}
    for d in docs:
        if d.get("kind") == "ConfigMap" and d["metadata"]["name"].startswith(("gitea-bootstrap-seed", "gitea-seed-")):
            data.update(d["data"])
    assert data, "no forge seed rendered"
    return {"data": data}


CATALOGUE_ASSISTANT = PROJECT_ROOT / "components/ckan/seed/assistant.yaml.gotmpl"


def render_catalogue_assistant(instance: dict, domain: str, name: str = "catalogue") -> str:
    """The AssistantDeployment a catalogue instance's block renders (T-4636), through Helm's own
    template engine: a throwaway chart whose one template includes the seed template, so the test
    reads exactly what the forge seed does."""
    if shutil.which("helm") is None:
        pytest.skip("helm not installed")
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        chart = Path(tmp)
        (chart / "templates").mkdir()
        (chart / "Chart.yaml").write_text("apiVersion: v2\nname: seed\nversion: 0.0.0\n")
        (chart / "templates/_seed.tpl").write_text(
            '{{- define "seed" -}}\n' + CATALOGUE_ASSISTANT.read_text() + "\n{{- end -}}\n")
        (chart / "templates/out.yaml").write_text(
            '{{ include "seed" (dict "name" .Values.name "instance" .Values.instance "domain" .Values.domain) }}\n')
        (chart / "values.yaml").write_text(yaml.safe_dump({"name": name, "instance": instance, "domain": domain}))
        out = subprocess.run(["helm", "template", "x", str(chart)], capture_output=True, text=True, check=True).stdout
    return out.split("# Source: seed/templates/out.yaml\n", 1)[1]


def dev_catalogue(name: str = "catalogue") -> dict:
    """`ckan.<name>` as dev sets it, over the component's defaults (its host label among them)."""
    defaults = {"subDomain": "opendata", "site": {"title": {}, "logo": ""}}
    values = yaml.safe_load((PROJECT_ROOT / "ci/example-deployments/environments/dev/ckan.yaml.gotmpl").read_text())
    return {**defaults, **values["ckan"][name]}
