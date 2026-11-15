"""What a Pipeline's `secretRef`s reach the runner through (T-0927, PL-15).

The Portal's reconciler resolves every reference of every pipeline and writes one Secret,
`pipeline-secrets`, into the runner's namespace. The runner takes the whole Secret as
environment, so a stream reads `${MQTT_PASSWORD}` and no chart, values file or manifest ever
names the value.
"""

import shutil

import pytest

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")

ENVIRONMENTS = ("local", "dev", "production")


def by_name(docs, kind, name):
    return next(d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name)


@requires_helmfile
@pytest.mark.parametrize("environment", ENVIRONMENTS)
def test_the_runner_takes_the_portal_s_secret_as_environment(rendered, environment):
    container = by_name(rendered(environment), "Deployment", "pipeline-runner")["spec"]["template"]["spec"][
        "containers"
    ][0]
    source = next(e for e in container["envFrom"] if e.get("secretRef", {}).get("name") == "pipeline-secrets")
    # The runner starts before the first sync has written the Secret, and in an environment
    # where no pipeline asks for a credential at all.
    assert source["secretRef"]["optional"] is True


@requires_helmfile
@pytest.mark.parametrize("environment", ENVIRONMENTS)
def test_no_credential_is_named_beside_it(rendered, environment):
    """The runner's own identity is the realm client it logs in with; a pipeline's credential
    is resolved by the Portal from the manifests and arrives in the Secret above. A second
    `secretKeyRef` here would be a second place to keep that credential in step."""
    container = by_name(rendered(environment), "Deployment", "pipeline-runner")["spec"]["template"]["spec"][
        "containers"
    ][0]
    for variable in container.get("env", []):
        secret = variable.get("valueFrom", {}).get("secretKeyRef")
        if secret is None:
            continue
        assert secret["name"].startswith("keycloak-client-"), f"{variable['name']} -> {secret['name']}"


@requires_helmfile
@pytest.mark.parametrize("environment", ENVIRONMENTS)
def test_one_backend_at_most_per_environment(rendered, environment):
    """SOPS or OpenBao, never both (ADR-N-012): with two the Portal reads whichever the
    binary prefers, and the installation believes the other one."""
    container = by_name(rendered(environment), "Deployment", "portal")["spec"]["template"]["spec"]["containers"][0]
    named = {variable["name"] for variable in container.get("env", [])}
    assert not ({"JC_PORTAL_SOPS_AGE_KEY_FILE"} & named and {"JC_PORTAL_OPENBAO_ADDR"} & named)


@requires_helmfile
def test_dev_decrypts_with_an_age_identity_it_only_mounts(rendered):
    """dev's backend is SOPS: the identity is a Secret this namespace alone mounts, read-only,
    readable by the pod's group and nobody else. The file is never an env var and never a
    ConfigMap: the reconciler reads it per resolution and holds nothing."""
    pod = by_name(rendered("dev"), "Deployment", "portal")["spec"]["template"]["spec"]
    container = pod["containers"][0]
    key_file = next(e for e in container["env"] if e["name"] == "JC_PORTAL_SOPS_AGE_KEY_FILE")["value"]

    mount = next(m for m in container["volumeMounts"] if m["mountPath"] == key_file.rsplit("/", 1)[0])
    assert mount["readOnly"] is True
    volume = next(v for v in pod["volumes"] if v["name"] == mount["name"])
    assert volume["secret"]["secretName"] == "portal-age-identity"
    assert volume["secret"]["defaultMode"] == 0o440
    # The Secret is operator-supplied and created once; a Portal without it runs and refuses.
    assert volume["secret"]["optional"] is True


@requires_helmfile
@pytest.mark.parametrize("environment", ENVIRONMENTS)
def test_no_age_identity_is_rendered_anywhere(rendered, environment):
    """The private half never leaves `.secrets/`: nothing in a rendered environment carries an
    age identity, in a ConfigMap, a Secret's data or an env value (CC-06)."""
    for document in rendered(environment):
        text = str(document)
        assert "AGE-SECRET-KEY-" not in text, document.get("metadata", {}).get("name")
