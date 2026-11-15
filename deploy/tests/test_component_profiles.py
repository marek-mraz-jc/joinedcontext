"""The small component profiles, minimal and broker-only (T-4856).

`componentProfile` in an environment picks the component list (helmfile-root.yaml.gotmpl);
`componentDependencies` stops a render whose list deploys a component without what it needs.
The two committed environments `minimal` and `broker-only` are each profile as CI renders it.
"""

import shutil
import subprocess

import pytest
import yaml
from conftest import PROJECT_ROOT

requires_helmfile = pytest.mark.skipif(shutil.which("helmfile") is None, reason="helmfile not installed")

PROFILES = {
    "minimal": {"prepare", "secrets", "networkpolicies", "runtime-policies", "postgres", "keycloak",
                "apisix", "context-broker", "portal"},
    "broker-only": {"prepare", "secrets", "networkpolicies", "postgres", "context-broker"},
}
BROKER_POD = {"app.kubernetes.io/name": "context-broker-broker"}


def release_components(cwd, env):
    """The `component` label of every release `helmfile list` shows for an environment."""
    result = subprocess.run(
        ["helmfile", "-f", "deployment/helmfile.yaml", "-e", env, "list"],
        check=False, cwd=str(cwd), capture_output=True, text=True,
    )
    assert result.returncode == 0, result.stderr
    return {
        label.removeprefix("component:")
        for line in result.stdout.splitlines()[1:]
        for label in (line.split()[4].split(",") if len(line.split()) > 4 else [])
        if label.startswith("component:")
    }


@requires_helmfile
@pytest.mark.parametrize("profile", sorted(PROFILES))
def test_a_profile_deploys_its_list_and_nothing_else(profile):
    if not (PROJECT_ROOT / f"deployment/environments/{profile}").is_dir():
        pytest.skip("run `just _dev-assemble` first")
    deployed = release_components(PROJECT_ROOT, profile)
    assert deployed <= PROFILES[profile], deployed - PROFILES[profile]
    assert {"context-broker", "postgres", "networkpolicies"} <= deployed


@requires_helmfile
@pytest.mark.parametrize("profile", sorted(PROFILES))
def test_a_profile_keeps_default_deny(rendered, profile):
    """Non-root pods and the other admission properties are conftest's (policies/), which the
    fast lane runs on both environments."""
    deny = {
        d["metadata"]["name"]
        for d in rendered(profile)
        if d.get("kind") == "NetworkPolicy" and d["spec"].get("podSelector") == {}
        and set(d["spec"].get("policyTypes", [])) == {"Ingress", "Egress"}
    }
    assert {"default-deny-context-broker", "default-deny-postgres"} <= deny


@requires_helmfile
def test_minimal_terminates_tls_at_the_edge(rendered):
    ingresses = [d for d in rendered("minimal") if d.get("kind") == "Ingress"]
    assert ingresses and all(d["spec"].get("tls") for d in ingresses)
    assert any(d.get("kind") == "Certificate" for d in rendered("minimal"))


@requires_helmfile
def test_broker_only_reaches_the_broker_from_inside_the_cluster_only(rendered):
    """No edge and no identity provider in front of it: no Ingress, no Service past ClusterIP,
    and no NetworkPolicy that admits anyone to the broker pod."""
    docs = rendered("broker-only")
    assert not [d for d in docs if d.get("kind") in ("Ingress", "HTTPRoute", "Gateway")]
    assert {d["spec"].get("type", "ClusterIP") for d in docs if d.get("kind") == "Service"} == {"ClusterIP"}
    broker_rules = [
        rule
        for d in docs
        if d.get("kind") == "NetworkPolicy"
        and (d["spec"].get("podSelector") or {}).get("matchLabels") == BROKER_POD
        for rule in d["spec"].get("ingress", [])
    ]
    assert broker_rules == [], broker_rules


def broker_env(docs):
    deployment = next(d for d in docs if d.get("kind") == "Deployment"
                      and d["spec"]["template"]["metadata"]["labels"].get("app.kubernetes.io/name")
                      == BROKER_POD["app.kubernetes.io/name"])
    container = deployment["spec"]["template"]["spec"]["containers"][0]
    return {e["name"]: e.get("value") for e in container.get("env", [])}


