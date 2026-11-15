"""The admission policy that bounds the Portal's cluster-wide namespace rights (AP-117), judged
by the kyverno CLI as the chart renders it.

The Portal may create, change and delete a namespace only when it is `{release}-{project}-apps`
and labelled as the Portal's with that project, and may bind only `{release}-portal-apps`, to
itself, in such a namespace. Anyone else's requests are not the policy's business."""

import shutil
import subprocess
import textwrap
from pathlib import Path

import pytest
import yaml

PROJECT_ROOT = Path(__file__).resolve().parent.parent
CHART = PROJECT_ROOT / "components/portal/charts/namespaces"
PORTAL = "system:serviceaccount:dev:portal"
CRDS = Path(__file__).resolve().parent / "fixtures/crds/cert-manager-certificate.yaml"

requires_kyverno = pytest.mark.skipif(shutil.which("kyverno") is None, reason="kyverno CLI not installed")
requires_helm = pytest.mark.skipif(shutil.which("helm") is None, reason="helm not installed")

pytestmark = [requires_kyverno, requires_helm]


def rendered(tmp_path_factory, template: str) -> Path:
    out = tmp_path_factory.mktemp("policy") / "policy.yaml"
    out.write_text(
        subprocess.run(
            ["helm", "template", "portal-namespaces", str(CHART), "--show-only", f"templates/{template}",
             "--set", "release=dev", "--set", "serviceAccount.namespace=dev"],
            capture_output=True, text=True, check=True,
        ).stdout
    )
    return out


@pytest.fixture(scope="module")
def policy(tmp_path_factory):
    return rendered(tmp_path_factory, "policy.yaml")


@pytest.fixture(scope="module")
def workload_policy(tmp_path_factory):
    return rendered(tmp_path_factory, "workload-policy.yaml")


def judged(policy: Path, resource: dict, tmp_path: Path, username: str = PORTAL, operation: str = "CREATE") -> str:
    """"pass", "fail" or "skip": what admission does with this request."""
    (tmp_path / "resource.yaml").write_text(yaml.safe_dump(resource))
    (tmp_path / "userinfo.yaml").write_text(textwrap.dedent(f"""\
        apiVersion: cli.kyverno.io/v1alpha1
        kind: UserInfo
        metadata: {{name: user}}
        userInfo:
          username: {username}
          groups: [system:serviceaccounts, system:authenticated]
        """))
    (tmp_path / "values.yaml").write_text(textwrap.dedent(f"""\
        apiVersion: cli.kyverno.io/v1alpha1
        kind: Values
        metadata: {{name: values}}
        globalValues:
          request.operation: {operation}
        """))
    result = subprocess.run(
        ["kyverno", "apply", str(policy), "--resource", str(tmp_path / "resource.yaml"),
         "--userinfo", str(tmp_path / "userinfo.yaml"), "--values-file", str(tmp_path / "values.yaml"),
         "--crd-paths", str(CRDS)],
        capture_output=True, text=True, check=False,
    )
    summary = next(line for line in result.stdout.splitlines() if line.startswith("pass:"))
    counts = dict(part.strip().split(": ") for part in summary.split(","))
    assert counts["error"] == "0", result.stdout
    if counts["fail"] != "0":
        return "fail"
    return "pass" if counts["pass"] != "0" else "skip"


def namespace(name: str, **labels: str) -> dict:
    return {"apiVersion": "v1", "kind": "Namespace", "metadata": {"name": name, "labels": labels}}


MANAGED = {"joinedcontext.com/managed-by": "joinedcontext-portal"}
# What the Portal writes on every apps namespace (joinedcontext-portal src/apps/project_namespace.rs).
RESTRICTED = {f"pod-security.kubernetes.io/{mode}": "restricted" for mode in ("enforce", "audit", "warn")}


