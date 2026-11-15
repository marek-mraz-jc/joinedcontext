"""The root credential of the artifact store goes to the reconciler and to nowhere else.

T-0422: the store ships one generated root key, and every writer and reader would otherwise
share it. The Portal's reconciler mints a scoped pair per organization from it, so the only
holders are the store, which owns it, and the Portal, which mints with it; no other workload's
environment names it (PF-32, CC-06).

Every environment deploys the store since T-0926, so the case of an environment without it is
rendered from a copy of `dev` with the component dropped: where the component is absent the
Portal must carry none of the wiring, because a half configured store is a refusal on every
sync rather than a feature.
"""

import shutil

import pytest

from conftest import drop_component

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")

ROOT_SECRET = "artifact-store-root"
WITH_THE_STORE = ("local", "production", "dev")


def workload(docs, kind, name):
    match = [d for d in docs if d.get("kind") == kind and d["metadata"]["name"] == name]
    assert match, f"{kind}/{name} is not in the render"
    return match[0]


def portal_container(docs):
    return workload(docs, "Deployment", "portal")["spec"]["template"]["spec"]["containers"][0]


def secret_readers(docs, secret):
    """Every (workload, container, variable) that reads one Secret into an environment."""
    found = []
    for doc in docs:
        template = (doc.get("spec") or {}).get("template")
        pod = template.get("spec") or {} if isinstance(template, dict) else {}
        containers = (pod.get("containers") or []) + (pod.get("initContainers") or [])
        for container in containers:
            for entry in container.get("env") or []:
                ref = ((entry.get("valueFrom") or {}).get("secretKeyRef") or {}).get("name")
                if ref == secret:
                    found.append((doc["metadata"]["name"], container["name"], entry["name"]))
            for source in container.get("envFrom") or []:
                if (source.get("secretRef") or {}).get("name") == secret:
                    found.append((doc["metadata"]["name"], container["name"], "envFrom"))
    return found


@requires_helmfile
@pytest.mark.parametrize("env", WITH_THE_STORE)
def test_the_portal_is_told_where_the_store_is_and_gets_the_root_key_by_reference(rendered, env):
    """Both halves or neither: the Portal issues nothing without an endpoint and signs nothing
    without the credential. The key is a `secretKeyRef` — a literal here would be a credential
    in Git (CC-06)."""
    env_of = {e["name"]: e for e in portal_container(rendered(env))["env"]}

    endpoint = env_of["JC_PORTAL_ARTIFACT_STORE_ENDPOINT"]["value"]
    assert endpoint.startswith("http://artifact-store.")
    assert endpoint.endswith(":9000")
    assert env_of["JC_PORTAL_ARTIFACT_STORE_BUCKET"]["value"] == "jc-artifacts"

    for name, key in (
        ("JC_PORTAL_ARTIFACT_STORE_ACCESS_KEY", "ACCESS_KEY_ID"),
        ("JC_PORTAL_ARTIFACT_STORE_SECRET_KEY", "ACCESS_SECRET_KEY"),
    ):
        assert "value" not in env_of[name], f"{name} is a literal credential"
        assert env_of[name]["valueFrom"]["secretKeyRef"] == {"name": ROOT_SECRET, "key": key}


@requires_helmfile
@pytest.mark.parametrize("env", WITH_THE_STORE)
def test_the_root_key_is_generated_where_the_store_and_the_reconciler_are_and_nowhere_else(
    rendered, env
):
    """A generated Secret is rendered into every namespace its declaration lists. Those two are
    the store, which owns the key, and the Portal, which mints with it; a third holder would be
    a workload able to read every organization's artifacts."""
    docs = rendered(env)
    holders = {
        d["metadata"]["namespace"]
        for d in docs
        if d.get("kind") == "Secret" and d["metadata"]["name"] == ROOT_SECRET
    }
    assert holders, "the root credential is generated nowhere"
    expected = {
        workload(docs, "StatefulSet", "artifact-store")["metadata"]["namespace"],
        workload(docs, "Deployment", "portal")["metadata"]["namespace"],
    }
    assert holders == expected, f"the root credential is in {sorted(holders)}"


