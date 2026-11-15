"""What the Portal has to trust to finish OIDC discovery (T-0350).

`JC_OIDC_ISSUER` is the public host, because the `iss` claim of every token carries it. Under
the self-signed cluster issuer that host is served with the cluster CA, and the Portal binary
carries the Mozilla bundle alone: discovery dies at the handshake and the pod exits before it
serves anything. The render is the only place this is decidable, so it is the only place a test
can catch it before a variant run spends ten minutes discovering the same thing.
"""

import re
import shutil
from pathlib import Path

import pytest
import yaml


PROJECT_ROOT = Path(__file__).resolve().parent.parent

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")

CA_MOUNT = "/etc/jc/ca"
CA_SECRET = "custom-ca-cert"


def portal_pod_spec(docs):
    deployment = next(
        d for d in docs
        if d.get("kind") == "Deployment" and d["metadata"]["name"] == "portal"
    )
    return deployment["spec"]["template"]["spec"]


def portal_env(pod_spec):
    container = next(c for c in pod_spec["containers"] if c["name"].startswith("portal"))
    return {e["name"]: e.get("value") for e in container.get("env", [])}


@requires_helmfile
def test_selfsigned_local_hands_the_cluster_ca_to_the_portal(rendered):
    """`local` inherits `clusterIssuer: selfsigned-ca` from the defaults, so the CA the prepare
    component distributes has to be mounted AND named, or discovery fails with a bare
    "Request failed" and the deployment-variants lane goes red on a rollout timeout."""
    pod_spec = portal_pod_spec(rendered("local"))
    ca_file = portal_env(pod_spec).get("JC_OIDC_CA_FILE")
    assert ca_file == f"{CA_MOUNT}/ca.crt", "the portal must be told which root to add"

    mount = next(
        (m for c in pod_spec["containers"] for m in c.get("volumeMounts", [])
         if m["mountPath"] == CA_MOUNT),
        None,
    )
    assert mount is not None, f"{ca_file} is configured but nothing mounts {CA_MOUNT}"
    assert mount.get("readOnly") is True

    volume = next((v for v in pod_spec["volumes"] if v["name"] == mount["name"]), None)
    assert volume is not None, f"volumeMount {mount['name']} has no volume"
    # The key is `ca.crt` because that is what components/prepare writes into the Secret.
    assert volume["secret"]["secretName"] == CA_SECRET


def gitea_pod_spec(docs):
    deployment = next(
        d for d in docs
        if d.get("kind") == "Deployment" and d["metadata"]["name"] == "gitea"
    )
    return deployment["spec"]["template"]["spec"]


@requires_helmfile
def test_selfsigned_local_hands_the_cluster_ca_to_gitea(rendered):
    """Gitea runs the same discovery the Portal does, from its `configure-gitea` INIT container
    (`gitea admin auth add-oauth`), and being Go it reads `SSL_CERT_FILE`. Without the CA that
    container exits `x509: certificate signed by unknown authority` and crashloops before the
    server ever starts. The main container needs it too: it contacts the issuer on every login."""
    pod_spec = gitea_pod_spec(rendered("local"))
    every_container = pod_spec.get("initContainers", []) + pod_spec["containers"]
    configure = next(c for c in every_container if c["name"] == "configure-gitea")
    for container in (configure, next(c for c in pod_spec["containers"] if c["name"] == "gitea")):
        env = {e["name"]: e.get("value") for e in container.get("env", [])}
        assert env.get("SSL_CERT_FILE") == f"{CA_MOUNT}/ca.crt", (
            f"{container['name']} must be told which roots to trust"
        )
        assert any(m["mountPath"] == CA_MOUNT for m in container.get("volumeMounts", [])), (
            f"{container['name']} has SSL_CERT_FILE but nothing mounted at {CA_MOUNT}"
        )
    volume = next((v for v in pod_spec["volumes"] if v["name"] == "cluster-ca"), None)
    assert volume is not None and volume["secret"]["secretName"] == CA_SECRET


@requires_helmfile
@pytest.mark.parametrize("env", ["dev", "production"])
def test_a_publicly_trusted_issuer_mounts_no_extra_root(rendered, env):
    """Widening the trust anchors of a public deployment would be a real weakening, so the
    mount exists only where the issuer is self-signed."""
    for pod_spec in (portal_pod_spec(rendered(env)), gitea_pod_spec(rendered(env))):
        for container in pod_spec.get("initContainers", []) + pod_spec["containers"]:
            env_names = {e["name"] for e in container.get("env", [])}
            assert "JC_OIDC_CA_FILE" not in env_names
            assert "SSL_CERT_FILE" not in env_names
        assert not [
            v for v in pod_spec.get("volumes", [])
            if v.get("secret", {}).get("secretName") == CA_SECRET
        ]


