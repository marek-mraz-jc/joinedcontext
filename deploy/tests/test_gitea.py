"""The Git forge, judged as the deployment renders it (T-0032).

Two things here are coupled across files and break silently when only one moves: the port
the chart writes into the Service and into the container, which it derives from a value
another template sets as a side effect; and the `/git` prefix, which lives in Gitea's
ROOT_URL and in the APISIX rewrite that takes it off again."""

import re
import shutil

import pytest
import yaml

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")

HTTP_PORT = 3000
POD_LABELS = {"app.kubernetes.io/name": "gitea", "app.kubernetes.io/instance": "gitea-forge"}


@pytest.fixture(scope="module")
def forge(rendered):
    """Everything the gitea component contributes to the local environment."""
    docs = rendered("local")
    return {
        (d["kind"], d["metadata"]["name"]): d
        for d in docs
        if d.get("metadata", {}).get("labels", {}).get("component") == "gitea"
        or d.get("metadata", {}).get("name", "").startswith("gitea")
    }


@pytest.fixture(scope="module")
def deployment(forge):
    return forge[("Deployment", "gitea")]


@pytest.fixture(scope="module")
def containers(deployment):
    spec = deployment["spec"]["template"]["spec"]
    return spec.get("initContainers", []) + spec["containers"]


@requires_helmfile
def test_the_forge_renders_a_deployment_a_service_and_a_volume_claim(forge):
    """docs Architecture/14: a Deployment over one ReadWriteOnce volume, not a StatefulSet —
    the chart dropped the StatefulSet in version 10 and the claim is what holds the
    repositories."""
    claim = forge[("PersistentVolumeClaim", "gitea-shared-storage")]
    assert claim["spec"]["accessModes"] == ["ReadWriteOnce"]
    assert claim["spec"]["resources"]["requests"]["storage"] == "20Gi"
    assert forge[("Service", "gitea-http")]["spec"]["type"] == "ClusterIP"
    assert ("StatefulSet", "gitea") not in forge


@requires_helmfile
def test_the_service_and_the_container_agree_on_the_http_port(forge, containers):
    """The chart reads both from gitea.config.server.HTTP_PORT, which it defaults from
    service.http.port while rendering a different template. Rendered before that side
    effect lands, the Service points at nothing."""
    service = forge[("Service", "gitea-http")]["spec"]
    http = next(p for p in service["ports"] if p["name"] == "http")
    assert (http["port"], http["targetPort"]) == (HTTP_PORT, HTTP_PORT)
    assert service["selector"] == POD_LABELS

    app = containers[-1]
    assert {"name": "http", "containerPort": HTTP_PORT} in app["ports"]


@requires_helmfile
def test_the_route_matches_the_edge_table_and_strips_the_prefix_root_url_keeps(rendered):
    """docs Deployment/10 section 2. Gitea listens at the root and renders links from
    ROOT_URL, so the prefix has to come off at the gateway and stay in the config."""
    docs = rendered("local")
    cm = next(d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "apisix-standalone-base")
    apisix = yaml.safe_load(cm["data"]["apisix.yaml"])

    route = next(r for r in apisix["routes"] if r["id"] == "gitea-forge")
    assert route["uri"] == "/git/*"
    assert route["priority"] == 10, "the portal's /* route on the same host sits at 1"
    assert route["host"] == "joinedcontext.test"
    upstream = next(u for u in apisix["upstreams"] if u["id"] == "gitea-forge")
    assert list(upstream["nodes"]) == [f"gitea-http.local.svc.cluster.local:{HTTP_PORT}"]

    plugins = next(p for p in apisix["plugin_configs"] if p["id"] == "gitea-forge")["plugins"]
    assert plugins["proxy-rewrite"]["regex_uri"] == ["^/git/?(.*)", "/$1"]
    assert plugins["request-id"]["include_in_response"] is True

    config = next(d for d in docs if d.get("kind") == "Secret" and d["metadata"]["name"] == "gitea-inline-config")
    server = dict(line.split("=", 1) for line in config["stringData"]["server"].splitlines())
    assert server["ROOT_URL"] == "https://joinedcontext.test/git/"


