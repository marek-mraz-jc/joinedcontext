"""Both namespace strategies render, and neither breaks what the other relies on (T-0043,
OPS-03, PF-01).

`global.singleNamespace` decides whether every component shares one namespace or gets its own.
The two modes fail in opposite directions and a render only ever exercises one of them: in one
namespace two components can collide on a name, and in several a reference that used to be a
bare Service name stops resolving and a NetworkPolicy peer that used to mean "next door" stops
matching. Both renders are asserted here, from one `helmfile template` each.
"""

from __future__ import annotations

import re
import shutil
import subprocess
from collections import defaultdict
from pathlib import Path

import pytest
from conftest import load_yaml_all

# Writes `deployment/environments/testing` in its own copy of the tree (`own_tree`), so it
# shares nothing with another module. Its own xdist group keeps the module on one worker, so its
# module-scoped renders happen once rather than once per worker (T-2895).
pytestmark = pytest.mark.xdist_group("namespace-modes")

SLUG = "dev"

#: A fully qualified in-cluster host, `<service>.<namespace>.svc.cluster.local`: the form that
#: resolves from any namespace, and the one that says which namespace it expects.
QUALIFIED = re.compile(r"\b([a-z0-9][a-z0-9-]*)\.([a-z0-9][a-z0-9-]*)\.svc\.cluster\.local\b")
# A host with a port and no dot: either right after a scheme, or standing on its own. The
# scheme case is the one that matters and the one a plain word boundary misses, because the
# `//` in `http://portal:8080` looks like the middle of a path.
BARE_HOST_PORT = re.compile(
    r"(?:(?<=://)|(?<![\w./:-]))([a-z][a-z0-9-]{2,})(?::\d{2,5})(?![\w.])"
)


def _render(deployment: Path, mode: bool) -> list[dict]:
    env_dir = deployment / "environments" / "testing"
    if env_dir.exists():
        shutil.rmtree(env_dir)
    env_dir.mkdir(parents=True)
    (env_dir / "global.yaml.gotmpl").write_text(
        f"global:\n  singleNamespace: {str(mode).lower()}\n  instanceSlug: {SLUG}\n"
    )
    try:
        result = subprocess.run(
            ["helmfile", "template", "-f", "helmfile.yaml", "--skip-deps", "-e", "testing"],
            cwd=deployment,
            capture_output=True,
            text=True,
            check=False,
        )
    finally:
        shutil.rmtree(env_dir, ignore_errors=True)
    assert result.returncode == 0, result.stderr[-4000:]
    return [d for d in load_yaml_all(result.stdout) if isinstance(d, dict)]


@pytest.fixture(scope="module")
def single(own_tree) -> list[dict]:
    return _render(own_tree / "deployment", True)


@pytest.fixture(scope="module")
def multi(own_tree) -> list[dict]:
    return _render(own_tree / "deployment", False)


def _namespaces(docs: list[dict]) -> set[str]:
    return {
        ns
        for d in docs
        if (ns := d.get("metadata", {}).get("namespace"))
        # Namespace objects name themselves in metadata.name, not in a namespace.
        and d.get("kind") != "Namespace"
    }


def _identities(docs: list[dict]) -> dict[tuple, list[str]]:
    """Every rendered object by the identity the API server enforces uniqueness on."""
    seen: dict[tuple, list[str]] = defaultdict(list)
    for d in docs:
        meta = d.get("metadata") or {}
        name = meta.get("name")
        if not name:
            continue
        key = (d.get("apiVersion"), d.get("kind"), meta.get("namespace"), name)
        seen[key].append(d.get("kind", "?"))
    return seen


def _services(docs: list[dict]) -> dict[str, set[str]]:
    services: dict[str, set[str]] = defaultdict(set)
    for d in docs:
        if d.get("kind") == "Service":
            services[d["metadata"]["name"]].add(d["metadata"].get("namespace"))
    return services