@requires_helmfile
@pytest.mark.parametrize("env", ["local", "dev", "production"])
def test_every_tls_terminating_ingress_has_something_issuing_its_secret(rendered, env):
    """An Ingress that terminates TLS on a Secret nobody fills is not a missing certificate,
    it is a WORKING endpoint serving the controller's built-in fake one — no error anywhere,
    and every client that verifies gets `UnknownIssuer`. That is how T-0350 survived a fix to
    the Portal: `local` published the APISIX Ingress (its `enabled` is a constant in the chart)
    while the `apisix-edge` Certificate stayed off, because `global.ingress.enabled` gates the
    Certificate and nothing else. Either a Certificate covers the Secret and its hosts, or the
    Ingress carries a cert-manager annotation and the shim issues one."""
    docs = rendered(env)
    certs = [d for d in docs if d.get("kind") == "Certificate"]
    for ingress in [d for d in docs if d.get("kind") == "Ingress"]:
        name = ingress["metadata"]["name"]
        annotations = ingress["metadata"].get("annotations") or {}
        for entry in ingress.get("spec", {}).get("tls") or []:
            secret = entry.get("secretName")
            if not secret or any(k.startswith("cert-manager.io/") for k in annotations):
                continue
            owner = next((c for c in certs if c["spec"].get("secretName") == secret), None)
            assert owner is not None, (
                f"Ingress {name} terminates TLS on Secret {secret} in {env}, and no Certificate "
                f"fills it: the controller will serve its fake certificate instead"
            )
            covered = set(owner["spec"].get("dnsNames") or [])
            missing = [h for h in entry.get("hosts") or [] if h not in covered]
            assert not missing, (
                f"Certificate {owner['metadata']['name']} does not cover {missing} of "
                f"Ingress {name} in {env}"
            )


def test_the_variant_script_publishes_a_real_certificate():
    """`scripts/test-deployment-variants.sh` does not use a committed environment: it generates
    `deployment/environments/testing/global.yaml.gotmpl` per variant, so the render tests above
    cannot see it. Without `ingress.enabled` that overlay renders one TLS-terminating Ingress
    and zero Certificates, and the lane spends ten minutes crashlooping the Portal against a
    fake certificate (T-0350). Cheaper to assert on the heredoc than to render a variant."""
    script = (PROJECT_ROOT / "scripts/test-deployment-variants.sh").read_text()
    overlay = script.split("cat > \"$ENV_FILE\" <<EOF", 1)[1].split("\nEOF", 1)[0]
    # `<<EOF` is unquoted so the variant toggles interpolate, which also makes a backtick a
    # command substitution: one in a comment runs as a command and drops its output into the
    # generated file. That happened, and cost a CI cycle.
    assert "`" not in overlay, "backticks in this heredoc run as commands, comments included"
    body = yaml.safe_load(re.sub(r"\$\{[^}]+\}", "placeholder", overlay))
    assert body["global"].get("ingress", {}).get("enabled") is True, (
        "the generated variant environment must issue the edge certificate, or the k3d cluster "
        "serves ingress-nginx's fake one and only a verifying client notices"
    )


@requires_helmfile
def test_the_variant_script_only_probes_hosts_the_edge_serves(rendered):
    """`smoke_ingress_http()` asserts real requests traverse ingress -> APISIX -> backend. It
    spent an unknown number of runs probing `nifi.<domain>`, which no component has served since
    Bento replaced NiFi: APISIX matched no route, answered 404, and the check could never pass —
    a check that cannot pass asserts nothing. Every host it probes must be one the edge actually
    serves, so a component being removed breaks this test rather than the lane."""
    script = (PROJECT_ROOT / "scripts/test-deployment-variants.sh").read_text()
    function = script.split("smoke_ingress_http() {", 1)[1].split("\n}\n", 1)[0]
    probed = set(re.findall(r'_host="([^"]+)"', function))
    assert probed, "no probe hosts found; the function was renamed or restructured"

    ingress = next(
        d for d in rendered("local")
        if d.get("kind") == "Ingress" and d["metadata"]["name"] == "apisix"
    )
    domain = "joinedcontext.test"
    served = {rule["host"] for rule in ingress["spec"]["rules"]}
    for host in probed:
        resolved = host.replace("${domain}", domain)
        assert resolved in served, (
            f"the variant script probes {resolved}, which the APISIX Ingress does not serve "
            f"({sorted(served)}) — that request can only ever be a 404"
        )