@requires_helmfile
@pytest.mark.parametrize("profile, policy, surfaces", [
    ("broker-only", "allow-all", "admin"),
    ("minimal", "joinedcontext", None),
])
def test_a_profile_decides_requests_by_what_stands_in_front(rendered, profile, policy, surfaces):
    """broker-only has no gateway to name the caller, so the joinedcontext engine would refuse
    every NGSI-LD request (ci-full 39537767356: 403 AccessDenied); it serves the bare NGSI-LD API
    under the broker's own engine. minimal keeps the gateway's engine."""
    env = broker_env(rendered(profile))
    assert env.get("ANTARES_POLICY") == policy
    if surfaces:
        assert env.get("ANTARES_API_SURFACES") == surfaces

def list_releases(tree, values):
    env_dir = tree / "deployment/environments/testing"
    shutil.rmtree(env_dir, ignore_errors=True)
    env_dir.mkdir(parents=True)
    (env_dir / "global.yaml.gotmpl").write_text(yaml.safe_dump(values))
    try:
        return subprocess.run(
            ["helmfile", "-f", "deployment/helmfile.yaml", "-e", "testing", "list"],
            check=False, cwd=str(tree), capture_output=True, text=True,
        )
    finally:
        shutil.rmtree(env_dir, ignore_errors=True)


@requires_helmfile
@pytest.mark.parametrize(
    "values, message",
    [
        ({"componentProfile": "broker-only",
          "componentProfiles": {"broker-only": ["prepare", "secrets", "networkpolicies", "context-broker"]}},
         'component "context-broker" needs "postgres", which componentProfile "broker-only" does not deploy'),
        ({"components": ["prepare", "secrets", "postgres", "context-broker"]},
         'component "context-broker" needs "networkpolicies", which componentProfile "full" does not deploy'),
        ({"componentProfile": "tiny"}, 'componentProfile "tiny" is unknown: use full, broker-only, minimal'),
    ],
    ids=["profile-without-postgres", "own-list-without-networkpolicies", "unknown-profile"],
)
def test_a_list_missing_a_dependency_stops_the_render(own_tree, values, message):
    result = list_releases(own_tree, values)
    assert result.returncode != 0, result.stdout
    assert message in result.stderr, result.stderr


@requires_helmfile
def test_a_profile_wins_over_the_environments_own_list(own_tree):
    result = list_releases(own_tree, {"componentProfile": "broker-only", "components": ["prepare", "gitea"]})
    assert result.returncode == 0, result.stderr
    releases = {line.split()[0] for line in result.stdout.splitlines()[1:] if line.strip()}
    assert "context-broker-broker" in releases
    assert not {r for r in releases if r.startswith(("gitea", "keycloak", "portal", "apisix"))}


def k8s_mib(value):
    units = {"Ki": 1 / 1024, "Mi": 1, "Gi": 1024}
    number = value.rstrip("KiMG")
    return float(number) * units[value[len(number):]]


def memory_requests_mib(docs):
    """What the scheduler has to find on the node: the memory every long-running pod asks for."""
    total = 0.0
    for d in docs:
        if d.get("kind") in ("Deployment", "StatefulSet"):
            replicas = d["spec"].get("replicas", 1)
            for c in d["spec"]["template"]["spec"]["containers"]:
                total += replicas * k8s_mib(((c.get("resources") or {}).get("requests") or {}).get("memory", "0Mi"))
        elif d.get("kind") == "Cluster" and d["apiVersion"].startswith("postgresql.cnpg.io"):
            total += d["spec"].get("instances", 1) * k8s_mib(d["spec"]["resources"]["requests"]["memory"])
    return total


@requires_helmfile
@pytest.mark.parametrize("profile, budget_mib", [("broker-only", 512), ("minimal", 2688)])
def test_a_profile_asks_for_what_its_node_has(rendered, profile, budget_mib):
    """broker-only is for a 1 GB node and minimal for a 4 GB one, each beside k3s itself
    (about 500MiB); componentProfileValues sizes them so."""
    assert memory_requests_mib(rendered(profile)) <= budget_mib