def _strings(node, path=""):
    if isinstance(node, dict):
        for key, value in node.items():
            yield from _strings(value, f"{path}.{key}")
    elif isinstance(node, list):
        for index, value in enumerate(node):
            yield from _strings(value, f"{path}[{index}]")
    elif isinstance(node, str):
        yield path, node


@pytest.mark.parametrize("mode", ["single", "multi"])
def test_every_object_has_one_identity(mode, request):
    """The single-namespace hazard: two components rendering the same object into one
    namespace. Helm would apply the second over the first and the loser is whichever
    release syncs last."""
    docs = request.getfixturevalue(mode)
    duplicates = {key: kinds for key, kinds in _identities(docs).items() if len(kinds) > 1}
    assert duplicates == {}, f"the same object is rendered twice: {sorted(duplicates)}"


def test_single_namespace_puts_everything_in_the_instance_namespace(single):
    # The one exception is not a component's home but a fence: a run's tests execute the
    # model's code in `{slug}-app-tests`, never beside the Portal (SDK-38), the way a project's
    # Apps run in the `{slug}-{project}-apps` namespaces the Portal creates. What may live there
    # is pinned by test_portal_app_tests.py.
    assert _namespaces(single) - {f"{SLUG}-app-tests"} == {SLUG}


def test_multi_namespace_gives_every_component_its_own(multi):
    namespaces = _namespaces(multi)
    assert len(namespaces) > 1
    assert all(ns == SLUG or ns.startswith(f"{SLUG}-") for ns in namespaces), namespaces
    # The components of the demo path, each in its own namespace, so a policy that names one
    # of them names a real place (PF-01: one instance, several namespaces).
    assert {f"{SLUG}-postgres", f"{SLUG}-keycloak", f"{SLUG}-apisix"} <= namespaces


def misqualified_hosts(docs: list[dict], runtime: dict[str, set[str]]) -> list[tuple]:
    """Every `x.y.svc.cluster.local` naming a Service that is not in namespace `y`.

    `runtime` names the Services an operator creates at apply time, which a render cannot see.
    """
    known = {**_services(docs), **runtime}
    return [
        (doc.get("kind"), doc["metadata"].get("name"), path, service, namespace)
        for doc in docs
        for path, value in _strings(doc)
        for service, namespace in QUALIFIED.findall(value)
        if service in known and namespace not in known[service]
    ]


def bare_cross_namespace_hosts(docs: list[dict]) -> list[tuple]:
    """Every bare `name:port` naming a Service that does not live in the referrer's namespace.

    NetworkPolicies are skipped: they carry ports and selectors, never hosts.
    """
    services = _services(docs)
    return [
        (doc.get("kind"), doc["metadata"].get("name"), path, host, owner)
        for doc in docs
        if (owner := doc.get("metadata", {}).get("namespace")) and doc.get("kind") != "NetworkPolicy"
        for path, value in _strings(doc)
        for host in BARE_HOST_PORT.findall(value)
        if host in services and owner not in services[host]
    ]


def peers_without_a_namespace(docs: list[dict]) -> list[tuple]:
    """Every NetworkPolicy peer that selects pods without saying where they are."""
    silent = []
    for policy in (d for d in docs if d.get("kind") == "NetworkPolicy"):
        spec = policy.get("spec") or {}
        for direction in ("ingress", "egress"):
            for rule in spec.get(direction) or []:
                for peer in rule.get("from") or rule.get("to") or []:
                    if "podSelector" in peer and "namespaceSelector" not in peer:
                        silent.append(
                            (policy["metadata"].get("namespace"), policy["metadata"]["name"], direction)
                        )
    return silent


@pytest.mark.parametrize("mode", ["single", "multi"])
def test_every_in_cluster_host_names_a_namespace_that_holds_the_service(mode, request):
    """A `.svc.cluster.local` host is a promise: this Service exists in that namespace. The
    promise is only checkable per mode, and the mode nobody renders is the one that breaks."""
    docs = request.getfixturevalue(mode)
    # CNPG creates this one from the Cluster resource, so it is not in the render.
    postgres = f"{SLUG}-postgres" if mode == "multi" else SLUG
    assert misqualified_hosts(docs, {"postgres-cluster-rw": {postgres}}) == []