@requires_helmfile
@pytest.mark.parametrize("env", WITH_THE_STORE)
def test_no_workload_but_the_portal_reads_the_root_key(rendered, env):
    """The Secret existing in a namespace is not the same as a container reading it. The store
    reads its own key to serve; the Portal reads it to mint. Nothing else may."""
    readers = secret_readers(rendered(env), ROOT_SECRET)
    assert {name for name, _, _ in readers} <= {"portal", "artifact-store", "artifact-store-bucket"}, readers
    portal_variables = {variable for name, _, variable in readers if name == "portal"}
    assert portal_variables == {
        "JC_PORTAL_ARTIFACT_STORE_ACCESS_KEY",
        "JC_PORTAL_ARTIFACT_STORE_SECRET_KEY",
    }, readers


@requires_helmfile
@pytest.mark.parametrize("env", WITH_THE_STORE)
def test_the_reconciler_may_reach_the_store_and_the_store_answers_it(rendered, env):
    """The admin API is the root credential's whole surface, and default-deny means both halves
    have to exist: the Portal's egress to 9000 and the store's ingress from the Portal."""
    docs = rendered(env)
    policies = {d["metadata"]["name"]: d for d in docs if d.get("kind") == "NetworkPolicy"}

    reachable = [
        peer
        for rule in policies["portal"]["spec"].get("egress") or []
        for peer in rule["to"]
        if peer.get("podSelector", {}).get("matchLabels", {}).get("app.kubernetes.io/component")
        == "store"
        for port in rule["ports"]
        if port["port"] == 9000
    ]
    assert reachable, "the reconciler cannot reach the store"

    callers = [
        peer["podSelector"]["matchLabels"]
        for rule in policies["artifact-store"]["spec"]["ingress"]
        for peer in rule["from"]
        if "podSelector" in peer
    ]
    assert any(
        labels.get("app.kubernetes.io/name") == "portal-portal" for labels in callers
    ), callers


@requires_helmfile
def test_an_environment_without_the_store_wires_none_of_it(rendered_variant):
    """A half configured store is a refusal on every sync rather than a feature, so an
    environment that drops the component must carry neither the endpoint nor the key. Rendered
    from a copy of `dev` with the component taken out, since every environment deploys it
    (T-0926). The WebAssembly host keeps the Apps' blobs in the store, so it goes with it."""

    def without_store(tree):
        drop_component(tree, "dev", "artifact-store")
        drop_component(tree, "dev", "wasm-host")
        # dev archives Postgres to the store (T-5289); without it, the backups go too.
        pg = tree / "deployment/environments/dev/postgres.yaml.gotmpl"
        pg.write_text(pg.read_text().replace("target: 'artifact-store'", "target: ''").replace("enabled: true", "enabled: false"))
        # and so does apps-db's (T-5662).
        (tree / "deployment/environments/dev/apps-db.yaml.gotmpl").unlink()

    docs = rendered_variant("dev", without_store)
    assert [e["name"] for e in portal_container(docs)["env"] if "ARTIFACT_STORE" in e["name"]] == []
    assert secret_readers(docs, ROOT_SECRET) == []


@requires_helmfile
def test_backups_to_a_store_the_environment_lacks_refuse_to_render(rendered_variant):
    """T-5289: a backup target that is not deployed would archive nowhere and say nothing."""
    with pytest.raises(AssertionError, match="does not deploy the artifact-store component"):
        rendered_variant("dev", lambda tree: [drop_component(tree, "dev", c) for c in ("artifact-store", "wasm-host")])


@requires_helmfile
def test_dev_deploys_the_store_and_wires_the_reconciler_to_it(rendered):
    """T-0926: the one cluster the platform runs on had no store, so everything built against
    it was render-tested and never run."""
    docs = rendered("dev")
    store = workload(docs, "StatefulSet", "artifact-store")
    # T-4473: dev requests what kubectl top measured, not the chart's 256Mi; T-5897: 694Mi over
    # 2026-10-25..28 since the WAL archive grew, + 20 %.
    assert store["spec"]["template"]["spec"]["containers"][0]["resources"]["requests"] == {
        "cpu": "100m",
        "memory": "832Mi",
    }
    env_of = {e["name"]: e for e in portal_container(docs)["env"]}
    assert env_of["JC_PORTAL_ARTIFACT_STORE_BUCKET"]["value"] == "jc-artifacts"
    assert env_of["JC_PORTAL_ARTIFACT_STORE_ACCESS_KEY"]["valueFrom"]["secretKeyRef"] == {
        "name": ROOT_SECRET,
        "key": "ACCESS_KEY_ID",
    }
