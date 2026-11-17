"""What the edge-file composer may do in the APISIX namespace (evidence/T-7129/design.md §3 and
§7, T-7138).

One Role, `edge-file-composer`, bound to the ServiceAccount that composes (the Portal's, or jc's
when the Portal runs inside jc): no ClusterRole, no `watch`. RBAC cannot narrow `create` and
`delete` to the objects the composer labelled, so a Kyverno ClusterPolicy for that
ServiceAccount alone holds the names, labels, TLS Secret, issuer and backend of each Certificate
and Ingress it sends, and refuses a delete of anything it did not label, the edge's own `apisix`
Ingress above all. Each refusal below was red before the policy existed."""

import copy
import re
import shutil
import subprocess
import textwrap
from pathlib import Path

import pytest
import yaml

PROJECT_ROOT = Path(__file__).resolve().parent.parent
CHART = PROJECT_ROOT / "components/apisix/charts/configuration"
NS = "apisix"
COMPOSER = "system:serviceaccount:jc:jc"
SOMEONE_ELSE = "system:serviceaccount:cert-manager:cert-manager"
NAME = "jc-org-smartcity-bbsk-sk-1a2b3c4d"

requires_kyverno = pytest.mark.skipif(shutil.which("kyverno") is None, reason="kyverno CLI not installed")
requires_helm = pytest.mark.skipif(shutil.which("helm") is None, reason="helm not installed")


def render(*sets: str) -> list[dict]:
    args = ["helm", "template", "configuration", str(CHART), "--namespace", NS,
            "--set", "composer.enabled=true", "--set", "composer.serviceAccount=jc",
            "--set", "composer.namespace=jc", "--set", "edge.clusterIssuer=letsencrypt-prod",
            "--set", "edge.ingressClass=traefik", "--set", "edge.hosts={dev.example,login.dev.example}"]
    for s in sets:
        args += ["--set", s]
    out = subprocess.run(args, capture_output=True, text=True, check=True).stdout
    return [d for d in yaml.safe_load_all(out) if d]


@pytest.fixture(scope="module")
def policy(tmp_path_factory) -> Path:
    docs = [d for d in render("composer.dns01Issuer=jc-dns01-apisix") if d["kind"] == "ClusterPolicy"]
    assert len(docs) == 1
    path = tmp_path_factory.mktemp("policy") / "policy.yaml"
    path.write_text(yaml.safe_dump(docs[0]))
    return path


def labels(component: str = "org-host") -> dict:
    return {"app.kubernetes.io/managed-by": "joinedcontext-portal", "app.kubernetes.io/component": component}


def certificate(**spec) -> dict:
    return {
        "apiVersion": "cert-manager.io/v1",
        "kind": "Certificate",
        "metadata": {"name": NAME, "namespace": NS, "labels": labels()},
        "spec": {
            "secretName": f"{NAME}-tls",
            "issuerRef": {"name": "letsencrypt-prod", "kind": "ClusterIssuer"},
            "dnsNames": ["smartcity.bbsk.sk"],
            **spec,
        },
    }


def ingress(service: str = "apisix-gateway", tls: bool = True, name: str = NAME, meta_labels=None) -> dict:
    spec = {
        "ingressClassName": "traefik",
        "rules": [{"host": "smartcity.bbsk.sk", "http": {"paths": [{
            "path": "/", "pathType": "Prefix",
            "backend": {"service": {"name": service, "port": {"number": 80}}},
        }]}}],
    }
    if tls:
        spec["tls"] = [{"hosts": ["smartcity.bbsk.sk"], "secretName": f"{name}-tls"}]
    return {
        "apiVersion": "networking.k8s.io/v1",
        "kind": "Ingress",
        "metadata": {"name": name, "namespace": NS, "labels": labels() if meta_labels is None else meta_labels},
        "spec": spec,
    }


