"""The forge's Actions runner and what the forge needs for it (T-2608, ADR-N-028, AP-80, AP-81).

The runner executes every application repository's workflow in its own container (host mode),
so an application's code runs in this pod: it is the untrusted zone of T-1707. These tests
hold the walls the render decides; the loop that re-registers per job and kills what a job
leaves behind is `builder/runner.sh` in the portal repository.
"""

import base64
import re
import shutil

import pytest
import yaml

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")

ENVIRONMENTS = ["local", "production"]


def one(docs, kind, name):
    return next(d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name)


def config_of(docs, name):
    return yaml.safe_load(one(docs, "ConfigMap", name)["data"]["runner.yaml"])


def runner_pod(docs):
    return one(docs, "Deployment", "gitea-runner")["spec"]["template"]["spec"]


@requires_helmfile
@pytest.mark.parametrize("environment", ENVIRONMENTS)
def test_the_runner_holds_no_kubernetes_token_and_no_privilege(rendered, environment):
    """AP-81: no service-account token, non-root, no privilege escalation, every capability
    dropped and a read-only root, in the runner and in its init container alike."""
    pod = runner_pod(rendered(environment))
    assert pod["automountServiceAccountToken"] is False
    assert pod["securityContext"]["runAsNonRoot"] is True
    for container in pod["initContainers"] + pod["containers"]:
        context = container["securityContext"]
        assert not context.get("privileged"), container["name"]
        assert context["allowPrivilegeEscalation"] is False, container["name"]
        assert context["capabilities"]["drop"] == ["ALL"], container["name"]
        assert context["readOnlyRootFilesystem"] is True, container["name"]
        assert "@sha256:" in container["image"], container["name"]
    # No container runtime reaches the pod: nothing mounts a host path or a socket.
    assert not any("hostPath" in volume for volume in pod["volumes"])


@requires_helmfile
@pytest.mark.parametrize("environment", ENVIRONMENTS)
def test_only_the_init_container_mounts_the_registration_token(rendered, environment):
    """ADR-N-028 §5: a job runs as the runner's own user, so the registration token must not be
    a file or a variable the runner container holds. The init container copies it into memory,
    and `runner` reads it once and deletes it."""
    pod = runner_pod(rendered(environment))
    secret_volumes = {v["name"] for v in pod["volumes"] if "secret" in v}
    assert {v["secret"]["secretName"] for v in pod["volumes"] if "secret" in v} == {"gitea-runner-registration"}
    runner = pod["containers"][0]
    assert not secret_volumes & {m["name"] for m in runner.get("volumeMounts", [])}
    assert not any("valueFrom" in e for e in runner.get("env", []))
    assert not runner.get("envFrom")
    init = pod["initContainers"][0]
    assert secret_volumes <= {m["name"] for m in init["volumeMounts"]}
    handoff = next(v for v in pod["volumes"] if v["name"] == "runner-secret")
    assert handoff["emptyDir"]["medium"] == "Memory"


@requires_helmfile
@pytest.mark.parametrize("environment", ENVIRONMENTS)
def test_jobs_run_on_the_host_label_with_no_shared_cache_and_in_cluster_addresses(rendered, environment):
    """ADR-N-028 §3.3: the node-22 label runs on the builder image itself, no repository's
    cache feeds another's, and a job is told the forge's and the Portal's in-cluster addresses,
    since its egress reaches nothing else."""
    docs = rendered(environment)
    config = yaml.safe_load(one(docs, "ConfigMap", "gitea-runner")["data"]["runner.yaml"])
    assert config["runner"]["labels"] == ["node-22:host"]
    assert config["runner"]["capacity"] == 1
    assert config["cache"]["enabled"] is False
    envs = config["runner"]["envs"]
    assert envs["JC_FORGE_URL"].startswith("http://gitea-http.") and envs["JC_FORGE_URL"].endswith(":3000")
    assert envs["JC_PORTAL_URL"].startswith("http://portal.")
    pod = runner_pod(docs)
    env = {e["name"]: e.get("value") for e in pod["containers"][0]["env"]}
    assert env["GITEA_INSTANCE"] == envs["JC_FORGE_URL"]