@requires_helmfile
def test_the_database_connection_is_encrypted(rendered):
    """CloudNativePG's pg_hba carries `hostssl` rules only, and Gitea defaults SSL_MODE to
    disable: without this the connection is refused before the password is read."""
    config = next(d for d in rendered("local")
                  if d.get("kind") == "Secret" and d["metadata"]["name"] == "gitea-inline-config")
    database = dict(line.split("=", 1) for line in config["stringData"]["database"].splitlines())
    assert database["SSL_MODE"] == "require"
    assert database["DB_TYPE"] == "postgres"


@requires_helmfile
def test_every_container_meets_the_restricted_pod_security_standards(deployment, containers):
    """Kyverno enforces these in this deployment's namespaces (T-0008); a container that
    misses one of them is not rejected here but on the cluster, at admission."""
    pod = deployment["spec"]["template"]["spec"]["securityContext"]
    assert pod["runAsNonRoot"] is True
    assert pod["runAsUser"] > 0
    assert pod["seccompProfile"]["type"] == "RuntimeDefault"

    for container in containers:
        security = container["securityContext"]
        assert security["capabilities"]["drop"] == ["ALL"], container["name"]
        assert security["readOnlyRootFilesystem"] is True, container["name"]
        assert security["allowPrivilegeEscalation"] is False, container["name"]
        assert security.get("runAsUser", 1000) > 0, container["name"]


@requires_helmfile
def test_every_container_runs_the_same_image_pinned_by_digest(containers):
    images = {c["image"] for c in containers}
    assert len(images) == 1, images
    image = images.pop()
    assert "@sha256:" in image, image
    assert "-rootless@" in image, "the root-based image cannot run under the restricted PSS"


@requires_helmfile
def test_the_forge_is_meshed(deployment):
    assert deployment["spec"]["template"]["metadata"]["annotations"]["linkerd.io/inject"] == "enabled"


def test_the_meshed_forge_does_not_route_its_own_https_through_the_proxy(deployment):
    """Gitea's OIDC discovery goes to the public issuer host, which resolves onto the cluster's
    own ingress on 443; under the outbound proxy that hop is reset and the forge crashloops
    (the k3d linkerd variant, T-0658). 443 leaves the redirect as it does for the Portal."""
    annotations = deployment["spec"]["template"]["metadata"]["annotations"]
    ports = [p.strip() for p in annotations.get("config.linkerd.io/skip-outbound-ports", "").split(",")]
    assert "443" in ports


@requires_helmfile
def test_no_bundled_database_queue_or_cache_is_deployed(rendered):
    """The chart ships PostgreSQL, PostgreSQL-HA, Valkey and a Valkey cluster, and enables
    the last one by default. The platform's database is CloudNativePG's."""
    names = {d["metadata"]["name"] for d in rendered("local") if d.get("kind") in ("StatefulSet", "Deployment")}
    assert not {n for n in names if "valkey" in n or "redis" in n or n.startswith("gitea-postgresql")}, names


@requires_helmfile
def test_no_credential_reaches_the_forge_as_a_literal(deployment, containers):
    """The database password and the Keycloak client secret are generated elsewhere and
    referenced; a value spelled out here would be a secret in the rendered manifest."""
    referenced = {}
    for container in containers:
        for env in container.get("env", []):
            if env["name"] in ("GITEA__database__PASSWD", "GITEA_OAUTH_SECRET_0",
                               "GITEA_ADMIN_USERNAME", "GITEA_ADMIN_PASSWORD"):
                assert "value" not in env, f"{container['name']}/{env['name']} is a literal"
                referenced[env["name"]] = env["valueFrom"]["secretKeyRef"]

    assert referenced["GITEA__database__PASSWD"] == {"name": "db-gitea", "key": "password"}
    assert referenced["GITEA_OAUTH_SECRET_0"] == {"name": "keycloak-client-gitea", "key": "client-secret"}
    assert referenced["GITEA_ADMIN_PASSWORD"] == {"name": "gitea-admin-credentials", "key": "password"}


