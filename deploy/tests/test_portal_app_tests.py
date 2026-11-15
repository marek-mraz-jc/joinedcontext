"""Where a run tests the model's code before it offers publication (SDK-38, T-2675).

The Portal starts one Job per version in `{release}-app-tests` on the lane's builder image. The
model's code runs there, so the namespace is the whole fence: the restricted Pod Security
Standard, no traffic in or out, a pod cap, and a Role that lets the Portal create, read and
delete ConfigMaps and Jobs and read pods and their logs, and nothing else.
"""

import shutil

import pytest

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")


def portal_env(docs):
    portal = next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal")
    container = next(c for c in portal["spec"]["template"]["spec"]["containers"] if c["name"].startswith("portal"))
    return {e["name"]: e.get("value") for e in container.get("env", [])}


def in_namespace(docs, namespace):
    return {(d["kind"], d["metadata"]["name"]): d for d in docs if d["metadata"].get("namespace") == namespace}


@requires_helmfile
@pytest.mark.parametrize("environment", ["local", "dev"])
def test_the_portal_tests_in_a_namespace_of_their_own_on_the_lanes_image(rendered, environment):
    docs = rendered(environment)
    env = portal_env(docs)
    namespace = env["JC_PORTAL_APP_TESTS_NAMESPACE"]
    assert namespace == f"{env['JC_PORTAL_RELEASE']}-app-tests"
    assert namespace != env["JC_PORTAL_APPS_NAMESPACE"], "the model's code must not share the Portal's namespace"

    runner = next(d for d in docs if d.get("kind") == "Deployment" and d["metadata"]["name"] == "gitea-runner")
    lane_image = runner["spec"]["template"]["spec"]["containers"][0]["image"]
    assert env["JC_PORTAL_APP_TESTS_IMAGE"] == lane_image, "the sandbox runs the tests as the lane does"
    assert "@sha256:" in env["JC_PORTAL_APP_TESTS_IMAGE"]
    # A wasm App's server tests run on the rust runner the build pods use (T-3618).
    rust = env["JC_PORTAL_APP_TESTS_RUST_IMAGE"]
    assert rust.startswith("ghcr.io/marek-mraz-jc/joinedcontext-app-builder-rust:") and "@sha256:" in rust, rust

    ns = next(d for d in docs if d.get("kind") == "Namespace" and d["metadata"]["name"] == namespace)
    labels = ns["metadata"]["labels"]
    for mode in ("enforce", "warn", "audit"):
        assert labels[f"pod-security.kubernetes.io/{mode}"] == "restricted"
    assert ns["metadata"]["annotations"]["linkerd.io/inject"] == "disabled"


@requires_helmfile
def test_nothing_reaches_a_test_pod_and_a_test_pod_reaches_nothing(rendered):
    docs = rendered("local")
    namespace = portal_env(docs)["JC_PORTAL_APP_TESTS_NAMESPACE"]
    objects = in_namespace(docs, namespace)
    policies = [d for (kind, _), d in objects.items() if kind == "NetworkPolicy"]
    assert len(policies) == 1, "one policy, and it allows nothing"
    spec = policies[0]["spec"]
    assert spec["podSelector"] == {}
    assert sorted(spec["policyTypes"]) == ["Egress", "Ingress"]
    assert "ingress" not in spec and "egress" not in spec

    quota = objects[("ResourceQuota", "app-tests")]["spec"]["hard"]
    assert quota["pods"] == "4" and quota["count/jobs.batch"] == "4"
    limit = objects[("LimitRange", "app-tests")]["spec"]["limits"][0]
    assert limit["max"] == {"cpu": "1", "memory": "1Gi"}

    # Nothing in the namespace but the fence and the Portal's grant: no workload of the platform.
    assert {kind for kind, _ in objects} == {"NetworkPolicy", "ResourceQuota", "LimitRange", "Role", "RoleBinding"}


@requires_helmfile
def test_the_portal_may_run_a_job_there_and_read_its_log_and_nothing_else(rendered):
    docs = rendered("local")
    env = portal_env(docs)
    objects = in_namespace(docs, env["JC_PORTAL_APP_TESTS_NAMESPACE"])
    granted = {
        (group, resource, verb)
        for rule in objects[("Role", "app-test-runner")]["rules"]
        for group in rule["apiGroups"]
        for resource in rule["resources"]
        for verb in rule["verbs"]
    }
    assert granted == {
        ("", "configmaps", "create"), ("", "configmaps", "get"), ("", "configmaps", "delete"),
        ("batch", "jobs", "create"), ("batch", "jobs", "get"), ("batch", "jobs", "delete"),
        ("", "pods", "list"),
        ("", "pods/log", "get"),
    }
    binding = objects[("RoleBinding", "app-test-runner")]
    assert binding["roleRef"]["name"] == "app-test-runner"
    assert binding["subjects"] == [
        {"kind": "ServiceAccount", "name": env["JC_PORTAL_SERVICE_ACCOUNT"], "namespace": env["JC_PORTAL_APPS_NAMESPACE"]}
    ]


@requires_helmfile
def test_an_installation_without_the_forges_runner_has_no_sandbox(rendered):
    """Without the builder image there is nothing to test with: no namespace, no variables, and
    the run says its tests were not run (SDK-38)."""
    docs = rendered("production")
    env = portal_env(docs)
    if any(d.get("kind") == "Deployment" and d["metadata"]["name"] == "gitea-runner" for d in docs):
        pytest.skip("production deploys the forge's runner")
    assert "JC_PORTAL_APP_TESTS_NAMESPACE" not in env
    assert not any(d.get("kind") == "Namespace" and d["metadata"]["name"].endswith("-app-tests") for d in docs)