def binding(ns: str, role: str = "dev-portal-apps", kind: str = "ClusterRole", subjects=None) -> dict:
    return {
        "apiVersion": "rbac.authorization.k8s.io/v1",
        "kind": "RoleBinding",
        "metadata": {"name": "joinedcontext-portal", "namespace": ns},
        "roleRef": {"apiGroup": "rbac.authorization.k8s.io", "kind": kind, "name": role},
        "subjects": subjects if subjects is not None else [{"kind": "ServiceAccount", "name": "portal", "namespace": "dev"}],
    }


@pytest.mark.parametrize("operation", ["CREATE", "UPDATE", "DELETE"])
def test_the_portal_may_write_its_projects_apps_namespace(policy, tmp_path, operation):
    ns = namespace("dev-ovzdusie-apps", **MANAGED, **RESTRICTED, **{"joinedcontext.com/project": "ovzdusie"})
    assert judged(policy, ns, tmp_path, operation=operation) == "pass"


@pytest.mark.parametrize(
    "pss",
    [
        {},
        {**RESTRICTED, "pod-security.kubernetes.io/enforce": "privileged"},
        {**RESTRICTED, "pod-security.kubernetes.io/enforce": "baseline"},
        {**RESTRICTED, "pod-security.kubernetes.io/enforce-version": "v1.0"},
    ],
    ids=["no-pod-security", "privileged", "baseline", "pinned-old-version"],
)
@pytest.mark.parametrize("operation", ["CREATE", "UPDATE"])
def test_an_apps_namespace_without_restricted_pod_security_is_refused(policy, tmp_path, pss, operation):
    """T-4775: Kyverno's Pod Security policies list the deployment's own namespaces, not the
    apps namespaces the Portal makes at run time, so the namespace's Pod Security label is the
    only thing between a Deployment the Portal writes and a privileged pod with the node's
    filesystem. The Portal must not be able to leave it off or lower it."""
    ns = namespace("dev-ovzdusie-apps", **MANAGED, **pss, **{"joinedcontext.com/project": "ovzdusie"})
    assert judged(policy, ns, tmp_path, operation=operation) == "fail"


def test_the_portal_may_name_the_latest_pod_security_version(policy, tmp_path):
    labels = {**MANAGED, **RESTRICTED, "pod-security.kubernetes.io/enforce-version": "latest"}
    ns = namespace("dev-ovzdusie-apps", **labels, **{"joinedcontext.com/project": "ovzdusie"})
    assert judged(policy, ns, tmp_path) == "pass"


@pytest.mark.parametrize(
    "ns",
    [
        namespace("kube-system", **MANAGED, **{"joinedcontext.com/project": "ovzdusie"}),
        namespace("dev", **MANAGED),
        namespace("dev-ovzdusie-apps", **{"joinedcontext.com/project": "ovzdusie"}),
        namespace("dev-ovzdusie-apps", **MANAGED, **{"joinedcontext.com/project": "doprava"}),
        namespace("dev-ovzdusie-apps", **MANAGED),
        namespace("dev--apps", **MANAGED, **{"joinedcontext.com/project": ""}),
        namespace("prod-ovzdusie-apps", **MANAGED, **{"joinedcontext.com/project": "ovzdusie"}),
    ],
    ids=["system", "its-own", "unmanaged", "other-project", "no-project", "empty-project", "other-release"],
)
@pytest.mark.parametrize("operation", ["CREATE", "DELETE"])
def test_any_other_namespace_is_refused_to_the_portal(policy, tmp_path, ns, operation):
    assert judged(policy, ns, tmp_path, operation=operation) == "fail"


def test_someone_else_is_not_the_policys_business(policy, tmp_path):
    ns = namespace("kube-system")
    assert judged(policy, ns, tmp_path, username="system:serviceaccount:dev:someone") == "skip"


def test_the_portal_may_bind_the_apps_role_to_itself_in_an_apps_namespace(policy, tmp_path):
    assert judged(policy, binding("dev-ovzdusie-apps"), tmp_path) == "pass"