def failed(policy: Path, resource: dict, tmp_path: Path, user: str = COMPOSER, operation: str = "CREATE") -> set:
    """The rules `kyverno apply` reports failed for `resource` sent by `user`."""
    res = tmp_path / "resource.yaml"
    res.write_text(yaml.safe_dump(resource))
    info = tmp_path / "userinfo.yaml"
    info.write_text(textwrap.dedent(f"""\
        apiVersion: cli.kyverno.io/v1alpha1
        kind: UserInfo
        metadata:
          name: user
        userInfo:
          username: {user}
          groups: [system:serviceaccounts, system:authenticated]
        """))
    args = ["kyverno", "apply", str(policy), "--resource", str(res), "--userinfo", str(info)]
    if operation != "CREATE":
        values = tmp_path / "values.yaml"
        values.write_text(textwrap.dedent(f"""\
            apiVersion: cli.kyverno.io/v1alpha1
            kind: Values
            metadata:
              name: values
            globalValues:
              request.operation: {operation}
            """))
        args += ["-f", str(values)]
    result = subprocess.run(args, capture_output=True, text=True, check=False)
    assert result.returncode in (0, 1), result.stderr or result.stdout
    assert "error: 0" in result.stdout, result.stdout
    return set(re.findall(r"^\d+ - (\S+) ", result.stdout, re.M))


@requires_helm
def test_one_role_bound_to_the_composer_no_cluster_role_no_watch():
    docs = render()
    roles = [d for d in docs if d["kind"] in ("Role", "ClusterRole")]
    assert [(d["kind"], d["metadata"]["name"]) for d in roles] == [("Role", "edge-file-composer")]
    assert roles[0]["metadata"]["namespace"] == NS
    verbs = {v for rule in roles[0]["rules"] for v in rule["verbs"]}
    assert not verbs & {"watch", "patch", "*"}, verbs
    # update only on the served Secret, by name.
    for rule in roles[0]["rules"]:
        if "update" in rule["verbs"]:
            assert rule["resourceNames"] == ["apisix-standalone-config"]
    bindings = [d for d in docs if d["kind"] in ("RoleBinding", "ClusterRoleBinding")]
    assert [(d["kind"], d["metadata"]["namespace"]) for d in bindings] == [("RoleBinding", NS)]
    assert bindings[0]["subjects"] == [{"kind": "ServiceAccount", "name": "jc", "namespace": "jc"}]


@requires_helm
def test_no_policy_without_the_composer_or_where_kyverno_is_not_installed():
    out = subprocess.run(
        ["helm", "template", "configuration", str(CHART), "--namespace", NS],
        capture_output=True, text=True, check=True,
    ).stdout
    assert "ClusterPolicy" not in out
    assert not [d for d in render("composer.admission.enabled=false") if d["kind"] == "ClusterPolicy"]


@requires_kyverno
@requires_helm
def test_the_composers_own_host_objects_pass(policy, tmp_path):
    assert failed(policy, certificate(), tmp_path) == set()
    assert failed(policy, certificate(issuerRef={"name": "jc-dns01-apisix", "kind": "ClusterIssuer"}), tmp_path) == set()
    assert failed(policy, ingress(), tmp_path) == set()
    assert failed(policy, ingress(tls=False), tmp_path) == set(), "the ACME-only Ingress"
    app = ingress(name="app-bikes", meta_labels=labels("app-host"))
    assert failed(policy, app, tmp_path) == set(), "the App hosts' wave keeps working"
    assert failed(policy, ingress(), tmp_path, operation="DELETE") == set()


@requires_kyverno
@requires_helm
def test_a_certificate_naming_the_edge_tls_secret_is_refused(policy, tmp_path):
    assert failed(policy, certificate(secretName="apisix-edge-tls"), tmp_path) == {"certificate-secret-and-issuer"}


@requires_kyverno
@requires_helm
def test_a_certificate_from_another_issuer_is_refused(policy, tmp_path):
    for ref in ({"name": "attacker", "kind": "ClusterIssuer"}, {"name": "letsencrypt-prod", "kind": "Issuer"}):
        assert failed(policy, certificate(issuerRef=ref), tmp_path) == {"certificate-secret-and-issuer"}


@requires_kyverno
@requires_helm
def test_an_ingress_to_another_service_or_secret_is_refused(policy, tmp_path):
    assert failed(policy, ingress(service="jc"), tmp_path) == {"ingress-secret-and-backend"}
    stolen = ingress()
    stolen["spec"]["tls"][0]["secretName"] = "apisix-edge-tls"
    assert failed(policy, stolen, tmp_path) == {"ingress-secret-and-backend"}
    default = ingress()
    default["spec"]["defaultBackend"] = {"service": {"name": "jc", "port": {"number": 8080}}}
    assert failed(policy, default, tmp_path) == {"ingress-secret-and-backend"}