def test_no_workload_reaches_another_component_by_a_bare_service_name(multi):
    """A bare `name:port` resolves inside the pod's own namespace. It is correct while every
    component shares one, and silently reaches nothing the moment they do not."""
    assert bare_cross_namespace_hosts(multi) == []


@pytest.mark.parametrize("mode", ["single", "multi"])
def test_every_policy_peer_says_which_namespace_it_means(mode, request):
    """A peer with only a `podSelector` means the policy's own namespace. That is right in one
    mode and wrong in the other, so the render carries the namespace either way and the two
    modes cannot disagree about who may talk to whom."""
    assert peers_without_a_namespace(request.getfixturevalue(mode)) == []


def test_the_three_scanners_see_the_breakage_they_are_here_for():
    """The renders are clean, so each scanner is shown one document that is not: a test that
    only ever passes proves nothing about the mode nobody renders."""
    reference = [
        {"apiVersion": "v1", "kind": "Service", "metadata": {"name": "portal", "namespace": "dev-portal"}}
    ]

    wrong_namespace = reference + [
        {
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": {"name": "gateway-config", "namespace": "dev-context-gateway"},
            "data": {"url": "http://portal.dev-keycloak.svc.cluster.local:8080/ingest"},
        }
    ]
    assert misqualified_hosts(wrong_namespace, {}) != []
    assert bare_cross_namespace_hosts(wrong_namespace) == [], "a qualified host is not bare"

    bare = reference + [
        {
            "apiVersion": "v1",
            "kind": "ConfigMap",
            "metadata": {"name": "gateway-config", "namespace": "dev-context-gateway"},
            "data": {"url": "http://portal:8080/ingest"},
        }
    ]
    assert bare_cross_namespace_hosts(bare) != []
    # The same reference from the Service's own namespace is how a workload talks to a peer.
    bare[1]["metadata"]["namespace"] = "dev-portal"
    assert bare_cross_namespace_hosts(bare) == []

    next_door_only = [
        {
            "apiVersion": "networking.k8s.io/v1",
            "kind": "NetworkPolicy",
            "metadata": {"name": "portal-from-observability", "namespace": "dev-portal"},
            "spec": {"ingress": [{"from": [{"podSelector": {"matchLabels": {"app": "collector"}}}]}]},
        }
    ]
    assert peers_without_a_namespace(next_door_only) != []
    next_door_only[0]["spec"]["ingress"][0]["from"][0]["namespaceSelector"] = {}
    assert peers_without_a_namespace(next_door_only) == []


def test_a_namespace_left_out_of_a_pod_policy_is_seen():
    """The same proof for the Kyverno scope: the render covers every namespace, so the gap
    this test exists for is shown to it directly."""
    docs = [
        {
            "apiVersion": "apps/v1",
            "kind": "Deployment",
            "metadata": {"name": "portal", "namespace": "dev-portal"},
        },
        {
            "apiVersion": "kyverno.io/v1",
            "kind": "ClusterPolicy",
            "metadata": {"name": "drop-all-capabilities"},
            "spec": {
                "rules": [
                    {
                        "name": "require-drop-all",
                        "match": {"any": [{"resources": {"kinds": ["Pod"], "namespaces": ["dev-keycloak"]}}]},
                    }
                ]
            },
        },
    ]
    running = _pod_namespaces(docs)
    (_, covered), = _pod_policy_scopes(docs)
    assert running - covered == {"dev-portal"}

    docs[1]["spec"]["rules"][0]["match"]["any"][0]["resources"]["namespaces"].append("dev-portal")
    (_, covered), = _pod_policy_scopes(docs)
    assert running - covered == set()


#: Workload kinds that put a Pod on a node, so a Kyverno Pod policy has to cover their
#: namespace. A CNPG `Cluster` makes pods too, from a CR the render does not expand.
POD_BEARING = ("Deployment", "StatefulSet", "DaemonSet", "Job", "CronJob", "Pod", "Cluster")