@requires_helmfile
@pytest.mark.parametrize("environment", ENVIRONMENTS)
def test_the_forge_runs_actions_and_packages_and_resolves_actions_on_itself(rendered, environment):
    """ADR-N-028 §3.1, §3.4: Actions and the package registry are on, and a `uses:` resolves on
    this forge, never on github.com."""
    docs = rendered(environment)
    inline = one(docs, "Secret", "gitea-inline-config")
    data = inline.get("stringData") or {k: base64.b64decode(v).decode() for k, v in inline["data"].items()}
    actions = dict(line.split("=", 1) for line in data["actions"].splitlines() if "=" in line)
    packages = dict(line.split("=", 1) for line in data["packages"].splitlines() if "=" in line)
    assert actions["ENABLED"] == "true"
    assert actions["DEFAULT_ACTIONS_URL"] == "self"
    assert packages["ENABLED"] == "true"


@requires_helmfile
@pytest.mark.parametrize("environment", ENVIRONMENTS)
def test_the_bootstrap_writes_the_registration_token_where_the_runner_reads_it(rendered, environment):
    """The bootstrap Job may write that one Secret in the runner's namespace, and is told to."""
    docs = rendered(environment)
    job = one(docs, "Job", "gitea-bootstrap")
    env = {e["name"]: e.get("value") for e in job["spec"]["template"]["spec"]["containers"][0]["env"]}
    runner_namespace = one(docs, "Deployment", "gitea-runner")["metadata"]["namespace"]
    assert env["RUNNER_SECRET"] == "gitea-runner-registration"
    assert env["RUNNER_NAMESPACES"].split() == [runner_namespace]
    role = next(
        d for d in docs
        if d.get("kind") == "Role" and d["metadata"]["name"] == "gitea-bootstrap"
        and d["metadata"]["namespace"] == runner_namespace
    )
    names = {n for rule in role["rules"] for n in rule.get("resourceNames", [])}
    assert "gitea-runner-registration" in names


@pytest.fixture(scope="module")
def bootstrap_script(rendered):
    return one(rendered("local"), "Job", "gitea-bootstrap")["spec"]["template"]["spec"]["containers"][0]["args"][0]


@requires_helmfile
def test_the_bootstrap_script_writes_the_registration_token_into_the_runner_namespace(bootstrap_script, tmp_path):
    """ADR-N-028 §3.3: the Job asks the forge for the organization's registration token and
    writes it as the Secret the runner's init container mounts."""
    from test_forge_seed_converges import Forge

    forge = Forge(tmp_path, {})
    result = forge.run(bootstrap_script, RUNNER_SECRET="gitea-runner-registration", RUNNER_NAMESPACES="runners")
    assert result.returncode == 0, result.stderr
    assert any(line.endswith("/orgs/joinedcontext/actions/runners/registration-token") and line.startswith("POST")
               for line in forge.log)
    written = forge.secrets["gitea-runner-registration"]
    assert base64.b64decode(written["data"]["token"]).decode() == "R" * 40
    assert any("/namespaces/runners/secrets/gitea-runner-registration" in line for line in forge.log)
    # The token is never printed, only where it went.
    assert "R" * 40 not in result.stdout + result.stderr


@requires_helmfile
def test_without_a_runner_the_bootstrap_asks_for_no_registration_token(bootstrap_script, tmp_path):
    """No runner in the environment, no registration token lying around in a Secret."""
    from test_forge_seed_converges import Forge

    forge = Forge(tmp_path, {})
    result = forge.run(bootstrap_script, RUNNER_SECRET="gitea-runner-registration", RUNNER_NAMESPACES="")
    assert result.returncode == 0, result.stderr
    assert not any("registration-token" in line for line in forge.log)
    assert "gitea-runner-registration" not in forge.secrets


def with_rust_runner(digest):
    """The copied tree with the rust-1.90 runner switched on and its image pinned to `digest`."""

    def edit(tree):
        environment = tree / "components/gitea-runner/default-environment.yaml.gotmpl"
        text = environment.read_text()
        head, rust = text.split("  rust:\n", 1)
        environment.write_text(head + "  rust:\n" + rust.replace("enabled: false", "enabled: true", 1))
        images = tree / "components/gitea-runner/images.yaml"
        head, rust = images.read_text().split("  rust:\n", 1)
        rust = re.sub(r"digest: '[^']*'", f"digest: '{digest}'", rust, count=1)
        images.write_text(head + "  rust:\n" + rust)

    return edit


