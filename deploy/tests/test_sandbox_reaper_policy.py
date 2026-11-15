"""T-4775: the admission policy that holds the sandbox reaper's cluster-wide `delete` on
namespaces to the namespaces its script reaps, judged by the kyverno CLI as the chart renders it.

RBAC cannot name a label, so without the policy the reaper's token deletes kube-system as readily
as an expired sandbox."""

import shutil
import subprocess
import textwrap
from pathlib import Path

import pytest
import yaml

PROJECT_ROOT = Path(__file__).resolve().parent.parent
CHART = PROJECT_ROOT / "components/sandbox-reaper/charts/reaper"
REAPER = "system:serviceaccount:ops:sandbox-reaper"
LABEL = "sandbox.joinedcontext.com/lifecycle"

pytestmark = [
    pytest.mark.skipif(shutil.which("kyverno") is None, reason="kyverno CLI not installed"),
    pytest.mark.skipif(shutil.which("helm") is None, reason="helm not installed"),
]


def template(tmp_path_factory, name: str) -> Path:
    out = tmp_path_factory.mktemp("policy") / name
    out.write_text(
        subprocess.run(
            ["helm", "template", "sandbox-reaper", str(CHART), "--namespace", "ops",
             "--show-only", f"templates/{name}"],
            capture_output=True, text=True, check=True,
        ).stdout
    )
    return out


@pytest.fixture(scope="module")
def policy(tmp_path_factory):
    return template(tmp_path_factory, "policy.yaml")


@pytest.fixture(scope="module")
def preview_names(tmp_path_factory):
    return template(tmp_path_factory, "preview-names.yaml")


def judged(policy: Path, labels: dict, tmp_path: Path, username: str = REAPER,
           name: str = "target", operation: str = "DELETE") -> str:
    """"pass", "fail" or "skip": what admission does with `operation` on a namespace with these labels."""
    resource = {"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": name, "labels": labels}}
    (tmp_path / "resource.yaml").write_text(yaml.safe_dump(resource))
    (tmp_path / "userinfo.yaml").write_text(textwrap.dedent(f"""\
        apiVersion: cli.kyverno.io/v1alpha1
        kind: UserInfo
        metadata: {{name: user}}
        userInfo:
          username: {username}
          groups: [system:serviceaccounts, system:authenticated]
        """))
    (tmp_path / "values.yaml").write_text(textwrap.dedent("""\
        apiVersion: cli.kyverno.io/v1alpha1
        kind: Values
        metadata: {name: values}
        globalValues:
          request.operation: OPERATION
        """).replace("OPERATION", operation))
    result = subprocess.run(
        ["kyverno", "apply", str(policy), "--resource", str(tmp_path / "resource.yaml"),
         "--userinfo", str(tmp_path / "userinfo.yaml"), "--values-file", str(tmp_path / "values.yaml")],
        capture_output=True, text=True, check=False,
    )
    summary = next(line for line in result.stdout.splitlines() if line.startswith("pass:"))
    counts = dict(part.strip().split(": ") for part in summary.split(","))
    assert counts["error"] == "0", result.stdout
    if counts["fail"] != "0":
        return "fail"
    return "pass" if counts["pass"] != "0" else "skip"


@pytest.mark.parametrize("lifecycle", ["unmanaged", "preview"])
def test_the_reaper_may_delete_a_sandbox(policy, tmp_path, lifecycle):
    assert judged(policy, {LABEL: lifecycle}, tmp_path) == "pass"


@pytest.mark.parametrize(
    "labels",
    [{}, {LABEL: "managed"}, {LABEL: ""}, {"kubernetes.io/metadata.name": "kube-system"}],
    ids=["unlabelled", "other-lifecycle", "empty-lifecycle", "system"],
)
def test_the_reaper_may_not_delete_any_other_namespace(policy, tmp_path, labels):
    assert judged(policy, labels, tmp_path) == "fail"


def test_someone_else_is_not_the_policys_business(policy, tmp_path):
    assert judged(policy, {}, tmp_path, username="system:serviceaccount:ops:someone") == "skip"


@pytest.mark.parametrize("name", ["pv-fix-login", "pv-a", "pv-" + "a" * 40])
def test_a_preview_namespace_may_carry_the_preview_label(preview_names, tmp_path, name):
    assert judged(preview_names, {LABEL: "preview"}, tmp_path, username="admin", name=name, operation="CREATE") == "pass"


@pytest.mark.parametrize("name", ["dev-city-apps", "dev", "pv-", "pv-Bad", "pv-" + "a" * 41, "xpv-a"])
def test_no_other_namespace_may_carry_the_preview_label(preview_names, tmp_path, name):
    """T-5548: the label opens Keycloak and Postgres on dev, so the Portal's apps namespaces and
    every other name are refused it, on create and on a later relabel."""
    for operation in ("CREATE", "UPDATE"):
        assert judged(preview_names, {LABEL: "preview"}, tmp_path, username="system:serviceaccount:dev:portal",
                      name=name, operation=operation) == "fail"


def test_other_lifecycles_keep_any_name(preview_names, tmp_path):
    assert judged(preview_names, {LABEL: "unmanaged"}, tmp_path, username="admin", name="sandbox-x",
                  operation="CREATE") == "pass"