@pytest.mark.parametrize(
    "rb",
    [
        binding("dev"),
        binding("kube-system"),
        binding("dev-ovzdusie-apps", role="cluster-admin"),
        binding("dev-ovzdusie-apps", kind="Role"),
        binding("dev-ovzdusie-apps", subjects=[{"kind": "ServiceAccount", "name": "default", "namespace": "dev-ovzdusie-apps"}]),
        binding("dev-ovzdusie-apps", subjects=[
            {"kind": "ServiceAccount", "name": "portal", "namespace": "dev"},
            {"kind": "Group", "name": "system:authenticated", "apiGroup": "rbac.authorization.k8s.io"},
        ]),
        binding("dev-ovzdusie-apps", subjects=[]),
    ],
    ids=["its-own-namespace", "system", "other-role", "a-role", "other-subject", "extra-subject", "no-subject"],
)
def test_any_other_binding_is_refused_to_the_portal(policy, tmp_path, rb):
    assert judged(policy, rb, tmp_path) == "fail"


def workload(kind: str = "Deployment", **pod) -> dict:
    """What the Portal's App reconciler writes (joinedcontext-portal src/apps/reconciler.rs), with
    `pod` merged over its pod spec."""
    spec = {
        "automountServiceAccountToken": False,
        "containers": [{"name": "app", "image": "registry.example/app@sha256:" + "0" * 64}],
        **pod,
    }
    body = {"template": {"metadata": {"labels": {"app": "a"}}, "spec": spec}}
    if kind == "Deployment":
        body["selector"] = {"matchLabels": {"app": "a"}}
    return {
        "apiVersion": "apps/v1" if kind == "Deployment" else "batch/v1",
        "kind": kind,
        "metadata": {"name": "a", "namespace": "dev"},
        "spec": body,
    }


@pytest.mark.parametrize("kind", ["Deployment", "Job"])
@pytest.mark.parametrize("operation", ["CREATE", "UPDATE"])
def test_the_portal_may_write_a_workload_without_a_token(workload_policy, tmp_path, kind, operation):
    assert judged(workload_policy, workload(kind), tmp_path, operation=operation) == "pass"


TOKEN_VOLUME = {"name": "t", "projected": {"sources": [{"serviceAccountToken": {"path": "token"}}]}}


@pytest.mark.parametrize(
    "pod",
    [
        {"automountServiceAccountToken": True, "serviceAccountName": "postgres-cluster-dr-drill"},
        {"automountServiceAccountToken": None},
        {"serviceAccountName": "pipeline-tokens", "volumes": [TOKEN_VOLUME]},
        {"volumes": [{"name": "root", "hostPath": {"path": "/"}}]},
        {"hostNetwork": True},
        {"hostPID": True},
        {"hostIPC": True},
        {"containers": [{"name": "app", "image": "x", "securityContext": {"privileged": True}}]},
        {"initContainers": [{"name": "init", "image": "x", "securityContext": {"privileged": True}}]},
        # F-R2DEP2-024: what Pod Security refuses only in the apps namespaces.
        {"containers": [{"name": "app", "image": "x", "securityContext": {"capabilities": {"add": ["SYS_ADMIN"]}}}]},
        {"initContainers": [{"name": "init", "image": "x", "securityContext": {"capabilities": {"add": ["NET_ADMIN"]}}}]},
        {"containers": [{"name": "app", "image": "x", "ports": [{"containerPort": 80, "hostPort": 80}]}]},
        {"containers": [{"name": "app", "image": "x", "securityContext": {"allowPrivilegeEscalation": True}}]},
    ],
    ids=["another-accounts-token", "token-by-default", "projected-token", "host-path", "host-network",
         "host-pid", "host-ipc", "privileged", "privileged-init", "added-capability", "added-capability-init",
         "host-port", "privilege-escalation"],
)
@pytest.mark.parametrize("kind", ["Deployment", "Job"])
def test_a_workload_with_a_token_or_the_node_is_refused_to_the_portal(workload_policy, tmp_path, pod, kind):
    """T-4775: the Portal's namespace holds the ServiceAccounts of the restore drill, the token
    minters and the forge bootstrap. A Deployment or Job the Portal writes must not run as one of
    them with its token, nor reach the node."""
    resource = workload(kind, **pod)
    spec = resource["spec"]["template"]["spec"]
    if spec.get("automountServiceAccountToken") is None:
        del spec["automountServiceAccountToken"]
    assert judged(workload_policy, resource, tmp_path) == "fail"