def _pod_namespaces(docs: list[dict]) -> set[str]:
    return {
        ns
        for d in docs
        if d.get("kind") in POD_BEARING and (ns := d.get("metadata", {}).get("namespace"))
    }


def _pod_policy_scopes(docs: list[dict]) -> list[tuple[str, set[str]]]:
    """Every Kyverno rule matching Pods by namespace, as (policy/rule, the namespaces)."""
    scopes = []
    for policy in (d for d in docs if d.get("kind") == "ClusterPolicy"):
        for rule in policy.get("spec", {}).get("rules") or []:
            for block in (rule.get("match") or {}).get("any") or []:
                resources = block.get("resources") or {}
                if "Pod" in (resources.get("kinds") or []) and resources.get("namespaces"):
                    scopes.append(
                        (
                            f"{policy['metadata']['name']}/{rule.get('name')}",
                            set(resources["namespaces"]),
                        )
                    )
    return scopes


#: Pod rules that are no gate of the instance and name one namespace outside it on purpose,
#: with exactly that namespace. The Linkerd CNI plugin's opt-out reason (T-2909) holds for the
#: plugin alone; stating it in an instance namespace would let any pod there skip the mesh.
OUTSIDE_THE_INSTANCE = {
    "explain-linkerd-cni-opt-out/linkerd-cni-pod-reason": {"linkerd-cni"},
}


def _gates(scopes: list[tuple[str, set[str]]]) -> list[tuple[str, set[str]]]:
    """The scopes that are gates of the instance. An exception keeps its one namespace:
    widened into the instance, it is a gate again and fails here."""
    for name, covered in scopes:
        if name in OUTSIDE_THE_INSTANCE:
            assert covered == OUTSIDE_THE_INSTANCE[name], (name, sorted(covered))
    return [(name, covered) for name, covered in scopes if name not in OUTSIDE_THE_INSTANCE]


def test_an_exception_widened_into_the_instance_is_refused():
    """T-2913: the linkerd-cni exception drops out of the gates only while it names linkerd-cni
    alone; the same rule reaching dev-portal is checked, and refused, like any gate."""
    rule = "explain-linkerd-cni-opt-out/linkerd-cni-pod-reason"
    assert _gates([(rule, {"linkerd-cni"}), ("drop-all-capabilities/x", {"dev"})]) == [
        ("drop-all-capabilities/x", {"dev"})
    ]
    with pytest.raises(AssertionError):
        _gates([(rule, {"linkerd-cni", "dev-portal"})])


@pytest.mark.parametrize("mode", ["single", "multi"])
def test_every_namespace_that_runs_a_pod_is_inside_every_pod_policy(mode, request):
    """The Kyverno gates (no capabilities, read-only root, non-root user, no automounted
    token) name the namespaces they apply to. A component whose namespace is missing from
    that list runs unguarded, and splitting the instance into namespaces is exactly the
    moment such a list goes out of date (OPS-03)."""
    docs = request.getfixturevalue(mode)
    running = _pod_namespaces(docs)
    scopes = _pod_policy_scopes(docs)
    assert running and scopes, "nothing to check means the render changed shape"

    scopes = _gates(scopes)

    unguarded = {name: sorted(running - covered) for name, covered in scopes if running - covered}
    assert unguarded == {}, unguarded


@pytest.mark.parametrize("mode", ["single", "multi"])
def test_each_component_keeps_its_own_service_account(mode, request):
    """Co-locating components must not merge their identities: the ServiceAccount is what a
    Keycloak client and every RBAC binding are bound to."""
    docs = request.getfixturevalue(mode)
    accounts = [
        (d["metadata"].get("namespace"), d["metadata"]["name"])
        for d in docs
        if d.get("kind") == "ServiceAccount"
    ]
    assert len(accounts) == len(set(accounts))
    assert len({name for _, name in accounts}) == len(accounts), (
        "two components share a ServiceAccount name; in single-namespace mode that is one "
        f"identity for both: {sorted(accounts)}"
    )