@requires_helmfile
def test_the_rust_runner_is_the_same_walls_under_its_own_label_and_memory(rendered_variant):
    """AP-105, AP-106: the second runner runs the rust-1.90 image under the label the fullstack
    workflow asks for, with the job memory a release link needs, and carries every wall of the
    first: no Kubernetes token, no privilege, the registration token copied into memory, and the
    pod label the runner NetworkPolicy selects."""
    digest = "sha256:" + "ab" * 32
    docs = rendered_variant("dev", with_rust_runner(digest))
    node = one(docs, "Deployment", "gitea-runner")["spec"]["template"]
    rust = one(docs, "Deployment", "gitea-runner-rust")["spec"]["template"]

    container = rust["spec"]["containers"][0]
    assert container["image"].endswith(f"joinedcontext-app-builder-rust:main@{digest}")
    assert container["resources"]["limits"]["memory"] == "4Gi"
    # dev asks what the runner holds idle and keeps the build's limit; capacity 1 keeps it to one
    # build at a time (T-4358).
    assert container["resources"]["requests"]["memory"] == "256Mi"
    assert config_of(docs, "gitea-runner-rust")["runner"]["capacity"] == 1
    assert rust["spec"]["automountServiceAccountToken"] is False
    assert container["securityContext"] == node["spec"]["containers"][0]["securityContext"]
    assert rust["metadata"]["labels"]["app.kubernetes.io/name"] == "gitea-runner"
    assert [c["image"] for c in rust["spec"]["initContainers"]] == [container["image"]]

    config = yaml.safe_load(one(docs, "ConfigMap", "gitea-runner-rust")["data"]["runner.yaml"])
    assert config["runner"]["labels"] == ["rust-1.90:host"]
    assert config["runner"]["timeout"] == "30m"
    assert config["runner"]["envs"]["JC_FORGE_URL"].endswith(":3000")
    node_config = yaml.safe_load(one(docs, "ConfigMap", "gitea-runner")["data"]["runner.yaml"])
    assert node_config["runner"]["labels"] == ["node-22:host"]
    assert node_config["runner"]["timeout"] == "20m"


@requires_helmfile
def test_the_rust_runner_does_not_render_without_its_digest(rendered_variant):
    """AP-13, AP-106: switched on with no pinned digest, the render stops and names the file to
    pin, rather than deploying a tag."""
    with pytest.raises(AssertionError, match="needs its image digest"):
        rendered_variant("dev", with_rust_runner(""))


def peers(policy):
    """Each egress peer of a policy as (namespace, pod labels, ports)."""
    found = set()
    for rule in policy["spec"].get("egress", []):
        ports = tuple(sorted(port["port"] for port in rule.get("ports", [])))
        for peer in rule["to"]:
            assert "ipBlock" not in peer, f"{policy['metadata']['name']}: an address range is a way out"
            namespace = peer.get("namespaceSelector", {}).get("matchLabels", {}).get("kubernetes.io/metadata.name")
            labels = tuple(sorted(peer.get("podSelector", {}).get("matchLabels", {}).items()))
            found.add((namespace, labels, ports))
    return found


@requires_helmfile
@pytest.mark.parametrize("environment", ["local", "dev", "production"])
def test_a_build_reaches_the_forge_the_portal_and_dns_and_nothing_else(rendered, environment):
    """T-1707 step 3, AP-81: an application's own install, test and build code runs in the runner,
    so its egress is the lane's and no more: the forge, the Portal's API, DNS, and, where the mesh
    runs, the same two peers on the proxy's port and the mesh's control plane. No address range, no
    ingress, nothing on the internet. A rule added for convenience fails here, not in a breach."""
    docs = rendered(environment)
    policies = [
        d for d in docs
        if d.get("kind") == "NetworkPolicy"
        and d["spec"].get("podSelector", {}).get("matchLabels", {}).get("app.kubernetes.io/name") == "gitea-runner"
    ]
    assert policies, "the runner has a policy of its own"
    forge = (("app.kubernetes.io/instance", "gitea-forge"), ("app.kubernetes.io/name", "gitea"))
    portal = (("app.kubernetes.io/name", "portal-portal"),)
    dns = (("k8s-app", "kube-dns"),)
    allowed = {(forge, (3000,)), (portal, (8080,)), (dns, (53, 53)), (forge, (4143,)), (portal, (4143,)), ((), (8080, 8086, 8090))}
    for policy in policies:
        for rule in policy["spec"].get("ingress", []):
            assert "from" not in rule, f"{policy['metadata']['name']}: nothing calls a runner"
        for namespace, labels, ports in peers(policy):
            assert (labels, ports) in allowed, f"{policy['metadata']['name']}: {namespace} {labels} {ports}"
            if labels == ():
                assert namespace == "linkerd", "a selector-less peer is the mesh's control plane alone"