def test_a_hardened_workload_with_a_container_port_passes(workload_policy, tmp_path):
    """The bound refuses what reaches the node, not what every App writes."""
    resource = workload(containers=[{"name": "app", "image": "x", "ports": [{"containerPort": 8080}],
                                     "securityContext": {"allowPrivilegeEscalation": False,
                                                         "capabilities": {"drop": ["ALL"]}}}])
    assert judged(workload_policy, resource, tmp_path) == "pass"


def test_someone_elses_workload_is_not_the_policys_business(workload_policy, tmp_path):
    resource = workload(automountServiceAccountToken=True)
    assert judged(workload_policy, resource, tmp_path, username="system:serviceaccount:dev:someone") == "skip"


@pytest.fixture(scope="module")
def secrets_policy(tmp_path_factory):
    return rendered(tmp_path_factory, "secrets-policy.yaml")


def secret(name: str, ns: str = "dev", kind: str = "Opaque") -> dict:
    return {"apiVersion": "v1", "kind": "Secret", "type": kind, "metadata": {"name": name, "namespace": ns}}


# What the Portal hands over outside an apps namespace (joinedcontext-portal pipeline_secrets.rs,
# artifact_store.rs, apps/build_pods.rs) and the edge file it composes (reconciler/edge_file.rs:
# refused, no route or limit change reaches APISIX, prod 2026-10-20 07:30Z).
@pytest.mark.parametrize(
    "name",
    ["pipeline-secrets", "artifact-store-reader-hel", "build-bikes-42", "apps-host-1-store", "apisix-standalone-config",
     "personal-data-keys", "agent-run-0f3a9c2e"],
)
@pytest.mark.parametrize("operation", ["CREATE", "UPDATE"])
def test_the_portal_may_write_the_secrets_it_hands_over(secrets_policy, tmp_path, name, operation):
    assert judged(secrets_policy, secret(name), tmp_path, operation=operation) == "pass"


@pytest.mark.parametrize(
    "resource",
    [
        secret("postgres-cluster-app"),
        secret("postgres-cluster-superuser"),
        secret("keycloak-admin"),
        secret("gitea-admin"),
        secret("gitea-runner-registration"),
        secret("artifact-store-root"),
        secret("artifact-store-reader-hel", kind="kubernetes.io/service-account-token"),
        secret("build-x-1", kind="kubernetes.io/dockerconfigjson"),
    ],
    ids=["database-app", "database-superuser", "keycloak-admin", "forge-admin", "runner-registration",
         "artifact-store-root", "service-account-token-type", "dockerconfig-type"],
)
@pytest.mark.parametrize("operation", ["CREATE", "UPDATE"])
def test_a_platform_secret_or_a_token_type_is_refused_to_the_portal(secrets_policy, tmp_path, resource, operation):
    """T-4798: the release namespace holds the platform's credentials. The Portal's Role there
    grants `patch` without names (RBAC has no prefix), so this policy is what keeps a server-side
    apply from overwriting one, and a token-typed Secret from handing it another account's token."""
    assert judged(secrets_policy, resource, tmp_path, operation=operation) == "fail"


def test_in_a_projects_apps_namespace_secrets_are_not_the_policys_business(secrets_policy, tmp_path):
    resource = secret("app-air-endpoint", ns="dev-helsinki-apps")
    assert judged(secrets_policy, resource, tmp_path) == "skip"