@requires_kyverno
@requires_helm
def test_an_ingress_with_a_shim_middleware_or_another_class_is_refused(policy, tmp_path):
    """T-7138 bounce row 1: a cert-manager annotation would order a Certificate past the issuer
    rule, a middleware would rewrite what the gateway receives; only the listener's pass."""
    listener = ingress()
    listener["metadata"]["annotations"] = {"traefik.ingress.kubernetes.io/router.tls.options": "apisix-bsi-tr-02102@kubernetescrd"}
    assert failed(policy, listener, tmp_path) == set()
    for key, value in (
        ("cert-manager.io/cluster-issuer", "attacker"),
        ("cert-manager.io/issuer", "attacker"),
        ("traefik.ingress.kubernetes.io/router.middlewares", "jc-strip@kubernetescrd"),
    ):
        shim = ingress()
        shim["metadata"]["annotations"] = {key: value}
        assert failed(policy, shim, tmp_path) == {"ingress-secret-and-backend"}, key
    other = ingress()
    other["spec"]["ingressClassName"] = "nginx"
    assert failed(policy, other, tmp_path) == {"ingress-secret-and-backend"}


@requires_kyverno
@requires_helm
def test_the_edges_own_hosts_and_another_issuer_group_are_refused(policy, tmp_path):
    """T-7138 bounce row 2: a composer object never names a host of the edge's own (the sign-in
    host above all), a Certificate names one host, and its issuer is cert-manager's."""
    login = ingress()
    login["spec"]["rules"][0]["host"] = "login.dev.example"
    assert failed(policy, login, tmp_path) == {"ingress-secret-and-backend"}
    for spec in (
        {"dnsNames": ["login.dev.example"]},
        {"dnsNames": ["smartcity.bbsk.sk", "opendata.bbsk.sk"]},
        {"issuerRef": {"name": "letsencrypt-prod", "kind": "ClusterIssuer", "group": "attacker.io"}},
    ):
        assert failed(policy, certificate(**spec), tmp_path) == {"certificate-secret-and-issuer"}, spec
    grouped = certificate(issuerRef={"name": "letsencrypt-prod", "kind": "ClusterIssuer", "group": "cert-manager.io"})
    assert failed(policy, grouped, tmp_path) == set()


@requires_helm
def test_jcs_composer_does_not_render_without_the_policy():
    """T-7138 bounce row 3: jc's composer without Kyverno would hold create and delete on every
    Ingress and Certificate of the namespace with nothing narrowing them."""
    with pytest.raises(subprocess.CalledProcessError) as refused:
        render("composer.admission.required=true", "composer.admission.enabled=false")
    assert "composer.admission.enabled" in refused.value.stderr
    assert [d for d in render("composer.admission.required=true") if d["kind"] == "ClusterPolicy"]


@requires_kyverno
@requires_helm
def test_other_names_or_labels_are_refused(policy, tmp_path):
    assert "names-and-labels" in failed(policy, ingress(name="apisix"), tmp_path)
    assert "names-and-labels" in failed(policy, ingress(meta_labels={}), tmp_path)
    foreign = certificate()
    foreign["metadata"]["labels"]["app.kubernetes.io/component"] = "edge"
    assert "names-and-labels" in failed(policy, foreign, tmp_path)


@requires_kyverno
@requires_helm
def test_deleting_the_unlabelled_apisix_ingress_is_refused(policy, tmp_path):
    edge = ingress(name="apisix", meta_labels={"app.kubernetes.io/managed-by": "Helm"})
    assert failed(policy, edge, tmp_path, operation="DELETE") == {"delete-labelled-only"}
    edge_cert = certificate()
    edge_cert["metadata"] = {"name": "apisix-edge", "namespace": NS}
    assert failed(policy, edge_cert, tmp_path, operation="DELETE") == {"delete-labelled-only"}


@requires_kyverno
@requires_helm
def test_the_policy_binds_the_composer_alone(policy, tmp_path):
    """cert-manager and helm write the edge's own objects; the policy does not reach them."""
    edge_cert = copy.deepcopy(certificate(secretName="apisix-edge-tls"))
    edge_cert["metadata"] = {"name": "apisix-edge", "namespace": NS}
    assert failed(policy, edge_cert, tmp_path, user=SOMEONE_ELSE) == set()