@requires_helmfile
@pytest.mark.parametrize("environment", ["dev", "production"])
def test_a_build_pod_runs_under_its_class_label_with_the_shared_runners_walls(rendered, environment):
    """AP-130: an App's `build` job asks for `app-build-node` or `app-build-rust`, which only the
    pod the Portal starts for it carries. Its configuration is the shared runner's in every other
    respect: one job at a time, the forge's shared cache off (the App's own cache is its claim,
    AP-131), and the forge and the Portal at their in-cluster addresses."""
    docs = rendered(environment)
    shared = yaml.safe_load(one(docs, "ConfigMap", "gitea-runner")["data"]["runner.yaml"])
    for build_class, timeout in (("node", "20m"), ("rust", "30m")):
        config_map = one(docs, "ConfigMap", f"gitea-runner-build-{build_class}")
        assert config_map["metadata"]["namespace"] == one(docs, "ConfigMap", "gitea-runner")["metadata"]["namespace"]
        config = yaml.safe_load(config_map["data"]["runner.yaml"])
        assert config["runner"]["labels"] == [f"app-build-{build_class}:host"]
        assert config["runner"]["timeout"] == timeout
        assert config["runner"]["capacity"] == 1
        assert config["runner"]["file"] == shared["runner"]["file"]
        assert config["cache"]["enabled"] is False
        assert config["runner"]["envs"] == shared["runner"]["envs"]
    assert shared["runner"]["labels"] == ["node-22:host"], "the shared runner never takes an App's build"


@requires_helmfile
@pytest.mark.parametrize("environment", ["dev", "production"])
def test_the_portal_may_start_build_jobs_in_the_runner_namespace_and_nothing_else(rendered, environment):
    """AP-130: the Portal's rights in the runner's namespace are a Job, its token Secret and the
    App's cache claim, by server-side apply, bound to the Portal's ServiceAccount alone; Jobs are
    listed to run one build at a time (T-5815). No pods, no exec, no logs, and no list of Secrets. The Portal is told that namespace and the two runner
    images, each pinned by digest."""
    docs = rendered(environment)
    namespace = one(docs, "ConfigMap", "gitea-runner")["metadata"]["namespace"]
    role = one(docs, "Role", "portal-build-pods")
    assert role["metadata"]["namespace"] == namespace
    rules = {(tuple(r["apiGroups"]), tuple(r["resources"])): set(r["verbs"]) for r in role["rules"]}
    assert rules == {
        (("batch",), ("jobs",)): {"get", "list", "create", "patch", "delete"},
        (("",), ("secrets",)): {"create", "patch"},
        (("",), ("persistentvolumeclaims",)): {"get", "list", "create", "patch", "delete"},
    }
    assert not any("resourceNames" in r or "*" in r["verbs"] for r in role["rules"])

    binding = one(docs, "RoleBinding", "portal-build-pods")
    portal = one(docs, "Deployment", "portal")
    assert binding["metadata"]["namespace"] == namespace
    assert binding["roleRef"] == {"apiGroup": "rbac.authorization.k8s.io", "kind": "Role", "name": "portal-build-pods"}
    assert binding["subjects"] == [
        {"kind": "ServiceAccount", "name": portal["spec"]["template"]["spec"]["serviceAccountName"], "namespace": portal["metadata"]["namespace"]}
    ]

    env = {e["name"]: e.get("value") for e in portal["spec"]["template"]["spec"]["containers"][0]["env"]}
    assert env["JC_PORTAL_BUILD_NAMESPACE"] == namespace
    runner = one(docs, "Deployment", "gitea-runner")["spec"]["template"]["spec"]["containers"][0]["image"]
    assert env["JC_PORTAL_BUILD_IMAGE_NODE"] == runner.replace(":main@", "@")
    assert re.fullmatch(r"ghcr\.io/[\w./-]+/joinedcontext-app-builder-rust@sha256:[0-9a-f]{64}", env["JC_PORTAL_BUILD_IMAGE_RUST"])
    assert "JC_PORTAL_BUILD_CACHE_SIZE" not in env, "1Gi, the Portal's default"