def test_someone_elses_secret_is_not_the_policys_business(secrets_policy, tmp_path):
    resource = secret("keycloak-admin")
    assert judged(secrets_policy, resource, tmp_path, username="system:serviceaccount:dev:someone") == "skip"


@pytest.fixture(scope="module")
def edge_policy(tmp_path_factory):
    return rendered(tmp_path_factory, "edge-policy.yaml")


def app_ingress(name: str = "app-bikes", component: str = "app-host", service: str = "apisix-gateway",
                secret_name: str | None = None, labelled: bool = True) -> dict:
    """An Ingress as joinedcontext-portal reconciler/app_hosts.rs named_ingress() writes it."""
    labels = {"app.kubernetes.io/managed-by": "joinedcontext-portal", "app.kubernetes.io/component": component,
              "joinedcontext.com/app": "bikes"} if labelled else {}
    host = "bikes.apps.city.example"
    return {"apiVersion": "networking.k8s.io/v1", "kind": "Ingress",
            "metadata": {"name": name, "namespace": "dev-apisix", "labels": labels},
            "spec": {"ingressClassName": "traefik",
                     "tls": [{"hosts": [host], "secretName": secret_name or f"{name}-tls"}],
                     "rules": [{"host": host, "http": {"paths": [{
                         "path": "/", "pathType": "ImplementationSpecific",
                         "backend": {"service": {"name": service, "port": {"number": 80}}}}]}}]}}


def app_certificate(name: str = "app-bikes", secret_name: str | None = None) -> dict:
    return {"apiVersion": "cert-manager.io/v1", "kind": "Certificate",
            "metadata": {"name": name, "namespace": "dev-apisix",
                         "labels": {"app.kubernetes.io/managed-by": "joinedcontext-portal",
                                    "app.kubernetes.io/component": "app-host", "joinedcontext.com/app": "bikes"}},
            "spec": {"secretName": secret_name or f"{name}-tls", "dnsNames": ["bikes.apps.city.example"],
                     "issuerRef": {"name": "letsencrypt-prod", "kind": "ClusterIssuer"}}}


@pytest.mark.parametrize("resource", [app_ingress(), app_ingress("app-bikes-at-x7", component="app-hostname"),
                                      app_certificate()], ids=["host-ingress", "hostname-ingress", "certificate"])
@pytest.mark.parametrize("operation", ["CREATE", "UPDATE"])
def test_the_portal_may_write_an_apps_edge_objects(edge_policy, tmp_path, resource, operation):
    assert judged(edge_policy, resource, tmp_path, operation=operation) == "pass"


@pytest.mark.parametrize(
    "resource",
    [app_ingress(service="keycloak"), app_ingress(labelled=False), app_ingress(name="apisix"),
     app_ingress(secret_name="apisix-edge-tls"), app_certificate(secret_name="apisix-edge-tls"),
     app_ingress(component="portal")],
    ids=["backend-keycloak", "unlabelled", "edge-name", "edge-tls-secret", "certificate-into-edge-secret",
         "other-component"],
)
def test_an_edge_object_beyond_an_apps_host_is_refused_to_the_portal(edge_policy, tmp_path, resource):
    """T-5702 (F-DEP1-001): RBAC lets the Portal create any Ingress and Certificate in the APISIX
    namespace; an Ingress straight to another Service would skip the edge, and a Certificate
    naming a platform Secret would overwrite it."""
    assert judged(edge_policy, resource, tmp_path) == "fail"


def annotated(annotations: dict, ingress_class: str = "traefik") -> dict:
    resource = app_ingress()
    resource["metadata"]["annotations"] = annotations
    resource["spec"]["ingressClassName"] = ingress_class
    return resource


EDGE_ANNOTATIONS = {"ingress.kubernetes.io/affinity": "cookie",
                    "traefik.ingress.kubernetes.io/router.tls.options": "dev-apisix-bsi-tr-02102@kubernetescrd",
                    "nginx.ingress.kubernetes.io/proxy-buffer-size": "16k"}