# T-0400: a meshed Portal reaches the PUBLIC issuer host, which resolves onto the cluster's
# own ingress controller. Linkerd's outbound proxy routes that hop as HTTP and resets the
# connection, and discovery is the first thing the Portal does, so the pod never starts.
SKIP_OUTBOUND = "config.linkerd.io/skip-outbound-ports"


def portal_pods(docs):
    """Every Deployment the Portal component renders, by its pod template."""
    return [
        doc["spec"]["template"]
        for doc in docs
        if doc.get("kind") == "Deployment" and doc["metadata"]["name"] == "portal"
    ]


@requires_helmfile
@pytest.mark.parametrize("env", ["local", "dev", "production"])
def test_a_meshed_portal_does_not_route_its_own_https_through_the_proxy(rendered, env):
    """The Portal's OIDC discovery goes to the public issuer, so the hop leaves the cluster's
    DNS and comes back to its ingress controller on 443. Meshed, that connection is reset and
    the Portal exits before it serves anything; the unmeshed profile passes for exactly this
    reason. Taking 443 out of the outbound redirect makes the hop identical to the unmeshed
    one, and loses no mTLS: the peer terminates public TLS, so the mesh never wrapped it."""
    for template in portal_pods(rendered(env)):
        annotations = template["metadata"].get("annotations") or {}
        if annotations.get("linkerd.io/inject") != "enabled":
            continue
        ports = [port.strip() for port in annotations.get(SKIP_OUTBOUND, "").split(",")]
        assert "443" in ports, (
            f"the Portal is meshed in {env} without {SKIP_OUTBOUND} covering 443: its OIDC "
            "discovery against the public issuer will be reset and the pod will not start"
        )


# T-0418: the five meshed pods that dial the public host on 443 keep it out of the outbound
# redirect. Opaque ports on the ingress controller did not keep that hop alive (ci-full
# 34784223904), so dropping the skip from any of them brings the reset back.
PUBLIC_HOST_CLIENTS = ("portal", "apisix", "pipeline-runner", "gitea", "agent-proxy")


@requires_helmfile
def test_every_meshed_pod_that_dials_the_public_host_skips_443_outbound(rendered):
    templates = {
        doc["metadata"]["name"]: doc["spec"]["template"]
        for doc in rendered("dev")
        if doc.get("kind") == "Deployment"
    }
    for name in PUBLIC_HOST_CLIENTS:
        assert name in templates, f"{name} is not rendered in dev: {sorted(templates)}"
        annotations = templates[name]["metadata"].get("annotations") or {}
        ports = [port.strip() for port in annotations.get(SKIP_OUTBOUND, "").split(",")]
        assert "443" in ports, f"{name} dials the public host on 443 through the outbound proxy"


@requires_helmfile
def test_the_journey_users_are_the_usernames_the_realm_stores(rendered):
    """The realm keeps a demo person as `{name}@{orgDomain}` (registrationEmailAsUsername), and
    the Portal compares `JC_PORTAL_JOURNEY_USERS` with that username: the bare `demo.steward`
    made every live journey's `X-JC-Run-Origin` a 403 (AG-93, T-2816)."""
    users = portal_env(portal_pod_spec(rendered("dev")))["JC_PORTAL_JOURNEY_USERS"].split(",")
    assert "demo.steward@hel.fi" in users
    assert all(user.endswith("@hel.fi") for user in users), users


@requires_helmfile
def test_the_portal_reaches_keycloak_at_the_service_and_checks_tokens_at_the_public_issuer(rendered):
    """T-4450 (T-4432): the reconciler's token and admin calls go to the Keycloak Service, an
    origin with no path, so they do not pass the public edge and its per-address limit; the
    issuer every token is checked against stays the public host."""
    env = portal_env(portal_pod_spec(rendered("dev")))
    admin = env["JC_KEYCLOAK_ADMIN_URL"]
    assert re.fullmatch(r"http://keycloak-app-keycloakx-http\.[a-z0-9-]+\.svc\.cluster\.local:80", admin), admin
    assert env["JC_OIDC_ISSUER"].startswith("https://idm."), env["JC_OIDC_ISSUER"]