@requires_helmfile
def test_the_forge_may_reach_the_database_the_reconciler_and_the_issuer_and_nothing_else(rendered):
    """Default-deny is on both directions (T-0017), so every call the forge makes has to be
    named here — and the reverse rules have to exist on the peers."""
    policies = {
        d["metadata"]["name"]: d
        for d in rendered("local")
        if d.get("kind") == "NetworkPolicy"
    }
    egress = policies["gitea"]["spec"]["egress"]
    ports = {
        (peer.get("podSelector", {}).get("matchLabels", {}).get("app.kubernetes.io/name")
         or peer.get("podSelector", {}).get("matchLabels", {}).get("cnpg.io/cluster")
         or peer.get("podSelector", {}).get("matchLabels", {}).get("k8s-app")
         or "internet", port["port"])
        for rule in egress for peer in rule["to"] for port in rule["ports"]
    }
    assert ports == {
        ("kube-dns", 53),
        ("postgres-cluster", 5432),
        ("portal-portal", 8080),
        # Itself, for the migration that duplicates a project's repository (PF-89).
        ("gitea", 3000),
        # The issuer is reached on its public URL, and the port the rule sees after DNAT is
        # the ingress controller's container port, not 443.
        ("internet", 443),
        ("internet", 8443),
    }

    for name in ("postgres-from-gitea", "portal-from-gitea",
                 "postgres-linkerd-from-gitea", "portal-linkerd-from-gitea"):
        source = policies[name]["spec"]["ingress"][0]["from"][0]["podSelector"]["matchLabels"]
        assert source == POD_LABELS, name


def test_the_forge_recreates_its_pod_instead_of_rolling(deployment):
    """The forge holds one RWO volume. A rolling update starts the new pod while the old one is
    still up, and the new Gitea dies on the level-db lock under /data/queues until the old one
    is gone; Recreate stops the old pod first."""
    assert deployment["spec"]["strategy"]["type"] == "Recreate"


def test_nobody_but_the_administrator_creates_or_forks_a_repository(rendered):
    """T-1461: a reader's fork is a copy of the private configuration outside the team map."""
    inline = next(
        d for d in rendered("local")
        if d.get("kind") == "Secret" and d["metadata"]["name"] == "gitea-inline-config"
    )["stringData"]["repository"]
    assert "MAX_CREATION_LIMIT=0" in inline.splitlines()
    assert "ALLOW_FORK_WITHOUT_MAXIMUM_LIMIT=false" in inline.splitlines()


def test_a_migration_reaches_the_forge_itself_and_nothing_else(rendered):
    """PF-89: a duplicate is migrated from the forge's own Service; the private-address
    exemption names that Service alone, so a migration cannot be pointed into the cluster."""
    docs = rendered("local")
    inline = next(
        d for d in docs
        if d.get("kind") == "Secret" and d["metadata"]["name"] == "gitea-inline-config"
    )["stringData"]["migrations"].splitlines()
    service = next(d for d in docs if d.get("kind") == "Service" and d["metadata"]["name"] == "gitea-http")
    assert "ALLOW_LOCALNETWORKS=true" in inline
    allowed = [line for line in inline if line.startswith("ALLOWED_DOMAINS=")]
    assert len(allowed) == 1, inline
    assert allowed[0].split("=", 1)[1].split(",") == [
        f"gitea-http.{service['metadata']['namespace']}.svc.cluster.local"
    ]


def test_no_push_repacks_a_repository_and_the_forge_compacts_them_nightly(rendered):
    """T-3027: git's `gc --auto` after a push (receive.autogc) repacked the configuration
    repository while the Portal's merge wrote its commit, and main pointed at a missing object.
    The forge's git runs no automatic gc at all; Gitea's own cron compacts every repository once
    a day, at midnight."""
    inline = next(
        d for d in rendered("local")
        if d.get("kind") == "Secret" and d["metadata"]["name"] == "gitea-inline-config"
    )["stringData"]
    git = inline["git.config"].splitlines()
    # `_0X2E_` is the chart's escape for a dot; Gitea decodes it into `receive.autogc`.
    assert "receive_0X2E_autogc=false" in git, git
    assert "gc_0X2E_auto=0" in git, git
    cron = inline["cron.git_gc_repos"].splitlines()
    assert "ENABLED=true" in cron, cron
    assert "SCHEDULE=@midnight" in cron, cron


def test_every_inline_setting_is_a_name_the_init_script_can_export(rendered):
    """T-3027: the chart's init script (`set -euo pipefail`) exports each inline setting as
    `GITEA__{SECTION}__{KEY}` and escapes the dots of the section only. A key with a dot or a
    dash is "not a valid identifier", the init container exits 1 and the forge never starts."""
    inline = next(
        d for d in rendered("local")
        if d.get("kind") == "Secret" and d["metadata"]["name"] == "gitea-inline-config"
    )["stringData"]
    for section, body in inline.items():
        for line in body.splitlines():
            key = line.split("=", 1)[0].strip()
            assert re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", key), f"[{section}] {key}"
