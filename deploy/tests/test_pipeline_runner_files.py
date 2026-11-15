"""The resident runner's files volume (PL-50, Architecture/08 §6): a `csv`, `file`, `file_tail`
or `parquet` DataSource names paths under /data/, and /data is a PersistentVolumeClaim of the
runner's own, kept across a redeploy."""

import shutil

import pytest

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")


@pytest.fixture(scope="module")
def dev(rendered):
    return rendered("dev")


def by_name(docs, kind, name):
    return next(d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name)


@requires_helmfile
def test_the_runner_mounts_its_files_claim_at_data(dev):
    claim = by_name(dev, "PersistentVolumeClaim", "pipeline-runner-files")
    assert claim["spec"]["accessModes"] == ["ReadWriteOnce"]
    assert claim["spec"]["resources"]["requests"]["storage"] == "1Gi"
    # A redeploy must not lose a file somebody dropped in.
    assert claim["metadata"]["annotations"]["helm.sh/resource-policy"] == "keep"

    pod = by_name(dev, "Deployment", "pipeline-runner")["spec"]["template"]["spec"]
    volume = next(v for v in pod["volumes"] if v["name"] == "files")
    assert volume["persistentVolumeClaim"]["claimName"] == "pipeline-runner-files"
    mount = next(m for m in pod["containers"][0]["volumeMounts"] if m["name"] == "files")
    assert mount["mountPath"] == "/data"
    assert claim["metadata"]["namespace"] == by_name(dev, "Deployment", "pipeline-runner")["metadata"]["namespace"]


def _projects_with_pipelines():
    """Every seeded project that has at least one `Pipeline` manifest."""
    import pathlib

    import yaml

    seed = pathlib.Path(__file__).resolve().parent.parent / "components/context-gateway/seed"
    projects = set()
    for path in sorted(seed.glob("*/*.yaml")):
        if path.name == "index.yaml":
            continue
        try:
            docs = list(yaml.safe_load_all(path.read_text()))
        except yaml.YAMLError:
            continue
        for doc in docs:
            if isinstance(doc, dict) and doc.get("kind") == "Pipeline":
                projects.add(doc["metadata"]["namespace"])
    return projects


def _secret_variable(project: str) -> str:
    """The runner variable one project's streams name, as the reconciler mints it (T-2384)."""
    return "JC_CLIENT_SECRET_" + "".join(
        c.upper() if c.isalnum() else "_" for c in project
    )


@pytest.fixture(scope="module")
def local(rendered):
    return rendered("local")


@requires_helmfile
def test_every_project_with_pipelines_has_its_own_client_and_secret(local):
    """PF-46, AG-52, T-2384: a stream presents its own project's ServiceAccount, not Helsinki's.

    The Portal's reconciler renders `{project}-pipelines` as the stream's `client_key` and names
    `JC_CLIENT_SECRET_<PROJECT>` for its secret. Until this landed the runner had one client for
    every stream, so every write to a Banská Bystrica endpoint was refused with `token is not
    bound to this resource`. This is the deployment's half: the client exists, its secret is
    generated per cluster, and the runner carries it under the name the reconciler mints.
    Where the streams run as their pipelines' own identities (dev, T-4044, T-4050) the runner
    carries none: test_pipeline_tokens.py holds that side.
    """
    import pathlib

    import yaml

    clients_file = (
        pathlib.Path(__file__).resolve().parent.parent
        / "components/pipeline-runner/keycloak-clients.yaml"
    )
    clients = yaml.safe_load(clients_file.read_text())

    pod = by_name(local, "Deployment", "pipeline-runner")["spec"]["template"]["spec"]
    env = {e["name"]: e for e in pod["containers"][0]["env"]}
    secrets = {
        d["metadata"]["name"]
        for d in local
        if d.get("kind") == "Secret" and d["metadata"]["name"].startswith("keycloak-client-")
    }

    projects = _projects_with_pipelines()
    assert projects, "the seed holds pipelines"
    for project in sorted(projects):
        client = f"{project}-pipelines"
        assert client in clients, f"{project} has pipelines and no Keycloak client"
        variable = _secret_variable(project)
        assert variable in env, f"the runner carries no {variable}"
        assert env[variable]["valueFrom"]["secretKeyRef"] == {
            "name": f"keycloak-client-{client}",
            "key": "client-secret",
        }, env[variable]
        assert f"keycloak-client-{client}" in secrets, f"{client}'s secret is not generated"
        # The secret is never a literal in the rendered manifest: it is a reference (PF-46).
        assert "value" not in env[variable], env[variable]


@requires_helmfile
def test_a_pipeline_client_is_bound_to_its_own_projects_endpoints_only(dev):
    """PF-46: an audience mapper names a slug of the client's own project, and of no other.

    A token bound to one project must never be accepted for another, so widening an audience is
    exactly the fix this task may not make (T-2384).
    """
    import pathlib

    import yaml

    root = pathlib.Path(__file__).resolve().parent.parent
    clients = yaml.safe_load(
        (root / "components/pipeline-runner/keycloak-clients.yaml").read_text()
    )
    slugs = {}
    for path in sorted((root / "components/context-gateway/seed").glob("*/*.yaml")):
        if path.name == "index.yaml":
            continue
        try:
            docs = list(yaml.safe_load_all(path.read_text()))
        except yaml.YAMLError:
            continue
        for doc in docs:
            if isinstance(doc, dict) and doc.get("kind") == "Endpoint":
                slug = (doc.get("spec") or {}).get("slug")
                if slug:
                    slugs[slug] = doc["metadata"]["namespace"]

    for client, spec in clients.items():
        project = client.removesuffix("-pipelines")
        for mapper in spec["rawValues"]["protocolMappers"]:
            audience = mapper["config"]["included.custom.audience"]
            # `portal-internal` is the Portal's own listener, not an endpoint slug (T-2271).
            if audience == "portal-internal":
                continue
            assert audience in slugs, f"{client} names {audience}, which no Endpoint declares"
            assert slugs[audience] == project, (
                f"{client} is bound to {audience}, an endpoint of project {slugs[audience]}"
            )


def test_every_projects_pipeline_client_reaches_the_portals_internal_listener():
    """T-4038: a stream's outcome, rejected and samples sinks post with its project's own client,
    and the Portal's internal listener takes only a token issued for `portal-internal`. A project
    whose client lacked the audience had every report refused with 401 (zilina on dev)."""
    import pathlib

    import yaml

    root = pathlib.Path(__file__).resolve().parent.parent
    clients = yaml.safe_load(
        (root / "components/pipeline-runner/keycloak-clients.yaml").read_text()
    )
    for client, spec in clients.items():
        audiences = {
            mapper["config"]["included.custom.audience"]
            for mapper in spec["rawValues"]["protocolMappers"]
        }
        assert "portal-internal" in audiences, client