def test_an_apps_ingress_keeps_the_edges_class_and_annotations(edge_policy, tmp_path):
    """F-R2DEP2-006: app_hosts.rs copies the edge Ingress's class and annotations; those pass."""
    assert judged(edge_policy, annotated(EDGE_ANNOTATIONS), tmp_path) == "pass"


@pytest.mark.parametrize("resource", [
    annotated({**EDGE_ANNOTATIONS, "nginx.ingress.kubernetes.io/server-snippet": "location / { return 200; }"}),
    annotated({"traefik.ingress.kubernetes.io/router.middlewares": "dev-keycloak-strip@kubernetescrd"}),
    annotated(EDGE_ANNOTATIONS, ingress_class="nginx-internal"),
], ids=["nginx-snippet", "foreign-middleware", "other-class"])
def test_an_apps_ingress_that_changes_the_edge_is_refused_to_the_portal(edge_policy, tmp_path, resource):
    """F-R2DEP2-006: with the class or an annotation of its own choosing, an Ingress the Portal
    writes would change what the edge does beyond routing to APISIX."""
    assert judged(edge_policy, resource, tmp_path) == "fail"


def test_in_a_projects_apps_namespace_ingresses_are_not_the_policys_business(edge_policy, tmp_path):
    resource = app_ingress(service="app-web")
    resource["metadata"]["namespace"] = "dev-helsinki-apps"
    assert judged(edge_policy, resource, tmp_path) == "skip"


def test_someone_elses_ingress_is_not_the_policys_business(edge_policy, tmp_path):
    resource = app_ingress(service="keycloak", labelled=False)
    assert judged(edge_policy, resource, tmp_path, username="system:serviceaccount:dev:someone") == "skip"


def test_the_portal_deletes_only_the_edge_objects_it_labelled(edge_policy, tmp_path):
    assert judged(edge_policy, app_ingress(), tmp_path, operation="DELETE") == "pass"
    assert judged(edge_policy, app_ingress(name="apisix", labelled=False), tmp_path, operation="DELETE") == "fail"


def mqtt_certificate(name: str = "mqtt-sensors", secret_name: str | None = None, part_of: str = "mqtt-brokers") -> dict:
    """A broker's Certificate as components/mqtt-brokers _helpers.tpl renders it and
    joinedcontext-portal reconciler/mqtt_brokers.rs applies it, in the brokers' namespace."""
    return {"apiVersion": "cert-manager.io/v1", "kind": "Certificate",
            "metadata": {"name": name, "namespace": "dev",
                         "labels": {"app.kubernetes.io/managed-by": "joinedcontext-portal",
                                    "app.kubernetes.io/part-of": part_of, "app.kubernetes.io/instance": name}},
            "spec": {"secretName": secret_name or f"{name}-tls", "dnsNames": ["sensors.mqtt.city.example"],
                     "issuerRef": {"name": "letsencrypt-prod", "kind": "ClusterIssuer"}}}


@pytest.mark.parametrize("operation", ["CREATE", "UPDATE", "DELETE"])
def test_the_portal_may_write_and_retire_an_mqtt_brokers_certificate(edge_policy, tmp_path, operation):
    """The Portal applies and retires a Certificate per MQTT broker; the edge bound must leave
    those through (integrator review of T-5702)."""
    assert judged(edge_policy, mqtt_certificate(), tmp_path, operation=operation) == "pass"


@pytest.mark.parametrize(
    "resource",
    [mqtt_certificate(secret_name="apisix-edge-tls"), mqtt_certificate(name="apisix"),
     mqtt_certificate(part_of="portal")],
    ids=["into-a-platform-secret", "not-mqtt-named", "not-part-of-mqtt-brokers"],
)
def test_an_mqtt_certificate_beyond_its_own_secret_is_refused_to_the_portal(edge_policy, tmp_path, resource):
    assert judged(edge_policy, resource, tmp_path) == "fail"
